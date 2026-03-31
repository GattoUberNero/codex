use anyhow::Context;
use anyhow::Result;
use app_test_support::McpProcess;
use app_test_support::create_mock_responses_server_repeating_assistant;
use app_test_support::to_response;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::SessionSource;
use codex_app_server_protocol::ThreadRolloutAnalyzeParams;
use codex_app_server_protocol::ThreadRolloutAnalyzeResponse;
use codex_app_server_protocol::ThreadRolloutBackupDeleteParams;
use codex_app_server_protocol::ThreadRolloutBackupDeleteResponse;
use codex_app_server_protocol::ThreadRolloutBackupRestoreParams;
use codex_app_server_protocol::ThreadRolloutBackupRestoreResponse;
use codex_app_server_protocol::ThreadRolloutTrimParams;
use codex_app_server_protocol::ThreadRolloutTrimResponse;
use codex_protocol::ThreadId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::CompactedItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::SessionMeta;
use codex_protocol::protocol::SessionMetaLine;
use codex_protocol::protocol::SessionSource as CoreSessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::protocol::ThreadRolledBackEvent;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use tempfile::TempDir;
use tokio::time::timeout;
use uuid::Uuid;

const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[tokio::test]
async fn thread_rollout_trim_round_trip_via_rpc() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;
    let (thread_id, path) = write_rollout(codex_home.path(), CoreSessionSource::Cli)?;
    let original = fs::read_to_string(&path)?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let analyze_id = mcp
        .send_thread_rollout_analyze_request(ThreadRolloutAnalyzeParams {
            thread_id: thread_id.clone(),
            keep_tail_lines: 3,
        })
        .await?;
    let analyze_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(analyze_id)),
    )
    .await??;
    let analyzed: ThreadRolloutAnalyzeResponse =
        to_response::<ThreadRolloutAnalyzeResponse>(analyze_resp)?;
    assert!(analyzed.eligible);
    assert_eq!(
        analyzed
            .trim
            .as_ref()
            .expect("trim preview")
            .protected_head_end_line,
        3
    );
    let fingerprint = analyzed
        .analysis_fingerprint
        .clone()
        .expect("analysis fingerprint");

    let trim_id = mcp
        .send_thread_rollout_trim_request(ThreadRolloutTrimParams {
            thread_id: thread_id.clone(),
            keep_tail_lines: 3,
            analysis_fingerprint: fingerprint,
        })
        .await?;
    let trim_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(trim_id)),
    )
    .await??;
    let trimmed: ThreadRolloutTrimResponse = to_response::<ThreadRolloutTrimResponse>(trim_resp)?;
    assert!(trimmed.backup.backup_rollout_path.exists());
    let trimmed_text = fs::read_to_string(&path)?;
    assert!(trimmed_text.len() < original.len());

    let restore_id = mcp
        .send_thread_rollout_backup_restore_request(ThreadRolloutBackupRestoreParams {
            thread_id: thread_id.clone(),
        })
        .await?;
    let restore_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(restore_id)),
    )
    .await??;
    let restored: ThreadRolloutBackupRestoreResponse =
        to_response::<ThreadRolloutBackupRestoreResponse>(restore_resp)?;
    assert_eq!(restored.thread_id, thread_id);
    let restored_text = fs::read_to_string(&path)?;
    assert_eq!(restored_text, original);

    let delete_id = mcp
        .send_thread_rollout_backup_delete_request(ThreadRolloutBackupDeleteParams {
            thread_id: thread_id.clone(),
        })
        .await?;
    let delete_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(delete_id)),
    )
    .await??;
    let deleted: ThreadRolloutBackupDeleteResponse =
        to_response::<ThreadRolloutBackupDeleteResponse>(delete_resp)?;
    assert!(deleted.deleted);
    assert!(
        !codex_home
            .path()
            .join(".nerobar-session-rollout-backups")
            .join(thread_id)
            .exists()
    );

    Ok(())
}

#[tokio::test]
async fn thread_rollout_analyze_blocks_subagent_sessions() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;
    let (thread_id, _) = write_rollout(
        codex_home.path(),
        CoreSessionSource::SubAgent(SubAgentSource::Other("review".to_string())),
    )?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let analyze_id = mcp
        .send_thread_rollout_analyze_request(ThreadRolloutAnalyzeParams {
            thread_id,
            keep_tail_lines: 3,
        })
        .await?;
    let analyze_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(analyze_id)),
    )
    .await??;
    let analyzed: ThreadRolloutAnalyzeResponse =
        to_response::<ThreadRolloutAnalyzeResponse>(analyze_resp)?;
    assert!(!analyzed.eligible);
    assert!(
        analyzed
            .blockers
            .iter()
            .any(|item| item == "subagent_session_not_supported")
    );
    assert!(matches!(analyzed.source, Some(SessionSource::SubAgent(_))));

    Ok(())
}

fn create_config_toml(codex_home: &Path, server_uri: &str) -> std::io::Result<()> {
    let config_toml = codex_home.join("config.toml");
    std::fs::write(
        config_toml,
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "read-only"

model_provider = "mock_provider"

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#
        ),
    )
}

fn write_rollout(home: &Path, source: CoreSessionSource) -> Result<(String, PathBuf)> {
    let thread_id = Uuid::new_v4().to_string();
    let thread_uuid = ThreadId::from_string(&thread_id)?;
    let path = home
        .join("sessions/2026/03/29")
        .join(format!("rollout-2026-03-29T12-00-00-{thread_id}.jsonl"));
    let parent = path
        .parent()
        .context("rollout path is missing a parent directory")?;
    fs::create_dir_all(parent)?;
    let lines = vec![
        serde_json::to_string(&RolloutItem::SessionMeta(SessionMetaLine {
            meta: SessionMeta {
                id: thread_uuid,
                forked_from_id: None,
                timestamp: "2026-03-29T12:00:00Z".to_string(),
                cwd: PathBuf::from("/workspace/test"),
                originator: "codex".to_string(),
                cli_version: "0.0.0".to_string(),
                source,
                agent_nickname: None,
                agent_role: None,
                model_provider: Some("mock".to_string()),
                base_instructions: None,
                dynamic_tools: None,
                memory_mode: None,
            },
            git: None,
        }))?,
        serde_json::to_string(&RolloutItem::ResponseItem(ResponseItem::Message {
            id: None,
            role: "developer".to_string(),
            content: vec![ContentItem::OutputText {
                text: "seed".to_string(),
            }],
            end_turn: None,
            phase: None,
        }))?,
        serde_json::to_string(&RolloutItem::ResponseItem(ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "<environment_context>ignored</environment_context>".to_string(),
            }],
            end_turn: None,
            phase: None,
        }))?,
        serde_json::to_string(&RolloutItem::ResponseItem(ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "real user one".to_string(),
            }],
            end_turn: None,
            phase: None,
        }))?,
        serde_json::to_string(&RolloutItem::Compacted(CompactedItem {
            message: "checkpoint".to_string(),
            replacement_history: Some(vec![ResponseItem::Message {
                id: None,
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText {
                    text: "summary".to_string(),
                }],
                end_turn: None,
                phase: None,
            }]),
        }))?,
        serde_json::to_string(&RolloutItem::ResponseItem(ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: "assistant one".to_string(),
            }],
            end_turn: None,
            phase: None,
        }))?,
        serde_json::to_string(&RolloutItem::EventMsg(EventMsg::ThreadRolledBack(
            ThreadRolledBackEvent { num_turns: 0 },
        )))?,
        serde_json::to_string(&RolloutItem::ResponseItem(ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "real user two".to_string(),
            }],
            end_turn: None,
            phase: None,
        }))?,
        serde_json::to_string(&RolloutItem::ResponseItem(ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: "assistant two".to_string(),
            }],
            end_turn: None,
            phase: None,
        }))?,
        serde_json::to_string(&RolloutItem::ResponseItem(ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "real user three".to_string(),
            }],
            end_turn: None,
            phase: None,
        }))?,
        serde_json::to_string(&RolloutItem::ResponseItem(ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: "assistant three".to_string(),
            }],
            end_turn: None,
            phase: None,
        }))?,
    ];
    fs::write(&path, lines.join("\n") + "\n")?;
    Ok((thread_id, path))
}

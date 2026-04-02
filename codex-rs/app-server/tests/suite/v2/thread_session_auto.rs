use anyhow::Result;
use app_test_support::McpProcess;
use app_test_support::create_mock_responses_server_repeating_assistant;
use app_test_support::to_response;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::SessionSource;
use codex_app_server_protocol::ThreadSessionAutoApplied;
use codex_app_server_protocol::ThreadSessionAutoAuthorityMode;
use codex_app_server_protocol::ThreadSessionAutoReadParams;
use codex_app_server_protocol::ThreadSessionAutoReadResponse;
use codex_app_server_protocol::ThreadSessionAutoUpdateParams;
use codex_app_server_protocol::ThreadSessionAutoUpdateResponse;
use codex_app_server_protocol::ThreadSetNameParams;
use codex_app_server_protocol::ThreadSetNameResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_protocol::protocol::NeroAutoRuntimeConfig;
use codex_protocol::protocol::SubAgentSource;
use pretty_assertions::assert_eq;
use serde_json::Value;
use std::fs;
use std::path::Path;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const INVALID_REQUEST_ERROR_CODE: i64 = -32600;
const INTERNAL_ERROR_CODE: i64 = -32603;

#[tokio::test]
async fn thread_session_auto_read_and_update_proxy_through_bridge() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;
    let bridge_dir = write_fake_runtime_bridge()?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T12-00-00",
        "2026-04-01T12:00:00Z",
        "hello",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let bridge_cwd = bridge_dir.path().to_string_lossy().to_string();
    let mut mcp = McpProcess::new_with_env(
        codex_home.path(),
        &[
            ("NERO_RUNTIME_STATE_CONTROL_CWD", Some(bridge_cwd.as_str())),
            (
                "NERO_RUNTIME_STATE_CONTROL_MODULE",
                Some("fake_runtime_bridge"),
            ),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let read_id = mcp
        .send_thread_session_auto_read_request(ThreadSessionAutoReadParams {
            thread_id: thread_id.clone(),
        })
        .await?;
    let read_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(read_id)),
    )
    .await??;
    let initial: ThreadSessionAutoReadResponse =
        to_response::<ThreadSessionAutoReadResponse>(read_resp)?;
    assert_eq!(initial.thread_id, thread_id);
    assert_eq!(
        initial.authority,
        ThreadSessionAutoAuthorityMode::BridgeProxy
    );
    assert_eq!(initial.state.session_source, SessionSource::Cli);
    assert_eq!(
        initial.state.effective.runtime,
        NeroAutoRuntimeConfig::default()
    );

    let update_id = mcp
        .send_thread_session_auto_update_request(ThreadSessionAutoUpdateParams {
            thread_id: thread_id.clone(),
            expected_version: initial.state.version.clone(),
            expected_session_source: None,
            enabled: Some(Some(true)),
            autonomy_level: Some(Some(9)),
            autonomy_step_per_round: Some(Some(1.75)),
            max_auto_rounds: Some(Some(12)),
            done_stop_scope: Some(Some("campaign".to_string())),
            auto_rounds: Some(Some(4)),
            reset_counter: false,
        })
        .await?;
    let update_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(update_id)),
    )
    .await??;
    let updated: ThreadSessionAutoUpdateResponse =
        to_response::<ThreadSessionAutoUpdateResponse>(update_resp)?;
    assert!(updated.applied);
    assert!(!updated.conflict);
    let updated_state = updated.state.expect("updated state");
    assert_eq!(
        updated_state.effective.runtime,
        NeroAutoRuntimeConfig {
            enabled: true,
            autonomy_level: 9,
            max_auto_rounds: 12,
        }
    );
    assert_eq!(updated_state.effective.source, "session-override");
    assert_eq!(updated_state.applied.enabled, Some(true));
    assert_eq!(updated_state.applied.autonomy_step_per_round, Some(1.75));
    assert_eq!(
        updated_state.applied.done_stop_scope.as_deref(),
        Some("campaign")
    );
    assert_eq!(updated_state.applied.max_auto_rounds, Some(12));
    assert_eq!(updated_state.effective.autonomy_step_per_round, 1.75);
    assert_eq!(updated_state.effective.done_stop_scope, "campaign");
    assert_eq!(updated_state.applied.auto_rounds, 4);
    assert_eq!(updated_state.effective.auto_rounds, 4);
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_update_conflicts_on_session_source_mismatch() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;
    let bridge_dir = write_fake_runtime_bridge()?;
    let record_path = bridge_dir.path().join("apply-record.json");
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T12-30-00",
        "2026-04-01T12:30:00Z",
        "hello",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let bridge_cwd = bridge_dir.path().to_string_lossy().to_string();
    let record_path_text = record_path.to_string_lossy().to_string();
    let mut mcp = McpProcess::new_with_env(
        codex_home.path(),
        &[
            ("NERO_RUNTIME_STATE_CONTROL_CWD", Some(bridge_cwd.as_str())),
            (
                "NERO_RUNTIME_STATE_CONTROL_MODULE",
                Some("fake_runtime_bridge"),
            ),
            (
                "FAKE_RUNTIME_BRIDGE_RECORD_PATH",
                Some(record_path_text.as_str()),
            ),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let read_id = mcp
        .send_thread_session_auto_read_request(ThreadSessionAutoReadParams {
            thread_id: thread_id.clone(),
        })
        .await?;
    let read_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(read_id)),
    )
    .await??;
    let initial: ThreadSessionAutoReadResponse =
        to_response::<ThreadSessionAutoReadResponse>(read_resp)?;

    let update_id = mcp
        .send_thread_session_auto_update_request(ThreadSessionAutoUpdateParams {
            thread_id: thread_id.clone(),
            expected_version: initial.state.version.clone(),
            expected_session_source: Some(SessionSource::VsCode),
            enabled: Some(Some(true)),
            autonomy_level: Some(Some(9)),
            autonomy_step_per_round: None,
            max_auto_rounds: Some(Some(12)),
            done_stop_scope: None,
            auto_rounds: None,
            reset_counter: false,
        })
        .await?;
    let update_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(update_id)),
    )
    .await??;
    let conflict: ThreadSessionAutoUpdateResponse =
        to_response::<ThreadSessionAutoUpdateResponse>(update_resp)?;
    assert!(!conflict.applied);
    assert!(conflict.conflict);
    assert_eq!(
        conflict.error_code.as_deref(),
        Some("session_source_mismatch")
    );
    assert_eq!(
        conflict
            .state
            .as_ref()
            .map(|state| state.session_source.clone()),
        Some(SessionSource::Cli)
    );
    assert!(!record_path.exists());
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_update_reports_version_conflict_with_current_state() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;
    let bridge_dir = write_fake_runtime_bridge()?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T13-00-00",
        "2026-04-01T13:00:00Z",
        "hello again",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let bridge_cwd = bridge_dir.path().to_string_lossy().to_string();
    let mut mcp = McpProcess::new_with_env(
        codex_home.path(),
        &[
            ("NERO_RUNTIME_STATE_CONTROL_CWD", Some(bridge_cwd.as_str())),
            (
                "NERO_RUNTIME_STATE_CONTROL_MODULE",
                Some("fake_runtime_bridge"),
            ),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let read_id = mcp
        .send_thread_session_auto_read_request(ThreadSessionAutoReadParams {
            thread_id: thread_id.clone(),
        })
        .await?;
    let read_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(read_id)),
    )
    .await??;
    let initial: ThreadSessionAutoReadResponse =
        to_response::<ThreadSessionAutoReadResponse>(read_resp)?;

    let first_update_id = mcp
        .send_thread_session_auto_update_request(ThreadSessionAutoUpdateParams {
            thread_id: thread_id.clone(),
            expected_version: initial.state.version.clone(),
            expected_session_source: None,
            enabled: Some(Some(true)),
            autonomy_level: Some(Some(7)),
            autonomy_step_per_round: None,
            max_auto_rounds: Some(Some(10)),
            done_stop_scope: None,
            auto_rounds: None,
            reset_counter: false,
        })
        .await?;
    let first_update_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(first_update_id)),
    )
    .await??;
    let first_update: ThreadSessionAutoUpdateResponse =
        to_response::<ThreadSessionAutoUpdateResponse>(first_update_resp)?;
    assert!(first_update.applied);
    let stale_version = initial.state.version;

    let second_update_id = mcp
        .send_thread_session_auto_update_request(ThreadSessionAutoUpdateParams {
            thread_id,
            expected_version: stale_version,
            expected_session_source: None,
            enabled: Some(Some(false)),
            autonomy_level: Some(Some(4)),
            autonomy_step_per_round: None,
            max_auto_rounds: Some(Some(5)),
            done_stop_scope: None,
            auto_rounds: None,
            reset_counter: false,
        })
        .await?;
    let second_update_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(second_update_id)),
    )
    .await??;
    let second_update: ThreadSessionAutoUpdateResponse =
        to_response::<ThreadSessionAutoUpdateResponse>(second_update_resp)?;
    assert!(!second_update.applied);
    assert!(second_update.conflict);
    let current_state = second_update.state.expect("current state");
    assert_eq!(current_state.effective.runtime.enabled, true);
    assert_eq!(current_state.effective.runtime.autonomy_level, 7);
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_update_supports_null_clear_semantics() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;
    let bridge_dir = write_fake_runtime_bridge()?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T13-15-00",
        "2026-04-01T13:15:00Z",
        "hello clear",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let bridge_cwd = bridge_dir.path().to_string_lossy().to_string();
    let mut mcp = McpProcess::new_with_env(
        codex_home.path(),
        &[
            ("NERO_RUNTIME_STATE_CONTROL_CWD", Some(bridge_cwd.as_str())),
            (
                "NERO_RUNTIME_STATE_CONTROL_MODULE",
                Some("fake_runtime_bridge"),
            ),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let read_id = mcp
        .send_thread_session_auto_read_request(ThreadSessionAutoReadParams {
            thread_id: thread_id.clone(),
        })
        .await?;
    let read_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(read_id)),
    )
    .await??;
    let initial: ThreadSessionAutoReadResponse =
        to_response::<ThreadSessionAutoReadResponse>(read_resp)?;

    let set_id = mcp
        .send_thread_session_auto_update_request(ThreadSessionAutoUpdateParams {
            thread_id: thread_id.clone(),
            expected_version: initial.state.version.clone(),
            expected_session_source: None,
            enabled: Some(Some(true)),
            autonomy_level: Some(Some(9)),
            autonomy_step_per_round: Some(Some(1.75)),
            max_auto_rounds: Some(Some(12)),
            done_stop_scope: Some(Some("campaign".to_string())),
            auto_rounds: None,
            reset_counter: false,
        })
        .await?;
    let set_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(set_id)),
    )
    .await??;
    let set_state: ThreadSessionAutoUpdateResponse =
        to_response::<ThreadSessionAutoUpdateResponse>(set_resp)?;
    assert!(set_state.applied);
    let set_version = set_state.state.expect("set state").version;

    let clear_id = mcp
        .send_thread_session_auto_update_request(ThreadSessionAutoUpdateParams {
            thread_id,
            expected_version: set_version,
            expected_session_source: None,
            enabled: Some(None),
            autonomy_level: Some(None),
            autonomy_step_per_round: Some(None),
            max_auto_rounds: Some(None),
            done_stop_scope: Some(None),
            auto_rounds: None,
            reset_counter: false,
        })
        .await?;
    let clear_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(clear_id)),
    )
    .await??;
    let cleared: ThreadSessionAutoUpdateResponse =
        to_response::<ThreadSessionAutoUpdateResponse>(clear_resp)?;
    assert!(cleared.applied);
    let cleared_state = cleared.state.expect("cleared state");
    assert_eq!(
        cleared_state.applied,
        ThreadSessionAutoApplied {
            enabled: None,
            autonomy_level: None,
            autonomy_step_per_round: None,
            max_auto_rounds: None,
            done_stop_scope: None,
            auto_rounds: 0,
            updated_at: Some("2026-04-01T00:00:00Z".to_string()),
        }
    );
    assert_eq!(
        cleared_state.effective.runtime,
        NeroAutoRuntimeConfig::default()
    );
    assert_eq!(cleared_state.effective.autonomy_step_per_round, 1.0);
    assert_eq!(cleared_state.effective.done_stop_scope, "active_phase");
    assert_eq!(cleared_state.effective.source, "config-default");
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_update_surfaces_bridge_error_metadata() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;
    let bridge_dir = write_fake_runtime_bridge()?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T13-20-00",
        "2026-04-01T13:20:00Z",
        "hello metadata",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let bridge_cwd = bridge_dir.path().to_string_lossy().to_string();
    let mut mcp = McpProcess::new_with_env(
        codex_home.path(),
        &[
            ("NERO_RUNTIME_STATE_CONTROL_CWD", Some(bridge_cwd.as_str())),
            (
                "NERO_RUNTIME_STATE_CONTROL_MODULE",
                Some("fake_runtime_bridge"),
            ),
            (
                "FAKE_RUNTIME_BRIDGE_FORCE_ERROR",
                Some("runtime_msg_unavailable_for_auto"),
            ),
            (
                "FAKE_RUNTIME_BRIDGE_FORCE_REASON_CODE",
                Some("delivery_contract_missing"),
            ),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let read_id = mcp
        .send_thread_session_auto_read_request(ThreadSessionAutoReadParams {
            thread_id: thread_id.clone(),
        })
        .await?;
    let read_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(read_id)),
    )
    .await??;
    let initial: ThreadSessionAutoReadResponse =
        to_response::<ThreadSessionAutoReadResponse>(read_resp)?;

    let update_id = mcp
        .send_thread_session_auto_update_request(ThreadSessionAutoUpdateParams {
            thread_id,
            expected_version: initial.state.version.clone(),
            expected_session_source: None,
            enabled: Some(Some(true)),
            autonomy_level: Some(Some(8)),
            autonomy_step_per_round: None,
            max_auto_rounds: Some(Some(11)),
            done_stop_scope: None,
            auto_rounds: None,
            reset_counter: false,
        })
        .await?;
    let update_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(update_id)),
    )
    .await??;
    let failed: ThreadSessionAutoUpdateResponse =
        to_response::<ThreadSessionAutoUpdateResponse>(update_resp)?;
    assert!(!failed.applied);
    assert!(!failed.conflict);
    assert_eq!(
        failed.error_code.as_deref(),
        Some("runtime_msg_unavailable_for_auto")
    );
    assert_eq!(
        failed.reason_code.as_deref(),
        Some("delivery_contract_missing")
    );
    assert_eq!(
        failed.state.as_ref().map(|state| state.version.clone()),
        Some(initial.state.version)
    );
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_update_uses_loaded_thread_context_and_bridge_ack_version() -> Result<()>
{
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;
    let bridge_dir = write_fake_runtime_bridge()?;
    let bridge_cwd = bridge_dir.path().to_string_lossy().to_string();
    let bridge_record_path = codex_home.path().join("bridge-request.json");
    let bridge_record_path_text = bridge_record_path.to_string_lossy().to_string();
    let workspace_dir = codex_home.path().join("workspace");
    fs::create_dir_all(&workspace_dir)?;
    let workspace_dir_text = workspace_dir.to_string_lossy().to_string();

    let mut mcp = McpProcess::new_with_env(
        codex_home.path(),
        &[
            ("NERO_RUNTIME_STATE_CONTROL_CWD", Some(bridge_cwd.as_str())),
            (
                "NERO_RUNTIME_STATE_CONTROL_MODULE",
                Some("fake_runtime_bridge"),
            ),
            (
                "FAKE_RUNTIME_BRIDGE_RECORD_PATH",
                Some(bridge_record_path_text.as_str()),
            ),
            ("FAKE_RUNTIME_BRIDGE_POST_APPLY_DRIFT", Some("1")),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let start_id = mcp
        .send_thread_start_request(ThreadStartParams {
            cwd: Some(workspace_dir_text.clone()),
            ..Default::default()
        })
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let started: ThreadStartResponse = to_response::<ThreadStartResponse>(start_resp)?;
    let thread_id = started.thread.id;

    let set_name_id = mcp
        .send_thread_set_name_request(ThreadSetNameParams {
            thread_id: thread_id.clone(),
            name: "operator-session".to_string(),
        })
        .await?;
    let set_name_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(set_name_id)),
    )
    .await??;
    let _: ThreadSetNameResponse = to_response::<ThreadSetNameResponse>(set_name_resp)?;

    let read_id = mcp
        .send_thread_session_auto_read_request(ThreadSessionAutoReadParams {
            thread_id: thread_id.clone(),
        })
        .await?;
    let read_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(read_id)),
    )
    .await??;
    let initial: ThreadSessionAutoReadResponse =
        to_response::<ThreadSessionAutoReadResponse>(read_resp)?;
    let initial_session_source = initial.state.session_source.clone();

    let update_id = mcp
        .send_thread_session_auto_update_request(ThreadSessionAutoUpdateParams {
            thread_id: thread_id.clone(),
            expected_version: initial.state.version.clone(),
            expected_session_source: None,
            enabled: Some(Some(true)),
            autonomy_level: Some(Some(8)),
            autonomy_step_per_round: Some(Some(2.25)),
            max_auto_rounds: Some(Some(11)),
            done_stop_scope: Some(Some("campaign".to_string())),
            auto_rounds: Some(Some(5)),
            reset_counter: true,
        })
        .await?;
    let update_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(update_id)),
    )
    .await??;
    let updated: ThreadSessionAutoUpdateResponse =
        to_response::<ThreadSessionAutoUpdateResponse>(update_resp)?;
    assert!(!updated.applied);
    assert!(updated.conflict);
    assert_eq!(updated.error_code.as_deref(), Some("version_conflict"));
    assert_eq!(updated.reason_code.as_deref(), Some("post_apply_drift"));
    let updated_state = updated.state.expect("updated state");
    assert_eq!(updated_state.session_source, initial_session_source);
    assert_eq!(
        updated_state.thread_name.as_deref(),
        Some("operator-session")
    );
    assert_eq!(updated_state.effective.runtime.autonomy_level, 2);
    assert_eq!(updated_state.effective.autonomy_step_per_round, 0.5);
    assert_eq!(updated_state.effective.done_stop_scope, "active_phase");

    let recorded: Value = serde_json::from_str(&fs::read_to_string(&bridge_record_path)?)?;
    let recorded_session_source = match updated_state.session_source {
        SessionSource::Cli => "cli",
        SessionSource::VsCode => "vscode",
        SessionSource::Exec => "exec",
        SessionSource::AppServer => "appServer",
        SessionSource::Custom(_) => unreachable!("custom session source is not expected here"),
        SessionSource::SubAgent(_) => unreachable!("subagent session source is not expected here"),
        SessionSource::Unknown => "unknown",
    };
    assert_eq!(recorded["command"], "apply-session-auto");
    assert_eq!(
        recorded["request"]["sessionSource"],
        recorded_session_source
    );
    assert_eq!(recorded["request"]["cwd"], workspace_dir_text);
    assert_eq!(recorded["request"]["threadName"], "operator-session");
    assert_eq!(recorded["request"]["hasAutonomyStep"], true);
    assert_eq!(recorded["request"]["hasDoneStopScope"], true);
    assert_eq!(recorded["request"]["hasAutoRounds"], true);
    assert_eq!(recorded["request"]["autoRounds"], 5);

    let reread_id = mcp
        .send_thread_session_auto_read_request(ThreadSessionAutoReadParams { thread_id })
        .await?;
    let reread_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(reread_id)),
    )
    .await??;
    let reread: ThreadSessionAutoReadResponse =
        to_response::<ThreadSessionAutoReadResponse>(reread_resp)?;
    assert_eq!(updated_state.version, reread.state.version);
    assert_eq!(updated_state.effective.runtime.autonomy_level, 2);
    assert_eq!(reread.state.effective.runtime.autonomy_level, 2);
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_update_rejects_bridge_apply_config_path_mismatch() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;
    let bridge_dir = write_fake_runtime_bridge()?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T13-45-00",
        "2026-04-01T13:45:00Z",
        "hello",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let bridge_cwd = bridge_dir.path().to_string_lossy().to_string();
    let mut mcp = McpProcess::new_with_env(
        codex_home.path(),
        &[
            ("NERO_RUNTIME_STATE_CONTROL_CWD", Some(bridge_cwd.as_str())),
            (
                "NERO_RUNTIME_STATE_CONTROL_MODULE",
                Some("fake_runtime_bridge"),
            ),
            (
                "FAKE_RUNTIME_BRIDGE_APPLY_CONFIG_PATH_OVERRIDE",
                Some("/tmp/unexpected-config-path.toml"),
            ),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let read_id = mcp
        .send_thread_session_auto_read_request(ThreadSessionAutoReadParams {
            thread_id: thread_id.clone(),
        })
        .await?;
    let read_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(read_id)),
    )
    .await??;
    let initial: ThreadSessionAutoReadResponse =
        to_response::<ThreadSessionAutoReadResponse>(read_resp)?;

    let update_id = mcp
        .send_thread_session_auto_update_request(ThreadSessionAutoUpdateParams {
            thread_id,
            expected_version: initial.state.version,
            expected_session_source: None,
            enabled: Some(Some(true)),
            autonomy_level: None,
            autonomy_step_per_round: None,
            max_auto_rounds: None,
            done_stop_scope: None,
            auto_rounds: None,
            reset_counter: false,
        })
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(update_id)),
    )
    .await??;
    assert_eq!(error.error.code, INTERNAL_ERROR_CODE);
    assert!(error.error.message.contains("configPath mismatch"));
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_update_rejects_subagent_threads() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;
    let bridge_dir = write_fake_runtime_bridge()?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T14-00-00",
        "2026-04-01T14:00:00Z",
        "subagent",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::SubAgent(SubAgentSource::Review),
    )?;

    let bridge_cwd = bridge_dir.path().to_string_lossy().to_string();
    let mut mcp = McpProcess::new_with_env(
        codex_home.path(),
        &[
            ("NERO_RUNTIME_STATE_CONTROL_CWD", Some(bridge_cwd.as_str())),
            (
                "NERO_RUNTIME_STATE_CONTROL_MODULE",
                Some("fake_runtime_bridge"),
            ),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let update_id = mcp
        .send_thread_session_auto_update_request(ThreadSessionAutoUpdateParams {
            thread_id,
            expected_version: "sha256:test".to_string(),
            expected_session_source: None,
            enabled: Some(Some(true)),
            autonomy_level: Some(Some(6)),
            autonomy_step_per_round: None,
            max_auto_rounds: Some(Some(9)),
            done_stop_scope: None,
            auto_rounds: None,
            reset_counter: false,
        })
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(update_id)),
    )
    .await??;
    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert!(
        error
            .error
            .message
            .contains("unsupported for subagent sessions")
    );
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_read_rejects_missing_bridge_session_source_echo() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;
    let bridge_dir = write_fake_runtime_bridge()?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T14-30-00",
        "2026-04-01T14:30:00Z",
        "hello",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let bridge_cwd = bridge_dir.path().to_string_lossy().to_string();
    let mut mcp = McpProcess::new_with_env(
        codex_home.path(),
        &[
            ("NERO_RUNTIME_STATE_CONTROL_CWD", Some(bridge_cwd.as_str())),
            (
                "NERO_RUNTIME_STATE_CONTROL_MODULE",
                Some("fake_runtime_bridge"),
            ),
            ("FAKE_RUNTIME_BRIDGE_OMIT_SESSION_SOURCE", Some("1")),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let read_id = mcp
        .send_thread_session_auto_read_request(ThreadSessionAutoReadParams {
            thread_id: thread_id.clone(),
        })
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(read_id)),
    )
    .await??;
    assert_eq!(error.error.code, INTERNAL_ERROR_CODE);
    assert!(
        error
            .error
            .message
            .contains("sessionSource echo is missing")
    );
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_read_rejects_bridge_session_source_mismatch() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;
    let bridge_dir = write_fake_runtime_bridge()?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T14-45-00",
        "2026-04-01T14:45:00Z",
        "hello",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let bridge_cwd = bridge_dir.path().to_string_lossy().to_string();
    let mut mcp = McpProcess::new_with_env(
        codex_home.path(),
        &[
            ("NERO_RUNTIME_STATE_CONTROL_CWD", Some(bridge_cwd.as_str())),
            (
                "NERO_RUNTIME_STATE_CONTROL_MODULE",
                Some("fake_runtime_bridge"),
            ),
            (
                "FAKE_RUNTIME_BRIDGE_SESSION_SOURCE_OVERRIDE",
                Some("vscode"),
            ),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let read_id = mcp
        .send_thread_session_auto_read_request(ThreadSessionAutoReadParams {
            thread_id: thread_id.clone(),
        })
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(read_id)),
    )
    .await??;
    assert_eq!(error.error.code, INTERNAL_ERROR_CODE);
    assert!(
        error
            .error
            .message
            .contains("sessionSource mismatch: expected=cli, got=vscode")
    );
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_read_rejects_empty_bridge_stdout() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;
    let bridge_dir = write_fake_runtime_bridge()?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T15-00-00",
        "2026-04-01T15:00:00Z",
        "hello",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let bridge_cwd = bridge_dir.path().to_string_lossy().to_string();
    let mut mcp = McpProcess::new_with_env(
        codex_home.path(),
        &[
            ("NERO_RUNTIME_STATE_CONTROL_CWD", Some(bridge_cwd.as_str())),
            (
                "NERO_RUNTIME_STATE_CONTROL_MODULE",
                Some("fake_runtime_bridge"),
            ),
            ("FAKE_RUNTIME_BRIDGE_EMPTY_STDOUT", Some("1")),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let read_id = mcp
        .send_thread_session_auto_read_request(ThreadSessionAutoReadParams {
            thread_id: thread_id.clone(),
        })
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(read_id)),
    )
    .await??;
    assert_eq!(error.error.code, INTERNAL_ERROR_CODE);
    assert!(
        error
            .error
            .message
            .contains("runtime bridge returned empty stdout")
    );
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_read_rejects_non_zero_bridge_exit_even_with_json_stdout() -> Result<()>
{
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;
    let bridge_dir = write_fake_runtime_bridge()?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T15-02-00",
        "2026-04-01T15:02:00Z",
        "hello",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let bridge_cwd = bridge_dir.path().to_string_lossy().to_string();
    let mut mcp = McpProcess::new_with_env(
        codex_home.path(),
        &[
            ("NERO_RUNTIME_STATE_CONTROL_CWD", Some(bridge_cwd.as_str())),
            (
                "NERO_RUNTIME_STATE_CONTROL_MODULE",
                Some("fake_runtime_bridge"),
            ),
            ("FAKE_RUNTIME_BRIDGE_EXIT_CODE_WITH_JSON", Some("2")),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let read_id = mcp
        .send_thread_session_auto_read_request(ThreadSessionAutoReadParams {
            thread_id: thread_id.clone(),
        })
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(read_id)),
    )
    .await??;
    assert_eq!(error.error.code, INTERNAL_ERROR_CODE);
    assert!(
        error
            .error
            .message
            .contains("runtime bridge command failed")
    );
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_read_times_out_when_bridge_hangs() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;
    let bridge_dir = write_fake_runtime_bridge()?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T15-15-00",
        "2026-04-01T15:15:00Z",
        "hello",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let bridge_cwd = bridge_dir.path().to_string_lossy().to_string();
    let mut mcp = McpProcess::new_with_env(
        codex_home.path(),
        &[
            ("NERO_RUNTIME_STATE_CONTROL_CWD", Some(bridge_cwd.as_str())),
            (
                "NERO_RUNTIME_STATE_CONTROL_MODULE",
                Some("fake_runtime_bridge"),
            ),
            ("NERO_RUNTIME_CONTROL_TIMEOUT_MS", Some("10")),
            ("FAKE_RUNTIME_BRIDGE_SLEEP_MS", Some("200")),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let read_id = mcp
        .send_thread_session_auto_read_request(ThreadSessionAutoReadParams {
            thread_id: thread_id.clone(),
        })
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(read_id)),
    )
    .await??;
    assert_eq!(error.error.code, INTERNAL_ERROR_CODE);
    assert!(
        error
            .error
            .message
            .contains("runtime bridge timed out after")
    );
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_update_rejects_bridge_reported_subagent_state() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;
    let bridge_dir = write_fake_runtime_bridge()?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T15-30-00",
        "2026-04-01T15:30:00Z",
        "hello",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let bridge_cwd = bridge_dir.path().to_string_lossy().to_string();
    let mut mcp = McpProcess::new_with_env(
        codex_home.path(),
        &[
            ("NERO_RUNTIME_STATE_CONTROL_CWD", Some(bridge_cwd.as_str())),
            (
                "NERO_RUNTIME_STATE_CONTROL_MODULE",
                Some("fake_runtime_bridge"),
            ),
            ("FAKE_RUNTIME_BRIDGE_FORCE_IS_SUBAGENT", Some("1")),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let read_id = mcp
        .send_thread_session_auto_read_request(ThreadSessionAutoReadParams {
            thread_id: thread_id.clone(),
        })
        .await?;
    let read_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(read_id)),
    )
    .await??;
    let state = to_response::<ThreadSessionAutoReadResponse>(read_resp)?;
    assert!(state.state.is_subagent);

    let update_id = mcp
        .send_thread_session_auto_update_request(ThreadSessionAutoUpdateParams {
            thread_id: thread_id.clone(),
            expected_version: state.state.version,
            expected_session_source: None,
            enabled: Some(Some(true)),
            autonomy_level: None,
            autonomy_step_per_round: None,
            max_auto_rounds: None,
            done_stop_scope: None,
            auto_rounds: None,
            reset_counter: false,
        })
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(update_id)),
    )
    .await??;
    assert_eq!(error.error.code, INTERNAL_ERROR_CODE);
    assert!(
        error
            .error
            .message
            .contains("unsupported for subagent sessions")
    );
    Ok(())
}

fn create_config_toml(codex_home: &Path, server_uri: &str) -> std::io::Result<()> {
    fs::write(
        codex_home.join("config.toml"),
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
    )?;
    fs::write(codex_home.join("config-nero-hook-auto.toml"), "")?;
    Ok(())
}

fn write_fake_runtime_bridge() -> Result<TempDir> {
    let bridge_dir = TempDir::new()?;
    fs::write(
        bridge_dir.path().join("fake_runtime_bridge.py"),
        FAKE_RUNTIME_BRIDGE,
    )?;
    Ok(bridge_dir)
}

const FAKE_RUNTIME_BRIDGE: &str = r#"
from __future__ import annotations

import hashlib
import json
import os
import sys
import time
from pathlib import Path


def state_path(request: dict) -> Path:
    config_path = Path(str(request.get('configPath') or '')).expanduser()
    return config_path.with_name('nero-auto-state.json')


def load_state(path: Path) -> dict:
    if not path.is_file():
        return {}
    return json.loads(path.read_text(encoding='utf-8'))


def write_state(path: Path, data: dict) -> None:
    path.write_text(json.dumps(data, sort_keys=True), encoding='utf-8')


def version_for(data: dict) -> str:
    raw = json.dumps(data, sort_keys=True)
    return 'sha256:' + hashlib.sha256(raw.encode('utf-8')).hexdigest()


def defaults() -> dict:
    return {
        'enabled': False,
        'autonomyLevel': 5,
        'autonomyStepPerRound': 1.0,
        'maxAutoRounds': 7,
        'doneStopScope': 'active_phase',
    }


def applied_from_state(data: dict) -> dict:
    return {
        'enabled': data.get('enabled'),
        'policyOverride': data.get('policyOverride') or None,
        'autoRounds': int(data.get('autoRounds') or 0),
        'updatedAt': data.get('updatedAt'),
    }


def effective_from_state(data: dict) -> dict:
    d = defaults()
    policy = data.get('policyOverride') or {}
    enabled = data.get('enabled')
    has_policy = any(value is not None for value in policy.values())
    if not isinstance(enabled, bool):
        enabled = d['enabled']
        source = 'session-override' if has_policy else 'config-default'
    else:
        source = 'session-override'
    return {
        'enabled': enabled,
        'source': source,
        'autonomyLevel': int(policy.get('autonomy_level', d['autonomyLevel'])),
        'autonomyStepPerRound': float(policy.get('autonomy_step_per_round', d['autonomyStepPerRound'])),
        'maxAutoRounds': int(policy.get('max_auto_rounds', d['maxAutoRounds'])),
        'doneStopScope': policy.get('done_stop_scope', d['doneStopScope']),
        'autoRounds': int(data.get('autoRounds') or 0),
    }


def read(request: dict) -> dict:
    thread_id = str(request.get('threadId') or '').strip()
    if not thread_id:
        return {'ok': False, 'error': 'invalid_input', 'message': 'threadId is required'}
    path = state_path(request)
    data = load_state(path)
    session_source_override = os.environ.get('FAKE_RUNTIME_BRIDGE_SESSION_SOURCE_OVERRIDE')
    if os.environ.get('FAKE_RUNTIME_BRIDGE_OMIT_SESSION_SOURCE'):
        session_source = None
    elif session_source_override is not None:
        session_source = session_source_override
    else:
        session_source = request.get('sessionSource')
    is_subagent = bool(os.environ.get('FAKE_RUNTIME_BRIDGE_FORCE_IS_SUBAGENT'))
    return {
        'ok': True,
        'path': str(path),
        'configPath': str(request.get('configPath') or ''),
        'version': version_for(data),
        'threadId': thread_id,
        'sessionSource': session_source,
        'isSubagent': is_subagent,
        'defaults': defaults(),
        'applied': applied_from_state(data),
        'effective': effective_from_state(data),
    }


def apply(request: dict) -> dict:
    thread_id = str(request.get('threadId') or '').strip()
    expected_version = str(request.get('expectedVersion') or '').strip()
    if not thread_id or not expected_version:
        return {'ok': False, 'error': 'invalid_input', 'message': 'expectedVersion and threadId are required'}
    path = state_path(request)
    current = load_state(path)
    current_version = version_for(current)
    if current_version != expected_version:
        return {
            'ok': False,
            'error': 'version_conflict',
            'conflict': True,
            'current': {'path': str(path), 'version': current_version},
        }
    forced_error = os.environ.get('FAKE_RUNTIME_BRIDGE_FORCE_ERROR')
    if forced_error:
        return {
            'ok': False,
            'error': forced_error,
            'reasonCode': os.environ.get('FAKE_RUNTIME_BRIDGE_FORCE_REASON_CODE'),
            'message': os.environ.get('FAKE_RUNTIME_BRIDGE_FORCE_MESSAGE') or forced_error,
        }
    if request.get('hasEnabled'):
        if request.get('enabled') is None:
            current.pop('enabled', None)
        else:
            current['enabled'] = bool(request.get('enabled'))
    policy = dict(current.get('policyOverride') or {})
    if request.get('hasAutonomyLevel'):
        if request.get('autonomyLevel') is None:
            policy.pop('autonomy_level', None)
        else:
            policy['autonomy_level'] = int(request.get('autonomyLevel'))
    if request.get('hasAutonomyStep'):
        if request.get('autonomyStepPerRound') is None:
            policy.pop('autonomy_step_per_round', None)
        else:
            policy['autonomy_step_per_round'] = float(request.get('autonomyStepPerRound'))
    if request.get('hasMaxRounds'):
        if request.get('maxAutoRounds') is None:
            policy.pop('max_auto_rounds', None)
        else:
            policy['max_auto_rounds'] = int(request.get('maxAutoRounds'))
    if request.get('hasDoneStopScope'):
        if request.get('doneStopScope') is None:
            policy.pop('done_stop_scope', None)
        else:
            policy['done_stop_scope'] = request.get('doneStopScope')
    if policy:
        current['policyOverride'] = policy
    else:
        current.pop('policyOverride', None)
    if request.get('hasAutoRounds'):
        current['autoRounds'] = int(request.get('autoRounds') or 0)
    if request.get('hasResetCounter') and request.get('resetCounter'):
        current['autoRounds'] = 0
    current['updatedAt'] = '2026-04-01T00:00:00Z'
    path.parent.mkdir(parents=True, exist_ok=True)
    write_state(path, current)
    record_path = os.environ.get('FAKE_RUNTIME_BRIDGE_RECORD_PATH')
    if record_path:
        Path(record_path).write_text(
            json.dumps({'command': 'apply-session-auto', 'request': request}, sort_keys=True),
            encoding='utf-8',
        )
    updated_version = version_for(current)
    apply_thread_id = os.environ.get('FAKE_RUNTIME_BRIDGE_APPLY_THREAD_ID_OVERRIDE') or thread_id
    apply_session_source = os.environ.get('FAKE_RUNTIME_BRIDGE_APPLY_SESSION_SOURCE_OVERRIDE')
    if apply_session_source is None:
        apply_session_source = request.get('sessionSource')
    apply_config_path = os.environ.get('FAKE_RUNTIME_BRIDGE_APPLY_CONFIG_PATH_OVERRIDE')
    if apply_config_path is None:
        apply_config_path = str(request.get('configPath') or '')
    if os.environ.get('FAKE_RUNTIME_BRIDGE_POST_APPLY_DRIFT'):
        drifted = dict(current)
        drifted['policyOverride'] = {
            'autonomy_level': 2,
            'autonomy_step_per_round': 0.5,
            'max_auto_rounds': 3,
            'done_stop_scope': 'active_phase',
        }
        write_state(path, drifted)
    return {
        'ok': True,
        'threadId': apply_thread_id,
        'sessionSource': apply_session_source,
        'configPath': apply_config_path,
        'path': str(path),
        'version': updated_version,
        'applied': applied_from_state(current),
    }


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(json.dumps({'ok': False, 'error': 'invalid_command'}))
        return 2
    request = json.load(sys.stdin)
    sleep_ms = os.environ.get('FAKE_RUNTIME_BRIDGE_SLEEP_MS')
    if sleep_ms:
        time.sleep(float(sleep_ms) / 1000.0)
    if os.environ.get('FAKE_RUNTIME_BRIDGE_EMPTY_STDOUT'):
        return 0
    forced_exit_with_json = os.environ.get('FAKE_RUNTIME_BRIDGE_EXIT_CODE_WITH_JSON')
    if forced_exit_with_json is not None:
        print(json.dumps({'ok': True, 'forced': True}))
        return int(forced_exit_with_json)
    command = argv[1]
    if command == 'read-session-auto':
        print(json.dumps(read(request)))
        return 0
    if command == 'apply-session-auto':
        print(json.dumps(apply(request)))
        return 0
    print(json.dumps({'ok': False, 'error': 'invalid_command'}))
    return 2


if __name__ == '__main__':
    raise SystemExit(main(sys.argv))
"#;

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
use codex_app_server_protocol::ThreadSessionAutoInputActivityKind;
use codex_app_server_protocol::ThreadSessionAutoInputActivityParams;
use codex_app_server_protocol::ThreadSessionAutoInputActivityResponse;
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
use std::fs;
use std::path::Path;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const INVALID_REQUEST_ERROR_CODE: i64 = -32600;
const INTERNAL_ERROR_CODE: i64 = -32603;

struct RuntimeAuthorityTestEnv {
    home_dir: String,
    auto_config_path: String,
    base_config_path: String,
    msg_config_path: String,
    dev_config_path: String,
}

impl RuntimeAuthorityTestEnv {
    fn env_overrides(&self) -> [(&str, Option<&str>); 5] {
        [
            ("HOME", Some(self.home_dir.as_str())),
            (
                "CODEXN_CONFIG_NERO_AUTO_PATH",
                Some(self.auto_config_path.as_str()),
            ),
            (
                "CODEXN_CONFIG_NERO_PATH",
                Some(self.base_config_path.as_str()),
            ),
            (
                "CODEXN_CONFIG_NERO_MSG_PATH",
                Some(self.msg_config_path.as_str()),
            ),
            (
                "CODEXN_CONFIG_NERO_DEV_PATH",
                Some(self.dev_config_path.as_str()),
            ),
        ]
    }
}

#[tokio::test]
async fn thread_session_auto_read_and_update_via_rust_authority() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    let runtime_env = create_config_toml(
        codex_home.path(),
        &server.uri(),
        /*require_runtime_msg_for_auto*/ false,
    )?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T12-00-00",
        "2026-04-01T12:00:00Z",
        "hello",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let env_overrides = runtime_env.env_overrides();
    let mut mcp = McpProcess::new_with_env(codex_home.path(), &env_overrides).await?;
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
        ThreadSessionAutoAuthorityMode::AppServerAuthority
    );
    assert_eq!(initial.state.session_source, SessionSource::Cli);
    assert_eq!(
        initial.state.effective.runtime,
        NeroAutoRuntimeConfig {
            enabled: false,
            autonomy_level: 5,
            max_auto_rounds: 7,
        }
    );
    set_thread_name(&mut mcp, &thread_id, "authority-thread").await?;

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
    let runtime_env = create_config_toml(
        codex_home.path(),
        &server.uri(),
        /*require_runtime_msg_for_auto*/ false,
    )?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T12-30-00",
        "2026-04-01T12:30:00Z",
        "hello",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let env_overrides = runtime_env.env_overrides();
    let mut mcp = McpProcess::new_with_env(codex_home.path(), &env_overrides).await?;
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
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_update_reports_version_conflict_with_current_state() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    let runtime_env = create_config_toml(
        codex_home.path(),
        &server.uri(),
        /*require_runtime_msg_for_auto*/ false,
    )?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T13-00-00",
        "2026-04-01T13:00:00Z",
        "hello again",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let env_overrides = runtime_env.env_overrides();
    let mut mcp = McpProcess::new_with_env(codex_home.path(), &env_overrides).await?;
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
    set_thread_name(&mut mcp, &thread_id, "version-thread").await?;

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
    let runtime_env = create_config_toml(
        codex_home.path(),
        &server.uri(),
        /*require_runtime_msg_for_auto*/ false,
    )?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T13-15-00",
        "2026-04-01T13:15:00Z",
        "hello clear",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let env_overrides = runtime_env.env_overrides();
    let mut mcp = McpProcess::new_with_env(codex_home.path(), &env_overrides).await?;
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
    set_thread_name(&mut mcp, &thread_id, "null-clear-thread").await?;

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
    let updated_at = cleared_state.applied.updated_at.clone();
    assert!(updated_at.is_some());
    assert_eq!(
        cleared_state.applied,
        ThreadSessionAutoApplied {
            enabled: None,
            autonomy_level: None,
            autonomy_step_per_round: None,
            max_auto_rounds: None,
            done_stop_scope: None,
            auto_rounds: 0,
            updated_at,
        }
    );
    assert_eq!(
        cleared_state.effective.runtime,
        NeroAutoRuntimeConfig {
            enabled: false,
            autonomy_level: 5,
            max_auto_rounds: 7,
        }
    );
    assert_eq!(cleared_state.effective.autonomy_step_per_round, 1.0);
    assert_eq!(cleared_state.effective.done_stop_scope, "active_phase");
    assert_eq!(cleared_state.effective.source, "config-default");
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_update_rejects_enable_when_runtime_guardrail_is_unsatisfied()
-> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    let runtime_env = create_config_toml(
        codex_home.path(),
        &server.uri(),
        /*require_runtime_msg_for_auto*/ true,
    )?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T13-20-00",
        "2026-04-01T13:20:00Z",
        "hello metadata",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let env_overrides = runtime_env.env_overrides();
    let mut mcp = McpProcess::new_with_env(codex_home.path(), &env_overrides).await?;
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
    assert_eq!(failed.reason_code.as_deref(), Some("missing_thread_name"));
    assert!(
        failed
            .message
            .as_deref()
            .is_some_and(|message| message.contains("runtime_msg delivery contract"))
    );
    assert_eq!(
        failed.state.as_ref().map(|state| state.version.clone()),
        Some(initial.state.version)
    );
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_update_uses_loaded_thread_context_and_preserves_version_stability()
-> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    let runtime_env = create_config_toml(
        codex_home.path(),
        &server.uri(),
        /*require_runtime_msg_for_auto*/ true,
    )?;
    let workspace_dir = codex_home.path().join("workspace");
    fs::create_dir_all(&workspace_dir)?;
    let workspace_dir_text = workspace_dir.to_string_lossy().to_string();

    let env_overrides = runtime_env.env_overrides();
    let mut mcp = McpProcess::new_with_env(codex_home.path(), &env_overrides).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let start_id = mcp
        .send_thread_start_request(ThreadStartParams {
            cwd: Some(workspace_dir_text),
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

    set_thread_name(&mut mcp, &thread_id, "operator-session").await?;

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
            auto_rounds: None,
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
    assert!(updated.applied);
    assert!(!updated.conflict);
    let updated_state = updated.state.expect("updated state");
    assert_eq!(updated_state.session_source, initial_session_source);
    assert_eq!(
        updated_state.thread_name.as_deref(),
        Some("operator-session")
    );
    assert_eq!(updated_state.effective.runtime.autonomy_level, 8);
    assert_eq!(updated_state.effective.autonomy_step_per_round, 2.25);
    assert_eq!(updated_state.effective.done_stop_scope, "campaign");
    assert_eq!(updated_state.effective.auto_rounds, 0);

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
    assert_eq!(updated_state.effective.runtime.autonomy_level, 8);
    assert_eq!(reread.state.effective.runtime.autonomy_level, 8);
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_input_activity_bumps_generation_epoch_for_loaded_threads() -> Result<()>
{
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    let runtime_env = create_config_toml(
        codex_home.path(),
        &server.uri(),
        /*require_runtime_msg_for_auto*/ false,
    )?;

    let env_overrides = runtime_env.env_overrides();
    let mut mcp = McpProcess::new_with_env(codex_home.path(), &env_overrides).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let start_id = mcp
        .send_thread_start_request(ThreadStartParams::default())
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let started: ThreadStartResponse = to_response::<ThreadStartResponse>(start_resp)?;
    let thread_id = started.thread.id;

    let first_activity_id = mcp
        .send_thread_session_auto_input_activity_request(ThreadSessionAutoInputActivityParams {
            thread_id: thread_id.clone(),
            activity: ThreadSessionAutoInputActivityKind::DraftChanged,
        })
        .await?;
    let first_activity_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(first_activity_id)),
    )
    .await??;
    let first_activity: ThreadSessionAutoInputActivityResponse =
        to_response::<ThreadSessionAutoInputActivityResponse>(first_activity_resp)?;
    assert_eq!(first_activity.thread_id, thread_id);
    assert!(first_activity.applied);
    assert_eq!(
        first_activity.authority,
        ThreadSessionAutoAuthorityMode::AppServerAuthority
    );

    let second_activity_id = mcp
        .send_thread_session_auto_input_activity_request(ThreadSessionAutoInputActivityParams {
            thread_id: thread_id.clone(),
            activity: ThreadSessionAutoInputActivityKind::DraftChanged,
        })
        .await?;
    let second_activity_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(second_activity_id)),
    )
    .await??;
    let second_activity: ThreadSessionAutoInputActivityResponse =
        to_response::<ThreadSessionAutoInputActivityResponse>(second_activity_resp)?;
    assert_eq!(second_activity.thread_id, thread_id);
    assert!(second_activity.applied);
    assert_eq!(
        second_activity.authority,
        ThreadSessionAutoAuthorityMode::AppServerAuthority
    );
    assert!(second_activity.generation_epoch > first_activity.generation_epoch);
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_input_activity_rejects_unloaded_threads() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    let runtime_env = create_config_toml(
        codex_home.path(),
        &server.uri(),
        /*require_runtime_msg_for_auto*/ false,
    )?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T13-30-00",
        "2026-04-01T13:30:00Z",
        "hello unloaded",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let env_overrides = runtime_env.env_overrides();
    let mut mcp = McpProcess::new_with_env(codex_home.path(), &env_overrides).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let activity_id = mcp
        .send_thread_session_auto_input_activity_request(ThreadSessionAutoInputActivityParams {
            thread_id: thread_id.clone(),
            activity: ThreadSessionAutoInputActivityKind::DraftChanged,
        })
        .await?;
    let activity_error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(activity_id)),
    )
    .await??;
    assert_eq!(activity_error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert!(activity_error.error.message.contains("thread not loaded"));
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_read_rejects_corrupted_persisted_state() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    let runtime_env = create_config_toml(
        codex_home.path(),
        &server.uri(),
        /*require_runtime_msg_for_auto*/ false,
    )?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T13-45-00",
        "2026-04-01T13:45:00Z",
        "hello",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let state_path = Path::new(&runtime_env.home_dir).join(".codex/log/nero-hook-auto-state.json");
    fs::create_dir_all(state_path.parent().expect("default state path parent"))?;
    fs::write(&state_path, "[]")?;

    let env_overrides = runtime_env.env_overrides();
    let mut mcp = McpProcess::new_with_env(codex_home.path(), &env_overrides).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let read_id = mcp
        .send_thread_session_auto_read_request(ThreadSessionAutoReadParams { thread_id })
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(read_id)),
    )
    .await??;
    assert_eq!(error.error.code, INTERNAL_ERROR_CODE);
    assert!(error.error.message.contains("is corrupted"));
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_update_rejects_subagent_threads() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    let runtime_env = create_config_toml(
        codex_home.path(),
        &server.uri(),
        /*require_runtime_msg_for_auto*/ false,
    )?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T14-00-00",
        "2026-04-01T14:00:00Z",
        "subagent",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::SubAgent(SubAgentSource::Review),
    )?;

    let env_overrides = runtime_env.env_overrides();
    let mut mcp = McpProcess::new_with_env(codex_home.path(), &env_overrides).await?;
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
    assert!(error.error.message.contains("confirmed main session"));
    Ok(())
}

#[tokio::test]
async fn thread_session_auto_update_rejects_corrupted_persisted_state() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    let runtime_env = create_config_toml(
        codex_home.path(),
        &server.uri(),
        /*require_runtime_msg_for_auto*/ false,
    )?;
    let thread_id = app_test_support::create_fake_rollout_with_source(
        codex_home.path(),
        "2026-04-01T14-30-00",
        "2026-04-01T14:30:00Z",
        "hello",
        Some("mock"),
        None,
        codex_protocol::protocol::SessionSource::Cli,
    )?;

    let state_path = Path::new(&runtime_env.home_dir).join(".codex/log/nero-hook-auto-state.json");
    fs::create_dir_all(state_path.parent().expect("default state path parent"))?;
    fs::write(&state_path, "{invalid")?;

    let env_overrides = runtime_env.env_overrides();
    let mut mcp = McpProcess::new_with_env(codex_home.path(), &env_overrides).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let update_id = mcp
        .send_thread_session_auto_update_request(ThreadSessionAutoUpdateParams {
            thread_id,
            expected_version: "sha256:test".to_string(),
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
    assert!(error.error.message.contains("is corrupted"));
    Ok(())
}

fn create_config_toml(
    codex_home: &Path,
    server_uri: &str,
    require_runtime_msg_for_auto: bool,
) -> std::io::Result<RuntimeAuthorityTestEnv> {
    let home_dir = codex_home.join("home");
    let base_config_path = codex_home.join("config-nero.toml");
    let msg_config_path = codex_home.join("config-nero-hook-msg.toml");
    let dev_config_path = codex_home.join("config-nero-dev.toml");
    let state_path = codex_home.join("nero-auto-state.json");
    let auto_config_path = codex_home.join("config-nero-hook-auto.toml");

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
    fs::create_dir_all(&home_dir)?;
    fs::write(&base_config_path, "")?;
    fs::write(&msg_config_path, "")?;
    fs::write(&dev_config_path, "")?;
    fs::write(
        &auto_config_path,
        format!(
            r#"
[nero.hook.runtime.delivery]
require_runtime_msg_for_auto = {require_runtime_msg_for_auto}

[nero.hook.runtime.auto.state]
path = "{}"
"#,
            state_path.display()
        ),
    )?;
    Ok(RuntimeAuthorityTestEnv {
        home_dir: home_dir.to_string_lossy().to_string(),
        auto_config_path: auto_config_path.to_string_lossy().to_string(),
        base_config_path: base_config_path.to_string_lossy().to_string(),
        msg_config_path: msg_config_path.to_string_lossy().to_string(),
        dev_config_path: dev_config_path.to_string_lossy().to_string(),
    })
}

async fn set_thread_name(mcp: &mut McpProcess, thread_id: &str, name: &str) -> Result<()> {
    let set_name_id = mcp
        .send_thread_set_name_request(ThreadSetNameParams {
            thread_id: thread_id.to_string(),
            name: name.to_string(),
        })
        .await?;
    let set_name_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(set_name_id)),
    )
    .await??;
    let _: ThreadSetNameResponse = to_response::<ThreadSetNameResponse>(set_name_resp)?;
    Ok(())
}

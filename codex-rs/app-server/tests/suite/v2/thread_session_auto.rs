use anyhow::Result;
use app_test_support::McpProcess;
use app_test_support::create_mock_responses_server_repeating_assistant;
use app_test_support::to_response;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::SessionSource;
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
            runtime: NeroAutoRuntimeConfig {
                enabled: true,
                autonomy_level: 9,
                max_auto_rounds: 12,
            },
            autonomy_step_per_round: Some(1.75),
            done_stop_scope: Some("campaign".to_string()),
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
    assert_eq!(updated_state.applied.auto_rounds, 0);
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
            runtime: NeroAutoRuntimeConfig {
                enabled: true,
                autonomy_level: 7,
                max_auto_rounds: 10,
            },
            autonomy_step_per_round: None,
            done_stop_scope: None,
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
            runtime: NeroAutoRuntimeConfig {
                enabled: false,
                autonomy_level: 4,
                max_auto_rounds: 5,
            },
            autonomy_step_per_round: None,
            done_stop_scope: None,
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
            runtime: NeroAutoRuntimeConfig {
                enabled: true,
                autonomy_level: 8,
                max_auto_rounds: 11,
            },
            autonomy_step_per_round: Some(2.25),
            done_stop_scope: Some("campaign".to_string()),
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
    let updated_state = updated.state.expect("updated state");
    assert_eq!(updated_state.session_source, initial_session_source);
    assert_eq!(
        updated_state.thread_name.as_deref(),
        Some("operator-session")
    );
    assert_eq!(updated_state.effective.runtime.autonomy_level, 8);
    assert_eq!(updated_state.effective.autonomy_step_per_round, 2.25);
    assert_eq!(updated_state.effective.done_stop_scope, "campaign");

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
    assert_ne!(updated_state.version, reread.state.version);
    assert_eq!(updated_state.effective.runtime.autonomy_level, 8);
    assert_eq!(reread.state.effective.runtime.autonomy_level, 2);
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
            runtime: NeroAutoRuntimeConfig {
                enabled: true,
                autonomy_level: 6,
                max_auto_rounds: 9,
            },
            autonomy_step_per_round: None,
            done_stop_scope: None,
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
    if not isinstance(enabled, bool):
        enabled = d['enabled']
        source = 'config-default'
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
    session_source = None if os.environ.get('FAKE_RUNTIME_BRIDGE_OMIT_SESSION_SOURCE') else request.get('sessionSource')
    return {
        'ok': True,
        'path': str(path),
        'configPath': str(request.get('configPath') or ''),
        'version': version_for(data),
        'threadId': thread_id,
        'sessionSource': session_source,
        'isSubagent': False,
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
    if request.get('hasEnabled'):
        current['enabled'] = bool(request.get('enabled'))
    policy = dict(current.get('policyOverride') or {})
    if request.get('hasAutonomyLevel'):
        policy['autonomy_level'] = int(request.get('autonomyLevel'))
    if request.get('hasAutonomyStep'):
        policy['autonomy_step_per_round'] = float(request.get('autonomyStepPerRound'))
    if request.get('hasMaxRounds'):
        policy['max_auto_rounds'] = int(request.get('maxAutoRounds'))
    if request.get('hasDoneStopScope'):
        policy['done_stop_scope'] = request.get('doneStopScope')
    if policy:
        current['policyOverride'] = policy
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
        'path': str(path),
        'version': updated_version,
        'applied': applied_from_state(current),
    }


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(json.dumps({'ok': False, 'error': 'invalid_command'}))
        return 2
    request = json.load(sys.stdin)
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

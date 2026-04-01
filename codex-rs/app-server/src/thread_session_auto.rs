use codex_app_server_protocol::SessionSource;
use codex_app_server_protocol::ThreadSessionAutoApplied;
use codex_app_server_protocol::ThreadSessionAutoAuthorityMode;
use codex_app_server_protocol::ThreadSessionAutoDefaults;
use codex_app_server_protocol::ThreadSessionAutoEffective;
use codex_app_server_protocol::ThreadSessionAutoReadResponse;
use codex_app_server_protocol::ThreadSessionAutoState;
use codex_app_server_protocol::ThreadSessionAutoUpdateParams;
use codex_app_server_protocol::ThreadSessionAutoUpdateResponse;
use codex_protocol::protocol::NeroAutoRuntimeConfig;
use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const NERO_RUNTIME_STATE_CONTROL_CWD_ENV: &str = "NERO_RUNTIME_STATE_CONTROL_CWD";
const NERO_RUNTIME_STATE_CONTROL_CWD_ENV_COMPAT: &str = "NEROBAR_NERO_RUNTIME_STATE_CONTROL_CWD";
const NERO_RUNTIME_STATE_CONTROL_MODULE_ENV: &str = "NERO_RUNTIME_STATE_CONTROL_MODULE";
const NERO_RUNTIME_STATE_CONTROL_MODULE_ENV_COMPAT: &str =
    "NEROBAR_NERO_RUNTIME_STATE_CONTROL_MODULE";
const NERO_RUNTIME_STATE_CONTROL_TIMEOUT_ENV: &str = "NERO_RUNTIME_CONTROL_TIMEOUT_MS";
const NERO_RUNTIME_STATE_CONTROL_TIMEOUT_ENV_COMPAT: &str =
    "NEROBAR_NERO_RUNTIME_CONTROL_TIMEOUT_MS";
const NERO_RUNTIME_STATE_CONTROL_PYTHON_ENV: &str = "NERO_RUNTIME_PYTHON_BIN";
const NERO_RUNTIME_STATE_CONTROL_PYTHON_ENV_COMPAT: &str = "NEROBAR_NERO_RUNTIME_PYTHON_BIN";
const NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE: &str = "nero_hook_runtime.state_runtime_control";
const NERO_RUNTIME_STATE_CONTROL_DEFAULT_TIMEOUT_MS: u64 = 2_500;
const NERO_AUTO_RUNTIME_CONFIG_ENV: &str = "CODEXN_CONFIG_NERO_AUTO_PATH";

#[derive(Debug, Clone)]
pub(crate) struct ThreadSessionAutoContext {
    pub(crate) thread_id: String,
    pub(crate) thread_name: Option<String>,
    pub(crate) session_source: SessionSource,
    pub(crate) cwd: PathBuf,
    pub(crate) loaded: bool,
}

#[derive(Debug, Clone)]
struct RuntimeBridgeSettings {
    cwd: PathBuf,
    module: String,
    python_bin: String,
    timeout: Duration,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BridgeReadRequest {
    thread_id: String,
    session_source: String,
    config_path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BridgeApplyRequest {
    path: String,
    config_path: String,
    expected_version: String,
    thread_id: String,
    session_source: String,
    thread_name: Option<String>,
    cwd: Option<String>,
    has_enabled: bool,
    enabled: bool,
    has_autonomy_level: bool,
    autonomy_level: i64,
    has_autonomy_step: bool,
    autonomy_step_per_round: Option<f64>,
    has_max_rounds: bool,
    max_auto_rounds: i64,
    has_done_stop_scope: bool,
    done_stop_scope: Option<String>,
    has_auto_rounds: bool,
    auto_rounds: Option<i64>,
    has_reset_counter: bool,
    reset_counter: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeRuntimeDefaults {
    enabled: bool,
    autonomy_level: i64,
    autonomy_step_per_round: f64,
    max_auto_rounds: i64,
    done_stop_scope: String,
}

#[derive(Debug, Clone, Deserialize)]
struct BridgeAppliedPolicyOverride {
    #[serde(rename = "autonomy_level")]
    autonomy_level: Option<i64>,
    #[serde(rename = "autonomy_step_per_round")]
    autonomy_step_per_round: Option<f64>,
    #[serde(rename = "max_auto_rounds")]
    max_auto_rounds: Option<i64>,
    #[serde(rename = "done_stop_scope")]
    done_stop_scope: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeAppliedState {
    enabled: Option<bool>,
    policy_override: Option<BridgeAppliedPolicyOverride>,
    auto_rounds: i64,
    updated_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeEffectiveState {
    enabled: bool,
    autonomy_level: i64,
    autonomy_step_per_round: f64,
    max_auto_rounds: i64,
    done_stop_scope: String,
    auto_rounds: i64,
    source: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeReadResponse {
    ok: bool,
    error: Option<String>,
    message: Option<String>,
    path: String,
    config_path: String,
    version: String,
    thread_id: String,
    session_source: Option<String>,
    is_subagent: bool,
    defaults: BridgeRuntimeDefaults,
    applied: BridgeAppliedState,
    effective: BridgeEffectiveState,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeConflictCurrent {
    #[serde(rename = "path")]
    _path: Option<String>,
    version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeApplyResponse {
    ok: bool,
    error: Option<String>,
    message: Option<String>,
    path: Option<String>,
    #[serde(rename = "version")]
    _version: Option<String>,
    applied: Option<BridgeAppliedState>,
    conflict: Option<bool>,
    current: Option<BridgeConflictCurrent>,
}

fn first_non_empty_env(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name).ok().and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
    })
}

fn resolve_runtime_bridge_settings() -> RuntimeBridgeSettings {
    let cwd = first_non_empty_env(&[
        NERO_RUNTIME_STATE_CONTROL_CWD_ENV,
        NERO_RUNTIME_STATE_CONTROL_CWD_ENV_COMPAT,
    ])
    .map(PathBuf::from)
    .unwrap_or_else(|| PathBuf::from("/workspace/purrnet/apps/codex-nero-sdk"));
    let module = first_non_empty_env(&[
        NERO_RUNTIME_STATE_CONTROL_MODULE_ENV,
        NERO_RUNTIME_STATE_CONTROL_MODULE_ENV_COMPAT,
    ])
    .unwrap_or_else(|| NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE.to_string());
    let python_bin = first_non_empty_env(&[
        NERO_RUNTIME_STATE_CONTROL_PYTHON_ENV,
        NERO_RUNTIME_STATE_CONTROL_PYTHON_ENV_COMPAT,
    ])
    .unwrap_or_else(|| "python3".to_string());
    let timeout_ms = first_non_empty_env(&[
        NERO_RUNTIME_STATE_CONTROL_TIMEOUT_ENV,
        NERO_RUNTIME_STATE_CONTROL_TIMEOUT_ENV_COMPAT,
    ])
    .and_then(|value| value.parse::<u64>().ok())
    .filter(|value| *value > 0)
    .unwrap_or(NERO_RUNTIME_STATE_CONTROL_DEFAULT_TIMEOUT_MS);
    RuntimeBridgeSettings {
        cwd,
        module,
        python_bin,
        timeout: Duration::from_millis(timeout_ms),
    }
}

fn runtime_bridge_config_path(codex_home: &Path) -> PathBuf {
    if let Some(path) = std::env::var_os(NERO_AUTO_RUNTIME_CONFIG_ENV)
        && !path.is_empty()
    {
        return PathBuf::from(path);
    }
    codex_home.join("config-nero-hook-auto.toml")
}

async fn run_runtime_bridge<T, U>(command_name: &str, payload: &T) -> Result<U, String>
where
    T: Serialize,
    U: for<'de> Deserialize<'de>,
{
    let settings = resolve_runtime_bridge_settings();
    let bridge_input =
        serde_json::to_vec(payload).map_err(|err| format!("serialize bridge payload: {err}"))?;
    let mut command = Command::new(&settings.python_bin);
    command
        .kill_on_drop(true)
        .arg("-m")
        .arg(&settings.module)
        .arg(command_name)
        .current_dir(&settings.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|err| format!("spawn runtime bridge {}: {err}", settings.module))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "runtime bridge stdin unavailable".to_string())?;
    stdin
        .write_all(&bridge_input)
        .await
        .map_err(|err| format!("write runtime bridge stdin: {err}"))?;
    drop(stdin);
    let output = tokio::time::timeout(settings.timeout, child.wait_with_output())
        .await
        .map_err(|_| {
            format!(
                "runtime bridge timed out after {}ms",
                settings.timeout.as_millis()
            )
        })?
        .map_err(|err| format!("wait runtime bridge output: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stdout.is_empty() {
        let detail = if stderr.is_empty() {
            format!("exit status {}", output.status)
        } else {
            stderr
        };
        return Err(format!("runtime bridge returned empty stdout: {detail}"));
    }
    serde_json::from_str::<U>(&stdout).map_err(|err| {
        if stderr.is_empty() {
            format!("parse runtime bridge payload failed: {err}; stdout={stdout}")
        } else {
            format!("parse runtime bridge payload failed: {err}; stderr={stderr}; stdout={stdout}")
        }
    })
}

fn bridge_failure_detail(response: &BridgeReadResponse) -> String {
    response
        .message
        .clone()
        .or(response.error.clone())
        .unwrap_or_else(|| "runtime bridge read failed".to_string())
}

fn bridge_update_failure_detail(response: &BridgeApplyResponse) -> String {
    response
        .message
        .clone()
        .or(response.error.clone())
        .unwrap_or_else(|| "runtime bridge update failed".to_string())
}

fn sanitize_runtime(runtime: NeroAutoRuntimeConfig) -> NeroAutoRuntimeConfig {
    NeroAutoRuntimeConfig {
        enabled: runtime.enabled,
        autonomy_level: runtime.autonomy_level.clamp(1, 10),
        max_auto_rounds: runtime.max_auto_rounds.max(0),
    }
}

fn session_source_wire_value(session_source: &SessionSource) -> &str {
    match session_source {
        SessionSource::Cli => "cli",
        SessionSource::VsCode => "vscode",
        SessionSource::Exec => "exec",
        SessionSource::AppServer => "appServer",
        SessionSource::Custom(source) => source.as_str(),
        SessionSource::SubAgent(_) => "subAgent",
        SessionSource::Unknown => "unknown",
    }
}

fn map_bridge_state(
    response: BridgeReadResponse,
    context: &ThreadSessionAutoContext,
) -> Result<ThreadSessionAutoState, String> {
    if response.thread_id.trim() != context.thread_id {
        return Err(format!(
            "runtime bridge thread mismatch: expected={}, got={}",
            context.thread_id,
            response.thread_id.trim()
        ));
    }
    let session_source = response
        .session_source
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "runtime bridge sessionSource echo is missing".to_string())?;
    if session_source != session_source_wire_value(&context.session_source) {
        return Err(format!(
            "runtime bridge sessionSource mismatch: expected={}, got={session_source}",
            session_source_wire_value(&context.session_source)
        ));
    }
    let applied_policy = response.applied.policy_override.as_ref();
    Ok(ThreadSessionAutoState {
        thread_name: context.thread_name.clone(),
        session_source: context.session_source.clone(),
        loaded: context.loaded,
        config_path: PathBuf::from(response.config_path),
        state_path: PathBuf::from(response.path),
        version: response.version,
        is_subagent: response.is_subagent,
        defaults: ThreadSessionAutoDefaults {
            runtime: sanitize_runtime(NeroAutoRuntimeConfig {
                enabled: response.defaults.enabled,
                autonomy_level: response.defaults.autonomy_level,
                max_auto_rounds: response.defaults.max_auto_rounds,
            }),
            autonomy_step_per_round: response.defaults.autonomy_step_per_round,
            done_stop_scope: response.defaults.done_stop_scope,
        },
        applied: ThreadSessionAutoApplied {
            enabled: response.applied.enabled,
            autonomy_level: applied_policy.and_then(|policy| policy.autonomy_level),
            autonomy_step_per_round: applied_policy
                .and_then(|policy| policy.autonomy_step_per_round),
            max_auto_rounds: applied_policy.and_then(|policy| policy.max_auto_rounds),
            done_stop_scope: applied_policy.and_then(|policy| policy.done_stop_scope.clone()),
            auto_rounds: response.applied.auto_rounds.max(0),
            updated_at: response.applied.updated_at,
        },
        effective: ThreadSessionAutoEffective {
            runtime: sanitize_runtime(NeroAutoRuntimeConfig {
                enabled: response.effective.enabled,
                autonomy_level: response.effective.autonomy_level,
                max_auto_rounds: response.effective.max_auto_rounds,
            }),
            autonomy_step_per_round: response.effective.autonomy_step_per_round,
            done_stop_scope: response.effective.done_stop_scope,
            auto_rounds: response.effective.auto_rounds.max(0),
            source: response.effective.source,
        },
    })
}

fn updated_effective_state(
    current: &ThreadSessionAutoState,
    applied: &ThreadSessionAutoApplied,
) -> ThreadSessionAutoEffective {
    let (enabled, source) = if current.is_subagent {
        (false, "subagent-forced-off".to_string())
    } else if let Some(enabled) = applied.enabled {
        (enabled, "session-override".to_string())
    } else {
        (
            current.defaults.runtime.enabled,
            "config-default".to_string(),
        )
    };
    ThreadSessionAutoEffective {
        runtime: sanitize_runtime(NeroAutoRuntimeConfig {
            enabled,
            autonomy_level: applied
                .autonomy_level
                .unwrap_or(current.defaults.runtime.autonomy_level),
            max_auto_rounds: applied
                .max_auto_rounds
                .unwrap_or(current.defaults.runtime.max_auto_rounds),
        }),
        autonomy_step_per_round: applied
            .autonomy_step_per_round
            .unwrap_or(current.defaults.autonomy_step_per_round),
        done_stop_scope: applied
            .done_stop_scope
            .clone()
            .unwrap_or_else(|| current.defaults.done_stop_scope.clone()),
        auto_rounds: applied.auto_rounds,
        source,
    }
}

pub(crate) async fn read_thread_session_auto(
    codex_home: &Path,
    context: &ThreadSessionAutoContext,
) -> Result<ThreadSessionAutoReadResponse, String> {
    let config_path = runtime_bridge_config_path(codex_home);
    let response: BridgeReadResponse = run_runtime_bridge(
        "read-session-auto",
        &BridgeReadRequest {
            thread_id: context.thread_id.clone(),
            session_source: session_source_wire_value(&context.session_source).to_string(),
            config_path: config_path.to_string_lossy().to_string(),
        },
    )
    .await?;
    if !response.ok {
        return Err(bridge_failure_detail(&response));
    }
    Ok(ThreadSessionAutoReadResponse {
        thread_id: context.thread_id.clone(),
        authority: ThreadSessionAutoAuthorityMode::BridgeProxy,
        state: map_bridge_state(response, context)?,
    })
}

pub(crate) async fn update_thread_session_auto(
    codex_home: &Path,
    context: &ThreadSessionAutoContext,
    params: &ThreadSessionAutoUpdateParams,
) -> Result<ThreadSessionAutoUpdateResponse, String> {
    let current = read_thread_session_auto(codex_home, context).await?;
    if current.state.is_subagent || matches!(context.session_source, SessionSource::SubAgent(_)) {
        return Err("session-auto updates are unsupported for subagent sessions".to_string());
    }
    if current.state.version != params.expected_version {
        return Ok(ThreadSessionAutoUpdateResponse {
            thread_id: context.thread_id.clone(),
            authority: ThreadSessionAutoAuthorityMode::BridgeProxy,
            applied: false,
            conflict: true,
            message: Some(format!(
                "session-auto version conflict: expected={}, current={}",
                params.expected_version, current.state.version
            )),
            state: Some(current.state),
        });
    }
    let runtime = sanitize_runtime(params.runtime);
    let response: BridgeApplyResponse = run_runtime_bridge(
        "apply-session-auto",
        &BridgeApplyRequest {
            path: current.state.state_path.to_string_lossy().to_string(),
            config_path: current.state.config_path.to_string_lossy().to_string(),
            expected_version: params.expected_version.clone(),
            thread_id: context.thread_id.clone(),
            session_source: session_source_wire_value(&context.session_source).to_string(),
            thread_name: context.thread_name.clone(),
            cwd: Some(context.cwd.to_string_lossy().to_string()),
            has_enabled: true,
            enabled: runtime.enabled,
            has_autonomy_level: true,
            autonomy_level: runtime.autonomy_level,
            has_autonomy_step: params.autonomy_step_per_round.is_some(),
            autonomy_step_per_round: params.autonomy_step_per_round,
            has_max_rounds: true,
            max_auto_rounds: runtime.max_auto_rounds,
            has_done_stop_scope: params.done_stop_scope.is_some(),
            done_stop_scope: params.done_stop_scope.clone(),
            has_auto_rounds: false,
            auto_rounds: None,
            has_reset_counter: params.reset_counter,
            reset_counter: params.reset_counter,
        },
    )
    .await?;
    if response.ok {
        let version = response
            ._version
            .clone()
            .ok_or_else(|| "runtime bridge update succeeded without a version".to_string())?;
        let applied = response
            .applied
            .as_ref()
            .ok_or_else(|| "runtime bridge update succeeded without applied payload".to_string())?;
        let state = ThreadSessionAutoState {
            thread_name: current.state.thread_name.clone(),
            session_source: current.state.session_source.clone(),
            loaded: current.state.loaded,
            config_path: current.state.config_path.clone(),
            state_path: response
                .path
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| current.state.state_path.clone()),
            version,
            is_subagent: current.state.is_subagent,
            defaults: current.state.defaults.clone(),
            applied: ThreadSessionAutoApplied {
                enabled: applied.enabled,
                autonomy_level: applied
                    .policy_override
                    .as_ref()
                    .and_then(|policy| policy.autonomy_level),
                autonomy_step_per_round: applied
                    .policy_override
                    .as_ref()
                    .and_then(|policy| policy.autonomy_step_per_round),
                max_auto_rounds: applied
                    .policy_override
                    .as_ref()
                    .and_then(|policy| policy.max_auto_rounds),
                done_stop_scope: applied
                    .policy_override
                    .as_ref()
                    .and_then(|policy| policy.done_stop_scope.clone()),
                auto_rounds: applied.auto_rounds.max(0),
                updated_at: applied.updated_at.clone(),
            },
            effective: ThreadSessionAutoEffective {
                runtime,
                autonomy_step_per_round: params
                    .autonomy_step_per_round
                    .unwrap_or(current.state.effective.autonomy_step_per_round),
                done_stop_scope: params
                    .done_stop_scope
                    .clone()
                    .unwrap_or_else(|| current.state.effective.done_stop_scope.clone()),
                auto_rounds: 0,
                source: "session-override".to_string(),
            },
        };
        let effective = updated_effective_state(&state, &state.applied);
        let state = ThreadSessionAutoState { effective, ..state };
        return Ok(ThreadSessionAutoUpdateResponse {
            thread_id: context.thread_id.clone(),
            authority: ThreadSessionAutoAuthorityMode::BridgeProxy,
            applied: true,
            conflict: false,
            message: None,
            state: Some(state),
        });
    }
    let message = bridge_update_failure_detail(&response);
    let state = read_thread_session_auto(codex_home, context)
        .await
        .ok()
        .map(|confirmed| confirmed.state);
    let conflict = response.conflict.unwrap_or(false)
        || response
            .current
            .as_ref()
            .and_then(|current| current.version.as_deref())
            .is_some();
    let message = if let Some(path) = response
        .path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        format!("{message} (state path: {path})")
    } else {
        message
    };
    let message = if let Some(applied) = response.applied.as_ref() {
        format!(
            "{message} (bridge returned partial applied payload with autoRounds={})",
            applied.auto_rounds
        )
    } else {
        message
    };
    Ok(ThreadSessionAutoUpdateResponse {
        thread_id: context.thread_id.clone(),
        authority: ThreadSessionAutoAuthorityMode::BridgeProxy,
        applied: false,
        conflict,
        message: Some(message),
        state,
    })
}

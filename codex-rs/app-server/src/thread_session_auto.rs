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
const CODEXN_ROOT_ENV: &str = "CODEXN_ROOT";
const NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE: &str = "nero_hook_runtime.session_auto_bridge";
const NERO_RUNTIME_STATE_CONTROL_RETIRED_MODULE: &str = "nero_hook_runtime.state_runtime_control";
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
    enabled: Option<bool>,
    has_autonomy_level: bool,
    autonomy_level: Option<i64>,
    has_autonomy_step: bool,
    autonomy_step_per_round: Option<f64>,
    has_max_rounds: bool,
    max_auto_rounds: Option<i64>,
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
    reason_code: Option<String>,
    thread_id: Option<String>,
    session_source: Option<String>,
    config_path: Option<String>,
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

fn normalize_runtime_state_control_module(module: String) -> String {
    if module == NERO_RUNTIME_STATE_CONTROL_RETIRED_MODULE {
        return NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE.to_string();
    }
    module
}

fn resolve_runtime_bridge_module(raw_module: Option<String>) -> (String, bool) {
    match raw_module {
        Some(module) => (normalize_runtime_state_control_module(module), true),
        None => (NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE.to_string(), false),
    }
}

fn resolve_runtime_bridge_module_from_env() -> (String, bool) {
    let raw_module = first_non_empty_env(&[
        NERO_RUNTIME_STATE_CONTROL_MODULE_ENV,
        NERO_RUNTIME_STATE_CONTROL_MODULE_ENV_COMPAT,
    ]);
    resolve_runtime_bridge_module(raw_module)
}

fn resolve_runtime_bridge_default_cwd_with_inputs(
    current_dir: &Path,
    codexn_root: Option<&str>,
) -> Result<PathBuf, String> {
    let mut candidates = Vec::with_capacity(8);
    let mut push_unique = |candidate: PathBuf| {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    };
    if let Some(root) = codexn_root.map(str::trim).filter(|value| !value.is_empty()) {
        let root_path = PathBuf::from(root);
        push_unique(root_path.join("apps/codex-nero-sdk"));
        if let Some(parent) = root_path.parent() {
            push_unique(parent.join("codex-nero-sdk"));
        }
    }
    push_unique(current_dir.to_path_buf());
    push_unique(current_dir.join("apps/codex-nero-sdk"));
    if let Some(parent) = current_dir.parent() {
        push_unique(parent.join("apps/codex-nero-sdk"));
        push_unique(parent.join("codex-nero-sdk"));
    }
    if let Some(candidate) = candidates
        .iter()
        .find(|candidate| candidate.join("nero_hook_runtime").is_dir())
    {
        return Ok(candidate.clone());
    }
    let searched = candidates
        .iter()
        .map(|candidate| candidate.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "runtime bridge bootstrap failed: no nero_hook_runtime package found under [{searched}]; set {NERO_RUNTIME_STATE_CONTROL_CWD_ENV} or {CODEXN_ROOT_ENV}"
    ))
}

fn resolve_runtime_bridge_default_cwd() -> Result<PathBuf, String> {
    let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    resolve_runtime_bridge_default_cwd_with_inputs(
        &current_dir,
        first_non_empty_env(&[CODEXN_ROOT_ENV]).as_deref(),
    )
}

fn resolve_runtime_bridge_settings() -> Result<RuntimeBridgeSettings, String> {
    let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (module, module_overridden) = resolve_runtime_bridge_module_from_env();
    let cwd = first_non_empty_env(&[
        NERO_RUNTIME_STATE_CONTROL_CWD_ENV,
        NERO_RUNTIME_STATE_CONTROL_CWD_ENV_COMPAT,
    ])
    .map(PathBuf::from);
    let cwd = match cwd {
        Some(path) => path,
        None => {
            if module_overridden {
                current_dir
            } else {
                resolve_runtime_bridge_default_cwd()?
            }
        }
    };
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
    Ok(RuntimeBridgeSettings {
        cwd,
        module,
        python_bin,
        timeout: Duration::from_millis(timeout_ms),
    })
}

fn runtime_bridge_config_path_from_env(
    codex_home: &Path,
    configured_path: Option<&std::ffi::OsStr>,
) -> PathBuf {
    if let Some(path) = configured_path
        && !path.is_empty()
    {
        return PathBuf::from(path);
    }
    codex_home.join("config-nero-hook-auto.toml")
}

fn runtime_bridge_config_path(codex_home: &Path) -> PathBuf {
    runtime_bridge_config_path_from_env(
        codex_home,
        std::env::var_os(NERO_AUTO_RUNTIME_CONFIG_ENV).as_deref(),
    )
}

async fn run_runtime_bridge<T, U>(command_name: &str, payload: &T) -> Result<U, String>
where
    T: Serialize,
    U: for<'de> Deserialize<'de>,
{
    let settings = resolve_runtime_bridge_settings()?;
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
    if !output.status.success() {
        let detail = match (stderr.is_empty(), stdout.is_empty()) {
            (true, true) => format!("exit status {}", output.status),
            (false, true) => format!("exit status {}; stderr={stderr}", output.status),
            (true, false) => format!("exit status {}; stdout={stdout}", output.status),
            (false, false) => format!(
                "exit status {}; stderr={stderr}; stdout={stdout}",
                output.status
            ),
        };
        return Err(format!("runtime bridge command failed: {detail}"));
    }
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

fn validate_bridge_apply_identity(
    response: &BridgeApplyResponse,
    context: &ThreadSessionAutoContext,
    expected_config_path: &Path,
) -> Result<(), String> {
    let thread_id = response
        .thread_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "runtime bridge apply threadId echo is missing".to_string())?;
    if thread_id != context.thread_id {
        return Err(format!(
            "runtime bridge apply thread mismatch: expected={}, got={thread_id}",
            context.thread_id
        ));
    }
    let session_source = response
        .session_source
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "runtime bridge apply sessionSource echo is missing".to_string())?;
    let expected_session_source = session_source_wire_value(&context.session_source);
    if session_source != expected_session_source {
        return Err(format!(
            "runtime bridge apply sessionSource mismatch: expected={expected_session_source}, got={session_source}"
        ));
    }
    let config_path = response
        .config_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "runtime bridge apply configPath echo is missing".to_string())?;
    let expected_config_path = expected_config_path.to_string_lossy();
    if config_path != expected_config_path {
        return Err(format!(
            "runtime bridge apply configPath mismatch: expected={expected_config_path}, got={config_path}"
        ));
    }
    Ok(())
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
        SessionSource::AppServer => "mcp",
        SessionSource::Custom(source) => source.as_str(),
        SessionSource::SubAgent(_) => "subAgent",
        SessionSource::Unknown => "unknown",
    }
}

fn map_bridge_state(
    response: BridgeReadResponse,
    context: &ThreadSessionAutoContext,
    expected_config_path: &Path,
) -> Result<ThreadSessionAutoState, String> {
    if response.thread_id.trim() != context.thread_id {
        return Err(format!(
            "runtime bridge thread mismatch: expected={}, got={}",
            context.thread_id,
            response.thread_id.trim()
        ));
    }
    let config_path = response.config_path.trim();
    if config_path.is_empty() {
        return Err("runtime bridge configPath echo is missing".to_string());
    }
    let expected_config_path = expected_config_path.to_string_lossy();
    if config_path != expected_config_path {
        return Err(format!(
            "runtime bridge configPath mismatch: expected={expected_config_path}, got={config_path}"
        ));
    }
    let state_path = response.path.trim();
    if state_path.is_empty() {
        return Err("runtime bridge state path is missing".to_string());
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
        config_path: PathBuf::from(config_path),
        state_path: PathBuf::from(state_path),
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
        state: map_bridge_state(response, context, &config_path)?,
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
    if let Some(expected_session_source) = &params.expected_session_source
        && current.state.session_source != *expected_session_source
    {
        let current_state = current.state;
        return Ok(ThreadSessionAutoUpdateResponse {
            thread_id: context.thread_id.clone(),
            authority: ThreadSessionAutoAuthorityMode::BridgeProxy,
            applied: false,
            conflict: true,
            message: Some(format!(
                "session-auto sessionSource mismatch: expected={}, current={}",
                session_source_wire_value(expected_session_source),
                session_source_wire_value(&current_state.session_source),
            )),
            error_code: Some("session_source_mismatch".to_string()),
            reason_code: None,
            state: Some(current_state),
        });
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
            error_code: Some("version_conflict".to_string()),
            reason_code: None,
            state: Some(current.state),
        });
    }
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
            has_enabled: params.enabled.is_some(),
            enabled: params.enabled.flatten(),
            has_autonomy_level: params.autonomy_level.is_some(),
            autonomy_level: params
                .autonomy_level
                .flatten()
                .map(|value| value.clamp(1, 10)),
            has_autonomy_step: params.autonomy_step_per_round.is_some(),
            autonomy_step_per_round: params
                .autonomy_step_per_round
                .flatten()
                .map(|value| value.clamp(0.0, 10.0)),
            has_max_rounds: params.max_auto_rounds.is_some(),
            max_auto_rounds: params.max_auto_rounds.flatten().map(|value| value.max(0)),
            has_done_stop_scope: params.done_stop_scope.is_some(),
            done_stop_scope: params.done_stop_scope.clone().flatten(),
            has_auto_rounds: params.auto_rounds.is_some(),
            auto_rounds: params.auto_rounds.flatten().map(|value| value.max(0)),
            has_reset_counter: params.reset_counter,
            reset_counter: params.reset_counter,
        },
    )
    .await?;
    if response.ok {
        validate_bridge_apply_identity(&response, context, &current.state.config_path)?;
        let applied_version = response
            ._version
            .clone()
            .ok_or_else(|| "runtime bridge update succeeded without a version".to_string())?;
        response
            .applied
            .as_ref()
            .ok_or_else(|| "runtime bridge update succeeded without applied payload".to_string())?;
        let confirmed = read_thread_session_auto(codex_home, context)
            .await
            .map_err(|err| {
                format!("runtime bridge update succeeded but confirmation read failed: {err}")
            })?;
        if confirmed.state.version != applied_version {
            return Ok(ThreadSessionAutoUpdateResponse {
                thread_id: context.thread_id.clone(),
                authority: ThreadSessionAutoAuthorityMode::BridgeProxy,
                applied: false,
                conflict: true,
                message: Some(format!(
                    "runtime bridge update confirmation version mismatch: applied={applied_version}, confirmed={}",
                    confirmed.state.version
                )),
                error_code: Some("version_conflict".to_string()),
                reason_code: Some("post_apply_drift".to_string()),
                state: Some(confirmed.state),
            });
        }
        return Ok(ThreadSessionAutoUpdateResponse {
            thread_id: context.thread_id.clone(),
            authority: ThreadSessionAutoAuthorityMode::BridgeProxy,
            applied: true,
            conflict: false,
            message: None,
            error_code: None,
            reason_code: None,
            state: Some(confirmed.state),
        });
    }
    let error_code = response
        .error
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let reason_code = response
        .reason_code
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
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
        error_code,
        reason_code,
        state,
    })
}

#[cfg(test)]
mod tests {
    use super::NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE;
    use super::NERO_RUNTIME_STATE_CONTROL_RETIRED_MODULE;
    use super::normalize_runtime_state_control_module;
    use super::resolve_runtime_bridge_default_cwd_with_inputs;
    use super::resolve_runtime_bridge_module;
    use super::runtime_bridge_config_path_from_env;
    use pretty_assertions::assert_eq;
    use std::ffi::OsStr;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn normalize_runtime_state_control_module_maps_retired_cli_module_to_current_bridge() {
        assert_eq!(
            normalize_runtime_state_control_module(
                NERO_RUNTIME_STATE_CONTROL_RETIRED_MODULE.to_string(),
            ),
            NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE.to_string()
        );
    }

    #[test]
    fn normalize_runtime_state_control_module_keeps_current_bridge_module() {
        assert_eq!(
            normalize_runtime_state_control_module(
                NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE.to_string(),
            ),
            NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE.to_string()
        );
    }

    #[test]
    fn runtime_bridge_config_path_uses_explicit_override_when_present() {
        assert_eq!(
            runtime_bridge_config_path_from_env(
                Path::new("/tmp/codex-home"),
                Some(OsStr::new("/tmp/explicit-nero-auto.toml"))
            ),
            Path::new("/tmp/explicit-nero-auto.toml")
        );
    }

    #[test]
    fn runtime_bridge_config_path_falls_back_for_empty_override() {
        assert_eq!(
            runtime_bridge_config_path_from_env(Path::new("/tmp/codex-home"), Some(OsStr::new(""))),
            Path::new("/tmp/codex-home/config-nero-hook-auto.toml")
        );
    }

    #[test]
    fn resolve_runtime_bridge_default_cwd_uses_codexn_root_sdk_when_available() {
        let root = tempdir().expect("tempdir root");
        let sdk_dir = root.path().join("apps/codex-nero-sdk/nero_hook_runtime");
        std::fs::create_dir_all(&sdk_dir).expect("create sdk package");
        let cwd = tempdir().expect("tempdir cwd");
        assert_eq!(
            resolve_runtime_bridge_default_cwd_with_inputs(
                cwd.path(),
                Some(root.path().to_string_lossy().as_ref()),
            ),
            Ok(root.path().join("apps/codex-nero-sdk"))
        );
    }

    #[test]
    fn resolve_runtime_bridge_default_cwd_uses_codexn_root_parent_sdk_when_available() {
        let workspace = tempdir().expect("tempdir workspace");
        let app_root = workspace.path().join("apps/codex-nero");
        std::fs::create_dir_all(&app_root).expect("create app root");
        let sdk_dir = workspace.path().join("codex-nero-sdk/nero_hook_runtime");
        std::fs::create_dir_all(&sdk_dir).expect("create sibling sdk package");
        let cwd = workspace.path().join("cwd");
        std::fs::create_dir_all(&cwd).expect("create cwd");
        let expected_sdk_root = workspace.path().join("codex-nero-sdk");
        assert_eq!(
            resolve_runtime_bridge_default_cwd_with_inputs(
                &cwd,
                Some(app_root.to_string_lossy().as_ref()),
            ),
            Ok(expected_sdk_root)
        );
    }

    #[test]
    fn resolve_runtime_bridge_module_uses_default_when_unset() {
        assert_eq!(
            resolve_runtime_bridge_module(None),
            (NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE.to_string(), false)
        );
    }

    #[test]
    fn resolve_runtime_bridge_default_cwd_fails_when_no_candidate_contains_package() {
        let workspace = tempdir().expect("tempdir workspace");
        let cwd = workspace.path().join("sandbox/cwd");
        std::fs::create_dir_all(&cwd).expect("create cwd");
        let err = resolve_runtime_bridge_default_cwd_with_inputs(&cwd, None)
            .expect_err("expected bootstrap failure");
        assert!(err.contains("runtime bridge bootstrap failed"));
        assert!(err.contains("set NERO_RUNTIME_STATE_CONTROL_CWD or CODEXN_ROOT"));
    }
}

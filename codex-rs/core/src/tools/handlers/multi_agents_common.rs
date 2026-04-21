use crate::agent::AgentStatus;
use crate::codex::Session;
use crate::codex::TurnContext;
use crate::config::Config;
use crate::config::SpawnDelegationReportProfile;
use crate::error::CodexErr;
use crate::function_tool::FunctionCallError;
use crate::models_manager::manager::RefreshStrategy;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use codex_features::Feature;
use codex_protocol::AgentPath;
use codex_protocol::ThreadId;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_protocol::protocol::CollabAgentRef;
use codex_protocol::protocol::CollabAgentStatusEntry;
use codex_protocol::protocol::DelegationOrchestrationContext;
use codex_protocol::protocol::DelegationReport;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SpawnContextInheritanceMode;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::user_input::UserInput;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Minimum wait timeout to prevent tight polling loops from burning CPU.
pub(crate) const MIN_WAIT_TIMEOUT_MS: i64 = 10_000;
pub(crate) const DEFAULT_WAIT_TIMEOUT_MS: i64 = 30_000;
pub(crate) const MAX_WAIT_TIMEOUT_MS: i64 = 3600 * 1000;
pub(crate) const WAIT_TIMEOUT_MULTIPLIER: i64 = 2;

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CloseAgentMode {
    #[default]
    SafeClose,
    ForceCancel,
}

pub(crate) fn effective_wait_timeout_ms(
    requested_timeout_ms: i64,
) -> Result<i64, FunctionCallError> {
    let requested_timeout_ms = match requested_timeout_ms {
        ms if ms <= 0 => {
            return Err(FunctionCallError::RespondToModel(
                "timeout_ms must be greater than zero".to_owned(),
            ));
        }
        ms => ms,
    };
    Ok(requested_timeout_ms
        .saturating_mul(WAIT_TIMEOUT_MULTIPLIER)
        .clamp(MIN_WAIT_TIMEOUT_MS, MAX_WAIT_TIMEOUT_MS))
}

pub(crate) fn is_active_agent_status(status: &AgentStatus) -> bool {
    matches!(
        status,
        AgentStatus::PendingInit | AgentStatus::Running | AgentStatus::Interrupted
    )
}

pub(crate) async fn validate_safe_close_subtree(
    session: &Session,
    agent_id: ThreadId,
) -> Result<(), FunctionCallError> {
    if let Some((active_agent_id, status)) = session
        .services
        .agent_control
        .first_active_agent_in_tree(agent_id)
        .await
        .map_err(|err| collab_agent_error(agent_id, err))?
    {
        return Err(safe_close_rejected_error(active_agent_id, &status));
    }

    Ok(())
}

pub(crate) fn safe_close_rejected_error(
    active_agent_id: ThreadId,
    status: &AgentStatus,
) -> FunctionCallError {
    FunctionCallError::RespondToModel(format!(
        "agent subtree still has active agent {active_agent_id} ({status:?}); use force_cancel only when you intentionally want to terminate running work"
    ))
}
// Delimiter for structured spawn delegation context embedded into child payload text.
// This is intentionally explicit so runtime can sanitize task summaries while preserving
// the structured block in transport.
const SPAWN_DELEGATION_CONTEXT_BLOCK_TAG: &str = "spawn_delegation_report_json";
const SPAWN_DELEGATION_ROUTER_COMMAND: &str = "render-spawn-orchestration-block";
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
const NERO_RUNTIME_STATE_CONTROL_DEFAULT_TIMEOUT_MS: u64 = 12_000;

pub(crate) fn function_arguments(payload: ToolPayload) -> Result<String, FunctionCallError> {
    match payload {
        ToolPayload::Function { arguments } => Ok(arguments),
        _ => Err(FunctionCallError::RespondToModel(
            "collab handler received unsupported payload".to_string(),
        )),
    }
}

pub(crate) fn tool_output_json_text<T>(value: &T, tool_name: &str) -> String
where
    T: Serialize,
{
    serde_json::to_string(value).unwrap_or_else(|err| {
        JsonValue::String(format!("failed to serialize {tool_name} result: {err}")).to_string()
    })
}

pub(crate) fn tool_output_response_item<T>(
    call_id: &str,
    payload: &ToolPayload,
    value: &T,
    success: Option<bool>,
    tool_name: &str,
) -> ResponseInputItem
where
    T: Serialize,
{
    FunctionToolOutput::from_text(tool_output_json_text(value, tool_name), success)
        .to_response_item(call_id, payload)
}

pub(crate) fn tool_output_code_mode_result<T>(value: &T, tool_name: &str) -> JsonValue
where
    T: Serialize,
{
    serde_json::to_value(value).unwrap_or_else(|err| {
        JsonValue::String(format!("failed to serialize {tool_name} result: {err}"))
    })
}

pub(crate) async fn current_wait_agent_statuses(
    session: &Session,
    receiver_thread_ids: &[ThreadId],
) -> HashMap<ThreadId, AgentStatus> {
    let mut current_statuses = HashMap::with_capacity(receiver_thread_ids.len());
    for thread_id in receiver_thread_ids {
        current_statuses.insert(
            *thread_id,
            session.services.agent_control.get_status(*thread_id).await,
        );
    }
    current_statuses
}

pub(crate) fn build_wait_agent_statuses(
    statuses: &HashMap<ThreadId, AgentStatus>,
    receiver_agents: &[CollabAgentRef],
) -> Vec<CollabAgentStatusEntry> {
    if statuses.is_empty() {
        return Vec::new();
    }

    let mut entries = Vec::with_capacity(statuses.len());
    let mut seen = HashMap::with_capacity(receiver_agents.len());
    for receiver_agent in receiver_agents {
        seen.insert(receiver_agent.thread_id, ());
        if let Some(status) = statuses.get(&receiver_agent.thread_id) {
            entries.push(CollabAgentStatusEntry {
                thread_id: receiver_agent.thread_id,
                agent_nickname: receiver_agent.agent_nickname.clone(),
                agent_role: receiver_agent.agent_role.clone(),
                status: status.clone(),
            });
        }
    }

    let mut extras = statuses
        .iter()
        .filter(|(thread_id, _)| !seen.contains_key(thread_id))
        .map(|(thread_id, status)| CollabAgentStatusEntry {
            thread_id: *thread_id,
            agent_nickname: None,
            agent_role: None,
            status: status.clone(),
        })
        .collect::<Vec<_>>();
    extras.sort_by(|left, right| left.thread_id.to_string().cmp(&right.thread_id.to_string()));
    entries.extend(extras);
    entries
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct WaitPendingAgent {
    pub(crate) id: String,
    pub(crate) state: AgentStatus,
}

pub(crate) fn build_wait_agent_pending(
    statuses: &HashMap<ThreadId, AgentStatus>,
    receiver_agents: &[CollabAgentRef],
) -> Vec<WaitPendingAgent> {
    if statuses.is_empty() {
        return Vec::new();
    }

    let mut entries = Vec::new();
    let mut seen = HashMap::with_capacity(receiver_agents.len());
    for receiver_agent in receiver_agents {
        seen.insert(receiver_agent.thread_id, ());
        if let Some(status) = statuses.get(&receiver_agent.thread_id)
            && is_active_agent_status(status)
        {
            entries.push(WaitPendingAgent {
                id: receiver_agent.thread_id.to_string(),
                state: status.clone(),
            });
        }
    }

    let mut extras = statuses
        .iter()
        .filter(|(thread_id, status)| {
            !seen.contains_key(thread_id) && is_active_agent_status(status)
        })
        .map(|(thread_id, status)| WaitPendingAgent {
            id: thread_id.to_string(),
            state: status.clone(),
        })
        .collect::<Vec<_>>();
    extras.sort_by(|left, right| left.id.cmp(&right.id));
    entries.extend(extras);
    entries
}

pub(crate) fn collab_spawn_error(err: CodexErr) -> FunctionCallError {
    match err {
        CodexErr::UnsupportedOperation(message) if message == "thread manager dropped" => {
            FunctionCallError::RespondToModel("collab manager unavailable".to_string())
        }
        CodexErr::UnsupportedOperation(message) => FunctionCallError::RespondToModel(message),
        err => FunctionCallError::RespondToModel(format!("collab spawn failed: {err}")),
    }
}

pub(crate) fn collab_agent_error(agent_id: ThreadId, err: CodexErr) -> FunctionCallError {
    match err {
        CodexErr::ThreadNotFound(id) => {
            FunctionCallError::RespondToModel(format!("agent with id {id} not found"))
        }
        CodexErr::InternalAgentDied => {
            FunctionCallError::RespondToModel(format!("agent with id {agent_id} is closed"))
        }
        CodexErr::UnsupportedOperation(_) => {
            FunctionCallError::RespondToModel("collab manager unavailable".to_string())
        }
        err => FunctionCallError::RespondToModel(format!("collab tool failed: {err}")),
    }
}

pub(crate) fn thread_spawn_source(
    parent_thread_id: ThreadId,
    parent_session_source: &SessionSource,
    depth: i32,
    agent_role: Option<&str>,
    task_name: Option<String>,
) -> Result<SessionSource, FunctionCallError> {
    let agent_path = task_name
        .as_deref()
        .map(|task_name| {
            parent_session_source
                .get_agent_path()
                .unwrap_or_else(AgentPath::root)
                .join(task_name)
                .map_err(FunctionCallError::RespondToModel)
        })
        .transpose()?;
    Ok(SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
        parent_thread_id,
        depth,
        agent_path,
        agent_nickname: None,
        agent_role: agent_role.map(str::to_string),
    }))
}

pub(crate) fn parse_collab_input(
    message: Option<String>,
    items: Option<Vec<UserInput>>,
) -> Result<Op, FunctionCallError> {
    match (message, items) {
        (Some(_), Some(_)) => Err(FunctionCallError::RespondToModel(
            "Provide either message or items, but not both".to_string(),
        )),
        (None, None) => Err(FunctionCallError::RespondToModel(
            "Provide one of: message or items".to_string(),
        )),
        (Some(message), None) => {
            if message.trim().is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "Empty message can't be sent to an agent".to_string(),
                ));
            }
            Ok(vec![UserInput::Text {
                text: message,
                text_elements: Vec::new(),
            }]
            .into())
        }
        (None, Some(items)) => {
            if items.is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "Items can't be empty".to_string(),
                ));
            }
            Ok(items.into())
        }
    }
}

pub(crate) fn validate_delegation_report(
    report: &DelegationReport,
) -> Result<(), FunctionCallError> {
    for (field_name, value) in [
        ("general_task_type", report.general_task_type.trim()),
        ("why_this_agent", report.why_this_agent.trim()),
        ("expected_output_shape", report.expected_output_shape.trim()),
        ("files_or_scope", report.files_or_scope.trim()),
        ("risks_or_unknowns", report.risks_or_unknowns.trim()),
    ] {
        if value.is_empty() {
            return Err(FunctionCallError::RespondToModel(format!(
                "delegation_report.{field_name} must be a non-empty string"
            )));
        }
    }

    for (field_name, value) in [
        ("task_difficulty_1_10", report.task_difficulty_1_10),
        ("brief_completeness_1_10", report.brief_completeness_1_10),
        (
            "task_self_sufficiency_1_10",
            report.task_self_sufficiency_1_10,
        ),
    ] {
        if !(1..=10).contains(&value) {
            return Err(FunctionCallError::RespondToModel(format!(
                "delegation_report.{field_name} must be between 1 and 10"
            )));
        }
    }
    if report.expected_duration_minutes == 0 {
        return Err(FunctionCallError::RespondToModel(
            "delegation_report.expected_duration_minutes must be greater than 0".to_string(),
        ));
    }
    if let Some(orchestration_context) = &report.orchestration_context {
        validate_delegation_orchestration_context(orchestration_context)?;
    }

    Ok(())
}

pub(crate) fn validate_spawn_delegation_report(
    report: &DelegationReport,
) -> Result<(), FunctionCallError> {
    validate_delegation_report(report)
}

fn validate_delegation_orchestration_context(
    orchestration_context: &DelegationOrchestrationContext,
) -> Result<(), FunctionCallError> {
    validate_optional_non_empty_string(
        "delegation_report.orchestration_context.action_type",
        orchestration_context.action_type.as_deref(),
    )?;
    validate_optional_non_empty_string(
        "delegation_report.orchestration_context.production_type",
        orchestration_context.production_type.as_deref(),
    )?;
    validate_optional_non_empty_string(
        "delegation_report.orchestration_context.execution_lane",
        orchestration_context.execution_lane.as_deref(),
    )?;
    validate_optional_pattern(
        "delegation_report.orchestration_context.campaign_id",
        orchestration_context.campaign_id.as_deref(),
        |value| {
            !value.is_empty()
                && value
                    .chars()
                    .all(|character| character.is_ascii_uppercase())
        },
        "`^[A-Z]+$`",
    )?;
    validate_optional_pattern(
        "delegation_report.orchestration_context.phase_id",
        orchestration_context.phase_id.as_deref(),
        |value| value.len() >= 2 && value.chars().all(|character| character.is_ascii_digit()),
        "`^\\d{2,}$`",
    )?;
    validate_optional_pattern(
        "delegation_report.orchestration_context.round_id",
        orchestration_context.round_id.as_deref(),
        is_valid_round_or_step_id,
        "`^[A-Za-z0-9._:-]{1,64}$`",
    )?;
    validate_optional_pattern(
        "delegation_report.orchestration_context.step_id",
        orchestration_context.step_id.as_deref(),
        is_valid_round_or_step_id,
        "`^[A-Za-z0-9._:-]{1,64}$`",
    )?;

    Ok(())
}

fn validate_optional_non_empty_string(
    field_name: &str,
    value: Option<&str>,
) -> Result<(), FunctionCallError> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.trim().is_empty() {
        return Err(FunctionCallError::RespondToModel(format!(
            "{field_name} must be a non-empty string when provided"
        )));
    }
    Ok(())
}

fn validate_optional_pattern(
    field_name: &str,
    value: Option<&str>,
    matcher: impl Fn(&str) -> bool,
    expected_pattern: &str,
) -> Result<(), FunctionCallError> {
    let Some(value) = value else {
        return Ok(());
    };
    if !matcher(value) {
        return Err(FunctionCallError::RespondToModel(format!(
            "{field_name} must match {expected_pattern}"
        )));
    }
    Ok(())
}

fn is_valid_round_or_step_id(value: &str) -> bool {
    let length = value.len();
    if !(1..=64).contains(&length) {
        return false;
    }
    value.chars().all(|character| {
        character.is_ascii_alphanumeric()
            || character == '.'
            || character == '_'
            || character == ':'
            || character == '-'
    })
}

#[derive(Serialize)]
struct SpawnDelegationContextOrchestration<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    action_type: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    production_type: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    campaign_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    round_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    step_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_lane: Option<&'a str>,
}

#[derive(Serialize)]
struct SpawnDelegationContextReport<'a> {
    general_task_type: &'a str,
    task_difficulty_1_10: u8,
    brief_completeness_1_10: u8,
    task_self_sufficiency_1_10: u8,
    expected_duration_minutes: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    why_this_agent: Option<&'a str>,
    expected_output_shape: &'a str,
    files_or_scope: &'a str,
    risks_or_unknowns: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    orchestration_context: Option<SpawnDelegationContextOrchestration<'a>>,
}

#[derive(Debug)]
struct SpawnDelegationRouterSettings {
    cwd: PathBuf,
    module: String,
    python_bin: String,
    timeout: Duration,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SpawnDelegationRouterRequest {
    thread_id: String,
    session_source: String,
    prompt: String,
    orchestration_context: DelegationOrchestrationContext,
    delegation_report: DelegationReport,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpawnDelegationRouterResponse {
    ok: bool,
    error: Option<String>,
    message: Option<String>,
    block_text: Option<String>,
}

fn first_non_empty_env(keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn normalize_runtime_state_control_module(module: String) -> String {
    if module == NERO_RUNTIME_STATE_CONTROL_RETIRED_MODULE {
        return NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE.to_string();
    }
    module
}

fn resolve_spawn_delegation_router_module(raw_module: Option<String>) -> (String, bool) {
    match raw_module {
        Some(module) => (normalize_runtime_state_control_module(module), true),
        None => (NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE.to_string(), false),
    }
}

fn resolve_spawn_delegation_router_module_from_env() -> (String, bool) {
    let raw_module = first_non_empty_env(&[
        NERO_RUNTIME_STATE_CONTROL_MODULE_ENV,
        NERO_RUNTIME_STATE_CONTROL_MODULE_ENV_COMPAT,
    ]);
    resolve_spawn_delegation_router_module(raw_module)
}

fn resolve_spawn_delegation_router_default_cwd_with_inputs(
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
        "spawn_agent runtime bridge bootstrap failed: no nero_hook_runtime package found under [{searched}]; set {NERO_RUNTIME_STATE_CONTROL_CWD_ENV} or {CODEXN_ROOT_ENV}"
    ))
}

fn resolve_spawn_delegation_router_default_cwd() -> Result<PathBuf, String> {
    let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    resolve_spawn_delegation_router_default_cwd_with_inputs(
        &current_dir,
        first_non_empty_env(&[CODEXN_ROOT_ENV]).as_deref(),
    )
}

fn resolve_spawn_delegation_router_settings() -> Result<SpawnDelegationRouterSettings, String> {
    let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (module, module_overridden) = resolve_spawn_delegation_router_module_from_env();
    let configured_cwd = first_non_empty_env(&[
        NERO_RUNTIME_STATE_CONTROL_CWD_ENV,
        NERO_RUNTIME_STATE_CONTROL_CWD_ENV_COMPAT,
    ])
    .map(PathBuf::from);
    let cwd = match configured_cwd {
        Some(path) => path,
        None => {
            if module_overridden {
                current_dir
            } else {
                resolve_spawn_delegation_router_default_cwd()?
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
    Ok(SpawnDelegationRouterSettings {
        cwd,
        module,
        python_bin,
        timeout: Duration::from_millis(timeout_ms),
    })
}

async fn render_spawn_orchestration_router_block(
    request: &SpawnDelegationRouterRequest,
) -> Result<String, FunctionCallError> {
    let settings =
        resolve_spawn_delegation_router_settings().map_err(FunctionCallError::RespondToModel)?;
    let bridge_input = serde_json::to_vec(request).map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "spawn_agent failed to serialize orchestration router payload: {err}"
        ))
    })?;
    let mut command = Command::new(&settings.python_bin);
    command
        .current_dir(&settings.cwd)
        .kill_on_drop(true)
        .arg("-m")
        .arg(&settings.module)
        .arg(SPAWN_DELEGATION_ROUTER_COMMAND)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "spawn_agent failed to spawn orchestration router command: {err}"
        ))
    })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        FunctionCallError::RespondToModel(
            "spawn_agent orchestration router stdin unavailable".to_string(),
        )
    })?;
    stdin.write_all(&bridge_input).await.map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "spawn_agent failed to write orchestration router payload: {err}"
        ))
    })?;
    drop(stdin);

    let output = tokio::time::timeout(settings.timeout, child.wait_with_output())
        .await
        .map_err(|_| {
            FunctionCallError::RespondToModel(format!(
                "spawn_agent orchestration router timed out after {}ms",
                settings.timeout.as_millis()
            ))
        })?
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "spawn_agent failed to wait for orchestration router response: {err}"
            ))
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() {
        return Err(FunctionCallError::RespondToModel(format!(
            "spawn_agent orchestration router command failed with exit status {}",
            output.status
        )));
    }
    if stdout.is_empty() {
        return Err(FunctionCallError::RespondToModel(format!(
            "spawn_agent orchestration router returned empty stdout (exit status {})",
            output.status
        )));
    }

    let response: SpawnDelegationRouterResponse = serde_json::from_str(&stdout).map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "spawn_agent failed to parse orchestration router payload: {err}"
        ))
    })?;

    if !response.ok {
        let detail = response
            .message
            .or(response.error)
            .unwrap_or_else(|| "router reported failure".to_string());
        return Err(FunctionCallError::RespondToModel(format!(
            "spawn_agent orchestration router rejected request: {detail}"
        )));
    }
    let block_text = response
        .block_text
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(
                "spawn_agent orchestration router did not return `blockText`".to_string(),
            )
        })?;

    ensure_spawn_delegation_context_block_tag(block_text)
}

fn ensure_spawn_delegation_context_block_tag(
    block_text: &str,
) -> Result<String, FunctionCallError> {
    let trimmed = block_text.trim();
    if trimmed.is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "spawn_agent orchestration router returned an empty block payload".to_string(),
        ));
    }
    let open_tag = format!("<{SPAWN_DELEGATION_CONTEXT_BLOCK_TAG}>");
    let close_tag = format!("</{SPAWN_DELEGATION_CONTEXT_BLOCK_TAG}>");
    let open_count = trimmed.matches(&open_tag).count();
    let close_count = trimmed.matches(&close_tag).count();
    match (open_count, close_count) {
        (0, 0) => Ok(format!("\n\n{open_tag}\n{trimmed}\n{close_tag}")),
        (1, 1) => {
            if !trimmed.starts_with(&open_tag) || !trimmed.ends_with(&close_tag) {
                return Err(FunctionCallError::RespondToModel(
                    "spawn_agent orchestration router returned inconsistent delegation block tags"
                        .to_string(),
                ));
            }
            let Some(open_index) = trimmed.find(&open_tag) else {
                return Err(FunctionCallError::RespondToModel(
                    "spawn_agent orchestration router returned malformed tagged block".to_string(),
                ));
            };
            let Some(close_index) = trimmed.rfind(&close_tag) else {
                return Err(FunctionCallError::RespondToModel(
                    "spawn_agent orchestration router returned malformed tagged block".to_string(),
                ));
            };
            if open_index >= close_index {
                return Err(FunctionCallError::RespondToModel(
                    "spawn_agent orchestration router returned malformed tagged block".to_string(),
                ));
            }
            Ok(format!("\n\n{trimmed}"))
        }
        _ => Err(FunctionCallError::RespondToModel(
            "spawn_agent orchestration router returned inconsistent delegation block tags"
                .to_string(),
        )),
    }
}

fn format_spawn_delegation_context_block(
    report: &DelegationReport,
    profile: SpawnDelegationReportProfile,
) -> Result<String, FunctionCallError> {
    // `why_this_agent` is telemetry/operator-facing and excluded by default from child prompt
    // enrichment to avoid injecting orchestrator rationale into task execution context.
    // `orchestration_context` is also excluded by default and can be enabled via profile.
    let orchestration_context = if profile.forward_orchestration_context() {
        report
            .orchestration_context
            .as_ref()
            .map(|context| SpawnDelegationContextOrchestration {
                action_type: context.action_type.as_deref(),
                production_type: context.production_type.as_deref(),
                campaign_id: context.campaign_id.as_deref(),
                phase_id: context.phase_id.as_deref(),
                round_id: context.round_id.as_deref(),
                step_id: context.step_id.as_deref(),
                execution_lane: context.execution_lane.as_deref(),
            })
    } else {
        None
    };
    let child_context = SpawnDelegationContextReport {
        general_task_type: report.general_task_type.as_str(),
        task_difficulty_1_10: report.task_difficulty_1_10,
        brief_completeness_1_10: report.brief_completeness_1_10,
        task_self_sufficiency_1_10: report.task_self_sufficiency_1_10,
        expected_duration_minutes: report.expected_duration_minutes,
        why_this_agent: profile
            .forward_why_this_agent()
            .then_some(report.why_this_agent.as_str()),
        expected_output_shape: report.expected_output_shape.as_str(),
        files_or_scope: report.files_or_scope.as_str(),
        risks_or_unknowns: report.risks_or_unknowns.as_str(),
        orchestration_context,
    };
    let json_body = serde_json::to_string_pretty(&child_context).map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "failed to serialize spawn delegation_report context: {err}"
        ))
    })?;
    Ok(format!(
        "\n\n<{SPAWN_DELEGATION_CONTEXT_BLOCK_TAG}>\n{json_body}\n</{SPAWN_DELEGATION_CONTEXT_BLOCK_TAG}>"
    ))
}

pub(crate) async fn build_spawn_delegation_context_block(
    session: &Session,
    turn: &TurnContext,
    prompt: &str,
    delegation_report: Option<&DelegationReport>,
    profile: SpawnDelegationReportProfile,
) -> Result<Option<String>, FunctionCallError> {
    if !profile.forward_in_spawn() {
        return Ok(None);
    }
    let Some(report) = delegation_report else {
        return Ok(None);
    };
    if profile.forward_via_orchestration_router() {
        let orchestration_context = report.orchestration_context.clone().ok_or_else(|| {
            FunctionCallError::RespondToModel(
                "spawn_agent requires delegation_report.orchestration_context when spawn_delegation_report_profile is `orchestration_router_block`".to_string(),
            )
        })?;
        let request = SpawnDelegationRouterRequest {
            thread_id: session.conversation_id.to_string(),
            session_source: turn.session_source.to_string(),
            prompt: prompt.to_string(),
            orchestration_context,
            delegation_report: report.clone(),
        };
        return render_spawn_orchestration_router_block(&request)
            .await
            .map(Some);
    }

    format_spawn_delegation_context_block(report, profile).map(Some)
}

pub(crate) fn build_follow_up_delegation_context_block(
    delegation_report: Option<&DelegationReport>,
    profile: SpawnDelegationReportProfile,
) -> Result<Option<String>, FunctionCallError> {
    if !profile.forward_in_spawn() {
        return Ok(None);
    }
    let Some(report) = delegation_report else {
        return Ok(None);
    };
    format_spawn_delegation_context_block(report, profile).map(Some)
}

pub(crate) fn inject_spawn_delegation_context_block(
    input: Op,
    context_block: Option<String>,
) -> Op {
    let Some(context_block) = context_block else {
        return input;
    };
    match input {
        Op::UserInput {
            mut items,
            final_output_json_schema,
        } => {
            items.push(UserInput::Text {
                text: context_block,
                text_elements: Vec::new(),
            });
            Op::UserInput {
                items,
                final_output_json_schema,
            }
        }
        other => other,
    }
}

pub(crate) fn append_delegation_context_block_to_text(
    content: String,
    context_block: Option<String>,
) -> String {
    match context_block {
        Some(context_block) => format!("{content}{context_block}"),
        None => content,
    }
}

pub(crate) struct SpawnContextInheritanceResolution {
    pub(crate) resolved_mode: SpawnContextInheritanceMode,
    pub(crate) requested_mode: Option<SpawnContextInheritanceMode>,
}

pub(crate) fn resolve_spawn_context_inheritance_mode(
    fork_context: Option<bool>,
    context_inheritance: Option<SpawnContextInheritanceMode>,
    _session_source: &SessionSource,
) -> Result<SpawnContextInheritanceResolution, FunctionCallError> {
    if fork_context.is_some() || context_inheritance.is_some() {
        return Err(FunctionCallError::RespondToModel(
            "spawn_agent context inheritance is disabled; do not pass fork_context or context_inheritance".to_string(),
        ));
    }

    Ok(SpawnContextInheritanceResolution {
        resolved_mode: SpawnContextInheritanceMode::Off,
        requested_mode: None,
    })
}

pub(crate) async fn resolve_spawn_bounded_fork_budget_proxy_tokens(
    session: &Session,
    config: &Config,
) -> Option<i64> {
    let model = session
        .services
        .models_manager
        .get_default_model(&config.model, RefreshStrategy::Offline)
        .await;
    if model.is_empty() {
        return None;
    }
    let auto_compact_token_limit = session
        .services
        .models_manager
        .get_model_info(&model, config)
        .await
        .auto_compact_token_limit()?;
    let usable_budget_tokens = auto_compact_token_limit
        .saturating_sub(config.agent_bounded_fork_startup_reserve_tokens)
        .max(0);
    Some(usable_budget_tokens)
}

/// Builds the base config snapshot for a newly spawned sub-agent.
///
/// The returned config starts from the parent's effective config and then refreshes the
/// runtime-owned fields carried on `turn`, including model selection, reasoning settings,
/// approval policy, sandbox, and cwd. Role-specific overrides are layered after this step;
/// skipping this helper and cloning stale config state directly can send the child agent out with
/// the wrong provider or runtime policy.
pub(crate) fn build_agent_spawn_config(
    base_instructions: &BaseInstructions,
    turn: &TurnContext,
) -> Result<Config, FunctionCallError> {
    let mut config = build_agent_shared_config(turn)?;
    crate::config::strip_codexn_fork_subagent_developer_instructions(&mut config).map_err(
        |err| {
            FunctionCallError::Fatal(format!(
                "failed to prepare subagent developer instructions for spawn: {err}"
            ))
        },
    )?;
    config.base_instructions = Some(base_instructions.text.clone());
    Ok(config)
}

pub(crate) fn build_agent_resume_config(
    turn: &TurnContext,
    child_depth: i32,
) -> Result<Config, FunctionCallError> {
    let mut config = build_agent_shared_config(turn)?;
    crate::config::strip_codexn_fork_subagent_developer_instructions(&mut config).map_err(
        |err| {
            FunctionCallError::Fatal(format!(
                "failed to prepare subagent developer instructions for resume: {err}"
            ))
        },
    )?;
    apply_spawn_agent_overrides(&mut config, child_depth);
    // For resume, keep base instructions sourced from rollout/session metadata.
    config.base_instructions = None;
    Ok(config)
}

fn build_agent_shared_config(turn: &TurnContext) -> Result<Config, FunctionCallError> {
    let base_config = turn.config.clone();
    let mut config = (*base_config).clone();
    config.model = Some(turn.model_info.slug.clone());
    config.model_provider = turn.provider.clone();
    config.model_reasoning_effort = turn.reasoning_effort;
    config.model_reasoning_summary = Some(turn.reasoning_summary);
    config.developer_instructions = turn.developer_instructions.clone();
    config.compact_prompt = turn.compact_prompt.clone();
    apply_spawn_agent_runtime_overrides(&mut config, turn)?;

    Ok(config)
}

/// Copies runtime-only turn state onto a child config before it is handed to `AgentControl`.
///
/// These values are chosen by the live turn rather than persisted config, so leaving them stale
/// can make a child agent disagree with its parent about approval policy, cwd, or sandboxing.
pub(crate) fn apply_spawn_agent_runtime_overrides(
    config: &mut Config,
    turn: &TurnContext,
) -> Result<(), FunctionCallError> {
    config
        .permissions
        .approval_policy
        .set(turn.approval_policy.value())
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("approval_policy is invalid: {err}"))
        })?;
    config.permissions.shell_environment_policy = turn.shell_environment_policy.clone();
    config.codex_linux_sandbox_exe = turn.codex_linux_sandbox_exe.clone();
    config.cwd = turn.cwd.clone();
    config
        .permissions
        .sandbox_policy
        .set(turn.sandbox_policy.get().clone())
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("sandbox_policy is invalid: {err}"))
        })?;
    config.permissions.file_system_sandbox_policy = turn.file_system_sandbox_policy.clone();
    config.permissions.network_sandbox_policy = turn.network_sandbox_policy;
    Ok(())
}

pub(crate) fn apply_spawn_agent_overrides(config: &mut Config, child_depth: i32) {
    if child_depth >= config.agent_max_depth && !config.features.enabled(Feature::MultiAgentV2) {
        let _ = config.features.disable(Feature::SpawnCsv);
        let _ = config.features.disable(Feature::Collab);
    }
}

pub(crate) async fn apply_requested_spawn_agent_model_overrides(
    session: &Session,
    turn: &TurnContext,
    config: &mut Config,
    requested_model: Option<&str>,
    requested_reasoning_effort: Option<ReasoningEffort>,
) -> Result<(), FunctionCallError> {
    if requested_model.is_none() && requested_reasoning_effort.is_none() {
        return Ok(());
    }

    if let Some(requested_model) = requested_model {
        let available_models = session
            .services
            .models_manager
            .list_models(RefreshStrategy::Offline)
            .await;
        let selected_model_name = find_spawn_agent_model_name(&available_models, requested_model)?;
        let selected_model_info = session
            .services
            .models_manager
            .get_model_info(&selected_model_name, config)
            .await;

        config.model = Some(selected_model_name.clone());
        if let Some(reasoning_effort) = requested_reasoning_effort {
            validate_spawn_agent_reasoning_effort(
                &selected_model_name,
                &selected_model_info.supported_reasoning_levels,
                reasoning_effort,
            )?;
            config.model_reasoning_effort = Some(reasoning_effort);
        } else {
            config.model_reasoning_effort = selected_model_info.default_reasoning_level;
        }

        return Ok(());
    }

    if let Some(reasoning_effort) = requested_reasoning_effort {
        validate_spawn_agent_reasoning_effort(
            &turn.model_info.slug,
            &turn.model_info.supported_reasoning_levels,
            reasoning_effort,
        )?;
        config.model_reasoning_effort = Some(reasoning_effort);
    }

    Ok(())
}

fn find_spawn_agent_model_name(
    available_models: &[codex_protocol::openai_models::ModelPreset],
    requested_model: &str,
) -> Result<String, FunctionCallError> {
    available_models
        .iter()
        .find(|model| model.model == requested_model)
        .map(|model| model.model.clone())
        .ok_or_else(|| {
            let available = available_models
                .iter()
                .map(|model| model.model.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            FunctionCallError::RespondToModel(format!(
                "Unknown model `{requested_model}` for spawn_agent. Available models: {available}"
            ))
        })
}

fn validate_spawn_agent_reasoning_effort(
    model: &str,
    supported_reasoning_levels: &[ReasoningEffortPreset],
    requested_reasoning_effort: ReasoningEffort,
) -> Result<(), FunctionCallError> {
    if supported_reasoning_levels
        .iter()
        .any(|preset| preset.effort == requested_reasoning_effort)
    {
        return Ok(());
    }

    let supported = supported_reasoning_levels
        .iter()
        .map(|preset| preset.effort.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(FunctionCallError::RespondToModel(format!(
        "Reasoning effort `{requested_reasoning_effort}` is not supported for model `{model}`. Supported reasoning efforts: {supported}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn validate_delegation_orchestration_context_rejects_empty_campaign_id() {
        let context = DelegationOrchestrationContext {
            action_type: None,
            production_type: None,
            campaign_id: Some(String::new()),
            phase_id: None,
            round_id: None,
            step_id: None,
            execution_lane: None,
        };

        let err = validate_delegation_orchestration_context(&context)
            .expect_err("empty campaign_id should be rejected");

        assert_eq!(
            err,
            FunctionCallError::RespondToModel(
                "delegation_report.orchestration_context.campaign_id must match `^[A-Z]+$`"
                    .to_string()
            )
        );
    }

    #[test]
    fn normalize_runtime_state_control_module_maps_retired_alias_to_default() {
        assert_eq!(
            normalize_runtime_state_control_module(
                NERO_RUNTIME_STATE_CONTROL_RETIRED_MODULE.to_string()
            ),
            NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE.to_string()
        );
        assert_eq!(
            normalize_runtime_state_control_module("custom.module".to_string()),
            "custom.module".to_string()
        );
    }

    #[test]
    fn resolve_spawn_delegation_router_module_marks_override_correctly() {
        assert_eq!(
            resolve_spawn_delegation_router_module(None),
            (NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE.to_string(), false)
        );
        assert_eq!(
            resolve_spawn_delegation_router_module(Some("x.y".to_string())),
            ("x.y".to_string(), true)
        );
    }

    #[test]
    fn resolve_spawn_delegation_router_default_cwd_uses_codexn_root_candidate() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let codexn_root = tempdir.path().join("worktree");
        let sdk_root = codexn_root.join("apps/codex-nero-sdk");
        std::fs::create_dir_all(sdk_root.join("nero_hook_runtime")).expect("mkdir runtime");
        let unrelated_current_dir = tempdir.path().join("other/current");
        std::fs::create_dir_all(&unrelated_current_dir).expect("mkdir current");

        let resolved = resolve_spawn_delegation_router_default_cwd_with_inputs(
            &unrelated_current_dir,
            Some(codexn_root.to_str().expect("utf8 path")),
        )
        .expect("resolver should use codexn root candidate");

        assert_eq!(resolved, sdk_root);
    }

    #[test]
    fn resolve_spawn_delegation_router_default_cwd_errors_when_package_missing() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let current_dir = tempdir.path().join("current");
        std::fs::create_dir_all(&current_dir).expect("mkdir current");

        let err = resolve_spawn_delegation_router_default_cwd_with_inputs(&current_dir, None)
            .expect_err("resolver should fail without nero_hook_runtime package");

        assert!(
            err.contains("no nero_hook_runtime package found"),
            "unexpected error: {err}"
        );
        assert!(
            err.contains(NERO_RUNTIME_STATE_CONTROL_CWD_ENV),
            "unexpected error: {err}"
        );
        assert!(err.contains(CODEXN_ROOT_ENV), "unexpected error: {err}");
    }

    #[test]
    fn ensure_spawn_delegation_context_block_tag_wraps_untagged_payload() {
        let output = ensure_spawn_delegation_context_block_tag("{\"k\":1}")
            .expect("untagged payload should be wrapped");
        assert!(output.starts_with("\n\n<spawn_delegation_report_json>"));
        assert!(output.contains("\n{\"k\":1}\n"));
        assert!(output.ends_with("</spawn_delegation_report_json>"));
    }

    #[test]
    fn ensure_spawn_delegation_context_block_tag_is_idempotent_for_tagged_payload() {
        let tagged = "<spawn_delegation_report_json>\n{\"k\":1}\n</spawn_delegation_report_json>";
        let output = ensure_spawn_delegation_context_block_tag(tagged)
            .expect("already tagged payload should pass through");
        assert_eq!(output, format!("\n\n{tagged}"));
    }

    #[test]
    fn ensure_spawn_delegation_context_block_tag_rejects_inconsistent_tags() {
        let err =
            ensure_spawn_delegation_context_block_tag("<spawn_delegation_report_json>{\"k\":1}")
                .expect_err("inconsistent tag counts should be rejected");
        assert_eq!(
            err,
            FunctionCallError::RespondToModel(
                "spawn_agent orchestration router returned inconsistent delegation block tags"
                    .to_string()
            )
        );
    }

    #[test]
    fn ensure_spawn_delegation_context_block_tag_rejects_prefix_or_suffix_text() {
        let with_prefix =
            "prefix\n<spawn_delegation_report_json>{\"k\":1}</spawn_delegation_report_json>";
        let err = ensure_spawn_delegation_context_block_tag(with_prefix)
            .expect_err("prefix text should be rejected for tagged payload");
        assert_eq!(
            err,
            FunctionCallError::RespondToModel(
                "spawn_agent orchestration router returned inconsistent delegation block tags"
                    .to_string()
            )
        );

        let with_suffix =
            "<spawn_delegation_report_json>{\"k\":1}</spawn_delegation_report_json>\nsuffix";
        let err = ensure_spawn_delegation_context_block_tag(with_suffix)
            .expect_err("suffix text should be rejected for tagged payload");
        assert_eq!(
            err,
            FunctionCallError::RespondToModel(
                "spawn_agent orchestration router returned inconsistent delegation block tags"
                    .to_string()
            )
        );
    }
}

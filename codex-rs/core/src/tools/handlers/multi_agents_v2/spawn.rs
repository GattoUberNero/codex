use super::*;
use crate::agent::control::SpawnAgentOptions;
use crate::agent::control::render_input_preview;
use crate::agent::fork_context::SpawnContextInheritanceReport;
use crate::agent::fork_context::should_fork_parent_context;
use crate::agent::next_thread_spawn_depth;
use crate::agent::role::DEFAULT_ROLE_NAME;
use crate::agent::role::apply_role_to_config;
use codex_protocol::AgentPath;
use codex_protocol::protocol::DelegationReport;
use codex_protocol::protocol::InterAgentCommunication;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::SpawnContextInheritanceEffectiveMode;
use codex_protocol::protocol::SpawnContextInheritanceMode;
use codex_protocol::protocol::SpawnContextInheritanceTelemetry;

pub(crate) struct Handler;

#[async_trait]
impl ToolHandler for Handler {
    type Output = SpawnAgentResult;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            payload,
            call_id,
            ..
        } = invocation;
        let arguments = function_arguments(payload)?;
        let args: SpawnAgentArgs = parse_arguments(&arguments)?;
        let spawn_delegation_profile = turn.config.spawn_delegation_report_profile;
        let delegation_report = args.delegation_report;
        if spawn_delegation_profile.required_in_spawn() && delegation_report.is_none() {
            return Err(FunctionCallError::RespondToModel(format!(
                "spawn_agent requires delegation_report when spawn_delegation_report_profile is `{}`",
                spawn_delegation_profile.as_config_key()
            )));
        }
        if let Some(report) = delegation_report.as_ref() {
            validate_spawn_delegation_report(report)?;
        }
        let delegation_report_for_ui = if spawn_delegation_profile.render_in_ui() {
            delegation_report.clone()
        } else {
            None
        };
        let context_inheritance_resolution = resolve_spawn_context_inheritance_mode(
            args.fork_context,
            args.context_inheritance,
            &turn.session_source,
        )?;
        let requested_context_inheritance = context_inheritance_resolution.resolved_mode;
        let role_name = args
            .agent_type
            .as_deref()
            .map(str::trim)
            .filter(|role| !role.is_empty());
        let requested_model = args.model.clone().unwrap_or_default();
        let requested_reasoning_effort = args.reasoning_effort.unwrap_or_default();

        let initial_operation = parse_collab_input(args.message, args.items)?;
        let prompt = render_input_preview(&initial_operation);
        let session_source = turn.session_source.clone();
        let child_depth = next_thread_spawn_depth(&session_source);
        let max_depth = turn.config.agent_max_depth;
        if exceeds_thread_spawn_depth_limit(child_depth, max_depth) {
            return Err(FunctionCallError::RespondToModel(
                "Agent depth limit reached. Solve the task yourself.".to_string(),
            ));
        }
        let context_block = build_spawn_delegation_context_block(
            &session,
            &turn,
            &prompt,
            delegation_report.as_ref(),
            spawn_delegation_profile,
        )
        .await?;
        let initial_operation =
            inject_spawn_delegation_context_block(initial_operation, context_block);
        let spawn_content = render_input_preview(&initial_operation);

        session
            .send_event(
                &turn,
                CollabAgentSpawnBeginEvent {
                    call_id: call_id.clone(),
                    sender_thread_id: session.conversation_id,
                    prompt: prompt.clone(),
                    model: requested_model.clone(),
                    reasoning_effort: requested_reasoning_effort,
                    context_inheritance_requested: context_inheritance_resolution.requested_mode,
                    delegation_report: delegation_report_for_ui.clone(),
                }
                .into(),
            )
            .await;
        let mut config =
            build_agent_spawn_config(&session.get_base_instructions().await, turn.as_ref())?;
        apply_requested_spawn_agent_model_overrides(
            &session,
            turn.as_ref(),
            &mut config,
            args.model.as_deref(),
            args.reasoning_effort,
        )
        .await?;
        apply_role_to_config(&mut config, role_name)
            .await
            .map_err(FunctionCallError::RespondToModel)?;
        apply_spawn_agent_runtime_overrides(&mut config, turn.as_ref())?;
        apply_spawn_agent_overrides(&mut config, child_depth);
        let bounded_fork_usable_context_budget_tokens =
            if requested_context_inheritance == SpawnContextInheritanceMode::Bounded {
                resolve_spawn_bounded_fork_budget_proxy_tokens(&session, &config).await
            } else {
                None
            };

        let spawn_source = thread_spawn_source(
            session.conversation_id,
            &turn.session_source,
            child_depth,
            role_name,
            Some(args.task_name.clone()),
        )?;
        let result = session
            .services
            .agent_control
            .spawn_agent_with_metadata(
                config,
                match (spawn_source.get_agent_path(), initial_operation) {
                    (Some(recipient), Op::UserInput { items, .. })
                        if items
                            .iter()
                            .all(|item| matches!(item, UserInput::Text { .. })) =>
                    {
                        Op::InterAgentCommunication {
                            communication: InterAgentCommunication::new(
                                turn.session_source
                                    .get_agent_path()
                                    .unwrap_or_else(AgentPath::root),
                                recipient,
                                Vec::new(),
                                spawn_content.clone(),
                                /*trigger_turn*/ true,
                            ),
                        }
                    }
                    (_, initial_operation) => initial_operation,
                },
                Some(spawn_source),
                SpawnAgentOptions {
                    fork_parent_spawn_call_id: should_fork_parent_context(
                        requested_context_inheritance,
                    )
                    .then(|| call_id.clone()),
                    fork_context_requested_mode: requested_context_inheritance,
                    bounded_fork_usable_context_budget_tokens,
                },
            )
            .await
            .map_err(collab_spawn_error);
        let (new_thread_id, new_agent_metadata, status, fork_context_report) = match &result {
            Ok(spawned_agent) => (
                Some(spawned_agent.thread_id),
                Some(spawned_agent.metadata.clone()),
                spawned_agent.status.clone(),
                spawned_agent.fork_context_report.clone(),
            ),
            Err(_) => (
                None,
                None,
                AgentStatus::NotFound,
                SpawnContextInheritanceReport::off(requested_context_inheritance),
            ),
        };
        let agent_snapshot = match new_thread_id {
            Some(thread_id) => {
                session
                    .services
                    .agent_control
                    .get_agent_config_snapshot(thread_id)
                    .await
            }
            None => None,
        };
        let (new_agent_path, new_agent_nickname, new_agent_role) =
            match (&agent_snapshot, new_agent_metadata) {
                (Some(snapshot), _) => (
                    snapshot.session_source.get_agent_path().map(String::from),
                    snapshot.session_source.get_nickname(),
                    snapshot.session_source.get_agent_role(),
                ),
                (None, Some(metadata)) => (
                    metadata.agent_path.map(String::from),
                    metadata.agent_nickname,
                    metadata.agent_role,
                ),
                (None, None) => (None, None, None),
            };
        let effective_model = agent_snapshot
            .as_ref()
            .map(|snapshot| snapshot.model.clone())
            .unwrap_or_else(|| requested_model.clone());
        let effective_reasoning_effort = agent_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.reasoning_effort)
            .unwrap_or(requested_reasoning_effort);
        let nickname = new_agent_nickname.clone();
        session
            .send_event(
                &turn,
                CollabAgentSpawnEndEvent {
                    call_id,
                    sender_thread_id: session.conversation_id,
                    new_thread_id,
                    new_agent_nickname,
                    new_agent_role,
                    prompt,
                    requested_model,
                    requested_reasoning_effort,
                    model: effective_model,
                    reasoning_effort: effective_reasoning_effort,
                    context_inheritance_requested: context_inheritance_resolution.requested_mode,
                    delegation_report: delegation_report_for_ui.clone(),
                    context_inheritance_effective: Some(fork_context_report.effective_mode),
                    context_inheritance_telemetry: fork_context_report.telemetry.clone(),
                    status,
                }
                .into(),
            )
            .await;
        let _ = result?;
        let role_tag = role_name.unwrap_or(DEFAULT_ROLE_NAME);
        turn.session_telemetry.counter(
            "codex.multi_agent.spawn",
            /*inc*/ 1,
            &[("role", role_tag)],
        );
        let task_name = new_agent_path.ok_or_else(|| {
            FunctionCallError::RespondToModel(
                "spawned agent is missing a canonical task name".to_string(),
            )
        })?;

        Ok(SpawnAgentResult {
            agent_id: None,
            task_name,
            nickname,
            delegation_report: delegation_report_for_ui,
            context_inheritance_requested: fork_context_report.requested_mode,
            context_inheritance_effective: fork_context_report.effective_mode,
            context_inheritance_telemetry: fork_context_report.telemetry,
        })
    }
}

#[derive(Debug, Deserialize)]
struct SpawnAgentArgs {
    message: Option<String>,
    items: Option<Vec<UserInput>>,
    task_name: String,
    agent_type: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<ReasoningEffort>,
    delegation_report: Option<DelegationReport>,
    #[serde(default)]
    fork_context: Option<bool>,
    context_inheritance: Option<SpawnContextInheritanceMode>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SpawnAgentResult {
    agent_id: Option<String>,
    task_name: String,
    nickname: Option<String>,
    delegation_report: Option<DelegationReport>,
    context_inheritance_requested: SpawnContextInheritanceMode,
    context_inheritance_effective: SpawnContextInheritanceEffectiveMode,
    context_inheritance_telemetry: Option<SpawnContextInheritanceTelemetry>,
}

impl ToolOutput for SpawnAgentResult {
    fn log_preview(&self) -> String {
        tool_output_json_text(self, "spawn_agent")
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        tool_output_response_item(call_id, payload, self, Some(true), "spawn_agent")
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        tool_output_code_mode_result(self, "spawn_agent")
    }
}

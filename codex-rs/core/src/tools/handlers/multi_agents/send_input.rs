use super::*;
use crate::agent::control::render_input_preview;
use codex_protocol::protocol::DelegationReport;

pub(crate) struct Handler;

#[async_trait]
impl ToolHandler for Handler {
    type Output = SendInputResult;

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
        let SendInputArgs {
            target,
            message,
            items,
            delegation_report,
            interrupt,
        } = parse_arguments(&arguments)?;
        let receiver_thread_id = parse_agent_id_target(&target)?;
        if let Some(report) = delegation_report.as_ref() {
            validate_delegation_report(report)?;
        }
        let input_items = parse_collab_input(message, items)?;
        let prompt = render_input_preview(&input_items);
        let delegation_report_for_ui = turn
            .config
            .spawn_delegation_report_profile
            .render_in_ui()
            .then(|| delegation_report.clone())
            .flatten();
        let context_block = build_follow_up_delegation_context_block(
            delegation_report.as_ref(),
            turn.config.spawn_delegation_report_profile,
        )?;
        let input_items = inject_spawn_delegation_context_block(input_items, context_block);
        let receiver_agent = session
            .services
            .agent_control
            .get_agent_metadata(receiver_thread_id)
            .unwrap_or_default();
        if interrupt {
            session
                .services
                .agent_control
                .interrupt_agent(receiver_thread_id)
                .await
                .map_err(|err| collab_agent_error(receiver_thread_id, err))?;
        }
        session
            .send_event(
                &turn,
                CollabAgentInteractionBeginEvent {
                    call_id: call_id.clone(),
                    sender_thread_id: session.conversation_id,
                    receiver_thread_id,
                    prompt: prompt.clone(),
                    delegation_report: delegation_report_for_ui.clone(),
                }
                .into(),
            )
            .await;
        let agent_control = session.services.agent_control.clone();
        let result = agent_control
            .send_input(receiver_thread_id, input_items)
            .await
            .map_err(|err| collab_agent_error(receiver_thread_id, err));
        let status = session
            .services
            .agent_control
            .get_status(receiver_thread_id)
            .await;
        session
            .send_event(
                &turn,
                CollabAgentInteractionEndEvent {
                    call_id,
                    sender_thread_id: session.conversation_id,
                    receiver_thread_id,
                    receiver_agent_nickname: receiver_agent.agent_nickname,
                    receiver_agent_role: receiver_agent.agent_role,
                    prompt,
                    delegation_report: delegation_report_for_ui,
                    status,
                }
                .into(),
            )
            .await;
        let submission_id = result?;

        Ok(SendInputResult { submission_id })
    }
}

#[derive(Debug, Deserialize)]
struct SendInputArgs {
    target: String,
    message: Option<String>,
    items: Option<Vec<UserInput>>,
    delegation_report: Option<DelegationReport>,
    #[serde(default)]
    interrupt: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct SendInputResult {
    submission_id: String,
}

impl ToolOutput for SendInputResult {
    fn log_preview(&self) -> String {
        tool_output_json_text(self, "send_input")
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        tool_output_response_item(call_id, payload, self, Some(true), "send_input")
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        tool_output_code_mode_result(self, "send_input")
    }
}

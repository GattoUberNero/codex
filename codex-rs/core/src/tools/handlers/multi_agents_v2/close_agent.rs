use super::*;

pub(crate) struct Handler;

#[async_trait]
impl ToolHandler for Handler {
    type Output = CloseAgentResult;

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
        let args: CloseAgentArgs = parse_arguments(&arguments)?;
        let mode = args.mode.unwrap_or_default();
        let agent_id = resolve_agent_target(&session, &turn, &args.target).await?;
        let receiver_agent = session
            .services
            .agent_control
            .get_agent_metadata(agent_id)
            .unwrap_or_default();
        if receiver_agent
            .agent_path
            .as_ref()
            .is_some_and(AgentPath::is_root)
        {
            return Err(FunctionCallError::RespondToModel(
                "root is not a spawned agent".to_string(),
            ));
        }

        if matches!(mode, CloseAgentMode::SafeClose) {
            validate_safe_close_subtree(&session, agent_id).await?;
        }
        let status = session.services.agent_control.get_status(agent_id).await;

        session
            .send_event(
                &turn,
                CollabCloseBeginEvent {
                    call_id: call_id.clone(),
                    sender_thread_id: session.conversation_id,
                    receiver_thread_id: agent_id,
                }
                .into(),
            )
            .await;

        let close_error = if status == AgentStatus::NotFound {
            None
        } else {
            session
                .services
                .agent_control
                .close_agent(agent_id)
                .await
                .err()
                .map(|err| collab_agent_error(agent_id, err))
        };
        let reported_status = match &close_error {
            Some(FunctionCallError::RespondToModel(message)) => {
                AgentStatus::Errored(message.clone())
            }
            Some(err) => AgentStatus::Errored(err.to_string()),
            None => status.clone(),
        };

        session
            .send_event(
                &turn,
                CollabCloseEndEvent {
                    call_id,
                    sender_thread_id: session.conversation_id,
                    receiver_thread_id: agent_id,
                    receiver_agent_nickname: receiver_agent.agent_nickname,
                    receiver_agent_role: receiver_agent.agent_role,
                    status: reported_status,
                }
                .into(),
            )
            .await;

        if let Some(err) = close_error {
            return Err(err);
        }

        Ok(CloseAgentResult {
            previous_status: status,
        })
    }
}

#[derive(Debug, Deserialize)]
struct CloseAgentArgs {
    target: String,
    mode: Option<CloseAgentMode>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct CloseAgentResult {
    pub(crate) previous_status: AgentStatus,
}

impl ToolOutput for CloseAgentResult {
    fn log_preview(&self) -> String {
        tool_output_json_text(self, "close_agent")
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        tool_output_response_item(call_id, payload, self, Some(true), "close_agent")
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        tool_output_code_mode_result(self, "close_agent")
    }
}

use super::*;
use crate::agent::agent_resolver::resolve_agent_target;
use crate::agent::status::is_final;
use crate::error::CodexErr;
use futures::FutureExt;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch::Receiver;
use tokio::time::Instant;
use tokio::time::timeout_at;

use crate::codex::Session;
use codex_protocol::ThreadId;
use codex_protocol::protocol::CollabAgentRef;
use codex_protocol::protocol::CollabWaitOutcome;

pub(crate) struct Handler;

#[async_trait]
impl ToolHandler for Handler {
    type Output = WaitAgentResult;

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
        let args: WaitArgs = parse_arguments(&arguments)?;
        let timeout_ms =
            effective_wait_timeout_ms(args.timeout_ms.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS))?;

        if let Some(targets) = args.targets {
            if targets.is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "targets must be non-empty when provided".to_string(),
                ));
            }

            let mut receiver_thread_ids = Vec::with_capacity(targets.len());
            let mut receiver_agents = Vec::with_capacity(targets.len());
            for target in targets {
                let agent_id = resolve_agent_target(&session, &turn, &target).await?;
                let agent_metadata = session
                    .services
                    .agent_control
                    .get_agent_metadata(agent_id)
                    .unwrap_or_default();
                receiver_thread_ids.push(agent_id);
                receiver_agents.push(CollabAgentRef {
                    thread_id: agent_id,
                    agent_nickname: agent_metadata.agent_nickname,
                    agent_role: agent_metadata.agent_role,
                });
            }

            session
                .send_event(
                    &turn,
                    CollabWaitingBeginEvent {
                        sender_thread_id: session.conversation_id,
                        receiver_thread_ids: receiver_thread_ids.clone(),
                        receiver_agents: receiver_agents.clone(),
                        call_id: call_id.clone(),
                    }
                    .into(),
                )
                .await;

            let mut status_rxs = Vec::with_capacity(receiver_thread_ids.len());
            let mut initial_final_statuses = Vec::new();
            for id in &receiver_thread_ids {
                match session.services.agent_control.subscribe_status(*id).await {
                    Ok(rx) => {
                        let status = rx.borrow().clone();
                        if is_final(&status) {
                            initial_final_statuses.push((*id, status));
                        }
                        status_rxs.push((*id, rx));
                    }
                    Err(CodexErr::ThreadNotFound(_)) => {
                        initial_final_statuses.push((*id, AgentStatus::NotFound));
                    }
                    Err(err) => {
                        let mut statuses = HashMap::with_capacity(1);
                        statuses.insert(*id, session.services.agent_control.get_status(*id).await);
                        session
                            .send_event(
                                &turn,
                                CollabWaitingEndEvent {
                                    sender_thread_id: session.conversation_id,
                                    call_id: call_id.clone(),
                                    wait_outcome: None,
                                    agent_statuses: build_wait_agent_statuses(
                                        &statuses,
                                        &receiver_agents,
                                    ),
                                    statuses,
                                }
                                .into(),
                            )
                            .await;
                        return Err(collab_agent_error(*id, err));
                    }
                }
            }

            let (statuses, wait_outcome) = if !initial_final_statuses.is_empty() {
                (
                    initial_final_statuses,
                    CollabWaitOutcome::CompletionAlreadyAvailable,
                )
            } else {
                let mut futures = FuturesUnordered::new();
                for (id, rx) in status_rxs.into_iter() {
                    let session = session.clone();
                    futures.push(wait_for_final_status(session, id, rx));
                }
                let mut results = Vec::new();
                let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
                loop {
                    match timeout_at(deadline, futures.next()).await {
                        Ok(Some(Some(result))) => {
                            results.push(result);
                            break;
                        }
                        Ok(Some(None)) => continue,
                        Ok(None) | Err(_) => break,
                    }
                }
                if !results.is_empty() {
                    loop {
                        match futures.next().now_or_never() {
                            Some(Some(Some(result))) => results.push(result),
                            Some(Some(None)) => continue,
                            Some(None) | None => break,
                        }
                    }
                }
                let wait_outcome = if results.is_empty() {
                    CollabWaitOutcome::ListenWindowEnded
                } else {
                    CollabWaitOutcome::CompletionObserved
                };
                (results, wait_outcome)
            };

            let timed_out = statuses.is_empty();
            let observed_statuses_by_id = statuses.clone().into_iter().collect::<HashMap<_, _>>();
            let current_statuses =
                current_wait_agent_statuses(&session, &receiver_thread_ids).await;
            let statuses_by_id = if timed_out {
                current_statuses.clone()
            } else {
                observed_statuses_by_id
            };
            let agent_statuses = build_wait_agent_statuses(&statuses_by_id, &receiver_agents);
            let pending = build_wait_agent_pending(&current_statuses, &receiver_agents);

            session
                .send_event(
                    &turn,
                    CollabWaitingEndEvent {
                        sender_thread_id: session.conversation_id,
                        call_id,
                        wait_outcome: Some(wait_outcome),
                        agent_statuses,
                        statuses: statuses_by_id,
                    }
                    .into(),
                )
                .await;

            return Ok(WaitAgentResult::from_outcome(wait_outcome, pending));
        }

        let mut mailbox_seq_rx = session.subscribe_mailbox_seq();

        session
            .send_event(
                &turn,
                CollabWaitingBeginEvent {
                    sender_thread_id: session.conversation_id,
                    receiver_thread_ids: Vec::new(),
                    receiver_agents: Vec::new(),
                    call_id: call_id.clone(),
                }
                .into(),
            )
            .await;

        let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
        let wait_outcome = if wait_for_mailbox_change(&mut mailbox_seq_rx, deadline).await {
            CollabWaitOutcome::ActivityObserved
        } else {
            CollabWaitOutcome::ListenWindowEnded
        };
        let result = WaitAgentResult::from_outcome(wait_outcome, Vec::new());

        session
            .send_event(
                &turn,
                CollabWaitingEndEvent {
                    sender_thread_id: session.conversation_id,
                    call_id,
                    wait_outcome: Some(wait_outcome),
                    agent_statuses: Vec::new(),
                    statuses: HashMap::new(),
                }
                .into(),
            )
            .await;

        Ok(result)
    }
}

#[derive(Debug, Deserialize)]
struct WaitArgs {
    #[serde(default)]
    targets: Option<Vec<String>>,
    timeout_ms: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct WaitAgentResult {
    pub(crate) message: String,
    pub(crate) pending: Vec<WaitPendingAgent>,
    pub(crate) timed_out: bool,
    pub(crate) wait_outcome: CollabWaitOutcome,
}

impl WaitAgentResult {
    fn from_outcome(wait_outcome: CollabWaitOutcome, pending: Vec<WaitPendingAgent>) -> Self {
        let pending_count = pending.len();
        let message = match wait_outcome {
            CollabWaitOutcome::CompletionAlreadyAvailable if pending_count == 0 => {
                "Completion was already available before this wait call.".to_string()
            }
            CollabWaitOutcome::CompletionAlreadyAvailable if pending_count == 1 => {
                "Completion already available; 1 target remains pending.".to_string()
            }
            CollabWaitOutcome::CompletionAlreadyAvailable => {
                format!("Completion already available; {pending_count} targets remain pending.")
            }
            CollabWaitOutcome::CompletionObserved if pending_count == 0 => {
                "Observed completion.".to_string()
            }
            CollabWaitOutcome::CompletionObserved if pending_count == 1 => {
                "Observed first completion; 1 target remains pending.".to_string()
            }
            CollabWaitOutcome::CompletionObserved => {
                format!("Observed first completion; {pending_count} targets remain pending.")
            }
            CollabWaitOutcome::ActivityObserved => "Observed activity.".to_string(),
            CollabWaitOutcome::ListenWindowEnded => {
                "Listen window ended; no completion observed yet. Agents may still be running."
                    .to_string()
            }
        };
        Self {
            message,
            pending,
            timed_out: matches!(wait_outcome, CollabWaitOutcome::ListenWindowEnded),
            wait_outcome,
        }
    }
}

impl ToolOutput for WaitAgentResult {
    fn log_preview(&self) -> String {
        tool_output_json_text(self, "wait_agent")
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        tool_output_response_item(call_id, payload, self, /*success*/ None, "wait_agent")
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        tool_output_code_mode_result(self, "wait_agent")
    }
}

async fn wait_for_mailbox_change(
    mailbox_seq_rx: &mut tokio::sync::watch::Receiver<u64>,
    deadline: Instant,
) -> bool {
    match timeout_at(deadline, mailbox_seq_rx.changed()).await {
        Ok(Ok(())) => true,
        Ok(Err(_)) | Err(_) => false,
    }
}

async fn wait_for_final_status(
    session: Arc<Session>,
    thread_id: ThreadId,
    mut status_rx: Receiver<AgentStatus>,
) -> Option<(ThreadId, AgentStatus)> {
    let mut status = status_rx.borrow().clone();
    if is_final(&status) {
        return Some((thread_id, status));
    }

    loop {
        if status_rx.changed().await.is_err() {
            let latest = session.services.agent_control.get_status(thread_id).await;
            return is_final(&latest).then_some((thread_id, latest));
        }
        status = status_rx.borrow().clone();
        if is_final(&status) {
            return Some((thread_id, status));
        }
    }
}

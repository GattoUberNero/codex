use crate::context_manager::is_user_turn_boundary;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::SpawnContextInheritanceEffectiveMode;
use codex_protocol::protocol::SpawnContextInheritanceMode;
use codex_protocol::protocol::SpawnContextInheritanceSuppressionReason;
use codex_protocol::protocol::SpawnContextInheritanceTelemetry;
use serde::Deserialize;
use serde::Serialize;

pub(crate) const fn should_fork_parent_context(mode: SpawnContextInheritanceMode) -> bool {
    !matches!(mode, SpawnContextInheritanceMode::Off)
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct SpawnContextInheritanceReport {
    pub(crate) requested_mode: SpawnContextInheritanceMode,
    pub(crate) effective_mode: SpawnContextInheritanceEffectiveMode,
    pub(crate) telemetry: Option<SpawnContextInheritanceTelemetry>,
}

impl SpawnContextInheritanceReport {
    pub(crate) fn exact_with_telemetry(
        requested_mode: SpawnContextInheritanceMode,
        telemetry: SpawnContextInheritanceTelemetry,
    ) -> Self {
        Self {
            requested_mode,
            effective_mode: SpawnContextInheritanceEffectiveMode::Exact,
            telemetry: Some(telemetry),
        }
    }

    pub(crate) const fn off(requested_mode: SpawnContextInheritanceMode) -> Self {
        Self {
            requested_mode,
            effective_mode: SpawnContextInheritanceEffectiveMode::Off,
            telemetry: None,
        }
    }

    pub(crate) fn off_with_telemetry(
        requested_mode: SpawnContextInheritanceMode,
        telemetry: SpawnContextInheritanceTelemetry,
    ) -> Self {
        Self {
            requested_mode,
            effective_mode: SpawnContextInheritanceEffectiveMode::Off,
            telemetry: Some(telemetry),
        }
    }

    pub(crate) fn bounded_full(
        requested_mode: SpawnContextInheritanceMode,
        telemetry: SpawnContextInheritanceTelemetry,
    ) -> Self {
        Self {
            requested_mode,
            effective_mode: SpawnContextInheritanceEffectiveMode::BoundedFull,
            telemetry: Some(telemetry),
        }
    }

    pub(crate) fn bounded_trimmed(
        requested_mode: SpawnContextInheritanceMode,
        telemetry: SpawnContextInheritanceTelemetry,
    ) -> Self {
        Self {
            requested_mode,
            effective_mode: SpawnContextInheritanceEffectiveMode::BoundedTrimmed,
            telemetry: Some(telemetry),
        }
    }

    pub(crate) fn bounded_suppressed(
        requested_mode: SpawnContextInheritanceMode,
        telemetry: SpawnContextInheritanceTelemetry,
    ) -> Self {
        Self {
            requested_mode,
            effective_mode: SpawnContextInheritanceEffectiveMode::BoundedSuppressed,
            telemetry: Some(telemetry),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BoundedForkContextSelection {
    pub(crate) report: SpawnContextInheritanceReport,
    pub(crate) rollout_items: Option<Vec<RolloutItem>>,
}

fn as_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn build_context_inheritance_telemetry(
    parent_replay_safe_turn_count: usize,
    shipped_replay_safe_turn_count: Option<usize>,
    estimated_shipped_tokens: Option<i64>,
    usable_context_budget_tokens: Option<i64>,
    suppression_reason: Option<SpawnContextInheritanceSuppressionReason>,
) -> SpawnContextInheritanceTelemetry {
    SpawnContextInheritanceTelemetry {
        parent_replay_safe_turn_count: Some(as_u32(parent_replay_safe_turn_count)),
        shipped_replay_safe_turn_count: shipped_replay_safe_turn_count.map(as_u32),
        estimated_shipped_tokens,
        usable_context_budget_tokens,
        suppression_reason,
    }
}

#[derive(Debug, Clone)]
struct ActiveReplayTurnSegment {
    start_idx: usize,
    turn_id: Option<String>,
    counts_as_user_turn: bool,
}

#[derive(Debug, Clone, Default)]
struct PrefixCompactionSegment {
    turn_id: Option<String>,
    counts_as_user_turn: bool,
    compacted_items: Vec<RolloutItem>,
}

fn turn_ids_are_compatible(lhs: Option<&str>, rhs: Option<&str>) -> bool {
    lhs.is_none() || rhs.is_none() || lhs == rhs
}

fn finalize_replay_turn_segment(
    active_segment: Option<ActiveReplayTurnSegment>,
    turn_start_positions: &mut Vec<usize>,
) {
    if let Some(active_segment) = active_segment
        && active_segment.counts_as_user_turn
    {
        turn_start_positions.push(active_segment.start_idx);
    }
}

fn finalize_prefix_compaction_segment(
    active_segment: Option<PrefixCompactionSegment>,
    segments: &mut Vec<PrefixCompactionSegment>,
) {
    if let Some(active_segment) = active_segment {
        segments.push(active_segment);
    }
}

fn drop_last_n_user_turn_prefix_segments(
    segments: &mut Vec<PrefixCompactionSegment>,
    remove_count: usize,
) {
    if remove_count == 0 {
        return;
    }

    let mut remove_count = remove_count;
    let mut retained_segments = Vec::with_capacity(segments.len());
    for segment in segments.drain(..).rev() {
        if remove_count > 0 && segment.counts_as_user_turn {
            remove_count -= 1;
            continue;
        }
        retained_segments.push(segment);
    }
    retained_segments.reverse();
    *segments = retained_segments;
}

pub(crate) fn replay_safe_turn_boundary_positions_in_rollout(items: &[RolloutItem]) -> Vec<usize> {
    let mut turn_positions = Vec::new();
    let mut active_segment: Option<ActiveReplayTurnSegment> = None;
    for (idx, item) in items.iter().enumerate() {
        match item {
            RolloutItem::EventMsg(EventMsg::TurnStarted(event)) => {
                finalize_replay_turn_segment(active_segment.take(), &mut turn_positions);
                active_segment = Some(ActiveReplayTurnSegment {
                    start_idx: idx,
                    turn_id: Some(event.turn_id.clone()),
                    counts_as_user_turn: false,
                });
            }
            RolloutItem::TurnContext(turn_context) => match active_segment.as_mut() {
                Some(active_segment)
                    if turn_ids_are_compatible(
                        active_segment.turn_id.as_deref(),
                        turn_context.turn_id.as_deref(),
                    ) =>
                {
                    if active_segment.turn_id.is_none() {
                        active_segment.turn_id = turn_context.turn_id.clone();
                    }
                }
                Some(_) => {
                    finalize_replay_turn_segment(active_segment.take(), &mut turn_positions);
                    active_segment = Some(ActiveReplayTurnSegment {
                        start_idx: idx,
                        turn_id: turn_context.turn_id.clone(),
                        counts_as_user_turn: false,
                    });
                }
                None => {
                    active_segment = Some(ActiveReplayTurnSegment {
                        start_idx: idx,
                        turn_id: turn_context.turn_id.clone(),
                        counts_as_user_turn: false,
                    });
                }
            },
            RolloutItem::EventMsg(EventMsg::UserMessage(_)) => {
                let active_segment = active_segment.get_or_insert(ActiveReplayTurnSegment {
                    start_idx: idx,
                    turn_id: None,
                    counts_as_user_turn: false,
                });
                active_segment.counts_as_user_turn = true;
            }
            RolloutItem::ResponseItem(item) if is_user_turn_boundary(item) => {
                let active_segment = active_segment.get_or_insert(ActiveReplayTurnSegment {
                    start_idx: idx,
                    turn_id: None,
                    counts_as_user_turn: false,
                });
                active_segment.counts_as_user_turn = true;
            }
            RolloutItem::EventMsg(EventMsg::TurnComplete(event)) => {
                if let Some(existing_segment) = active_segment.as_mut()
                    && turn_ids_are_compatible(
                        existing_segment.turn_id.as_deref(),
                        Some(event.turn_id.as_str()),
                    )
                {
                    if existing_segment.turn_id.is_none() {
                        existing_segment.turn_id = Some(event.turn_id.clone());
                    }
                    finalize_replay_turn_segment(active_segment.take(), &mut turn_positions);
                }
            }
            RolloutItem::EventMsg(EventMsg::TurnAborted(event)) => {
                if let Some(existing_segment) = active_segment.as_mut()
                    && turn_ids_are_compatible(
                        existing_segment.turn_id.as_deref(),
                        event.turn_id.as_deref(),
                    )
                {
                    if existing_segment.turn_id.is_none() {
                        existing_segment.turn_id = event.turn_id.clone();
                    }
                    finalize_replay_turn_segment(active_segment.take(), &mut turn_positions);
                }
            }
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(rollback)) => {
                let mut remove_count = usize::try_from(rollback.num_turns).unwrap_or(usize::MAX);
                if remove_count > 0 {
                    if let Some(existing_segment) = active_segment.take()
                        && existing_segment.counts_as_user_turn
                    {
                        remove_count = remove_count.saturating_sub(1);
                    }
                    let keep_count = turn_positions.len().saturating_sub(remove_count);
                    turn_positions.truncate(keep_count);
                }
            }
            _ => {}
        }
    }
    finalize_replay_turn_segment(active_segment, &mut turn_positions);
    turn_positions
}

fn surviving_prefix_compacted_items(
    full_rollout_items: &[RolloutItem],
    cut_idx: usize,
) -> Vec<RolloutItem> {
    let mut segments = Vec::new();
    let mut active_segment: Option<PrefixCompactionSegment> = None;

    for item in &full_rollout_items[..cut_idx] {
        match item {
            RolloutItem::EventMsg(EventMsg::TurnStarted(event)) => {
                finalize_prefix_compaction_segment(active_segment.take(), &mut segments);
                active_segment = Some(PrefixCompactionSegment {
                    turn_id: Some(event.turn_id.clone()),
                    counts_as_user_turn: false,
                    compacted_items: Vec::new(),
                });
            }
            RolloutItem::TurnContext(turn_context) => match active_segment.as_mut() {
                Some(active_segment)
                    if turn_ids_are_compatible(
                        active_segment.turn_id.as_deref(),
                        turn_context.turn_id.as_deref(),
                    ) =>
                {
                    if active_segment.turn_id.is_none() {
                        active_segment.turn_id = turn_context.turn_id.clone();
                    }
                }
                Some(_) => {
                    finalize_prefix_compaction_segment(active_segment.take(), &mut segments);
                    active_segment = Some(PrefixCompactionSegment {
                        turn_id: turn_context.turn_id.clone(),
                        counts_as_user_turn: false,
                        compacted_items: Vec::new(),
                    });
                }
                None => {
                    active_segment = Some(PrefixCompactionSegment {
                        turn_id: turn_context.turn_id.clone(),
                        counts_as_user_turn: false,
                        compacted_items: Vec::new(),
                    });
                }
            },
            RolloutItem::EventMsg(EventMsg::UserMessage(_)) => {
                let active_segment =
                    active_segment.get_or_insert_with(PrefixCompactionSegment::default);
                active_segment.counts_as_user_turn = true;
            }
            RolloutItem::ResponseItem(item) if is_user_turn_boundary(item) => {
                let active_segment =
                    active_segment.get_or_insert_with(PrefixCompactionSegment::default);
                active_segment.counts_as_user_turn = true;
            }
            RolloutItem::Compacted(compacted) => {
                let active_segment =
                    active_segment.get_or_insert_with(PrefixCompactionSegment::default);
                active_segment
                    .compacted_items
                    .push(RolloutItem::Compacted(compacted.clone()));
            }
            RolloutItem::EventMsg(EventMsg::TurnComplete(event)) => {
                if let Some(existing_segment) = active_segment.as_mut()
                    && turn_ids_are_compatible(
                        existing_segment.turn_id.as_deref(),
                        Some(event.turn_id.as_str()),
                    )
                {
                    if existing_segment.turn_id.is_none() {
                        existing_segment.turn_id = Some(event.turn_id.clone());
                    }
                    finalize_prefix_compaction_segment(active_segment.take(), &mut segments);
                }
            }
            RolloutItem::EventMsg(EventMsg::TurnAborted(event)) => {
                if let Some(existing_segment) = active_segment.as_mut()
                    && turn_ids_are_compatible(
                        existing_segment.turn_id.as_deref(),
                        event.turn_id.as_deref(),
                    )
                {
                    if existing_segment.turn_id.is_none() {
                        existing_segment.turn_id = event.turn_id.clone();
                    }
                    finalize_prefix_compaction_segment(active_segment.take(), &mut segments);
                }
            }
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(rollback)) => {
                let mut remove_count = usize::try_from(rollback.num_turns).unwrap_or(usize::MAX);
                if remove_count > 0 {
                    if let Some(existing_segment) = active_segment.take() {
                        if existing_segment.counts_as_user_turn {
                            remove_count = remove_count.saturating_sub(1);
                        } else {
                            active_segment = Some(existing_segment);
                        }
                    }
                    drop_last_n_user_turn_prefix_segments(&mut segments, remove_count);
                }
            }
            _ => {}
        }
    }

    finalize_prefix_compaction_segment(active_segment, &mut segments);
    segments
        .into_iter()
        .flat_map(|segment| segment.compacted_items)
        .collect()
}

fn build_replay_safe_candidate(
    full_rollout_items: &[RolloutItem],
    cut_idx: usize,
) -> Vec<RolloutItem> {
    let leading_session_meta_count = full_rollout_items
        .iter()
        .take_while(|item| matches!(item, RolloutItem::SessionMeta(_)))
        .count();
    let mut candidate = full_rollout_items[..leading_session_meta_count].to_vec();
    candidate.extend(surviving_prefix_compacted_items(
        full_rollout_items,
        cut_idx,
    ));
    candidate.extend_from_slice(&full_rollout_items[cut_idx..]);
    candidate
}

pub(crate) fn select_bounded_fork_context(
    full_rollout_items: Vec<RolloutItem>,
    parent_spawn_call_id: &str,
    usable_context_budget_tokens: i64,
    requested_mode: SpawnContextInheritanceMode,
) -> Result<BoundedForkContextSelection, serde_json::Error> {
    let parent_turn_count =
        replay_safe_turn_boundary_positions_in_rollout(&full_rollout_items).len();
    if usable_context_budget_tokens <= 0 {
        return Ok(BoundedForkContextSelection {
            report: SpawnContextInheritanceReport::bounded_suppressed(
                requested_mode,
                build_context_inheritance_telemetry(
                    parent_turn_count,
                    /*shipped_replay_safe_turn_count*/ None,
                    /*estimated_shipped_tokens*/ None,
                    Some(usable_context_budget_tokens),
                    Some(SpawnContextInheritanceSuppressionReason::BudgetExceeded),
                ),
            ),
            rollout_items: None,
        });
    }

    if !spawn_call_output_pairing_is_valid(&full_rollout_items, parent_spawn_call_id) {
        return Ok(BoundedForkContextSelection {
            report: SpawnContextInheritanceReport::bounded_suppressed(
                requested_mode,
                build_context_inheritance_telemetry(
                    parent_turn_count,
                    /*shipped_replay_safe_turn_count*/ None,
                    /*estimated_shipped_tokens*/ None,
                    Some(usable_context_budget_tokens),
                    Some(SpawnContextInheritanceSuppressionReason::InvalidParentSpawnPairing),
                ),
            ),
            rollout_items: None,
        });
    }

    let full_tokens = estimated_rollout_tokens(&full_rollout_items)?;
    if full_tokens <= usable_context_budget_tokens {
        return Ok(BoundedForkContextSelection {
            report: SpawnContextInheritanceReport::bounded_full(
                requested_mode,
                build_context_inheritance_telemetry(
                    parent_turn_count,
                    Some(parent_turn_count),
                    Some(full_tokens),
                    Some(usable_context_budget_tokens),
                    /*suppression_reason*/ None,
                ),
            ),
            rollout_items: Some(full_rollout_items),
        });
    }

    let turn_positions = replay_safe_turn_boundary_positions_in_rollout(&full_rollout_items);
    for &cut_idx in turn_positions.iter().skip(1) {
        let candidate = build_replay_safe_candidate(&full_rollout_items, cut_idx);
        if !spawn_call_output_pairing_is_valid(&candidate, parent_spawn_call_id) {
            continue;
        }
        let candidate_tokens = estimated_rollout_tokens(&candidate)?;
        if candidate_tokens <= usable_context_budget_tokens {
            let shipped_turn_count =
                replay_safe_turn_boundary_positions_in_rollout(&candidate).len();
            return Ok(BoundedForkContextSelection {
                report: SpawnContextInheritanceReport::bounded_trimmed(
                    requested_mode,
                    build_context_inheritance_telemetry(
                        parent_turn_count,
                        Some(shipped_turn_count),
                        Some(candidate_tokens),
                        Some(usable_context_budget_tokens),
                        /*suppression_reason*/ None,
                    ),
                ),
                rollout_items: Some(candidate),
            });
        }
    }

    Ok(BoundedForkContextSelection {
        report: SpawnContextInheritanceReport::bounded_suppressed(
            requested_mode,
            build_context_inheritance_telemetry(
                parent_turn_count,
                /*shipped_replay_safe_turn_count*/ None,
                /*estimated_shipped_tokens*/ None,
                Some(usable_context_budget_tokens),
                Some(SpawnContextInheritanceSuppressionReason::BudgetExceeded),
            ),
        ),
        rollout_items: None,
    })
}

fn estimated_rollout_tokens(items: &[RolloutItem]) -> Result<i64, serde_json::Error> {
    let serialized = serde_json::to_string(items)?;
    Ok(
        i64::try_from(codex_utils_output_truncation::approx_token_count(
            &serialized,
        ))
        .unwrap_or(i64::MAX),
    )
}

pub(crate) fn spawn_call_output_pairing_is_valid(
    items: &[RolloutItem],
    parent_spawn_call_id: &str,
) -> bool {
    enum PairingState {
        Missing,
        SawCall,
        Complete,
    }

    let mut pairing_state = PairingState::Missing;
    for item in items {
        match item {
            RolloutItem::ResponseItem(ResponseItem::FunctionCall { call_id, .. })
                if call_id == parent_spawn_call_id =>
            {
                if !matches!(pairing_state, PairingState::Missing) {
                    return false;
                }
                pairing_state = PairingState::SawCall;
            }
            RolloutItem::ResponseItem(ResponseItem::FunctionCallOutput { call_id, .. })
                if call_id == parent_spawn_call_id =>
            {
                if !matches!(pairing_state, PairingState::SawCall) {
                    return false;
                }
                pairing_state = PairingState::Complete;
            }
            _ => {}
        }
    }

    !matches!(pairing_state, PairingState::SawCall)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::AgentPath;
    use codex_protocol::models::ContentItem;
    use codex_protocol::protocol::InterAgentCommunication;
    use codex_protocol::protocol::SessionMeta;
    use codex_protocol::protocol::SessionMetaLine;
    use codex_protocol::protocol::ThreadRolledBackEvent;
    use codex_protocol::protocol::TurnAbortReason;
    use codex_protocol::protocol::TurnAbortedEvent;
    use codex_protocol::protocol::TurnCompleteEvent;
    use codex_protocol::protocol::TurnContextItem;
    use codex_protocol::protocol::TurnStartedEvent;
    use pretty_assertions::assert_eq;
    use serde_json::Value;

    fn session_meta_item() -> RolloutItem {
        RolloutItem::SessionMeta(SessionMetaLine {
            meta: SessionMeta::default(),
            git: None,
        })
    }

    fn user_message_item(text: &str) -> RolloutItem {
        RolloutItem::ResponseItem(ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: text.to_string(),
            }],
            end_turn: None,
            phase: None,
        })
    }

    fn assistant_text_item(text: &str) -> RolloutItem {
        RolloutItem::ResponseItem(ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: text.to_string(),
            }],
            end_turn: None,
            phase: None,
        })
    }

    fn assistant_inter_agent_instruction_item(text: &str) -> RolloutItem {
        let communication = InterAgentCommunication::new(
            AgentPath::root(),
            AgentPath::root(),
            Vec::new(),
            text.to_string(),
            /*trigger_turn*/ true,
        );
        RolloutItem::ResponseItem(ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: serde_json::to_string(&communication).expect("serialize inter-agent message"),
            }],
            end_turn: None,
            phase: None,
        })
    }

    fn turn_started_item(turn_id: &str) -> RolloutItem {
        RolloutItem::EventMsg(EventMsg::TurnStarted(TurnStartedEvent {
            turn_id: turn_id.to_string(),
            model_context_window: Some(128_000),
            collaboration_mode_kind: codex_protocol::config_types::ModeKind::Default,
        }))
    }

    fn turn_complete_item(turn_id: &str) -> RolloutItem {
        RolloutItem::EventMsg(EventMsg::TurnComplete(TurnCompleteEvent {
            turn_id: turn_id.to_string(),
            last_agent_message: None,
        }))
    }

    fn turn_aborted_item(turn_id: &str) -> RolloutItem {
        RolloutItem::EventMsg(EventMsg::TurnAborted(TurnAbortedEvent {
            turn_id: Some(turn_id.to_string()),
            reason: TurnAbortReason::Interrupted,
        }))
    }

    fn turn_context_item(turn_id: &str) -> RolloutItem {
        RolloutItem::TurnContext(TurnContextItem {
            turn_id: Some(turn_id.to_string()),
            trace_id: None,
            current_date: None,
            timezone: None,
            cwd: std::path::PathBuf::from("/tmp"),
            approval_policy: codex_protocol::protocol::AskForApproval::Never,
            sandbox_policy: codex_protocol::protocol::SandboxPolicy::DangerFullAccess,
            network: None,
            model: "gpt-5".to_string(),
            personality: None,
            collaboration_mode: None,
            realtime_active: None,
            effort: None,
            summary: codex_protocol::config_types::ReasoningSummary::Auto,
            user_instructions: None,
            developer_instructions: None,
            final_output_json_schema: None,
            truncation_policy: None,
        })
    }

    fn spawn_call_item(call_id: &str) -> RolloutItem {
        RolloutItem::ResponseItem(ResponseItem::FunctionCall {
            id: None,
            call_id: call_id.to_string(),
            name: "spawn_agent".to_string(),
            namespace: None,
            arguments: "{}".to_string(),
        })
    }

    fn spawn_output_item(call_id: &str) -> RolloutItem {
        RolloutItem::ResponseItem(ResponseItem::FunctionCallOutput {
            call_id: call_id.to_string(),
            output: codex_protocol::models::FunctionCallOutputPayload::from_text("ok".to_string()),
        })
    }

    fn rollout_items_json(items: Vec<RolloutItem>) -> Value {
        normalize_rollout_items_json(serde_json::to_value(items).expect("serialize rollout items"))
    }

    fn normalize_rollout_items_json(value: Value) -> Value {
        match value {
            Value::Array(items) => Value::Array(
                items
                    .into_iter()
                    .map(normalize_rollout_items_json)
                    .collect(),
            ),
            Value::Object(mut object) => {
                if object.get("type") == Some(&Value::String("session_meta".to_string()))
                    && let Some(Value::Object(payload)) = object.get_mut("payload")
                {
                    payload.remove("id");
                }
                Value::Object(
                    object
                        .into_iter()
                        .map(|(key, value)| (key, normalize_rollout_items_json(value)))
                        .collect(),
                )
            }
            other => other,
        }
    }

    #[test]
    fn replay_safe_turn_boundaries_track_inter_agent_instructions_and_rollbacks() {
        let items = vec![
            turn_started_item("u1"),
            user_message_item("u1"),
            turn_complete_item("u1"),
            turn_started_item("assistant-turn"),
            turn_context_item("assistant-turn"),
            assistant_inter_agent_instruction_item("delegate"),
            turn_complete_item("assistant-turn"),
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 1,
            })),
            turn_started_item("u2"),
            turn_context_item("u2"),
            user_message_item("u2"),
            turn_aborted_item("u2"),
        ];

        let boundaries = replay_safe_turn_boundary_positions_in_rollout(&items);
        assert_eq!(boundaries, vec![0, 8]);
    }

    #[test]
    fn replay_safe_turn_boundaries_do_not_resurrect_open_rolled_back_segments() {
        let items = vec![
            turn_started_item("u1"),
            user_message_item("u1"),
            turn_complete_item("u1"),
            turn_started_item("rolled-back"),
            turn_context_item("rolled-back"),
            user_message_item("rolled-back"),
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 1,
            })),
        ];

        let boundaries = replay_safe_turn_boundary_positions_in_rollout(&items);
        assert_eq!(boundaries, vec![0]);
    }

    #[test]
    fn bounded_selection_trims_at_replay_safe_turn_boundaries_and_keeps_session_meta() {
        let call_id = "spawn-call";
        let full = vec![
            session_meta_item(),
            turn_started_item("oldest"),
            turn_context_item("oldest"),
            user_message_item("oldest"),
            assistant_text_item("ack"),
            turn_complete_item("oldest"),
            turn_started_item("latest"),
            turn_context_item("latest"),
            user_message_item("latest"),
            spawn_call_item(call_id),
            spawn_output_item(call_id),
            turn_complete_item("latest"),
        ];
        let expected_trimmed = vec![
            session_meta_item(),
            turn_started_item("latest"),
            turn_context_item("latest"),
            user_message_item("latest"),
            spawn_call_item(call_id),
            spawn_output_item(call_id),
            turn_complete_item("latest"),
        ];
        let budget = estimated_rollout_tokens(&expected_trimmed).expect("estimate tokens");

        let selection = select_bounded_fork_context(
            full,
            call_id,
            budget,
            SpawnContextInheritanceMode::Bounded,
        )
        .expect("bounded selection should succeed");

        assert_eq!(
            selection.report.requested_mode,
            SpawnContextInheritanceMode::Bounded
        );
        assert_eq!(
            selection.report.effective_mode,
            SpawnContextInheritanceEffectiveMode::BoundedTrimmed
        );
        assert_eq!(
            selection.report.telemetry,
            Some(SpawnContextInheritanceTelemetry {
                parent_replay_safe_turn_count: Some(2),
                shipped_replay_safe_turn_count: Some(1),
                estimated_shipped_tokens: Some(budget),
                usable_context_budget_tokens: Some(budget),
                suppression_reason: None,
            })
        );
        assert_eq!(
            rollout_items_json(
                selection
                    .rollout_items
                    .expect("trimmed rollout should be present")
            ),
            rollout_items_json(expected_trimmed)
        );
    }

    #[test]
    fn bounded_selection_suppresses_when_no_replay_safe_candidate_fits_budget() {
        let call_id = "spawn-call";
        let full = vec![
            session_meta_item(),
            user_message_item("only-turn"),
            spawn_call_item(call_id),
            spawn_output_item(call_id),
        ];

        let selection = select_bounded_fork_context(
            full,
            call_id,
            /*usable_context_budget_tokens*/ 1,
            SpawnContextInheritanceMode::Bounded,
        )
        .expect("bounded selection should succeed");

        assert_eq!(
            selection.report.requested_mode,
            SpawnContextInheritanceMode::Bounded
        );
        assert_eq!(
            selection.report.effective_mode,
            SpawnContextInheritanceEffectiveMode::BoundedSuppressed
        );
        assert_eq!(
            selection.report.telemetry,
            Some(SpawnContextInheritanceTelemetry {
                parent_replay_safe_turn_count: Some(1),
                shipped_replay_safe_turn_count: None,
                estimated_shipped_tokens: None,
                usable_context_budget_tokens: Some(1),
                suppression_reason: Some(SpawnContextInheritanceSuppressionReason::BudgetExceeded,),
            })
        );
        assert!(selection.rollout_items.is_none());
    }

    #[test]
    fn bounded_selection_preserves_compaction_checkpoint_prefix_when_trimming() {
        let call_id = "spawn-call";
        let compaction = RolloutItem::Compacted(codex_protocol::protocol::CompactedItem {
            message: "summary".to_string(),
            replacement_history: Some(vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "summary".to_string(),
                }],
                end_turn: None,
                phase: None,
            }]),
        });
        let full = vec![
            session_meta_item(),
            compaction.clone(),
            turn_started_item("oldest"),
            user_message_item("oldest"),
            turn_complete_item("oldest"),
            turn_started_item("latest"),
            user_message_item("latest"),
            spawn_call_item(call_id),
            spawn_output_item(call_id),
            turn_complete_item("latest"),
        ];
        let expected_trimmed = vec![
            session_meta_item(),
            compaction,
            turn_started_item("latest"),
            user_message_item("latest"),
            spawn_call_item(call_id),
            spawn_output_item(call_id),
            turn_complete_item("latest"),
        ];
        let budget = estimated_rollout_tokens(&expected_trimmed).expect("estimate tokens");

        let selection = select_bounded_fork_context(
            full,
            call_id,
            budget,
            SpawnContextInheritanceMode::Bounded,
        )
        .expect("bounded selection should succeed");

        assert_eq!(
            rollout_items_json(
                selection
                    .rollout_items
                    .expect("trimmed rollout should be present")
            ),
            rollout_items_json(expected_trimmed)
        );
    }

    #[test]
    fn bounded_selection_drops_compactions_from_rolled_back_segments_when_trimming() {
        let call_id = "spawn-call";
        let baseline_compaction = RolloutItem::Compacted(codex_protocol::protocol::CompactedItem {
            message: "baseline".to_string(),
            replacement_history: Some(vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "baseline".to_string(),
                }],
                end_turn: None,
                phase: None,
            }]),
        });
        let rolled_back_compaction =
            RolloutItem::Compacted(codex_protocol::protocol::CompactedItem {
                message: "rolled-back".to_string(),
                replacement_history: Some(vec![ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: "rolled-back".to_string(),
                    }],
                    end_turn: None,
                    phase: None,
                }]),
            });
        let full = vec![
            session_meta_item(),
            baseline_compaction.clone(),
            turn_started_item("oldest"),
            user_message_item("oldest"),
            turn_complete_item("oldest"),
            turn_started_item("rolled-back"),
            rolled_back_compaction,
            user_message_item("rolled-back"),
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 1,
            })),
            turn_started_item("latest"),
            user_message_item("latest"),
            spawn_call_item(call_id),
            spawn_output_item(call_id),
            turn_complete_item("latest"),
        ];
        let expected_trimmed = vec![
            session_meta_item(),
            baseline_compaction,
            turn_started_item("latest"),
            user_message_item("latest"),
            spawn_call_item(call_id),
            spawn_output_item(call_id),
            turn_complete_item("latest"),
        ];
        let budget = estimated_rollout_tokens(&expected_trimmed).expect("estimate tokens");

        let selection = select_bounded_fork_context(
            full,
            call_id,
            budget,
            SpawnContextInheritanceMode::Bounded,
        )
        .expect("bounded selection should succeed");

        assert_eq!(
            rollout_items_json(
                selection
                    .rollout_items
                    .expect("trimmed rollout should be present")
            ),
            rollout_items_json(expected_trimmed)
        );
    }

    #[test]
    fn bounded_selection_suppresses_when_spawn_output_precedes_matching_call() {
        let call_id = "spawn-call";
        let full = vec![
            session_meta_item(),
            spawn_output_item(call_id),
            turn_started_item("latest"),
            user_message_item("latest"),
            spawn_call_item(call_id),
            turn_complete_item("latest"),
        ];

        let selection = select_bounded_fork_context(
            full,
            call_id,
            50_000,
            SpawnContextInheritanceMode::Bounded,
        )
        .expect("bounded selection should succeed");

        assert!(selection.rollout_items.is_none());
        assert_eq!(
            selection.report.requested_mode,
            SpawnContextInheritanceMode::Bounded
        );
        assert_eq!(
            selection.report.effective_mode,
            SpawnContextInheritanceEffectiveMode::BoundedSuppressed
        );
        assert_eq!(
            selection.report.telemetry,
            Some(SpawnContextInheritanceTelemetry {
                parent_replay_safe_turn_count: Some(1),
                shipped_replay_safe_turn_count: None,
                estimated_shipped_tokens: None,
                usable_context_budget_tokens: Some(50_000),
                suppression_reason: Some(
                    SpawnContextInheritanceSuppressionReason::InvalidParentSpawnPairing,
                ),
            })
        );
    }
}

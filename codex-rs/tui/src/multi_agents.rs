//! Helpers for rendering and navigating multi-agent state in the TUI.
//!
//! This module owns the shared presentation contracts for multi-agent history rows, `/agent` picker
//! entries, and the fast-switch keyboard shortcuts. Higher-level coordination, such as deciding
//! which thread becomes active or when a thread closes, stays in [`crate::app::App`].

use crate::history_cell::PlainHistoryCell;
use crate::render::line_utils::prefix_lines;
use crate::text_formatting::truncate_text;
use codex_protocol::ThreadId;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use codex_protocol::protocol::AgentStatus;
use codex_protocol::protocol::CollabAgentInteractionEndEvent;
use codex_protocol::protocol::CollabAgentRef;
use codex_protocol::protocol::CollabAgentSpawnEndEvent;
use codex_protocol::protocol::CollabAgentStatusEntry;
use codex_protocol::protocol::CollabCloseEndEvent;
use codex_protocol::protocol::CollabResumeBeginEvent;
use codex_protocol::protocol::CollabResumeEndEvent;
use codex_protocol::protocol::CollabWaitingBeginEvent;
use codex_protocol::protocol::CollabWaitingEndEvent;
use codex_protocol::protocol::DelegationReport;
use codex_protocol::protocol::SpawnContextInheritanceEffectiveMode;
use codex_protocol::protocol::SpawnContextInheritanceMode;
use codex_protocol::protocol::SpawnContextInheritanceSuppressionReason;
use codex_protocol::protocol::SpawnContextInheritanceTelemetry;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
#[cfg(target_os = "macos")]
use crossterm::event::KeyEventKind;
#[cfg(target_os = "macos")]
use crossterm::event::KeyModifiers;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use std::collections::HashMap;
use std::collections::HashSet;

const COLLAB_AGENT_ERROR_PREVIEW_GRAPHEMES: usize = 160;
const COLLAB_AGENT_RESPONSE_PREVIEW_GRAPHEMES: usize = 240;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentPickerThreadEntry {
    /// Human-friendly nickname shown in picker rows and footer labels.
    pub(crate) agent_nickname: Option<String>,
    /// Agent type shown in brackets when present, for example `worker`.
    pub(crate) agent_role: Option<String>,
    /// Whether the thread has emitted a close event and should render dimmed.
    pub(crate) is_closed: bool,
}

#[derive(Clone, Copy)]
struct AgentLabel<'a> {
    thread_id: Option<ThreadId>,
    nickname: Option<&'a str>,
    role: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpawnRequestSummary {
    pub(crate) model: String,
    pub(crate) reasoning_effort: ReasoningEffortConfig,
    pub(crate) context_inheritance_requested: Option<SpawnContextInheritanceMode>,
}

pub(crate) fn spawn_begin(prompt: &str, spawn_request: &SpawnRequestSummary) -> PlainHistoryCell {
    let mut spans = vec![Span::from("Spawning agent").bold()];
    spans.extend(spawn_request_spans(Some(spawn_request)));

    let mut details = Vec::new();
    if let Some(line) = prompt_line(prompt) {
        details.push(line);
    }
    if let Some(line) =
        requested_context_inheritance_line(spawn_request.context_inheritance_requested)
    {
        details.push(line);
    }

    collab_event(title_spans_line(spans), details)
}

pub(crate) fn agent_picker_status_dot_spans(is_closed: bool) -> Vec<Span<'static>> {
    let dot = if is_closed {
        "•".into()
    } else {
        "•".green()
    };
    vec![dot, " ".into()]
}

pub(crate) fn format_agent_picker_item_name(
    agent_nickname: Option<&str>,
    agent_role: Option<&str>,
    is_primary: bool,
) -> String {
    if is_primary {
        return "Main [default]".to_string();
    }

    let agent_nickname = agent_nickname
        .map(str::trim)
        .filter(|nickname| !nickname.is_empty());
    let agent_role = agent_role.map(str::trim).filter(|role| !role.is_empty());
    match (agent_nickname, agent_role) {
        (Some(agent_nickname), Some(agent_role)) => format!("{agent_nickname} [{agent_role}]"),
        (Some(agent_nickname), None) => agent_nickname.to_string(),
        (None, Some(agent_role)) => format!("[{agent_role}]"),
        (None, None) => "Agent".to_string(),
    }
}

pub(crate) fn previous_agent_shortcut() -> crate::key_hint::KeyBinding {
    crate::key_hint::alt(KeyCode::Left)
}

pub(crate) fn next_agent_shortcut() -> crate::key_hint::KeyBinding {
    crate::key_hint::alt(KeyCode::Right)
}

/// Matches the canonical "previous agent" binding plus platform-specific fallbacks that keep agent
/// navigation working when enhanced key reporting is unavailable.
pub(crate) fn previous_agent_shortcut_matches(
    key_event: KeyEvent,
    allow_word_motion_fallback: bool,
) -> bool {
    previous_agent_shortcut().is_press(key_event)
        || previous_agent_word_motion_fallback(key_event, allow_word_motion_fallback)
}

/// Matches the canonical "next agent" binding plus platform-specific fallbacks that keep agent
/// navigation working when enhanced key reporting is unavailable.
pub(crate) fn next_agent_shortcut_matches(
    key_event: KeyEvent,
    allow_word_motion_fallback: bool,
) -> bool {
    next_agent_shortcut().is_press(key_event)
        || next_agent_word_motion_fallback(key_event, allow_word_motion_fallback)
}

#[cfg(target_os = "macos")]
fn previous_agent_word_motion_fallback(
    key_event: KeyEvent,
    allow_word_motion_fallback: bool,
) -> bool {
    // Some terminals, especially on macOS, send Option+b/f as word-motion keys instead of
    // Option+arrow events unless enhanced keyboard reporting is enabled. Callers should only
    // enable this fallback when the composer is empty so draft editing retains the expected
    // word-wise motion behavior.
    allow_word_motion_fallback
        && matches!(
            key_event,
            KeyEvent {
                code: KeyCode::Char('b'),
                modifiers: KeyModifiers::ALT,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            }
        )
}

#[cfg(not(target_os = "macos"))]
fn previous_agent_word_motion_fallback(
    _key_event: KeyEvent,
    _allow_word_motion_fallback: bool,
) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn next_agent_word_motion_fallback(key_event: KeyEvent, allow_word_motion_fallback: bool) -> bool {
    // Some terminals, especially on macOS, send Option+b/f as word-motion keys instead of
    // Option+arrow events unless enhanced keyboard reporting is enabled. Callers should only
    // enable this fallback when the composer is empty so draft editing retains the expected
    // word-wise motion behavior.
    allow_word_motion_fallback
        && matches!(
            key_event,
            KeyEvent {
                code: KeyCode::Char('f'),
                modifiers: KeyModifiers::ALT,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            }
        )
}

#[cfg(not(target_os = "macos"))]
fn next_agent_word_motion_fallback(
    _key_event: KeyEvent,
    _allow_word_motion_fallback: bool,
) -> bool {
    false
}

pub(crate) fn spawn_end(
    ev: CollabAgentSpawnEndEvent,
    requested_spawn_request: Option<&SpawnRequestSummary>,
) -> PlainHistoryCell {
    let CollabAgentSpawnEndEvent {
        call_id: _,
        sender_thread_id: _,
        new_thread_id,
        new_agent_nickname,
        new_agent_role,
        prompt,
        requested_model,
        requested_reasoning_effort,
        model,
        reasoning_effort,
        context_inheritance_requested,
        context_inheritance_effective,
        context_inheritance_telemetry,
        delegation_report,
        status: _,
        ..
    } = ev;
    let replayed_without_begin_row = requested_spawn_request.is_none();
    let requested_spawn_request_fallback = SpawnRequestSummary {
        model: requested_model,
        reasoning_effort: requested_reasoning_effort,
        context_inheritance_requested,
    };
    let requested_spawn_request =
        requested_spawn_request.or(Some(&requested_spawn_request_fallback));
    let effective_spawn_request = SpawnRequestSummary {
        model,
        reasoning_effort,
        context_inheritance_requested: None,
    };

    let title = match new_thread_id {
        Some(thread_id) => title_with_agent(
            "Spawned",
            AgentLabel {
                thread_id: Some(thread_id),
                nickname: new_agent_nickname.as_deref(),
                role: new_agent_role.as_deref(),
            },
            Some(&effective_spawn_request),
        ),
        None => title_text("Agent spawn failed"),
    };

    let mut details = Vec::new();
    if let Some(line) = prompt_line(&prompt) {
        details.push(line);
    }
    let show_missing_delegation = replayed_without_begin_row;
    details.extend(delegation_report_lines(
        delegation_report.as_ref(),
        show_missing_delegation,
    ));
    if let Some(line) =
        requested_spawn_request_line(requested_spawn_request, &effective_spawn_request)
    {
        details.push(line);
    }
    if replayed_without_begin_row
        && let Some(line) = requested_context_inheritance_line(context_inheritance_requested)
    {
        details.push(line);
    }
    if let Some(line) =
        context_inheritance_line(context_inheritance_requested, context_inheritance_effective)
    {
        details.push(line);
    }
    details.extend(context_inheritance_telemetry_lines(
        context_inheritance_telemetry,
    ));
    collab_event(title, details)
}

pub(crate) fn interaction_end(ev: CollabAgentInteractionEndEvent) -> PlainHistoryCell {
    let CollabAgentInteractionEndEvent {
        call_id: _,
        sender_thread_id: _,
        receiver_thread_id,
        receiver_agent_nickname,
        receiver_agent_role,
        prompt,
        status: _,
    } = ev;

    let title = title_with_agent(
        "Sent input to",
        AgentLabel {
            thread_id: Some(receiver_thread_id),
            nickname: receiver_agent_nickname.as_deref(),
            role: receiver_agent_role.as_deref(),
        },
        /*spawn_request*/ None,
    );

    let mut details = Vec::new();
    if let Some(line) = prompt_line(&prompt) {
        details.push(line);
    }
    collab_event(title, details)
}

pub(crate) fn waiting_begin(ev: CollabWaitingBeginEvent) -> PlainHistoryCell {
    let CollabWaitingBeginEvent {
        sender_thread_id: _,
        receiver_thread_ids,
        receiver_agents,
        call_id: _,
    } = ev;
    let receiver_agents = merge_wait_receivers(&receiver_thread_ids, receiver_agents);

    let title = match receiver_agents.as_slice() {
        [receiver] => title_with_agent(
            "Waiting for",
            agent_label_from_ref(receiver),
            /*spawn_request*/ None,
        ),
        [] => title_text("Waiting for agents"),
        _ => title_text(format!("Waiting for {} agents", receiver_agents.len())),
    };

    let details = if receiver_agents.len() > 1 {
        receiver_agents
            .iter()
            .map(|receiver| agent_label_line(agent_label_from_ref(receiver)))
            .collect()
    } else {
        Vec::new()
    };

    collab_event(title, details)
}

pub(crate) fn waiting_end(ev: CollabWaitingEndEvent) -> PlainHistoryCell {
    let CollabWaitingEndEvent {
        call_id: _,
        sender_thread_id: _,
        agent_statuses,
        statuses,
    } = ev;
    let details = wait_complete_lines(&statuses, &agent_statuses);
    collab_event(title_text("Finished waiting"), details)
}

pub(crate) fn close_end(ev: CollabCloseEndEvent) -> PlainHistoryCell {
    let CollabCloseEndEvent {
        call_id: _,
        sender_thread_id: _,
        receiver_thread_id,
        receiver_agent_nickname,
        receiver_agent_role,
        status: _,
    } = ev;

    collab_event(
        title_with_agent(
            "Closed",
            AgentLabel {
                thread_id: Some(receiver_thread_id),
                nickname: receiver_agent_nickname.as_deref(),
                role: receiver_agent_role.as_deref(),
            },
            /*spawn_request*/ None,
        ),
        Vec::new(),
    )
}

pub(crate) fn resume_begin(ev: CollabResumeBeginEvent) -> PlainHistoryCell {
    let CollabResumeBeginEvent {
        call_id: _,
        sender_thread_id: _,
        receiver_thread_id,
        receiver_agent_nickname,
        receiver_agent_role,
    } = ev;

    collab_event(
        title_with_agent(
            "Resuming",
            AgentLabel {
                thread_id: Some(receiver_thread_id),
                nickname: receiver_agent_nickname.as_deref(),
                role: receiver_agent_role.as_deref(),
            },
            /*spawn_request*/ None,
        ),
        Vec::new(),
    )
}

pub(crate) fn resume_end(ev: CollabResumeEndEvent) -> PlainHistoryCell {
    let CollabResumeEndEvent {
        call_id: _,
        sender_thread_id: _,
        receiver_thread_id,
        receiver_agent_nickname,
        receiver_agent_role,
        status,
    } = ev;

    collab_event(
        title_with_agent(
            "Resumed",
            AgentLabel {
                thread_id: Some(receiver_thread_id),
                nickname: receiver_agent_nickname.as_deref(),
                role: receiver_agent_role.as_deref(),
            },
            /*spawn_request*/ None,
        ),
        vec![status_summary_line(&status)],
    )
}

fn collab_event(title: Line<'static>, details: Vec<Line<'static>>) -> PlainHistoryCell {
    let mut lines: Vec<Line<'static>> = vec![title];
    if !details.is_empty() {
        lines.extend(prefix_lines(details, "  └ ".dim(), "    ".into()));
    }
    PlainHistoryCell::new(lines)
}

fn title_text(title: impl Into<String>) -> Line<'static> {
    title_spans_line(vec![Span::from(title.into()).bold()])
}

fn title_with_agent(
    prefix: &str,
    agent: AgentLabel<'_>,
    spawn_request: Option<&SpawnRequestSummary>,
) -> Line<'static> {
    let mut spans = vec![Span::from(format!("{prefix} ")).bold()];
    spans.extend(agent_label_spans(agent));
    spans.extend(spawn_request_spans(spawn_request));
    title_spans_line(spans)
}

fn title_spans_line(mut spans: Vec<Span<'static>>) -> Line<'static> {
    let mut title = Vec::with_capacity(spans.len() + 1);
    title.push(Span::from("• ").dim());
    title.append(&mut spans);
    title.into()
}

fn agent_label_from_ref(agent: &CollabAgentRef) -> AgentLabel<'_> {
    AgentLabel {
        thread_id: Some(agent.thread_id),
        nickname: agent.agent_nickname.as_deref(),
        role: agent.agent_role.as_deref(),
    }
}

fn agent_label_line(agent: AgentLabel<'_>) -> Line<'static> {
    agent_label_spans(agent).into()
}

fn agent_label_spans(agent: AgentLabel<'_>) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let nickname = agent
        .nickname
        .map(str::trim)
        .filter(|nickname| !nickname.is_empty());
    let role = agent.role.map(str::trim).filter(|role| !role.is_empty());

    if let Some(nickname) = nickname {
        spans.push(Span::from(nickname.to_string()).cyan().bold());
    } else if let Some(thread_id) = agent.thread_id {
        spans.push(Span::from(thread_id.to_string()).cyan());
    } else {
        spans.push(Span::from("agent").cyan());
    }

    if let Some(role) = role {
        spans.push(Span::from(" ").dim());
        spans.push(Span::from(format!("[{role}]")));
    }

    spans
}

fn spawn_request_spans(spawn_request: Option<&SpawnRequestSummary>) -> Vec<Span<'static>> {
    let Some(spawn_request) = spawn_request else {
        return Vec::new();
    };

    let Some(details) = spawn_request_details_text(spawn_request) else {
        return Vec::new();
    };

    vec![Span::from(" ").dim(), Span::from(details).magenta()]
}

fn spawn_request_details_text(spawn_request: &SpawnRequestSummary) -> Option<String> {
    let model = spawn_request.model.trim();
    if model.is_empty() && spawn_request.reasoning_effort == ReasoningEffortConfig::default() {
        return None;
    }

    Some(if model.is_empty() {
        format!("({})", spawn_request.reasoning_effort)
    } else {
        format!("({model} {})", spawn_request.reasoning_effort)
    })
}

fn requested_spawn_request_line(
    requested: Option<&SpawnRequestSummary>,
    effective: &SpawnRequestSummary,
) -> Option<Line<'static>> {
    let requested = requested?;
    if requested.model == effective.model
        && requested.reasoning_effort == effective.reasoning_effort
    {
        return None;
    }

    let requested = spawn_request_details_text(requested)?;
    Some(Line::from(vec![
        "Requested model/reasoning: ".dim(),
        requested.magenta(),
    ]))
}

fn requested_context_inheritance_line(
    requested: Option<SpawnContextInheritanceMode>,
) -> Option<Line<'static>> {
    let requested = requested?;
    if requested == SpawnContextInheritanceMode::Off {
        return None;
    }

    Some(Line::from(vec![
        "Requested context inheritance: ".dim(),
        context_inheritance_mode_text(requested).into(),
    ]))
}

fn prompt_line(prompt: &str) -> Option<Line<'static>> {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(Line::from(trimmed.to_owned()))
    }
}

fn delegation_report_lines(
    report: Option<&DelegationReport>,
    show_not_provided_when_missing: bool,
) -> Vec<Line<'static>> {
    let Some(report) = report else {
        if show_not_provided_when_missing {
            return vec![Line::from(vec!["Delegation: ".dim(), "not provided".dim()])];
        }
        return Vec::new();
    };

    let mut lines = vec![Line::from(vec![
        "Delegation: ".dim(),
        format!(
            "{} | difficulty {}/10 | brief {}/10 | self {}/10 | ~{} min",
            report.general_task_type,
            report.task_difficulty_1_10,
            report.brief_completeness_1_10,
            report.task_self_sufficiency_1_10,
            report.expected_duration_minutes
        )
        .into(),
    ])];

    let output_shape = report.expected_output_shape.trim();
    let files_or_scope = report.files_or_scope.trim();
    let why_this_agent = report.why_this_agent.trim();
    if !why_this_agent.is_empty() {
        lines.push(Line::from(vec![
            "Why this agent: ".dim(),
            why_this_agent.to_owned().into(),
        ]));
    }
    if !output_shape.is_empty() || !files_or_scope.is_empty() {
        let mut details = Vec::new();
        if !output_shape.is_empty() {
            details.push(format!("output: {output_shape}"));
        }
        if !files_or_scope.is_empty() {
            details.push(format!("scope: {files_or_scope}"));
        }
        lines.push(Line::from(vec![
            "Deliverable: ".dim(),
            details.join(" | ").into(),
        ]));
    }

    let risks_or_unknowns = report.risks_or_unknowns.trim();
    if !risks_or_unknowns.is_empty() {
        lines.push(Line::from(vec![
            "Risks: ".dim(),
            risks_or_unknowns.to_owned().into(),
        ]));
    }
    if let Some(orchestration_context) = &report.orchestration_context {
        let mut details = Vec::new();
        if let Some(action_type) = orchestration_context
            .action_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            details.push(format!("action={action_type}"));
        }
        if let Some(production_type) = orchestration_context
            .production_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            details.push(format!("production={production_type}"));
        }
        if let Some(campaign_id) = orchestration_context
            .campaign_id
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            details.push(format!("campaign={campaign_id}"));
        }
        if let Some(phase_id) = orchestration_context
            .phase_id
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            details.push(format!("phase={phase_id}"));
        }
        if let Some(round_id) = orchestration_context
            .round_id
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            details.push(format!("round={round_id}"));
        }
        if let Some(step_id) = orchestration_context
            .step_id
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            details.push(format!("step={step_id}"));
        }
        if let Some(execution_lane) = orchestration_context
            .execution_lane
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            details.push(format!("lane={execution_lane}"));
        }
        if !details.is_empty() {
            lines.push(Line::from(vec![
                "Orchestration: ".dim(),
                details.join(" | ").into(),
            ]));
        }
    }

    lines
}

fn context_inheritance_line(
    requested: Option<SpawnContextInheritanceMode>,
    effective: Option<SpawnContextInheritanceEffectiveMode>,
) -> Option<Line<'static>> {
    let requested = requested?;
    let effective = effective?;
    if requested == SpawnContextInheritanceMode::Off
        && effective == SpawnContextInheritanceEffectiveMode::Off
    {
        return None;
    }

    Some(Line::from(vec![
        "Context inheritance: ".dim(),
        context_inheritance_mode_text(requested).into(),
        " -> ".dim(),
        context_inheritance_effective_mode_text(effective).into(),
    ]))
}

fn context_inheritance_mode_text(mode: SpawnContextInheritanceMode) -> &'static str {
    match mode {
        SpawnContextInheritanceMode::Off => "off",
        SpawnContextInheritanceMode::Exact => "exact",
        SpawnContextInheritanceMode::Bounded => "bounded",
    }
}

fn context_inheritance_effective_mode_text(
    mode: SpawnContextInheritanceEffectiveMode,
) -> &'static str {
    match mode {
        SpawnContextInheritanceEffectiveMode::Off => "off",
        SpawnContextInheritanceEffectiveMode::Exact => "exact",
        SpawnContextInheritanceEffectiveMode::BoundedFull => "bounded_full",
        SpawnContextInheritanceEffectiveMode::BoundedTrimmed => "bounded_trimmed",
        SpawnContextInheritanceEffectiveMode::BoundedSuppressed => "bounded_suppressed",
    }
}

fn context_inheritance_telemetry_lines(
    telemetry: Option<SpawnContextInheritanceTelemetry>,
) -> Vec<Line<'static>> {
    let Some(telemetry) = telemetry else {
        return Vec::new();
    };

    let mut metrics = Vec::new();
    if let Some(parent_turns) = telemetry.parent_replay_safe_turn_count {
        let turns = match telemetry.shipped_replay_safe_turn_count {
            Some(shipped_turns) => format!("turns {parent_turns} -> {shipped_turns}"),
            None => format!("turns {parent_turns} -> -"),
        };
        metrics.push(turns);
    }
    if let Some(shipped_tokens) = telemetry.estimated_shipped_tokens {
        let tokens = match telemetry.usable_context_budget_tokens {
            Some(budget_tokens) => format!("tokens ~{shipped_tokens}/{budget_tokens}"),
            None => format!("tokens ~{shipped_tokens}"),
        };
        metrics.push(tokens);
    } else if let Some(budget_tokens) = telemetry.usable_context_budget_tokens {
        metrics.push(format!("budget {budget_tokens}"));
    }

    let mut lines = Vec::new();
    if !metrics.is_empty() {
        lines.push(Line::from(vec![
            "Budget ".dim(),
            metrics.join(" | ").into(),
        ]));
    }

    if let Some(reason) = telemetry.suppression_reason {
        let reason = match reason {
            SpawnContextInheritanceSuppressionReason::InvalidParentSpawnPairing => {
                "invalid_parent_spawn_pairing"
            }
            SpawnContextInheritanceSuppressionReason::MissingBudgetProxy => "missing_budget_proxy",
            SpawnContextInheritanceSuppressionReason::BudgetExceeded => "budget_exceeded",
        };
        lines.push(Line::from(vec!["Suppressed ".dim(), reason.into()]));
    }

    lines
}

fn merge_wait_receivers(
    receiver_thread_ids: &[ThreadId],
    mut receiver_agents: Vec<CollabAgentRef>,
) -> Vec<CollabAgentRef> {
    if receiver_agents.is_empty() {
        return receiver_thread_ids
            .iter()
            .map(|thread_id| CollabAgentRef {
                thread_id: *thread_id,
                agent_nickname: None,
                agent_role: None,
            })
            .collect();
    }

    let mut seen = receiver_agents
        .iter()
        .map(|agent| agent.thread_id)
        .collect::<HashSet<_>>();
    for thread_id in receiver_thread_ids {
        if seen.insert(*thread_id) {
            receiver_agents.push(CollabAgentRef {
                thread_id: *thread_id,
                agent_nickname: None,
                agent_role: None,
            });
        }
    }
    receiver_agents
}

fn wait_complete_lines(
    statuses: &HashMap<ThreadId, AgentStatus>,
    agent_statuses: &[CollabAgentStatusEntry],
) -> Vec<Line<'static>> {
    if statuses.is_empty() && agent_statuses.is_empty() {
        return vec![Line::from(Span::from("No agents completed yet"))];
    }

    let entries = if agent_statuses.is_empty() {
        let mut entries = statuses
            .iter()
            .map(|(thread_id, status)| CollabAgentStatusEntry {
                thread_id: *thread_id,
                agent_nickname: None,
                agent_role: None,
                status: status.clone(),
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.thread_id.to_string().cmp(&right.thread_id.to_string()));
        entries
    } else {
        let mut entries = agent_statuses.to_vec();
        let seen = entries
            .iter()
            .map(|entry| entry.thread_id)
            .collect::<HashSet<_>>();
        let mut extras = statuses
            .iter()
            .filter(|(thread_id, _)| !seen.contains(thread_id))
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
    };

    entries
        .into_iter()
        .map(|entry| {
            let CollabAgentStatusEntry {
                thread_id,
                agent_nickname,
                agent_role,
                status,
            } = entry;
            let mut spans = agent_label_spans(AgentLabel {
                thread_id: Some(thread_id),
                nickname: agent_nickname.as_deref(),
                role: agent_role.as_deref(),
            });
            spans.push(Span::from(": ").dim());
            spans.extend(status_summary_spans(&status));
            spans.into()
        })
        .collect()
}

fn status_summary_line(status: &AgentStatus) -> Line<'static> {
    status_summary_spans(status).into()
}

fn status_summary_spans(status: &AgentStatus) -> Vec<Span<'static>> {
    match status {
        AgentStatus::PendingInit => vec![Span::from("Pending init").cyan()],
        AgentStatus::Running => vec![Span::from("Running").cyan().bold()],
        // Allow `.yellow()`
        #[allow(clippy::disallowed_methods)]
        AgentStatus::Interrupted => vec![Span::from("Interrupted").yellow()],
        AgentStatus::Completed(message) => {
            let mut spans = vec![Span::from("Completed").green()];
            if let Some(message) = message.as_ref() {
                let message_preview = truncate_text(
                    &message.split_whitespace().collect::<Vec<_>>().join(" "),
                    COLLAB_AGENT_RESPONSE_PREVIEW_GRAPHEMES,
                );
                if !message_preview.is_empty() {
                    spans.push(Span::from(" - ").dim());
                    spans.push(Span::from(message_preview));
                }
            }
            spans
        }
        AgentStatus::Errored(error) => {
            let mut spans = vec![Span::from("Error").red()];
            let error_preview = truncate_text(
                &error.split_whitespace().collect::<Vec<_>>().join(" "),
                COLLAB_AGENT_ERROR_PREVIEW_GRAPHEMES,
            );
            if !error_preview.is_empty() {
                spans.push(Span::from(" - ").dim());
                spans.push(Span::from(error_preview));
            }
            spans
        }
        AgentStatus::Shutdown => vec![Span::from("Shutdown")],
        AgentStatus::NotFound => vec![Span::from("Not found").red()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_cell::HistoryCell;
    #[cfg(target_os = "macos")]
    use crossterm::event::KeyEvent;
    #[cfg(target_os = "macos")]
    use crossterm::event::KeyModifiers;
    use insta::assert_snapshot;
    use pretty_assertions::assert_eq;
    use ratatui::style::Color;
    use ratatui::style::Modifier;

    #[test]
    fn collab_spawn_begin_snapshot() {
        let cell = spawn_begin(
            "Inspect the repo and report the likely failure mode.",
            &SpawnRequestSummary {
                model: "gpt-5".to_string(),
                reasoning_effort: ReasoningEffortConfig::High,
                context_inheritance_requested: Some(SpawnContextInheritanceMode::Bounded),
            },
        );

        assert_snapshot!("collab_spawn_begin", cell_to_text(&cell));
    }

    #[test]
    fn collab_events_snapshot() {
        let sender_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001")
            .expect("valid sender thread id");
        let robie_id = ThreadId::from_string("00000000-0000-0000-0000-000000000002")
            .expect("valid robie thread id");
        let bob_id = ThreadId::from_string("00000000-0000-0000-0000-000000000003")
            .expect("valid bob thread id");

        let spawn = spawn_end(
            CollabAgentSpawnEndEvent {
                call_id: "call-spawn".to_string(),
                sender_thread_id,
                new_thread_id: Some(robie_id),
                new_agent_nickname: Some("Robie".to_string()),
                new_agent_role: Some("explorer".to_string()),
                prompt: "Compute 11! and reply with just the integer result.".to_string(),
                requested_model: "gpt-5".to_string(),
                requested_reasoning_effort: ReasoningEffortConfig::High,
                model: "gpt-5".to_string(),
                reasoning_effort: ReasoningEffortConfig::High,
                context_inheritance_requested: Some(SpawnContextInheritanceMode::Bounded),
                context_inheritance_effective: Some(
                    SpawnContextInheritanceEffectiveMode::BoundedTrimmed,
                ),
                context_inheritance_telemetry: Some(SpawnContextInheritanceTelemetry {
                    parent_replay_safe_turn_count: Some(6),
                    shipped_replay_safe_turn_count: Some(2),
                    estimated_shipped_tokens: Some(18_500),
                    usable_context_budget_tokens: Some(24_000),
                    suppression_reason: None,
                }),
                delegation_report: None,
                status: AgentStatus::PendingInit,
            },
            /*requested_spawn_request*/ None,
        );

        let send = interaction_end(CollabAgentInteractionEndEvent {
            call_id: "call-send".to_string(),
            sender_thread_id,
            receiver_thread_id: robie_id,
            receiver_agent_nickname: Some("Robie".to_string()),
            receiver_agent_role: Some("explorer".to_string()),
            prompt: "Please continue and return the answer only.".to_string(),
            status: AgentStatus::Running,
        });

        let waiting = waiting_begin(CollabWaitingBeginEvent {
            sender_thread_id,
            receiver_thread_ids: vec![robie_id],
            receiver_agents: vec![CollabAgentRef {
                thread_id: robie_id,
                agent_nickname: Some("Robie".to_string()),
                agent_role: Some("explorer".to_string()),
            }],
            call_id: "call-wait".to_string(),
        });

        let mut statuses = HashMap::new();
        statuses.insert(
            robie_id,
            AgentStatus::Completed(Some("39916800".to_string())),
        );
        statuses.insert(bob_id, AgentStatus::Errored("tool timeout".to_string()));
        let finished = waiting_end(CollabWaitingEndEvent {
            sender_thread_id,
            call_id: "call-wait".to_string(),
            agent_statuses: vec![
                CollabAgentStatusEntry {
                    thread_id: robie_id,
                    agent_nickname: Some("Robie".to_string()),
                    agent_role: Some("explorer".to_string()),
                    status: AgentStatus::Completed(Some("39916800".to_string())),
                },
                CollabAgentStatusEntry {
                    thread_id: bob_id,
                    agent_nickname: Some("Bob".to_string()),
                    agent_role: Some("worker".to_string()),
                    status: AgentStatus::Errored("tool timeout".to_string()),
                },
            ],
            statuses,
        });

        let close = close_end(CollabCloseEndEvent {
            call_id: "call-close".to_string(),
            sender_thread_id,
            receiver_thread_id: robie_id,
            receiver_agent_nickname: Some("Robie".to_string()),
            receiver_agent_role: Some("explorer".to_string()),
            status: AgentStatus::Completed(Some("39916800".to_string())),
        });

        let snapshot = [spawn, send, waiting, finished, close]
            .iter()
            .map(cell_to_text)
            .collect::<Vec<_>>()
            .join("\n\n");
        assert_snapshot!("collab_agent_transcript", snapshot);
    }

    #[test]
    fn collab_spawn_end_replay_snapshot() {
        let sender_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000011")
            .expect("valid sender thread id");
        let spawned_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000012")
            .expect("valid spawned thread id");

        let cell = spawn_end(
            CollabAgentSpawnEndEvent {
                call_id: "call-spawn-replay".to_string(),
                sender_thread_id,
                new_thread_id: Some(spawned_thread_id),
                new_agent_nickname: Some("Robie".to_string()),
                new_agent_role: Some("explorer".to_string()),
                prompt: "Inspect the repo and report the likely failure mode.".to_string(),
                requested_model: "gpt-5".to_string(),
                requested_reasoning_effort: ReasoningEffortConfig::High,
                model: "gpt-5-mini".to_string(),
                reasoning_effort: ReasoningEffortConfig::Medium,
                context_inheritance_requested: Some(SpawnContextInheritanceMode::Bounded),
                context_inheritance_effective: Some(
                    SpawnContextInheritanceEffectiveMode::BoundedTrimmed,
                ),
                context_inheritance_telemetry: None,
                delegation_report: Some(codex_protocol::protocol::DelegationReport {
                    general_task_type: "code review".to_string(),
                    task_difficulty_1_10: 6,
                    brief_completeness_1_10: 8,
                    task_self_sufficiency_1_10: 7,
                    expected_duration_minutes: 15,
                    why_this_agent: "Knows this TUI surface".to_string(),
                    expected_output_shape: "Patch + short note".to_string(),
                    files_or_scope: "codex-rs/tui/src/multi_agents.rs".to_string(),
                    risks_or_unknowns: "Snapshot text may change".to_string(),
                    orchestration_context: None,
                }),
                status: AgentStatus::PendingInit,
            },
            /*requested_spawn_request*/ None,
        );

        assert_snapshot!("collab_spawn_end_replay", cell_to_text(&cell));
    }

    #[test]
    fn collab_spawn_end_renders_orchestration_context_details() {
        let sender_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000041")
            .expect("valid sender thread id");
        let spawned_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000042")
            .expect("valid spawned thread id");

        let cell = spawn_end(
            CollabAgentSpawnEndEvent {
                call_id: "call-spawn-orchestration".to_string(),
                sender_thread_id,
                new_thread_id: Some(spawned_thread_id),
                new_agent_nickname: Some("Ampere".to_string()),
                new_agent_role: Some("worker".to_string()),
                prompt: "Inspect and summarize".to_string(),
                requested_model: "gpt-5".to_string(),
                requested_reasoning_effort: ReasoningEffortConfig::High,
                model: "gpt-5-mini".to_string(),
                reasoning_effort: ReasoningEffortConfig::Medium,
                context_inheritance_requested: Some(SpawnContextInheritanceMode::Bounded),
                context_inheritance_effective: Some(
                    SpawnContextInheritanceEffectiveMode::BoundedTrimmed,
                ),
                context_inheritance_telemetry: None,
                delegation_report: Some(codex_protocol::protocol::DelegationReport {
                    general_task_type: "analysis".to_string(),
                    task_difficulty_1_10: 5,
                    brief_completeness_1_10: 8,
                    task_self_sufficiency_1_10: 8,
                    expected_duration_minutes: 12,
                    why_this_agent: "Fast executor".to_string(),
                    expected_output_shape: "Short summary".to_string(),
                    files_or_scope: "repo root".to_string(),
                    risks_or_unknowns: "none".to_string(),
                    orchestration_context: Some(
                        codex_protocol::protocol::DelegationOrchestrationContext {
                            action_type: Some("review".to_string()),
                            production_type: Some("ops".to_string()),
                            campaign_id: Some("NERO".to_string()),
                            phase_id: Some("12".to_string()),
                            round_id: Some("r1".to_string()),
                            step_id: Some("s2".to_string()),
                            execution_lane: Some("analysis".to_string()),
                        },
                    ),
                }),
                status: AgentStatus::PendingInit,
            },
            /*requested_spawn_request*/ None,
        );

        let rendered = cell_to_text(&cell);
        assert!(
            rendered.contains(
                "Orchestration: action=review | production=ops | campaign=NERO | phase=12 | round=r1 | step=s2 | lane=analysis"
            ),
            "expected orchestration context line in spawn-end rendering, got: {rendered}"
        );
    }

    #[test]
    fn collab_spawn_end_dedup_hides_not_provided_delegation_line() {
        let sender_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000031")
            .expect("valid sender thread id");
        let spawned_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000032")
            .expect("valid spawned thread id");
        let requested_spawn_request = SpawnRequestSummary {
            model: "gpt-5.4".to_string(),
            reasoning_effort: ReasoningEffortConfig::High,
            context_inheritance_requested: Some(SpawnContextInheritanceMode::Bounded),
        };

        let cell = spawn_end(
            CollabAgentSpawnEndEvent {
                call_id: "call-spawn-dedup".to_string(),
                sender_thread_id,
                new_thread_id: Some(spawned_thread_id),
                new_agent_nickname: Some("Copernicus".to_string()),
                new_agent_role: Some("reviewer-senior".to_string()),
                prompt: "Review the patch and report critical regressions.".to_string(),
                requested_model: "gpt-5.4".to_string(),
                requested_reasoning_effort: ReasoningEffortConfig::High,
                model: "gpt-5.4".to_string(),
                reasoning_effort: ReasoningEffortConfig::High,
                context_inheritance_requested: Some(SpawnContextInheritanceMode::Bounded),
                context_inheritance_effective: Some(
                    SpawnContextInheritanceEffectiveMode::BoundedFull,
                ),
                context_inheritance_telemetry: None,
                delegation_report: None,
                status: AgentStatus::PendingInit,
            },
            /*requested_spawn_request*/ Some(&requested_spawn_request),
        );

        let rendered = cell_to_text(&cell);
        assert!(!rendered.contains("Delegation: not provided"));
    }

    #[test]
    fn collab_spawn_begin_hides_default_context_inheritance_off() {
        let cell = spawn_begin(
            "Inspect the repo and report the likely failure mode.",
            &SpawnRequestSummary {
                model: "gpt-5".to_string(),
                reasoning_effort: ReasoningEffortConfig::High,
                context_inheritance_requested: Some(SpawnContextInheritanceMode::Off),
            },
        );

        let rendered = cell_to_text(&cell);
        assert!(!rendered.contains("Requested context inheritance"));
    }

    #[test]
    fn collab_spawn_end_hides_default_context_inheritance_mapping() {
        let sender_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000021")
            .expect("valid sender thread id");
        let spawned_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000022")
            .expect("valid spawned thread id");

        let cell = spawn_end(
            CollabAgentSpawnEndEvent {
                call_id: "call-spawn-off".to_string(),
                sender_thread_id,
                new_thread_id: Some(spawned_thread_id),
                new_agent_nickname: Some("Robie".to_string()),
                new_agent_role: Some("explorer".to_string()),
                prompt: "Inspect the repo and report the likely failure mode.".to_string(),
                requested_model: "gpt-5".to_string(),
                requested_reasoning_effort: ReasoningEffortConfig::High,
                model: "gpt-5".to_string(),
                reasoning_effort: ReasoningEffortConfig::High,
                context_inheritance_requested: Some(SpawnContextInheritanceMode::Off),
                context_inheritance_effective: Some(SpawnContextInheritanceEffectiveMode::Off),
                context_inheritance_telemetry: None,
                delegation_report: None,
                status: AgentStatus::PendingInit,
            },
            /*requested_spawn_request*/ None,
        );

        let rendered = cell_to_text(&cell);
        assert!(!rendered.contains("Requested context inheritance"));
        assert!(!rendered.contains("Context off -> off"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn agent_shortcut_matches_option_arrow_word_motion_fallbacks_only_when_allowed() {
        assert!(previous_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Left, KeyModifiers::ALT),
            /*allow_word_motion_fallback*/ false,
        ));
        assert!(next_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Right, KeyModifiers::ALT),
            /*allow_word_motion_fallback*/ false,
        ));
        assert!(previous_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT),
            /*allow_word_motion_fallback*/ true,
        ));
        assert!(next_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT),
            /*allow_word_motion_fallback*/ true,
        ));
        assert!(!previous_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT),
            /*allow_word_motion_fallback*/ false,
        ));
        assert!(!next_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT),
            /*allow_word_motion_fallback*/ false,
        ));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn agent_shortcut_matches_option_arrows_only() {
        assert!(previous_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Left, crossterm::event::KeyModifiers::ALT,),
            /*allow_word_motion_fallback*/ false
        ));
        assert!(next_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Right, crossterm::event::KeyModifiers::ALT,),
            /*allow_word_motion_fallback*/ false
        ));
        assert!(!previous_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Char('b'), crossterm::event::KeyModifiers::ALT,),
            /*allow_word_motion_fallback*/ false
        ));
        assert!(!next_agent_shortcut_matches(
            KeyEvent::new(KeyCode::Char('f'), crossterm::event::KeyModifiers::ALT,),
            /*allow_word_motion_fallback*/ false
        ));
    }

    #[test]
    fn title_styles_nickname_and_role() {
        let sender_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001")
            .expect("valid sender thread id");
        let robie_id = ThreadId::from_string("00000000-0000-0000-0000-000000000002")
            .expect("valid robie thread id");
        let cell = spawn_end(
            CollabAgentSpawnEndEvent {
                call_id: "call-spawn".to_string(),
                sender_thread_id,
                new_thread_id: Some(robie_id),
                new_agent_nickname: Some("Robie".to_string()),
                new_agent_role: Some("explorer".to_string()),
                prompt: String::new(),
                requested_model: "gpt-5".to_string(),
                requested_reasoning_effort: ReasoningEffortConfig::High,
                model: "gpt-5".to_string(),
                reasoning_effort: ReasoningEffortConfig::High,
                context_inheritance_requested: Some(SpawnContextInheritanceMode::Exact),
                context_inheritance_effective: Some(SpawnContextInheritanceEffectiveMode::Exact),
                context_inheritance_telemetry: Some(SpawnContextInheritanceTelemetry {
                    parent_replay_safe_turn_count: Some(3),
                    shipped_replay_safe_turn_count: Some(3),
                    estimated_shipped_tokens: Some(14_000),
                    usable_context_budget_tokens: None,
                    suppression_reason: None,
                }),
                delegation_report: None,
                status: AgentStatus::PendingInit,
            },
            /*requested_spawn_request*/ None,
        );

        let lines = cell.display_lines(/*width*/ 200);
        let title = &lines[0];
        assert_eq!(title.spans[2].content.as_ref(), "Robie");
        assert_eq!(title.spans[2].style.fg, Some(Color::Cyan));
        assert!(title.spans[2].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(title.spans[4].content.as_ref(), "[explorer]");
        assert_eq!(title.spans[4].style.fg, None);
        assert!(!title.spans[4].style.add_modifier.contains(Modifier::DIM));
        assert_eq!(title.spans[6].content.as_ref(), "(gpt-5 high)");
        assert_eq!(title.spans[6].style.fg, Some(Color::Magenta));
    }

    #[test]
    fn collab_resume_interrupted_snapshot() {
        let sender_thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001")
            .expect("valid sender thread id");
        let robie_id = ThreadId::from_string("00000000-0000-0000-0000-000000000002")
            .expect("valid robie thread id");

        let cell = resume_end(CollabResumeEndEvent {
            call_id: "call-resume".to_string(),
            sender_thread_id,
            receiver_thread_id: robie_id,
            receiver_agent_nickname: Some("Robie".to_string()),
            receiver_agent_role: Some("explorer".to_string()),
            status: AgentStatus::Interrupted,
        });

        assert_snapshot!("collab_resume_interrupted", cell_to_text(&cell));
    }

    fn cell_to_text(cell: &PlainHistoryCell) -> String {
        cell.display_lines(/*width*/ 200)
            .iter()
            .map(line_to_text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn line_to_text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("")
    }
}

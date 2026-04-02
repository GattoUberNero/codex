use std::path::PathBuf;

use codex_protocol::ThreadId;
use codex_protocol::protocol::HookCompletedEvent;
use codex_protocol::protocol::HookEventName;
use codex_protocol::protocol::HookOutputEntry;
use codex_protocol::protocol::HookOutputEntryKind;
use codex_protocol::protocol::HookRunStatus;
use codex_protocol::protocol::HookRunSummary;

use super::common;
use crate::HookCompactionTrigger;
use crate::engine::CommandShell;
use crate::engine::ConfiguredHandler;
use crate::engine::command_runner::CommandRunResult;
use crate::engine::dispatcher;
use crate::engine::output_parser;
use crate::schema::AfterCompactionCommandInput;
use crate::schema::NullableString;

#[derive(Debug, Clone)]
pub struct AfterCompactionRequest {
    pub session_id: ThreadId,
    pub turn_id: String,
    pub cwd: PathBuf,
    pub transcript_path: Option<PathBuf>,
    pub model: String,
    pub permission_mode: String,
    pub trigger: HookCompactionTrigger,
}

#[derive(Debug)]
pub struct AfterCompactionOutcome {
    pub hook_events: Vec<HookCompletedEvent>,
    pub additional_contexts: Vec<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct AfterCompactionHandlerData {
    additional_contexts_for_model: Vec<String>,
}

pub(crate) fn preview(
    handlers: &[ConfiguredHandler],
    _request: &AfterCompactionRequest,
) -> Vec<HookRunSummary> {
    dispatcher::select_handlers(
        handlers,
        HookEventName::AfterCompaction,
        /*matcher_input*/ None,
    )
    .into_iter()
    .map(|handler| dispatcher::running_summary(&handler))
    .collect()
}

pub(crate) async fn run(
    handlers: &[ConfiguredHandler],
    shell: &CommandShell,
    request: AfterCompactionRequest,
) -> AfterCompactionOutcome {
    let matched = dispatcher::select_handlers(
        handlers,
        HookEventName::AfterCompaction,
        /*matcher_input*/ None,
    );
    if matched.is_empty() {
        return AfterCompactionOutcome {
            hook_events: Vec::new(),
            additional_contexts: Vec::new(),
        };
    }

    let input_json = match serde_json::to_string(&AfterCompactionCommandInput {
        session_id: request.session_id.to_string(),
        turn_id: request.turn_id.clone(),
        transcript_path: NullableString::from_path(request.transcript_path.clone()),
        cwd: request.cwd.display().to_string(),
        hook_event_name: "AfterCompaction".to_string(),
        model: request.model.clone(),
        permission_mode: request.permission_mode.clone(),
        trigger: compaction_trigger_label(request.trigger).to_string(),
    }) {
        Ok(input_json) => input_json,
        Err(error) => {
            return serialization_failure_outcome(common::serialization_failure_hook_events(
                matched,
                Some(request.turn_id),
                format!("failed to serialize after compaction hook input: {error}"),
            ));
        }
    };

    let results = dispatcher::execute_handlers(
        shell,
        matched,
        input_json,
        request.cwd.as_path(),
        Some(request.turn_id),
        parse_completed,
    )
    .await;

    let additional_contexts = common::flatten_additional_contexts(
        results
            .iter()
            .map(|result| result.data.additional_contexts_for_model.as_slice()),
    );

    AfterCompactionOutcome {
        hook_events: results.into_iter().map(|result| result.completed).collect(),
        additional_contexts,
    }
}

fn parse_completed(
    handler: &ConfiguredHandler,
    run_result: CommandRunResult,
    turn_id: Option<String>,
) -> dispatcher::ParsedHandler<AfterCompactionHandlerData> {
    let mut entries = Vec::new();
    let mut status = HookRunStatus::Completed;
    let mut additional_contexts_for_model = Vec::new();

    match run_result.error.as_deref() {
        Some(error) => {
            status = HookRunStatus::Failed;
            entries.push(HookOutputEntry {
                kind: HookOutputEntryKind::Error,
                text: error.to_string(),
            });
        }
        None => match run_result.exit_code {
            Some(0) => {
                let trimmed_stdout = run_result.stdout.trim();
                if trimmed_stdout.is_empty() {
                } else if let Some(parsed) =
                    output_parser::parse_after_compaction(&run_result.stdout)
                {
                    if let Some(invalid_reason) = parsed.invalid_reason {
                        status = HookRunStatus::Failed;
                        entries.push(HookOutputEntry {
                            kind: HookOutputEntryKind::Error,
                            text: invalid_reason,
                        });
                    } else {
                        if let Some(system_message) = parsed.universal.system_message {
                            entries.push(HookOutputEntry {
                                kind: HookOutputEntryKind::Warning,
                                text: system_message,
                            });
                        }
                        if let Some(additional_context) = parsed.additional_context {
                            common::append_additional_context(
                                &mut entries,
                                &mut additional_contexts_for_model,
                                additional_context,
                            );
                        }
                    }
                } else if trimmed_stdout.starts_with('{') || trimmed_stdout.starts_with('[') {
                    status = HookRunStatus::Failed;
                    entries.push(HookOutputEntry {
                        kind: HookOutputEntryKind::Error,
                        text: "hook returned invalid after compaction JSON output".to_string(),
                    });
                } else {
                    common::append_additional_context(
                        &mut entries,
                        &mut additional_contexts_for_model,
                        trimmed_stdout.to_string(),
                    );
                }
            }
            Some(exit_code) => {
                status = HookRunStatus::Failed;
                entries.push(HookOutputEntry {
                    kind: HookOutputEntryKind::Error,
                    text: format!("hook exited with code {exit_code}"),
                });
            }
            None => {
                status = HookRunStatus::Failed;
                entries.push(HookOutputEntry {
                    kind: HookOutputEntryKind::Error,
                    text: "hook exited without a status code".to_string(),
                });
            }
        },
    }

    let completed = HookCompletedEvent {
        turn_id,
        run: dispatcher::completed_summary(handler, &run_result, status, entries),
    };

    dispatcher::ParsedHandler {
        completed,
        data: AfterCompactionHandlerData {
            additional_contexts_for_model,
        },
    }
}

fn serialization_failure_outcome(hook_events: Vec<HookCompletedEvent>) -> AfterCompactionOutcome {
    AfterCompactionOutcome {
        hook_events,
        additional_contexts: Vec::new(),
    }
}

fn compaction_trigger_label(trigger: HookCompactionTrigger) -> &'static str {
    match trigger {
        HookCompactionTrigger::Auto => "auto",
        HookCompactionTrigger::Manual => "manual",
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use codex_protocol::protocol::HookOutputEntry;
    use pretty_assertions::assert_eq;

    use super::AfterCompactionHandlerData;
    use super::parse_completed;
    use crate::HookCompactionTrigger;
    use crate::engine::ConfiguredHandler;
    use crate::engine::command_runner::CommandRunResult;

    fn command_result(stdout: &str) -> CommandRunResult {
        CommandRunResult {
            stdout: stdout.to_string(),
            stderr: String::new(),
            exit_code: Some(0),
            started_at: 1,
            completed_at: 2,
            duration_ms: 1,
            error: None,
        }
    }

    fn handler() -> ConfiguredHandler {
        ConfiguredHandler {
            event_name: codex_protocol::protocol::HookEventName::AfterCompaction,
            matcher: None,
            command: "python3 hook.py".to_string(),
            timeout_sec: 30,
            status_message: Some("running after compaction hook".to_string()),
            source_path: PathBuf::from("/tmp/hooks.json"),
            display_order: 0,
        }
    }

    #[test]
    fn plain_text_stdout_becomes_additional_context() {
        let parsed = parse_completed(
            &handler(),
            command_result("keep the migration checkpoint visible"),
            Some("turn-1".to_string()),
        );

        assert_eq!(
            parsed.data,
            AfterCompactionHandlerData {
                additional_contexts_for_model: vec![
                    "keep the migration checkpoint visible".to_string()
                ],
            }
        );
        assert_eq!(
            parsed.completed.run.entries,
            vec![HookOutputEntry {
                kind: codex_protocol::protocol::HookOutputEntryKind::Context,
                text: "keep the migration checkpoint visible".to_string(),
            }]
        );
    }

    #[test]
    fn unsupported_continue_false_is_reported_as_error() {
        let parsed = parse_completed(
            &handler(),
            command_result(r#"{"continue":false,"stopReason":"pause"}"#),
            Some("turn-2".to_string()),
        );

        assert_eq!(parsed.data, AfterCompactionHandlerData::default());
        assert_eq!(
            parsed.completed.run.entries,
            vec![HookOutputEntry {
                kind: codex_protocol::protocol::HookOutputEntryKind::Error,
                text: "AfterCompaction hook returned unsupported continue:false".to_string(),
            }]
        );
    }

    #[test]
    fn compaction_trigger_labels_are_stable() {
        assert_eq!(
            super::compaction_trigger_label(HookCompactionTrigger::Auto),
            "auto"
        );
        assert_eq!(
            super::compaction_trigger_label(HookCompactionTrigger::Manual),
            "manual"
        );
    }
}

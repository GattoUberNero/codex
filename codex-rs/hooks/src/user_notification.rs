use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::io::AsyncReadExt;
use tracing::warn;

use crate::Hook;
use crate::HookExecution;
use crate::HookEvent;
use crate::HookPayload;
use crate::HookResult;
use crate::command_from_argv;
use crate::parse_hook_actions_from_stdout;

const LEGACY_NOTIFY_TIMEOUT: Duration = Duration::from_millis(1500);
const LEGACY_NOTIFY_KILL_REAP_TIMEOUT: Duration = Duration::from_millis(250);

/// Legacy notify payload appended as the final argv argument for backward compatibility.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum UserNotification {
    #[serde(rename_all = "kebab-case")]
    AgentTurnComplete {
        thread_id: String,
        turn_id: String,
        cwd: String,

        /// Messages that the user sent to the agent to initiate the turn.
        input_messages: Vec<String>,

        /// The last message sent by the assistant in the turn.
        last_assistant_message: Option<String>,
    },
}

pub fn legacy_notify_json(hook_event: &HookEvent, cwd: &Path) -> Result<String, serde_json::Error> {
    match hook_event {
        HookEvent::AfterAgent { event } => {
            serde_json::to_string(&UserNotification::AgentTurnComplete {
                thread_id: event.thread_id.to_string(),
                turn_id: event.turn_id.clone(),
                cwd: cwd.display().to_string(),
                input_messages: event.input_messages.clone(),
                last_assistant_message: event.last_assistant_message.clone(),
            })
        }
        _ => Err(serde_json::Error::io(std::io::Error::other(
            "legacy notify payload is only supported for after_agent",
        ))),
    }
}

pub fn notify_hook(argv: Vec<String>) -> Hook {
    let argv = Arc::new(argv);
    Hook {
        name: "legacy_notify".to_string(),
        func: Arc::new(move |payload: &HookPayload| {
            let argv = Arc::clone(&argv);
            Box::pin(async move {
                let mut command = match command_from_argv(&argv) {
                    Some(command) => command,
                    None => {
                        return HookExecution {
                            result: HookResult::Success,
                            actions: Vec::new(),
                        };
                    }
                };
                if let Ok(notify_payload) = legacy_notify_json(&payload.hook_event, &payload.cwd) {
                    command.arg(notify_payload);
                }

                // Backwards-compat payload shape is preserved (argv + JSON arg).
                // We await completion so hooks can optionally emit JSON actions on stdout.
                command
                    .stdin(Stdio::null())
                    .stderr(Stdio::null());

                command.stdout(Stdio::piped());

                let mut child = match command.spawn() {
                    Ok(child) => child,
                    Err(err) => {
                        return HookExecution {
                            result: HookResult::FailedContinue(err.into()),
                            actions: Vec::new(),
                        };
                    }
                };

                let stdout_task = child.stdout.take().map(|mut stdout| {
                    tokio::spawn(async move {
                        let mut buf = Vec::new();
                        let _ = stdout.read_to_end(&mut buf).await;
                        buf
                    })
                });

                let status = match tokio::time::timeout(LEGACY_NOTIFY_TIMEOUT, child.wait()).await {
                    Ok(Ok(status)) => status,
                    Ok(Err(err)) => {
                        if let Some(task) = stdout_task {
                            task.abort();
                        }
                        return HookExecution {
                            result: HookResult::FailedContinue(err.into()),
                            actions: Vec::new(),
                        };
                    }
                    Err(_) => {
                        let _ = child.start_kill();
                        let _ = tokio::time::timeout(LEGACY_NOTIFY_KILL_REAP_TIMEOUT, child.wait())
                            .await;
                        if let Some(task) = stdout_task {
                            task.abort();
                        }
                        warn!(
                            timeout_ms = LEGACY_NOTIFY_TIMEOUT.as_millis() as u64,
                            kill_reap_timeout_ms = LEGACY_NOTIFY_KILL_REAP_TIMEOUT.as_millis() as u64,
                            "legacy_notify hook timed out; attempted to kill/reap direct child and continuing without actions"
                        );
                        return HookExecution {
                            result: HookResult::Success,
                            actions: Vec::new(),
                        };
                    }
                };

                let stdout_bytes = match stdout_task {
                    Some(task) => {
                        let mut task = task;
                        match tokio::time::timeout(LEGACY_NOTIFY_TIMEOUT, &mut task).await {
                            Ok(Ok(buf)) => buf,
                            Ok(Err(_join_err)) => Vec::new(),
                            Err(_) => {
                                task.abort();
                                warn!(
                                    timeout_ms = LEGACY_NOTIFY_TIMEOUT.as_millis() as u64,
                                    "legacy_notify stdout reader timed out; continuing without actions"
                                );
                                Vec::new()
                            }
                        }
                    }
                    None => Vec::new(),
                };

                let actions = match String::from_utf8(stdout_bytes) {
                    Ok(stdout) => match parse_hook_actions_from_stdout(&stdout) {
                        Ok(parsed) => parsed.actions,
                        Err(err) if stdout.trim_start().starts_with('{') => {
                            return HookExecution {
                                result: HookResult::FailedContinue(err.into()),
                                actions: Vec::new(),
                            };
                        }
                        Err(_) => Vec::new(),
                    },
                    Err(err) => {
                        return HookExecution {
                            result: HookResult::FailedContinue(err.into()),
                            actions: Vec::new(),
                        };
                    }
                };

                if status.success() {
                    HookExecution {
                        result: HookResult::Success,
                        actions,
                    }
                } else {
                    HookExecution {
                        result: HookResult::FailedContinue(
                            std::io::Error::other(format!(
                                "hook command exited with status {}",
                                status
                            ))
                            .into(),
                        ),
                        actions,
                    }
                }
            })
        }),
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use codex_protocol::ThreadId;
    use pretty_assertions::assert_eq;
    use serde_json::Value;
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    fn expected_notification_json() -> Value {
        json!({
            "type": "agent-turn-complete",
            "thread-id": "b5f6c1c2-1111-2222-3333-444455556666",
            "turn-id": "12345",
            "cwd": "/Users/example/project",
            "input-messages": ["Rename `foo` to `bar` and update the callsites."],
            "last-assistant-message": "Rename complete and verified `cargo build` succeeds.",
        })
    }

    #[test]
    fn test_user_notification() -> Result<()> {
        let notification = UserNotification::AgentTurnComplete {
            thread_id: "b5f6c1c2-1111-2222-3333-444455556666".to_string(),
            turn_id: "12345".to_string(),
            cwd: "/Users/example/project".to_string(),
            input_messages: vec!["Rename `foo` to `bar` and update the callsites.".to_string()],
            last_assistant_message: Some(
                "Rename complete and verified `cargo build` succeeds.".to_string(),
            ),
        };
        let serialized = serde_json::to_string(&notification)?;
        let actual: Value = serde_json::from_str(&serialized)?;
        assert_eq!(actual, expected_notification_json());
        Ok(())
    }

    #[test]
    fn legacy_notify_json_matches_historical_wire_shape() -> Result<()> {
        let hook_event = HookEvent::AfterAgent {
            event: crate::HookEventAfterAgent {
                thread_id: ThreadId::from_string("b5f6c1c2-1111-2222-3333-444455556666")
                    .expect("valid thread id"),
                turn_id: "12345".to_string(),
                input_messages: vec!["Rename `foo` to `bar` and update the callsites.".to_string()],
                last_assistant_message: Some(
                    "Rename complete and verified `cargo build` succeeds.".to_string(),
                ),
            },
        };

        let serialized = legacy_notify_json(&hook_event, Path::new("/Users/example/project"))?;
        let actual: Value = serde_json::from_str(&serialized)?;
        assert_eq!(actual, expected_notification_json());

        Ok(())
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn notify_hook_parses_actions_from_stdout() -> Result<()> {
        let hook = notify_hook(vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf '%s' '{\"actions\":[{\"type\":\"visible_note\",\"message\":\"hello\"}]}'"
                .to_string(),
        ]);

        let payload = HookPayload {
            session_id: ThreadId::new(),
            cwd: tempdir()?.path().to_path_buf(),
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterAgent {
                event: crate::HookEventAfterAgent {
                    thread_id: ThreadId::new(),
                    turn_id: "turn-x".to_string(),
                    input_messages: vec!["hi".to_string()],
                    last_assistant_message: Some("done".to_string()),
                },
            },
        };

        let outcome = hook.execute(&payload).await;
        assert!(matches!(outcome.result, HookResult::Success));
        assert_eq!(
            outcome.actions,
            vec![crate::HookAction::VisibleNote {
                message: "hello".to_string()
            }]
        );
        Ok(())
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn notify_hook_ignores_plain_stdout_for_compat() -> Result<()> {
        let hook = notify_hook(vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf '%s' 'legacy-notifier-ok'".to_string(),
        ]);

        let payload = HookPayload {
            session_id: ThreadId::new(),
            cwd: tempdir()?.path().to_path_buf(),
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterAgent {
                event: crate::HookEventAfterAgent {
                    thread_id: ThreadId::new(),
                    turn_id: "turn-y".to_string(),
                    input_messages: vec!["hi".to_string()],
                    last_assistant_message: Some("done".to_string()),
                },
            },
        };

        let outcome = hook.execute(&payload).await;
        assert!(matches!(outcome.result, HookResult::Success));
        assert!(outcome.actions.is_empty());
        Ok(())
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn notify_hook_times_out_fail_open() -> Result<()> {
        let hook = notify_hook(vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "sleep 3; printf '%s' '{\"actions\":[{\"type\":\"visible_note\",\"message\":\"late\"}]}'"
                .to_string(),
        ]);

        let payload = HookPayload {
            session_id: ThreadId::new(),
            cwd: tempdir()?.path().to_path_buf(),
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterAgent {
                event: crate::HookEventAfterAgent {
                    thread_id: ThreadId::new(),
                    turn_id: "turn-timeout".to_string(),
                    input_messages: vec!["hi".to_string()],
                    last_assistant_message: Some("done".to_string()),
                },
            },
        };

        let started = std::time::Instant::now();
        let outcome = hook.execute(&payload).await;
        let elapsed = started.elapsed();

        assert!(matches!(outcome.result, HookResult::Success));
        assert!(outcome.actions.is_empty());
        assert!(elapsed < Duration::from_secs(3));
        Ok(())
    }
}

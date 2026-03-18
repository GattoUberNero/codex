use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use std::{io, io::ErrorKind};

use serde::Serialize;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tracing::debug;
use tracing::warn;

use crate::Hook;
use crate::HookEvent;
use crate::HookExecution;
use crate::HookPayload;
use crate::HookResult;
use crate::command_from_argv;
use crate::parse_hook_actions_from_stdout;

const LEGACY_NOTIFY_TIMEOUT: Duration = Duration::from_millis(1500);
const LEGACY_NOTIFY_KILL_REAP_TIMEOUT: Duration = Duration::from_millis(250);
const LEGACY_NOTIFY_STDIN_WRITE_BASE_TIMEOUT_MS: u64 = 4_000;
const LEGACY_NOTIFY_STDIN_WRITE_PER_64KB_TIMEOUT_MS: u64 = 250;
const LEGACY_NOTIFY_STDIN_WRITE_MAX_TIMEOUT_MS: u64 = 10_000;

fn legacy_notify_timeout_error(stage: &str, timeout: Duration) -> io::Error {
    io::Error::new(
        ErrorKind::TimedOut,
        format!(
            "legacy notify {stage} timed out after {}ms",
            timeout.as_millis()
        ),
    )
}

fn legacy_notify_stdin_write_timeout(payload_bytes: usize) -> Duration {
    // Writing large payloads to stdin can block when the child consumes input
    // slower than pipe buffering; scale timeout with payload size.
    const PIPE_CHUNK_BYTES: usize = 64 * 1024;
    let chunks = payload_bytes.saturating_add(PIPE_CHUNK_BYTES.saturating_sub(1)) / PIPE_CHUNK_BYTES;
    let extra_ms = chunks
        .saturating_sub(1)
        .saturating_mul(LEGACY_NOTIFY_STDIN_WRITE_PER_64KB_TIMEOUT_MS as usize);
    let timeout_ms = LEGACY_NOTIFY_STDIN_WRITE_BASE_TIMEOUT_MS
        .saturating_add(extra_ms as u64)
        .min(LEGACY_NOTIFY_STDIN_WRITE_MAX_TIMEOUT_MS);
    Duration::from_millis(timeout_ms)
}

async fn collect_stdout_bytes(
    stdout_task: Option<tokio::task::JoinHandle<Vec<u8>>>,
    timeout: Duration,
) -> Result<Vec<u8>, io::Error> {
    match stdout_task {
        Some(task) => {
            let mut task = task;
            match tokio::time::timeout(timeout, &mut task).await {
                Ok(Ok(buf)) => Ok(buf),
                Ok(Err(join_err)) => Err(io::Error::other(format!(
                    "legacy notify stdout reader task failed: {join_err}"
                ))),
                Err(_) => {
                    task.abort();
                    Err(legacy_notify_timeout_error("stdout reader", timeout))
                }
            }
        }
        None => Ok(Vec::new()),
    }
}

fn parse_legacy_notify_stdout(stdout_bytes: Vec<u8>) -> Result<Vec<crate::HookAction>, io::Error> {
    match String::from_utf8(stdout_bytes) {
        Ok(stdout) => match parse_hook_actions_from_stdout(&stdout) {
            Ok(parsed) => {
                debug!(
                    hook_name = "legacy_notify",
                    parsed_actions = parsed.actions.len(),
                    ignored_unknown_actions = parsed.ignored_unknown_actions,
                    stdout_len = stdout.len(),
                    "parsed hook actions from legacy notify stdout"
                );
                Ok(parsed.actions)
            }
            Err(err) if stdout.trim_start().starts_with('{') => {
                debug!(
                    hook_name = "legacy_notify",
                    stdout_len = stdout.len(),
                    error = %err,
                    "legacy notify stdout looked like JSON but actions parsing failed"
                );
                Err(io::Error::other(err.to_string()))
            }
            Err(_) => {
                debug!(
                    hook_name = "legacy_notify",
                    stdout_len = stdout.len(),
                    "legacy notify stdout ignored (compat plain text)"
                );
                Ok(Vec::new())
            }
        },
        Err(err) => Err(io::Error::new(ErrorKind::InvalidData, err)),
    }
}

/// Notify payload sent to the external hook process over stdin.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum UserNotification {
    #[serde(rename_all = "kebab-case")]
    AgentTurnComplete {
        thread_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_name: Option<String>,
        turn_id: String,
        cwd: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        client: Option<String>,

        /// Messages that the user sent to the agent to initiate the turn.
        input_messages: Vec<String>,

        /// The last message sent by the assistant in the turn.
        last_assistant_message: Option<String>,
    },
}

pub fn legacy_notify_json(payload: &HookPayload) -> Result<String, serde_json::Error> {
    match &payload.hook_event {
        HookEvent::AfterAgent { event } => {
            serde_json::to_string(&UserNotification::AgentTurnComplete {
                thread_id: event.thread_id.to_string(),
                thread_name: event.thread_name.clone(),
                turn_id: event.turn_id.clone(),
                cwd: payload.cwd.display().to_string(),
                client: payload.client.clone(),
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
                let notify_payload = legacy_notify_json(payload).ok();
                let base_command_argv = argv.as_ref().clone();
                if command_from_argv(&base_command_argv).is_none() {
                    return HookExecution {
                        result: HookResult::Success,
                        actions: Vec::new(),
                    };
                }
                let mut command_argv = base_command_argv.clone();
                let payload_in_argv = if let Some(notify_payload) = notify_payload.as_ref() {
                    // Preserve the historical argv + JSON contract for legacy hooks
                    // while also streaming the payload over stdin for larger/newer hooks.
                    command_argv.push(notify_payload.clone());
                    true
                } else {
                    false
                };

                let build_command = |argv_values: &[String]| -> Option<tokio::process::Command> {
                    let mut command = command_from_argv(argv_values)?;
                    if let Some(session_source) = payload.session_source.as_deref() {
                        command.env("NERO_HOOK_SESSION_SOURCE", session_source);
                    }
                    if let Some(session_agent_role) = payload.session_agent_role.as_deref() {
                        command.env("NERO_HOOK_SESSION_AGENT_ROLE", session_agent_role);
                    }
                    if let Some(nero_auto_runtime) = payload.nero_auto_runtime {
                        command.env(
                            "NERO_HOOK_AUTO_ENABLED",
                            if nero_auto_runtime.enabled { "1" } else { "0" },
                        );
                        command.env(
                            "NERO_HOOK_AUTO_AUTONOMY_LEVEL",
                            nero_auto_runtime.autonomy_level.to_string(),
                        );
                        command.env(
                            "NERO_HOOK_AUTO_MAX_ROUNDS",
                            nero_auto_runtime.max_auto_rounds.to_string(),
                        );
                    }
                    command.stdin(Stdio::piped()).stderr(Stdio::null());
                    command.stdout(Stdio::piped());
                    Some(command)
                };

                let spawn_with_argv =
                    |argv_values: &[String]| -> io::Result<tokio::process::Child> {
                        let mut command = build_command(argv_values).ok_or_else(|| {
                            io::Error::new(
                                ErrorKind::InvalidInput,
                                "missing legacy notify command argv",
                            )
                        })?;
                        command.spawn()
                    };

                let mut using_stdin_only_payload = false;
                let mut child = match spawn_with_argv(&command_argv) {
                    Ok(child) => child,
                    Err(spawn_with_payload_err)
                        if payload_in_argv
                            && should_retry_without_argv_payload(&spawn_with_payload_err) =>
                    {
                        warn!(
                            hook_name = "legacy_notify",
                            payload_bytes = notify_payload.as_ref().map_or(0, |v| v.len()),
                            error = %spawn_with_payload_err,
                            "legacy notify spawn with argv payload failed; retrying with stdin-only payload"
                        );
                        match spawn_with_argv(&base_command_argv) {
                            Ok(child) => {
                                using_stdin_only_payload = true;
                                child
                            }
                            Err(retry_err) => {
                                let combined = io::Error::other(format!(
                                    "legacy notify spawn failed with argv payload: {spawn_with_payload_err}; stdin-only retry failed: {retry_err}"
                                ));
                                return HookExecution {
                                    result: HookResult::FailedContinue(combined.into()),
                                    actions: Vec::new(),
                                };
                            }
                        }
                    }
                    Err(err) => {
                        return HookExecution {
                            result: HookResult::FailedContinue(err.into()),
                            actions: Vec::new(),
                        };
                    }
                };

                debug!(
                    hook_name = "legacy_notify",
                    argv0 = argv.first().map(String::as_str).unwrap_or(""),
                    argv_len = if using_stdin_only_payload {
                        base_command_argv.len()
                    } else {
                        command_argv.len()
                    },
                    using_stdin_only_payload,
                    "spawning legacy notify hook process"
                );

                if let Some(mut stdin) = child.stdin.take() {
                    if let Some(notify_payload) = notify_payload {
                        let payload_bytes = notify_payload.len();
                        let stdin_write_timeout =
                            legacy_notify_stdin_write_timeout(payload_bytes);
                        let stdin_payload_authoritative = using_stdin_only_payload;
                        match tokio::time::timeout(
                            stdin_write_timeout,
                            stdin.write_all(notify_payload.as_bytes()),
                        )
                        .await
                        {
                            Ok(Ok(())) => {}
                            Ok(Err(err)) if err.kind() == std::io::ErrorKind::BrokenPipe => {
                                if stdin_payload_authoritative {
                                    // In stdin-only fallback mode this payload is authoritative.
                                    // BrokenPipe means the hook could not have consumed the full input.
                                    let _ = child.start_kill();
                                    let _ = tokio::time::timeout(
                                        LEGACY_NOTIFY_KILL_REAP_TIMEOUT,
                                        child.wait(),
                                    )
                                    .await;
                                    return HookExecution {
                                        result: HookResult::FailedContinue(
                                            io::Error::new(
                                                ErrorKind::BrokenPipe,
                                                "legacy notify stdin closed before payload was fully consumed in stdin-only fallback",
                                            )
                                            .into(),
                                        ),
                                        actions: Vec::new(),
                                    };
                                }
                                debug!(
                                    hook_name = "legacy_notify",
                                    "legacy notify stdin closed before payload was fully consumed"
                                );
                            }
                            Ok(Err(err)) => {
                                if stdin_payload_authoritative {
                                    if let Some(stdout) = child.stdout.take() {
                                        drop(stdout);
                                    }
                                    let _ = child.start_kill();
                                    let _ = tokio::time::timeout(
                                        LEGACY_NOTIFY_KILL_REAP_TIMEOUT,
                                        child.wait(),
                                    )
                                    .await;
                                    return HookExecution {
                                        result: HookResult::FailedContinue(err.into()),
                                        actions: Vec::new(),
                                    };
                                }
                                warn!(
                                    hook_name = "legacy_notify",
                                    payload_bytes,
                                    error = %err,
                                    "legacy_notify stdin write failed in argv-compat mode; continuing because payload is already provided via argv"
                                );
                            }
                            Err(_) => {
                                if stdin_payload_authoritative {
                                    let _ = child.start_kill();
                                    let _ = tokio::time::timeout(
                                        LEGACY_NOTIFY_KILL_REAP_TIMEOUT,
                                        child.wait(),
                                    )
                                    .await;
                                    warn!(
                                        timeout_ms = stdin_write_timeout.as_millis() as u64,
                                        payload_bytes,
                                        kill_reap_timeout_ms =
                                            LEGACY_NOTIFY_KILL_REAP_TIMEOUT.as_millis() as u64,
                                        "legacy_notify stdin writer timed out; attempted to kill/reap direct child and marking hook as failed_continue"
                                    );
                                    return HookExecution {
                                        result: HookResult::FailedContinue(
                                            legacy_notify_timeout_error(
                                                "stdin writer",
                                                stdin_write_timeout,
                                            )
                                            .into(),
                                        ),
                                        actions: Vec::new(),
                                    };
                                }
                                warn!(
                                    timeout_ms = stdin_write_timeout.as_millis() as u64,
                                    payload_bytes,
                                    "legacy_notify stdin writer timed out in argv-compat mode; continuing because payload is already provided via argv"
                                );
                            }
                        }
                    }
                }

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
                        let stdout_bytes = match collect_stdout_bytes(
                            stdout_task,
                            LEGACY_NOTIFY_KILL_REAP_TIMEOUT,
                        )
                        .await
                        {
                            Ok(buf) => buf,
                            Err(err) => {
                                warn!(
                                    timeout_ms = LEGACY_NOTIFY_TIMEOUT.as_millis() as u64,
                                    kill_reap_timeout_ms =
                                        LEGACY_NOTIFY_KILL_REAP_TIMEOUT.as_millis() as u64,
                                    error = %err,
                                    "legacy_notify hook timed out and stdout recovery failed"
                                );
                                return HookExecution {
                                    result: HookResult::FailedContinue(
                                        legacy_notify_timeout_error(
                                            "hook process",
                                            LEGACY_NOTIFY_TIMEOUT,
                                        )
                                        .into(),
                                    ),
                                    actions: Vec::new(),
                                };
                            }
                        };
                        if !stdout_bytes.is_empty() {
                            match parse_legacy_notify_stdout(stdout_bytes) {
                                Ok(actions) => {
                                    warn!(
                                        timeout_ms = LEGACY_NOTIFY_TIMEOUT.as_millis() as u64,
                                        kill_reap_timeout_ms =
                                            LEGACY_NOTIFY_KILL_REAP_TIMEOUT.as_millis() as u64,
                                        recovered_actions = actions.len(),
                                        "legacy_notify hook timed out after stdout activity; recovered actions after killing direct child"
                                    );
                                    return HookExecution {
                                        result: HookResult::Success,
                                        actions,
                                    };
                                }
                                Err(err) => {
                                    warn!(
                                        timeout_ms = LEGACY_NOTIFY_TIMEOUT.as_millis() as u64,
                                        kill_reap_timeout_ms =
                                            LEGACY_NOTIFY_KILL_REAP_TIMEOUT.as_millis() as u64,
                                        error = %err,
                                        "legacy_notify hook timed out after stdout activity, but recovered stdout could not be parsed"
                                    );
                                    return HookExecution {
                                        result: HookResult::FailedContinue(
                                            io::Error::other(format!(
                                                "{}; recovered stdout parse failed: {err}",
                                                legacy_notify_timeout_error(
                                                    "hook process",
                                                    LEGACY_NOTIFY_TIMEOUT,
                                                )
                                            ))
                                            .into(),
                                        ),
                                        actions: Vec::new(),
                                    };
                                }
                            }
                        }
                        warn!(
                            timeout_ms = LEGACY_NOTIFY_TIMEOUT.as_millis() as u64,
                            kill_reap_timeout_ms =
                                LEGACY_NOTIFY_KILL_REAP_TIMEOUT.as_millis() as u64,
                            "legacy_notify hook timed out before any stdout actions were recoverable; marking hook as failed_continue"
                        );
                        return HookExecution {
                            result: HookResult::FailedContinue(
                                legacy_notify_timeout_error("hook process", LEGACY_NOTIFY_TIMEOUT)
                                    .into(),
                            ),
                            actions: Vec::new(),
                        };
                    }
                };

                let stdout_bytes = match collect_stdout_bytes(stdout_task, LEGACY_NOTIFY_TIMEOUT)
                    .await
                {
                    Ok(buf) => buf,
                    Err(err) if err.kind() == ErrorKind::TimedOut => {
                        warn!(
                            timeout_ms = LEGACY_NOTIFY_TIMEOUT.as_millis() as u64,
                            "legacy_notify stdout reader timed out; marking hook as failed_continue"
                        );
                        return HookExecution {
                            result: HookResult::FailedContinue(err.into()),
                            actions: Vec::new(),
                        };
                    }
                    Err(err) => {
                        return HookExecution {
                            result: HookResult::FailedContinue(err.into()),
                            actions: Vec::new(),
                        };
                    }
                };

                let actions = match parse_legacy_notify_stdout(stdout_bytes) {
                    Ok(actions) => actions,
                    Err(err) => {
                        return HookExecution {
                            result: HookResult::FailedContinue(err.into()),
                            actions: Vec::new(),
                        };
                    }
                };

                if status.success() {
                    debug!(
                        hook_name = "legacy_notify",
                        actions_count = actions.len(),
                        "legacy notify hook completed successfully"
                    );
                    HookExecution {
                        result: HookResult::Success,
                        actions,
                    }
                } else {
                    debug!(hook_name = "legacy_notify", status = %status, "legacy notify hook exited non-zero");
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

fn should_retry_without_argv_payload(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        ErrorKind::ArgumentListTooLong | ErrorKind::InvalidInput
    )
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
            "client": "codex-tui",
            "input-messages": ["Rename `foo` to `bar` and update the callsites."],
            "last-assistant-message": "Rename complete and verified `cargo build` succeeds.",
        })
    }

    #[test]
    fn test_user_notification() -> Result<()> {
        let notification = UserNotification::AgentTurnComplete {
            thread_id: "b5f6c1c2-1111-2222-3333-444455556666".to_string(),
            thread_name: None,
            turn_id: "12345".to_string(),
            cwd: "/Users/example/project".to_string(),
            client: Some("codex-tui".to_string()),
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
        let payload = HookPayload {
            session_id: ThreadId::new(),
            cwd: std::path::Path::new("/Users/example/project").to_path_buf(),
            client: Some("codex-tui".to_string()),
            session_source: None,
            session_agent_role: None,
            nero_auto_runtime: None,
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterAgent {
                event: crate::HookEventAfterAgent {
                    thread_id: ThreadId::from_string("b5f6c1c2-1111-2222-3333-444455556666")
                        .expect("valid thread id"),
                    thread_name: None,
                    turn_id: "12345".to_string(),
                    input_messages: vec![
                        "Rename `foo` to `bar` and update the callsites.".to_string(),
                    ],
                    last_assistant_message: Some(
                        "Rename complete and verified `cargo build` succeeds.".to_string(),
                    ),
                },
            },
        };

        let serialized = legacy_notify_json(&payload)?;
        let actual: Value = serde_json::from_str(&serialized)?;
        assert_eq!(actual, expected_notification_json());

        Ok(())
    }

    #[test]
    fn legacy_notify_stdin_timeout_scales_with_payload_and_caps() {
        const CHUNK: usize = 64 * 1024;
        assert_eq!(
            legacy_notify_stdin_write_timeout(0),
            Duration::from_millis(LEGACY_NOTIFY_STDIN_WRITE_BASE_TIMEOUT_MS)
        );
        assert_eq!(
            legacy_notify_stdin_write_timeout(CHUNK),
            Duration::from_millis(LEGACY_NOTIFY_STDIN_WRITE_BASE_TIMEOUT_MS)
        );
        assert_eq!(
            legacy_notify_stdin_write_timeout(CHUNK + 1),
            Duration::from_millis(
                LEGACY_NOTIFY_STDIN_WRITE_BASE_TIMEOUT_MS
                    + LEGACY_NOTIFY_STDIN_WRITE_PER_64KB_TIMEOUT_MS
            )
        );
        assert_eq!(
            legacy_notify_stdin_write_timeout(2 * CHUNK),
            Duration::from_millis(
                LEGACY_NOTIFY_STDIN_WRITE_BASE_TIMEOUT_MS
                    + LEGACY_NOTIFY_STDIN_WRITE_PER_64KB_TIMEOUT_MS
            )
        );
        assert_eq!(
            legacy_notify_stdin_write_timeout(usize::MAX),
            Duration::from_millis(LEGACY_NOTIFY_STDIN_WRITE_MAX_TIMEOUT_MS)
        );
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
            client: None,
            session_source: None,
            session_agent_role: None,
            nero_auto_runtime: None,
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterAgent {
                event: crate::HookEventAfterAgent {
                    thread_id: ThreadId::new(),
                    thread_name: None,
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
    async fn notify_hook_writes_payload_to_stdin() -> Result<()> {
        let hook = notify_hook(vec![
            "python3".to_string(),
            "-c".to_string(),
            "import json,sys; payload=json.load(sys.stdin); print(json.dumps({'actions':[{'type':'visible_note','message': payload.get('thread-name') or 'missing'}]}), end='')".to_string(),
        ]);

        let payload = HookPayload {
            session_id: ThreadId::new(),
            cwd: tempdir()?.path().to_path_buf(),
            client: None,
            session_source: None,
            session_agent_role: None,
            nero_auto_runtime: None,
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterAgent {
                event: crate::HookEventAfterAgent {
                    thread_id: ThreadId::new(),
                    thread_name: Some("example-A-".to_string()),
                    turn_id: "turn-stdin".to_string(),
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
                message: "example-A-".to_string()
            }]
        );
        Ok(())
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn notify_hook_preserves_legacy_payload_as_cli_arg() -> Result<()> {
        let hook = notify_hook(vec![
            "python3".to_string(),
            "-c".to_string(),
            "import json,sys; payload=json.loads(sys.argv[-1]); print(json.dumps({'actions':[{'type':'visible_note','message': payload.get('thread-name') or 'missing'}]}), end='')".to_string(),
        ]);

        let payload = HookPayload {
            session_id: ThreadId::new(),
            cwd: tempdir()?.path().to_path_buf(),
            client: None,
            session_source: None,
            session_agent_role: None,
            nero_auto_runtime: None,
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterAgent {
                event: crate::HookEventAfterAgent {
                    thread_id: ThreadId::new(),
                    thread_name: Some("legacy-argv".to_string()),
                    turn_id: "turn-argv".to_string(),
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
                message: "legacy-argv".to_string()
            }]
        );
        Ok(())
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn notify_hook_handles_large_payload_over_stdin() -> Result<()> {
        let hook = notify_hook(vec![
            "python3".to_string(),
            "-c".to_string(),
            "import json,sys; payload=json.load(sys.stdin); msg=payload.get('last-assistant-message') or ''; print(json.dumps({'actions':[{'type':'visible_note','message': str(len(msg))}]}), end='')".to_string(),
        ]);

        let large_message = "x".repeat(300_000);
        let payload = HookPayload {
            session_id: ThreadId::new(),
            cwd: tempdir()?.path().to_path_buf(),
            client: None,
            session_source: None,
            session_agent_role: None,
            nero_auto_runtime: None,
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterAgent {
                event: crate::HookEventAfterAgent {
                    thread_id: ThreadId::new(),
                    thread_name: Some("example-A-".to_string()),
                    turn_id: "turn-large".to_string(),
                    input_messages: vec!["hi".to_string()],
                    last_assistant_message: Some(large_message.clone()),
                },
            },
        };

        let outcome = hook.execute(&payload).await;
        assert!(matches!(outcome.result, HookResult::Success));
        assert_eq!(
            outcome.actions,
            vec![crate::HookAction::VisibleNote {
                message: large_message.len().to_string()
            }]
        );
        Ok(())
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn notify_hook_falls_back_to_stdin_when_argv_payload_exceeds_os_limit() -> Result<()> {
        let hook = notify_hook(vec![
            "python3".to_string(),
            "-c".to_string(),
            "import json,sys; payload=json.load(sys.stdin); msg=payload.get('last-assistant-message') or ''; print(json.dumps({'actions':[{'type':'visible_note','message': str(len(msg))}]}), end='')".to_string(),
        ]);

        // Deliberately larger than typical ARG_MAX to exercise the argv->stdin retry path.
        let huge_message = "x".repeat(3_000_000);
        let payload = HookPayload {
            session_id: ThreadId::new(),
            cwd: tempdir()?.path().to_path_buf(),
            client: None,
            session_source: None,
            session_agent_role: None,
            nero_auto_runtime: None,
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterAgent {
                event: crate::HookEventAfterAgent {
                    thread_id: ThreadId::new(),
                    thread_name: Some("example-A-".to_string()),
                    turn_id: "turn-huge".to_string(),
                    input_messages: vec!["hi".to_string()],
                    last_assistant_message: Some(huge_message.clone()),
                },
            },
        };

        let outcome = hook.execute(&payload).await;
        assert!(matches!(outcome.result, HookResult::Success));
        assert_eq!(
            outcome.actions,
            vec![crate::HookAction::VisibleNote {
                message: huge_message.len().to_string()
            }]
        );
        Ok(())
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn notify_hook_fails_when_stdin_only_payload_hits_broken_pipe() -> Result<()> {
        let hook = notify_hook(vec![
            "python3".to_string(),
            "-c".to_string(),
            "import os,time; os.close(0); time.sleep(1)".to_string(),
        ]);

        // Force argv->stdin fallback path, then close stdin in child.
        let huge_message = "x".repeat(3_000_000);
        let payload = HookPayload {
            session_id: ThreadId::new(),
            cwd: tempdir()?.path().to_path_buf(),
            client: None,
            session_source: None,
            session_agent_role: None,
            nero_auto_runtime: None,
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterAgent {
                event: crate::HookEventAfterAgent {
                    thread_id: ThreadId::new(),
                    thread_name: Some("example-A-".to_string()),
                    turn_id: "turn-stdin-only-broken-pipe".to_string(),
                    input_messages: vec!["hi".to_string()],
                    last_assistant_message: Some(huge_message),
                },
            },
        };

        let outcome = hook.execute(&payload).await;
        assert!(outcome.actions.is_empty());
        match outcome.result {
            HookResult::FailedContinue(err) => {
                let io_err = err.downcast_ref::<std::io::Error>();
                assert!(io_err.is_some(), "{err}");
                let io_err = io_err.expect("downcast io::Error");
                assert_eq!(io_err.kind(), std::io::ErrorKind::BrokenPipe);
                let detail = io_err.to_string();
                assert!(
                    detail.contains("stdin-only fallback"),
                    "{detail}"
                );
            }
            other => panic!("expected FailedContinue, got {other:?}"),
        }
        Ok(())
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn notify_hook_times_out_when_stdin_consumer_stalls_as_failed_continue() -> Result<()> {
        let hook = notify_hook(vec![
            "python3".to_string(),
            "-c".to_string(),
            "import time; time.sleep(6)".to_string(),
        ]);

        let payload = HookPayload {
            session_id: ThreadId::new(),
            cwd: tempdir()?.path().to_path_buf(),
            client: None,
            session_source: None,
            session_agent_role: None,
            nero_auto_runtime: None,
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterAgent {
                event: crate::HookEventAfterAgent {
                    thread_id: ThreadId::new(),
                    thread_name: Some("example-A-".to_string()),
                    turn_id: "turn-stalled-stdin".to_string(),
                    input_messages: vec!["hi".to_string()],
                    // Keep payload large enough to fill pipe buffering, but bounded so
                    // the scaled stdin timeout stays below the child sleep.
                    last_assistant_message: Some("x".repeat(300_000)),
                },
            },
        };

        let started = std::time::Instant::now();
        let outcome = hook.execute(&payload).await;
        let elapsed = started.elapsed();

        assert!(matches!(outcome.result, HookResult::FailedContinue(_)));
        assert!(outcome.actions.is_empty());
        assert!(elapsed < Duration::from_secs(6));
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
            client: None,
            session_source: None,
            session_agent_role: None,
            nero_auto_runtime: None,
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterAgent {
                event: crate::HookEventAfterAgent {
                    thread_id: ThreadId::new(),
                    thread_name: None,
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
    async fn notify_hook_times_out_before_stdout_as_failed_continue() -> Result<()> {
        let hook = notify_hook(vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "sleep 3; printf '%s' '{\"actions\":[{\"type\":\"visible_note\",\"message\":\"late\"}]}'"
                .to_string(),
        ]);

        let payload = HookPayload {
            session_id: ThreadId::new(),
            cwd: tempdir()?.path().to_path_buf(),
            client: None,
            session_source: None,
            session_agent_role: None,
            nero_auto_runtime: None,
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterAgent {
                event: crate::HookEventAfterAgent {
                    thread_id: ThreadId::new(),
                    thread_name: None,
                    turn_id: "turn-timeout".to_string(),
                    input_messages: vec!["hi".to_string()],
                    last_assistant_message: Some("done".to_string()),
                },
            },
        };

        let started = std::time::Instant::now();
        let outcome = hook.execute(&payload).await;
        let elapsed = started.elapsed();

        assert!(matches!(outcome.result, HookResult::FailedContinue(_)));
        assert!(outcome.actions.is_empty());
        assert!(elapsed < Duration::from_secs(3));
        Ok(())
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn notify_hook_recovers_actions_when_process_hangs_after_emitting_stdout() -> Result<()> {
        let hook = notify_hook(vec![
            "python3".to_string(),
            "-c".to_string(),
            "import sys,time; sys.stdout.write('{\"actions\":[{\"type\":\"visible_note\",\"message\":\"late-but-buffered\"}]}'); sys.stdout.flush(); time.sleep(3)".to_string(),
        ]);

        let payload = HookPayload {
            session_id: ThreadId::new(),
            cwd: tempdir()?.path().to_path_buf(),
            client: None,
            session_source: None,
            session_agent_role: None,
            nero_auto_runtime: None,
            triggered_at: chrono::Utc::now(),
            hook_event: HookEvent::AfterAgent {
                event: crate::HookEventAfterAgent {
                    thread_id: ThreadId::new(),
                    thread_name: None,
                    turn_id: "turn-timeout-buffered-stdout".to_string(),
                    input_messages: vec!["hi".to_string()],
                    last_assistant_message: Some("done".to_string()),
                },
            },
        };

        let started = std::time::Instant::now();
        let outcome = hook.execute(&payload).await;
        let elapsed = started.elapsed();

        assert!(matches!(outcome.result, HookResult::Success));
        assert_eq!(
            outcome.actions,
            vec![crate::HookAction::VisibleNote {
                message: "late-but-buffered".to_string()
            }]
        );
        assert!(elapsed < Duration::from_secs(3));
        Ok(())
    }
}

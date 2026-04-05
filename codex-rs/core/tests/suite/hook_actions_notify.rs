#![cfg(not(target_os = "windows"))]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use codex_features::Feature;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::user_input::UserInput;
use core_test_support::fs_wait;
use core_test_support::responses;
use core_test_support::test_codex::TestCodexHarness;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::json;
use serial_test::serial;
use tempfile::TempDir;
use tracing_subscriber::EnvFilter;

use responses::ev_assistant_message;
use responses::ev_completed;
use responses::sse;

fn skip_if_no_linux_sandbox_bin() -> bool {
    #[cfg(target_os = "linux")]
    {
        if codex_utils_cargo_bin::cargo_bin("codex-linux-sandbox").is_err() {
            eprintln!("skipping hook_actions_notify e2e: codex-linux-sandbox binary not built");
            return true;
        }
    }
    false
}

fn write_notify_script(contents: &str) -> Result<String> {
    let dir = TempDir::new()?;
    let script = dir.path().join("notify.sh");
    std::fs::write(&script, contents)?;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
    // Leak tempdir for test process lifetime so the script path stays valid.
    let path = script.to_string_lossy().to_string();
    let _leaked = Box::leak(Box::new(dir));
    Ok(path)
}

fn write_stop_hook(home: &Path, command: &str) -> Result<()> {
    let hooks = json!({
        "hooks": {
            "Stop": [{
                "hooks": [{
                    "type": "command",
                    "command": command,
                    "statusMessage": "running stop hook",
                }]
            }]
        }
    });
    std::fs::write(home.join("hooks.json"), hooks.to_string())?;
    Ok(())
}

fn init_test_tracing() {
    let Ok(env_filter) = EnvFilter::try_from_default_env() else {
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter(env_filter)
        .try_init();
}

async fn submit_user_turn_no_wait(test: &TestCodexHarness, text: &str) -> Result<()> {
    let session_model = test.test().session_configured.model.clone();
    test.test()
        .codex
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: text.into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: test.cwd().to_path_buf(),
            approval_policy: AskForApproval::Never,
            approvals_reviewer: None,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            model: session_model,
            effort: None,
            summary: Some(ReasoningSummary::Auto),
            service_tier: None,
            collaboration_mode: None,
            personality: None,
        })
        .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_agent_visible_note_emits_warning_and_turn_completes() -> Result<()> {
    init_test_tracing();
    if skip_if_no_linux_sandbox_bin() {
        return Ok(());
    }
    let hook_dir = TempDir::new()?;
    let marker = hook_dir.path().join("visible.marker");
    let marker_str = marker.to_string_lossy().to_string();
    let script = write_notify_script(&format!(
        "#!/bin/bash\n: > \"{marker_str}\"\nprintf '%s' '{{\"actions\":[{{\"type\":\"visible_note\",\"message\":\"e2e visible\"}}]}}'\n"
    ))?;

    let test = TestCodexHarness::with_builder(
        core_test_support::test_codex::test_codex().with_config(move |cfg| {
            cfg.notify = Some(vec![script]);
        }),
    )
    .await?;
    responses::mount_sse_once(
        test.server(),
        sse(vec![ev_assistant_message("m1", "Done"), ev_completed("r1")]),
    )
    .await;

    submit_user_turn_no_wait(&test, "hello").await?;
    fs_wait::wait_for_path_exists(&marker, Duration::from_secs(5)).await?;

    let warning = wait_for_event(
        &test.test().codex,
        |ev| matches!(ev, EventMsg::Warning(w) if w.message.contains("[nero-hook] e2e visible")),
    )
    .await;
    assert!(
        matches!(warning, EventMsg::Warning(_)),
        "expected [nero-hook] visible_note warning"
    );

    let complete = wait_for_event(&test.test().codex, |ev| {
        matches!(ev, EventMsg::TurnComplete(_))
    })
    .await;
    assert!(matches!(complete, EventMsg::TurnComplete(_)));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_agent_nero_hook_msg_block_status_emits_structured_warning_and_turn_completes()
-> Result<()> {
    init_test_tracing();
    if skip_if_no_linux_sandbox_bin() {
        return Ok(());
    }
    let hook_dir = TempDir::new()?;
    let marker = hook_dir.path().join("nero_hook_msg_block.marker");
    let marker_str = marker.to_string_lossy().to_string();
    let script = write_notify_script(&format!(
        "#!/bin/bash\n: > \"{marker_str}\"\nprintf '%s' '{{\"actions\":[{{\"type\":\"nero_hook_msg\",\"mode\":\"tui-short\",\"show\":{{\"agent\":false,\"tui\":true}},\"format\":\"block\",\"msg\":{{\"full\":\"unused full\",\"short\":\"Structured e2e block\"}},\"status\":{{\"kind\":\"countdown\",\"text\":\"next update in 17s\"}}}}]}}'\n"
    ))?;

    let test = TestCodexHarness::with_builder(
        core_test_support::test_codex::test_codex().with_config(move |cfg| {
            cfg.notify = Some(vec![script]);
        }),
    )
    .await?;
    responses::mount_sse_once(
        test.server(),
        sse(vec![ev_assistant_message("m1", "Done"), ev_completed("r1")]),
    )
    .await;

    submit_user_turn_no_wait(&test, "hello nero_hook_msg block").await?;
    fs_wait::wait_for_path_exists(&marker, Duration::from_secs(5)).await?;

    let warning = wait_for_event(&test.test().codex, |ev| {
        matches!(ev, EventMsg::Warning(w)
            if w.message.contains("[nero-hook]\n------------\ncontent = Structured e2e block")
                && w.message.contains("\n------------\nstatus = countdown: next update in 17s"))
    })
    .await;
    assert!(
        matches!(warning, EventMsg::Warning(_)),
        "expected structured [nero-hook] warning with content+status"
    );

    let complete = wait_for_event(&test.test().codex, |ev| {
        matches!(ev, EventMsg::TurnComplete(_))
    })
    .await;
    assert!(matches!(complete, EventMsg::TurnComplete(_)));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_agent_plain_stdout_keeps_normal_flow() -> Result<()> {
    init_test_tracing();
    if skip_if_no_linux_sandbox_bin() {
        return Ok(());
    }
    let script = write_notify_script(
        r#"#!/bin/bash
printf '%s' 'notifier-ok'
"#,
    )?;

    let test = TestCodexHarness::with_builder(
        core_test_support::test_codex::test_codex().with_config(move |cfg| {
            cfg.notify = Some(vec![script]);
        }),
    )
    .await?;
    responses::mount_sse_once(
        test.server(),
        sse(vec![ev_assistant_message("m1", "Done"), ev_completed("r1")]),
    )
    .await;

    submit_user_turn_no_wait(&test, "hello plain stdout").await?;

    let _complete = wait_for_event(&test.test().codex, |ev| {
        matches!(ev, EventMsg::TurnComplete(_))
    })
    .await;

    // No extra warning expected for plain stdout actions path.
    let warning = tokio::time::timeout(
        Duration::from_millis(200),
        wait_for_event(&test.test().codex, |ev| matches!(ev, EventMsg::Warning(_))),
    )
    .await;
    assert!(
        warning.is_err(),
        "did not expect hook warning for plain stdout"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_agent_garbage_json_does_not_crash_turn_flow() -> Result<()> {
    init_test_tracing();
    if skip_if_no_linux_sandbox_bin() {
        return Ok(());
    }
    let script = write_notify_script(
        r#"#!/bin/bash
printf '%s' '{not-json'
"#,
    )?;

    let test = TestCodexHarness::with_builder(
        core_test_support::test_codex::test_codex().with_config(move |cfg| {
            cfg.notify = Some(vec![script]);
        }),
    )
    .await?;
    responses::mount_sse_once(
        test.server(),
        sse(vec![ev_assistant_message("m1", "Done"), ev_completed("r1")]),
    )
    .await;

    submit_user_turn_no_wait(&test, "hello garbage").await?;
    let _complete = wait_for_event(&test.test().codex, |ev| {
        matches!(ev, EventMsg::TurnComplete(_))
    })
    .await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_agent_both_actions_can_trigger_follow_up_turn_without_manual_input() -> Result<()> {
    init_test_tracing();
    if skip_if_no_linux_sandbox_bin() {
        return Ok(());
    }
    let hook_dir = TempDir::new()?;
    let marker = hook_dir.path().join("both.marker");
    let marker_str = marker.to_string_lossy().to_string();
    let once_marker = hook_dir.path().join("both.once");
    let once_marker_str = once_marker.to_string_lossy().to_string();
    let stop_script = write_notify_script(
        r#"#!/bin/bash
set -euo pipefail
payload="$(cat)"
if printf '%s' "$payload" | grep -q '"stop_hook_active":true'; then
  printf '%s' '{"decision":"continue"}'
else
  printf '%s' '{"decision":"block","reason":"both-actions checkpoint"}'
fi
"#,
    )?;
    let script = write_notify_script(&format!(
        "#!/bin/bash\nset -euo pipefail\n: > \"{marker_str}\"\nif [ -f \"{once_marker_str}\" ]; then\n  printf '%s' '{{\"actions\":[{{\"type\":\"visible_note\",\"message\":\"e2e both\"}}]}}'\nelse\n  : > \"{once_marker_str}\"\n  printf '%s' '{{\"actions\":[{{\"type\":\"visible_note\",\"message\":\"e2e both\"}},{{\"type\":\"auto_user_reply\",\"message\":\"continue\"}}]}}'\nfi\n"
    ))?;
    let stop_command = format!("bash {stop_script}");

    let test = TestCodexHarness::with_builder(
        core_test_support::test_codex::test_codex()
            .with_pre_build_hook(move |home| {
                if let Err(error) = write_stop_hook(home, &stop_command) {
                    panic!("failed to write stop hook fixture: {error}");
                }
            })
            .with_config(move |cfg| {
                cfg.notify = Some(vec![script]);
                cfg.features
                    .enable(Feature::CodexHooks)
                    .expect("test config should allow feature update");
            }),
    )
    .await?;
    responses::mount_sse_once(
        test.server(),
        sse(vec![ev_assistant_message("m1", "Done"), ev_completed("r1")]),
    )
    .await;
    responses::mount_sse_once(
        test.server(),
        sse(vec![
            ev_assistant_message("m2", "Done 2"),
            ev_completed("r2"),
        ]),
    )
    .await;
    responses::mount_sse_once(
        test.server(),
        sse(vec![
            ev_assistant_message("m3", "Done 3"),
            ev_completed("r3"),
        ]),
    )
    .await;

    submit_user_turn_no_wait(&test, "hello both").await?;
    fs_wait::wait_for_path_exists(&marker, Duration::from_secs(5)).await?;

    let warning = wait_for_event(
        &test.test().codex,
        |ev| matches!(ev, EventMsg::Warning(w) if w.message.contains("[nero-hook] e2e both")),
    )
    .await;
    assert!(matches!(warning, EventMsg::Warning(_)));

    let _first_complete = wait_for_event(&test.test().codex, |ev| {
        matches!(ev, EventMsg::TurnComplete(_))
    })
    .await;
    let second_complete = tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.test().codex, |ev| {
            matches!(ev, EventMsg::TurnComplete(_))
        }),
    )
    .await;
    assert!(
        second_complete.is_ok(),
        "expected second TurnComplete from auto_user_reply follow-up in no-race e2e test"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_agent_auto_user_reply_only_can_trigger_follow_up_turn_without_manual_input()
-> Result<()> {
    init_test_tracing();
    if skip_if_no_linux_sandbox_bin() {
        return Ok(());
    }
    let hook_dir = TempDir::new()?;
    let marker = hook_dir.path().join("auto_only.marker");
    let marker_str = marker.to_string_lossy().to_string();
    let once_marker = hook_dir.path().join("auto_only.once");
    let once_marker_str = once_marker.to_string_lossy().to_string();
    let stop_script = write_notify_script(
        r#"#!/bin/bash
set -euo pipefail
payload="$(cat)"
if printf '%s' "$payload" | grep -q '"stop_hook_active":true'; then
  printf '%s' '{"decision":"continue"}'
else
  printf '%s' '{"decision":"block","reason":"auto-only checkpoint"}'
fi
"#,
    )?;
    let script = write_notify_script(&format!(
        "#!/bin/bash\nset -euo pipefail\n: > \"{marker_str}\"\nif [ -f \"{once_marker_str}\" ]; then\n  printf '%s' '{{\"actions\":[]}}'\nelse\n  : > \"{once_marker_str}\"\n  printf '%s' '{{\"actions\":[{{\"type\":\"auto_user_reply\",\"message\":\"continue\"}}]}}'\nfi\n"
    ))?;
    let stop_command = format!("bash {stop_script}");

    let test = TestCodexHarness::with_builder(
        core_test_support::test_codex::test_codex()
            .with_pre_build_hook(move |home| {
                if let Err(error) = write_stop_hook(home, &stop_command) {
                    panic!("failed to write stop hook fixture: {error}");
                }
            })
            .with_config(move |cfg| {
                cfg.notify = Some(vec![script]);
                cfg.features
                    .enable(Feature::CodexHooks)
                    .expect("test config should allow feature update");
            }),
    )
    .await?;
    responses::mount_sse_once(
        test.server(),
        sse(vec![ev_assistant_message("m1", "Done"), ev_completed("r1")]),
    )
    .await;
    responses::mount_sse_once(
        test.server(),
        sse(vec![
            ev_assistant_message("m2", "Done 2"),
            ev_completed("r2"),
        ]),
    )
    .await;
    responses::mount_sse_once(
        test.server(),
        sse(vec![
            ev_assistant_message("m3", "Done 3"),
            ev_completed("r3"),
        ]),
    )
    .await;

    submit_user_turn_no_wait(&test, "hello auto").await?;
    fs_wait::wait_for_path_exists(&marker, Duration::from_secs(5)).await?;

    let _first_complete = wait_for_event(&test.test().codex, |ev| {
        matches!(ev, EventMsg::TurnComplete(_))
    })
    .await;
    let second_complete = tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.test().codex, |ev| {
            matches!(ev, EventMsg::TurnComplete(_))
        }),
    )
    .await;
    assert!(
        second_complete.is_ok(),
        "expected second TurnComplete from auto_user_reply-only follow-up in no-race e2e test"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_agent_runtime_delivery_contract_blocks_auto_enqueue_when_msg_is_tui_only()
-> Result<()> {
    init_test_tracing();
    if skip_if_no_linux_sandbox_bin() {
        return Ok(());
    }
    let script = write_notify_script(
        r#"#!/bin/bash
printf '%s' '{"actions":[{"type":"nero_hook_msg","mode":"tui-short","show":{"agent":false,"tui":true},"format":"block","status":{"kind":"warning","text":"runtime alert"},"msg":{"full":"runtime full","short":"runtime short"}},{"type":"auto_user_reply","message":"continue after tui-only"}]}'
"#,
    )?;

    let test = TestCodexHarness::with_builder(
        core_test_support::test_codex::test_codex().with_config(move |cfg| {
            cfg.notify = Some(vec![script]);
        }),
    )
    .await?;
    let request_log = responses::mount_sse_sequence(
        test.server(),
        vec![sse(vec![
            ev_assistant_message("m1", "Done"),
            ev_completed("r1"),
        ])],
    )
    .await;

    submit_user_turn_no_wait(&test, "contract should block tui-only runtime message").await?;

    let runtime_warning = tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.test().codex, |ev| {
            matches!(ev, EventMsg::Warning(w) if w.message.contains("content = runtime short"))
        }),
    )
    .await;
    assert!(
        runtime_warning.is_ok(),
        "expected tui-visible runtime hook warning before delivery contract check"
    );

    let contract_warning = tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.test().codex, |ev| {
            matches!(ev, EventMsg::Warning(w)
                if w.message.contains("Auto delivery blocked: STOP checkpoint was not delivered in this turn."))
        }),
    )
    .await;
    assert!(
        contract_warning.is_ok(),
        "expected delivery-contract warning for blocked auto_user_reply"
    );

    let hook_completed = tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.test().codex, |ev| {
            matches!(ev, EventMsg::HookCompleted(event)
                if event.run.event_name == codex_protocol::protocol::HookEventName::AfterAgent)
        }),
    )
    .await
    .expect("timed out waiting for after_agent HookCompleted event");
    let EventMsg::HookCompleted(hook_completed) = hook_completed else {
        panic!("expected HookCompleted event");
    };
    let meta = hook_completed
        .run
        .meta
        .expect("after_agent runtime hook should include meta");
    assert_eq!(meta["protocol"]["stop_checkpoint_expected"], json!(true));
    assert_eq!(meta["protocol"]["stop_checkpoint_delivered"], json!(false));
    assert_eq!(meta["protocol"]["contract_satisfied"], json!(false));
    assert_eq!(meta["protocol"]["auto_user_replies_blocked"], json!(1));
    assert_eq!(
        meta["follow_up"],
        json!({
            "status": "blocked-delivery-contract",
            "queued_count": 0,
            "blocked_count": 1,
        })
    );

    let _first_complete = wait_for_event(&test.test().codex, |ev| {
        matches!(ev, EventMsg::TurnComplete(_))
    })
    .await;
    let second_complete = tokio::time::timeout(
        Duration::from_millis(300),
        wait_for_event(&test.test().codex, |ev| {
            matches!(ev, EventMsg::TurnComplete(_))
        }),
    )
    .await;
    assert!(
        second_complete.is_err(),
        "did not expect follow-up turn completion when delivery contract is blocked"
    );
    assert_eq!(request_log.requests().len(), 1);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_agent_runtime_delivery_contract_allows_auto_enqueue_when_stop_delivery_is_true()
-> Result<()> {
    init_test_tracing();
    if skip_if_no_linux_sandbox_bin() {
        return Ok(());
    }
    let hook_dir = TempDir::new()?;
    let once_marker = hook_dir.path().join("delivery-contract.once");
    let once_marker_str = once_marker.to_string_lossy().to_string();
    let stop_script = write_notify_script(
        r#"#!/bin/bash
set -euo pipefail
payload="$(cat)"
if printf '%s' "$payload" | grep -q '"stop_hook_active":true'; then
  printf '%s' '{"decision":"continue"}'
else
  printf '%s' '{"decision":"block","reason":"runtime auto command checkpoint"}'
fi
"#,
    )?;
    let script = write_notify_script(&format!(
        "#!/bin/bash\nset -euo pipefail\nif [ -f \"{once_marker_str}\" ]; then\n  printf '%s' '{{\"actions\":[]}}'\nelse\n  : > \"{once_marker_str}\"\n  printf '%s' '{{\"actions\":[{{\"type\":\"auto_user_reply\",\"message\":\"continue after stop delivery\"}}]}}'\nfi\n"
    ))?;
    let stop_command = format!("bash {stop_script}");

    let test = TestCodexHarness::with_builder(
        core_test_support::test_codex::test_codex()
            .with_pre_build_hook(move |home| {
                if let Err(error) = write_stop_hook(home, &stop_command) {
                    panic!("failed to write stop hook fixture: {error}");
                }
            })
            .with_config(move |cfg| {
                cfg.notify = Some(vec![script]);
                cfg.features
                    .enable(Feature::CodexHooks)
                    .expect("test config should allow feature update");
            }),
    )
    .await?;
    let request_log = responses::mount_sse_sequence(
        test.server(),
        vec![
            sse(vec![ev_assistant_message("m1", "Done"), ev_completed("r1")]),
            sse(vec![
                ev_assistant_message("m2", "Done 2"),
                ev_completed("r2"),
            ]),
            sse(vec![
                ev_assistant_message("m3", "Done 3"),
                ev_completed("r3"),
            ]),
        ],
    )
    .await;

    submit_user_turn_no_wait(
        &test,
        "contract should allow stop-delivered runtime message",
    )
    .await?;

    let hook_completed = tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.test().codex, |ev| {
            matches!(ev, EventMsg::HookCompleted(event)
                if event.run.event_name == codex_protocol::protocol::HookEventName::AfterAgent)
        }),
    )
    .await
    .expect("timed out waiting for after_agent HookCompleted event");
    let EventMsg::HookCompleted(hook_completed) = hook_completed else {
        panic!("expected HookCompleted event");
    };
    let meta = hook_completed
        .run
        .meta
        .expect("after_agent runtime hook should include meta");
    assert_eq!(meta["protocol"]["stop_checkpoint_expected"], json!(true));
    assert_eq!(meta["protocol"]["stop_checkpoint_delivered"], json!(true));
    assert_eq!(meta["protocol"]["contract_satisfied"], json!(true));
    assert_eq!(meta["protocol"]["auto_user_replies_blocked"], json!(0));
    assert_eq!(
        meta["follow_up"],
        json!({
            "status": "queued",
            "queued_count": 1,
            "blocked_count": 0,
        })
    );

    let blocked_warning = tokio::time::timeout(
        Duration::from_millis(300),
        wait_for_event(&test.test().codex, |ev| {
            matches!(ev, EventMsg::Warning(w)
                if w.message.contains("Auto delivery blocked: STOP checkpoint was not delivered in this turn."))
        }),
    )
    .await;
    assert!(
        blocked_warning.is_err(),
        "did not expect delivery-contract warning when runtime message is stop-delivered"
    );

    let _first_complete = wait_for_event(&test.test().codex, |ev| {
        matches!(ev, EventMsg::TurnComplete(_))
    })
    .await;
    let second_complete = tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.test().codex, |ev| {
            matches!(ev, EventMsg::TurnComplete(_))
        }),
    )
    .await;
    assert!(
        second_complete.is_ok(),
        "expected second TurnComplete from delivery-contract-approved auto_user_reply"
    );

    let requests = request_log.requests();
    assert_eq!(requests.len(), 3);
    let third_user_texts = requests[2].message_input_texts("user");
    assert!(
        third_user_texts
            .iter()
            .any(|text| text.contains("continue after stop delivery")),
        "expected follow-up request to include queued auto_user_reply message"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_agent_runtime_delivery_contract_observes_msg_auto_ordering_before_queueing()
-> Result<()> {
    init_test_tracing();
    if skip_if_no_linux_sandbox_bin() {
        return Ok(());
    }
    let hook_dir = TempDir::new()?;
    let once_marker = hook_dir.path().join("ordering-contract.once");
    let once_marker_str = once_marker.to_string_lossy().to_string();
    let stop_script = write_notify_script(
        r#"#!/bin/bash
set -euo pipefail
payload="$(cat)"
if printf '%s' "$payload" | grep -q '"stop_hook_active":true'; then
  printf '%s' '{"decision":"continue"}'
else
  printf '%s' '{"decision":"block","reason":"ordering checkpoint"}'
fi
"#,
    )?;
    let script = write_notify_script(&format!(
        "#!/bin/bash\nset -euo pipefail\nif [ -f \"{once_marker_str}\" ]; then\n  printf '%s' '{{\"actions\":[]}}'\nelse\n  : > \"{once_marker_str}\"\n  printf '%s' '{{\"actions\":[{{\"type\":\"auto_user_reply\",\"message\":\"continue despite ordering\"}},{{\"type\":\"nero_hook_msg\",\"mode\":\"synced\",\"show\":{{\"agent\":true,\"tui\":false}},\"format\":\"block\",\"status\":{{\"kind\":\"warning\",\"text\":\"runtime after auto\"}},\"msg\":{{\"full\":\"agent runtime after auto\",\"short\":\"agent runtime after auto short\"}}}}]}}'\nfi\n"
    ))?;
    let stop_command = format!("bash {stop_script}");

    let test = TestCodexHarness::with_builder(
        core_test_support::test_codex::test_codex()
            .with_pre_build_hook(move |home| {
                if let Err(error) = write_stop_hook(home, &stop_command) {
                    panic!("failed to write stop hook fixture: {error}");
                }
            })
            .with_config(move |cfg| {
                cfg.notify = Some(vec![script]);
                cfg.features
                    .enable(Feature::CodexHooks)
                    .expect("test config should allow feature update");
            }),
    )
    .await?;
    let request_log = responses::mount_sse_sequence(
        test.server(),
        vec![
            sse(vec![ev_assistant_message("m1", "Done"), ev_completed("r1")]),
            sse(vec![
                ev_assistant_message("m2", "Done 2"),
                ev_completed("r2"),
            ]),
            sse(vec![
                ev_assistant_message("m3", "Done 3"),
                ev_completed("r3"),
            ]),
        ],
    )
    .await;

    submit_user_turn_no_wait(&test, "ordering contract check").await?;

    let hook_completed = tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.test().codex, |ev| {
            matches!(ev, EventMsg::HookCompleted(event)
                if event.run.event_name == codex_protocol::protocol::HookEventName::AfterAgent)
        }),
    )
    .await
    .expect("timed out waiting for after_agent HookCompleted event");
    let EventMsg::HookCompleted(hook_completed) = hook_completed else {
        panic!("expected HookCompleted event");
    };
    let meta = hook_completed
        .run
        .meta
        .expect("after_agent runtime hook should include meta");
    assert_eq!(meta["protocol"]["stop_checkpoint_expected"], json!(true));
    assert_eq!(meta["protocol"]["stop_checkpoint_delivered"], json!(true));
    assert_eq!(meta["protocol"]["contract_satisfied"], json!(true));
    assert_eq!(
        meta["follow_up"],
        json!({
            "status": "queued",
            "queued_count": 1,
            "blocked_count": 0,
        })
    );

    let _first_complete = wait_for_event(&test.test().codex, |ev| {
        matches!(ev, EventMsg::TurnComplete(_))
    })
    .await;
    let second_complete = tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.test().codex, |ev| {
            matches!(ev, EventMsg::TurnComplete(_))
        }),
    )
    .await;
    assert!(
        second_complete.is_ok(),
        "expected second TurnComplete when auto_user_reply appears before runtime message action"
    );

    let requests = request_log.requests();
    assert_eq!(requests.len(), 3);
    let first_user_texts = requests[0].message_input_texts("user");
    assert!(
        first_user_texts
            .iter()
            .all(|text| !text.contains("continue despite ordering")),
        "first request should not include auto_user_reply follow-up text"
    );
    let second_user_texts = requests[1].message_input_texts("user");
    assert!(
        second_user_texts
            .iter()
            .all(|text| !text.contains("continue despite ordering")),
        "second request should only carry stop-hook continuation prompt context"
    );
    let third_user_texts = requests[2].message_input_texts("user");
    assert!(
        third_user_texts
            .iter()
            .any(|text| text.contains("continue despite ordering")),
        "third request should include queued auto_user_reply text"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(nero_hook_subagent_env)]
async fn subagent_session_runs_stop_and_after_agent_but_filters_nero_actions() -> Result<()> {
    init_test_tracing();
    if skip_if_no_linux_sandbox_bin() {
        return Ok(());
    }

    let hook_dir = TempDir::new()?;
    let stop_marker = hook_dir.path().join("subagent-stop.marker");
    let stop_marker_str = stop_marker.to_string_lossy().to_string();
    let stop_script = write_notify_script(&format!(
        "#!/bin/bash\n: > \"{stop_marker_str}\"\nprintf '%s' '{{\"systemMessage\":\"stop hook ran\"}}'\n"
    ))?;

    let after_agent_marker = hook_dir.path().join("subagent-after-agent.marker");
    let after_agent_marker_str = after_agent_marker.to_string_lossy().to_string();
    let notify_script = write_notify_script(&format!(
        "#!/bin/bash\n: > \"{after_agent_marker_str}\"\nprintf '%s' '{{\"actions\":[{{\"type\":\"visible_note\",\"message\":\"subagent visible note\"}},{{\"type\":\"nero_hook_msg\",\"mode\":\"synced\",\"show\":{{\"agent\":true,\"tui\":true}},\"format\":\"block\",\"msg\":{{\"full\":\"SUBAGENT_NERO_FULL_BLOCKED\",\"short\":\"SUBAGENT_NERO_SHORT_BLOCKED\"}}}},{{\"type\":\"auto_user_reply\",\"message\":\"continue from subagent\"}}]}}'\n"
    ))?;

    let stop_command = format!("bash {stop_script}");
    let test = TestCodexHarness::with_builder(
        core_test_support::test_codex::test_codex()
            .with_pre_build_hook(move |home| {
                if let Err(error) = write_stop_hook(home, &stop_command) {
                    panic!("failed to write stop hook fixture: {error}");
                }
            })
            .with_session_source(SessionSource::SubAgent(SubAgentSource::Other(
                "hook-msg-v2".to_string(),
            )))
            .with_config(move |cfg| {
                cfg.notify = Some(vec![notify_script]);
                cfg.features
                    .enable(Feature::CodexHooks)
                    .expect("test config should allow feature update");
            }),
    )
    .await?;

    let request_log = responses::mount_sse_once(
        test.server(),
        sse(vec![ev_assistant_message("m1", "Done"), ev_completed("r1")]),
    )
    .await;

    submit_user_turn_no_wait(&test, "subagent stop and filter check").await?;
    fs_wait::wait_for_path_exists(&stop_marker, Duration::from_secs(5)).await?;
    fs_wait::wait_for_path_exists(&after_agent_marker, Duration::from_secs(5)).await?;

    let after_agent_completed = tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.test().codex, |ev| {
            matches!(ev, EventMsg::HookCompleted(event)
                if event.run.event_name == codex_protocol::protocol::HookEventName::AfterAgent)
        }),
    )
    .await;
    assert!(
        after_agent_completed.is_ok(),
        "expected after_agent hook completion for subagent session"
    );

    let visible_warning = tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.test().codex, |ev| {
            matches!(ev, EventMsg::Warning(w)
                if w.message.contains("[nero-hook] subagent visible note"))
        }),
    )
    .await;
    assert!(
        visible_warning.is_ok(),
        "expected visible_note to remain active for subagent after_agent hooks"
    );

    let _complete = wait_for_event(&test.test().codex, |ev| {
        matches!(ev, EventMsg::TurnComplete(_))
    })
    .await;

    assert!(
        tokio::time::timeout(
            Duration::from_millis(300),
            wait_for_event(&test.test().codex, |ev| {
                matches!(ev, EventMsg::TurnComplete(_))
            }),
        )
        .await
        .is_err(),
        "did not expect follow-up turn completion from subagent auto_user_reply"
    );
    assert!(
        tokio::time::timeout(
            Duration::from_millis(300),
            wait_for_event(&test.test().codex, |ev| {
                matches!(ev, EventMsg::Warning(w)
                    if w.message.contains("SUBAGENT_NERO_FULL_BLOCKED")
                        || w.message.contains("SUBAGENT_NERO_SHORT_BLOCKED"))
            }),
        )
        .await
        .is_err(),
        "did not expect filtered subagent Nero actions to emit warnings"
    );
    assert_eq!(request_log.requests().len(), 1);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_agent_msg_does_not_inject_agent_hook_prompts() -> Result<()> {
    init_test_tracing();
    if skip_if_no_linux_sandbox_bin() {
        return Ok(());
    }
    let hook_dir = TempDir::new()?;
    let once_marker = hook_dir.path().join("injection-check.once");
    let once_marker_str = once_marker.to_string_lossy().to_string();
    let stop_script = write_notify_script(
        r#"#!/bin/bash
set -euo pipefail
payload="$(cat)"
if printf '%s' "$payload" | grep -q '"stop_hook_active":true'; then
  printf '%s' '{"decision":"continue"}'
else
  printf '%s' '{"decision":"block","reason":"injection-checkpoint"}'
fi
"#,
    )?;
    let script = write_notify_script(&format!(
        "#!/bin/bash\nset -euo pipefail\nif [ -f \"{once_marker_str}\" ]; then\n  printf '%s' '{{\"actions\":[]}}'\nelse\n  : > \"{once_marker_str}\"\n  printf '%s' '{{\"actions\":[{{\"type\":\"nero_hook_msg\",\"mode\":\"synced\",\"show\":{{\"agent\":true,\"tui\":false}},\"format\":\"inline\",\"msg\":{{\"full\":\"NERO_AGENT_TOKEN\",\"short\":\"NERO_SHORT_TOKEN\"}}}},{{\"type\":\"auto_user_reply\",\"message\":\"continue stop-centric\"}}]}}'\nfi\n"
    ))?;
    let stop_command = format!("bash {stop_script}");

    let test = TestCodexHarness::with_builder(
        core_test_support::test_codex::test_codex()
            .with_pre_build_hook(move |home| {
                if let Err(error) = write_stop_hook(home, &stop_command) {
                    panic!("failed to write stop hook fixture: {error}");
                }
            })
            .with_config(move |cfg| {
                cfg.notify = Some(vec![script]);
                cfg.features
                    .enable(Feature::CodexHooks)
                    .expect("test config should allow feature update");
            }),
    )
    .await?;

    let request_log = responses::mount_sse_sequence(
        test.server(),
        vec![
            sse(vec![ev_assistant_message("m1", "Done"), ev_completed("r1")]),
            sse(vec![
                ev_assistant_message("m2", "Done 2"),
                ev_completed("r2"),
            ]),
            sse(vec![
                ev_assistant_message("m3", "Done 3"),
                ev_completed("r3"),
            ]),
        ],
    )
    .await;

    submit_user_turn_no_wait(&test, "stop-centric injection check").await?;

    let _first_complete = wait_for_event(&test.test().codex, |ev| {
        matches!(ev, EventMsg::TurnComplete(_))
    })
    .await;
    let second_complete = tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.test().codex, |ev| {
            matches!(ev, EventMsg::TurnComplete(_))
        }),
    )
    .await;
    assert!(
        second_complete.is_ok(),
        "expected second TurnComplete from auto_user_reply follow-up"
    );

    let requests = request_log.requests();
    assert_eq!(requests.len(), 3);
    let developer_texts: Vec<String> = requests
        .iter()
        .flat_map(|request| request.message_input_texts("developer"))
        .collect();
    assert!(
        developer_texts
            .iter()
            .all(|text| !text.contains("NERO_AGENT_TOKEN")),
        "nero_hook_msg agent delivery must not inject hook prompt into developer history"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(nero_hook_subagent_env)]
async fn after_agent_auto_user_reply_stays_blocked_for_subagent_session() -> Result<()> {
    init_test_tracing();
    if skip_if_no_linux_sandbox_bin() {
        return Ok(());
    }

    let script = write_notify_script(
        r#"#!/bin/bash
printf '%s' '{"actions":[{"type":"auto_user_reply","message":"continue from subagent"}]}'
"#,
    )?;

    let test = TestCodexHarness::with_builder(
        core_test_support::test_codex::test_codex()
            .with_session_source(SessionSource::SubAgent(SubAgentSource::Other(
                "hook-msg-v2".to_string(),
            )))
            .with_config(move |cfg| {
                cfg.notify = Some(vec![script]);
            }),
    )
    .await?;

    let request_log = responses::mount_sse_sequence(
        test.server(),
        vec![sse(vec![
            ev_assistant_message("m1", "Done"),
            ev_completed("r1"),
        ])],
    )
    .await;

    submit_user_turn_no_wait(&test, "subagent auto block check").await?;

    let hook_completed = tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.test().codex, |ev| {
            matches!(ev, EventMsg::HookCompleted(event)
                if event.run.event_name == codex_protocol::protocol::HookEventName::AfterAgent)
        }),
    )
    .await;
    assert!(
        hook_completed.is_ok(),
        "expected after_agent hook execution for subagent session"
    );

    let _first_complete = wait_for_event(&test.test().codex, |ev| {
        matches!(ev, EventMsg::TurnComplete(_))
    })
    .await;
    let second_complete = tokio::time::timeout(
        Duration::from_millis(300),
        wait_for_event(&test.test().codex, |ev| {
            matches!(ev, EventMsg::TurnComplete(_))
        }),
    )
    .await;
    assert!(
        second_complete.is_err(),
        "did not expect follow-up turn completion for subagent auto_user_reply"
    );
    assert_eq!(request_log.requests().len(), 1);

    Ok(())
}

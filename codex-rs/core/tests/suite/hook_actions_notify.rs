#![cfg(not(target_os = "windows"))]

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use anyhow::Result;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::EventMsg;
use codex_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::test_codex::TestCodexHarness;
use core_test_support::wait_for_event;
use tempfile::TempDir;

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

async fn submit_user_input(test: &TestCodexHarness, text: &str) -> Result<()> {
    test.test()
        .codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: text.into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
        })
        .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_agent_visible_note_emits_warning_and_turn_completes() -> Result<()> {
    if skip_if_no_linux_sandbox_bin() {
        return Ok(());
    }
    let server = responses::start_mock_server().await;
    responses::mount_sse_once(
        &server,
        sse(vec![ev_assistant_message("m1", "Done"), ev_completed("r1")]),
    )
    .await;

    let script = write_notify_script(
        r#"#!/bin/bash
printf '%s' '{"actions":[{"type":"visible_note","message":"e2e visible"}]}'
"#,
    )?;

    let test = TestCodexHarness::with_builder(
        core_test_support::test_codex::test_codex().with_config(move |cfg| {
            cfg.notify = Some(vec![script.clone()]);
        }),
    )
    .await?;

    submit_user_input(&test, "hello").await?;

    let warning = wait_for_event(&test.test().codex, |ev| {
        matches!(ev, EventMsg::Warning(w) if w.message.contains("[nero-hook] e2e visible"))
    })
    .await;
    assert!(
        matches!(warning, EventMsg::Warning(_)),
        "expected [nero-hook] visible_note warning"
    );

    let complete = wait_for_event(&test.test().codex, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;
    assert!(matches!(complete, EventMsg::TurnComplete(_)));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_agent_legacy_plain_stdout_keeps_normal_flow() -> Result<()> {
    if skip_if_no_linux_sandbox_bin() {
        return Ok(());
    }
    let server = responses::start_mock_server().await;
    responses::mount_sse_once(
        &server,
        sse(vec![ev_assistant_message("m1", "Done"), ev_completed("r1")]),
    )
    .await;

    let script = write_notify_script(
        r#"#!/bin/bash
printf '%s' 'legacy-notifier-ok'
"#,
    )?;

    let test = TestCodexHarness::with_builder(
        core_test_support::test_codex::test_codex().with_config(move |cfg| {
            cfg.notify = Some(vec![script.clone()]);
        }),
    )
    .await?;

    submit_user_input(&test, "hello legacy").await?;

    let _complete = wait_for_event(&test.test().codex, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    // No extra warning expected for plain legacy stdout actions path.
    let warning = tokio::time::timeout(
        Duration::from_millis(200),
        wait_for_event(&test.test().codex, |ev| matches!(ev, EventMsg::Warning(_))),
    )
    .await;
    assert!(warning.is_err(), "did not expect hook warning for legacy stdout");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_agent_garbage_json_does_not_crash_turn_flow() -> Result<()> {
    if skip_if_no_linux_sandbox_bin() {
        return Ok(());
    }
    let server = responses::start_mock_server().await;
    responses::mount_sse_once(
        &server,
        sse(vec![ev_assistant_message("m1", "Done"), ev_completed("r1")]),
    )
    .await;

    let script = write_notify_script(
        r#"#!/bin/bash
printf '%s' '{not-json'
"#,
    )?;

    let test = TestCodexHarness::with_builder(
        core_test_support::test_codex::test_codex().with_config(move |cfg| {
            cfg.notify = Some(vec![script.clone()]);
        }),
    )
    .await?;

    submit_user_input(&test, "hello garbage").await?;
    let _complete = wait_for_event(&test.test().codex, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_agent_both_actions_can_trigger_follow_up_turn_without_manual_input() -> Result<()> {
    if skip_if_no_linux_sandbox_bin() {
        return Ok(());
    }
    let server = responses::start_mock_server().await;
    responses::mount_sse_once(
        &server,
        sse(vec![ev_assistant_message("m1", "Done"), ev_completed("r1")]),
    )
    .await;
    responses::mount_sse_once(
        &server,
        sse(vec![ev_assistant_message("m2", "Done 2"), ev_completed("r2")]),
    )
    .await;

    let script = write_notify_script(
        r#"#!/bin/bash
printf '%s' '{"actions":[{"type":"visible_note","message":"e2e both"},{"type":"auto_user_reply","message":"continue"}]}'
"#,
    )?;

    let test = TestCodexHarness::with_builder(
        core_test_support::test_codex::test_codex().with_config(move |cfg| {
            cfg.notify = Some(vec![script.clone()]);
        }),
    )
    .await?;

    submit_user_input(&test, "hello both").await?;

    let warning = wait_for_event(&test.test().codex, |ev| {
        matches!(ev, EventMsg::Warning(w) if w.message.contains("[nero-hook] e2e both"))
    })
    .await;
    assert!(matches!(warning, EventMsg::Warning(_)));

    let _first_complete =
        wait_for_event(&test.test().codex, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;
    let second_complete = tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_event(&test.test().codex, |ev| matches!(ev, EventMsg::TurnComplete(_))),
    )
    .await;
    assert!(
        second_complete.is_ok(),
        "expected second TurnComplete from auto_user_reply follow-up in no-race e2e test"
    );
    Ok(())
}

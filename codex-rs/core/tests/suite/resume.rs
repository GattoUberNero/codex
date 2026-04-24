use anyhow::Result;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::ByteRange;
use codex_protocol::user_input::TextElement;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_reasoning_item;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::TestCodexBuilder;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use wiremock::MockServer;

async fn resume_until_initial_messages(
    builder: &mut TestCodexBuilder,
    server: &MockServer,
    home: Arc<TempDir>,
    rollout_path: PathBuf,
    predicate: impl Fn(&[EventMsg]) -> bool,
) -> Result<TestCodex> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let poll_interval = Duration::from_millis(10);
    let mut last_initial_messages = "<missing initial messages>".to_string();

    loop {
        let resumed = builder
            .resume(server, Arc::clone(&home), rollout_path.clone())
            .await?;
        if let Some(initial_messages) = resumed.session_configured.initial_messages.as_ref() {
            if predicate(initial_messages) {
                return Ok(resumed);
            }
            last_initial_messages = format!("{initial_messages:#?}");
        }

        if tokio::time::Instant::now() >= deadline {
            panic!(
                "timed out waiting for rollout resume messages to stabilize: {last_initial_messages}"
            );
        }

        drop(resumed);
        tokio::time::sleep(poll_interval).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_includes_initial_messages_from_rollout_events() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mut builder = test_codex();
    let initial = builder.build(&server).await?;
    let codex = Arc::clone(&initial.codex);
    let home = initial.home.clone();
    let rollout_path = initial
        .session_configured
        .rollout_path
        .clone()
        .expect("rollout path");

    let initial_sse = sse(vec![
        ev_response_created("resp-initial"),
        ev_assistant_message("msg-1", "Completed first turn"),
        ev_completed("resp-initial"),
    ]);
    mount_sse_once(&server, initial_sse).await;

    let text_elements = vec![TextElement::new(
        ByteRange { start: 0, end: 6 },
        Some("<note>".into()),
    )];

    codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "Record some messages".into(),
                text_elements: text_elements.clone(),
            }],
            final_output_json_schema: None,
        })
        .await?;

    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let resumed = resume_until_initial_messages(
        &mut builder,
        &server,
        home,
        rollout_path,
        |initial_messages| {
            matches!(
                initial_messages,
                [
                    EventMsg::TurnStarted(_),
                    EventMsg::UserMessage(_),
                    EventMsg::TokenCount(_),
                    EventMsg::AgentMessage(_),
                    EventMsg::TokenCount(_),
                    EventMsg::TurnComplete(_),
                ]
            )
        },
    )
    .await?;
    let initial_messages = resumed
        .session_configured
        .initial_messages
        .expect("expected initial messages to be present for resumed session");
    match initial_messages.as_slice() {
        [
            EventMsg::TurnStarted(started),
            EventMsg::UserMessage(first_user),
            EventMsg::TokenCount(_),
            EventMsg::AgentMessage(assistant_message),
            EventMsg::TokenCount(_),
            EventMsg::TurnComplete(completed),
        ] => {
            assert_eq!(first_user.message, "Record some messages");
            assert_eq!(first_user.text_elements, text_elements);
            assert_eq!(assistant_message.message, "Completed first turn");
            assert_eq!(completed.turn_id, started.turn_id);
            assert_eq!(
                completed.last_agent_message.as_deref(),
                Some("Completed first turn")
            );
        }
        other => panic!("unexpected initial messages after resume: {other:#?}"),
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_includes_initial_messages_from_reasoning_events() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config.show_raw_agent_reasoning = true;
    });
    let initial = builder.build(&server).await?;
    let codex = Arc::clone(&initial.codex);
    let home = initial.home.clone();
    let rollout_path = initial
        .session_configured
        .rollout_path
        .clone()
        .expect("rollout path");

    let initial_sse = sse(vec![
        ev_response_created("resp-initial"),
        ev_reasoning_item("reason-1", &["Summarized step"], &["raw detail"]),
        ev_assistant_message("msg-1", "Completed reasoning turn"),
        ev_completed("resp-initial"),
    ]);
    mount_sse_once(&server, initial_sse).await;

    codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "Record reasoning messages".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
        })
        .await?;

    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let resumed = resume_until_initial_messages(
        &mut builder,
        &server,
        home,
        rollout_path,
        |initial_messages| {
            matches!(
                initial_messages,
                [
                    EventMsg::TurnStarted(_),
                    EventMsg::UserMessage(_),
                    EventMsg::TokenCount(_),
                    EventMsg::AgentReasoning(_),
                    EventMsg::AgentReasoningRawContent(_),
                    EventMsg::AgentMessage(_),
                    EventMsg::TokenCount(_),
                    EventMsg::TurnComplete(_),
                ]
            )
        },
    )
    .await?;
    let initial_messages = resumed
        .session_configured
        .initial_messages
        .expect("expected initial messages to be present for resumed session");
    match initial_messages.as_slice() {
        [
            EventMsg::TurnStarted(started),
            EventMsg::UserMessage(first_user),
            EventMsg::TokenCount(_),
            EventMsg::AgentReasoning(reasoning),
            EventMsg::AgentReasoningRawContent(raw),
            EventMsg::AgentMessage(assistant_message),
            EventMsg::TokenCount(_),
            EventMsg::TurnComplete(completed),
        ] => {
            assert_eq!(first_user.message, "Record reasoning messages");
            assert_eq!(reasoning.text, "Summarized step");
            assert_eq!(raw.text, "raw detail");
            assert_eq!(assistant_message.message, "Completed reasoning turn");
            assert_eq!(completed.turn_id, started.turn_id);
            assert_eq!(
                completed.last_agent_message.as_deref(),
                Some("Completed reasoning turn")
            );
        }
        other => panic!("unexpected initial messages after resume: {other:#?}"),
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_switches_models_preserves_base_instructions() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config.model = Some("gpt-5.2".to_string());
    });
    let initial = builder.build(&server).await?;
    let codex = Arc::clone(&initial.codex);
    let home = initial.home.clone();
    let rollout_path = initial
        .session_configured
        .rollout_path
        .clone()
        .expect("rollout path");

    let initial_sse = sse(vec![
        ev_response_created("resp-initial"),
        ev_assistant_message("msg-1", "Completed first turn"),
        ev_completed("resp-initial"),
    ]);
    let initial_mock = mount_sse_once(&server, initial_sse).await;

    codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "Record initial instructions".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
        })
        .await?;
    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let initial_body = initial_mock.single_request().body_json();
    let initial_instructions = initial_body
        .get("instructions")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let resumed_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-resume-1"),
                ev_assistant_message("msg-2", "Resumed turn"),
                ev_completed("resp-resume-1"),
            ]),
            sse(vec![
                ev_response_created("resp-resume-2"),
                ev_assistant_message("msg-3", "Second resumed turn"),
                ev_completed("resp-resume-2"),
            ]),
        ],
    )
    .await;

    let mut resume_builder = test_codex().with_config(|config| {
        config.model = Some("gpt-5.2-codex".to_string());
    });
    let resumed = resume_builder.resume(&server, home, rollout_path).await?;
    resumed
        .codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "Resume with different model".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
        })
        .await?;
    wait_for_event(&resumed.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    resumed
        .codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "Second turn after resume".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
        })
        .await?;
    wait_for_event(&resumed.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let requests = resumed_mock.requests();
    assert_eq!(requests.len(), 2, "expected two resumed requests");

    let first_resumed = &requests[0];
    assert_eq!(first_resumed.instructions_text(), initial_instructions);
    let first_developer_texts = first_resumed.message_input_texts("developer");
    assert!(
        !first_developer_texts
            .iter()
            .any(|text| text.contains("<model_switch>")),
        "did not expect model switch message on first post-resume turn"
    );

    let second_resumed = &requests[1];
    assert_eq!(second_resumed.instructions_text(), initial_instructions);
    let second_developer_texts = second_resumed.message_input_texts("developer");
    assert!(
        !second_developer_texts
            .iter()
            .any(|text| text.contains("<model_switch>")),
        "did not expect model switch message after resume without explicit switch"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_explicit_override_rebases_base_instructions_without_model_switch_message()
-> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config.model = Some("gpt-5.2".to_string());
    });
    let initial = builder.build(&server).await?;
    let codex = Arc::clone(&initial.codex);
    let home = initial.home.clone();
    let rollout_path = initial
        .session_configured
        .rollout_path
        .clone()
        .expect("rollout path");

    let initial_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-initial"),
            ev_assistant_message("msg-1", "Completed first turn"),
            ev_completed("resp-initial"),
        ]),
    )
    .await;
    codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "Record initial instructions".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
        })
        .await?;
    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;
    let _ = initial_mock.single_request();

    let resumed_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-resume"),
            ev_assistant_message("msg-2", "Resumed turn"),
            ev_completed("resp-resume"),
        ]),
    )
    .await;

    let mut resume_builder = test_codex().with_config(|config| {
        config.model = Some("gpt-5.2-codex".to_string());
    });
    let resumed = resume_builder.resume(&server, home, rollout_path).await?;
    resumed
        .codex
        .submit(Op::OverrideTurnContext {
            cwd: None,
            approval_policy: None,
            approvals_reviewer: None,
            sandbox_policy: None,
            windows_sandbox_level: None,
            model: Some("gpt-5.1-codex-max".to_string()),
            effort: None,
            summary: None,
            service_tier: None,
            collaboration_mode: None,
            personality: None,
            nero_auto_runtime: None,
        })
        .await?;
    resumed
        .codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "first turn after override".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
        })
        .await?;
    wait_for_event(&resumed.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let request = resumed_mock.single_request();
    let expected_instructions = codex_core::test_support::construct_model_info_offline(
        "gpt-5.1-codex-max",
        &resumed.config,
    )
    .get_model_instructions(resumed.config.personality);
    assert_eq!(
        request.instructions_text(),
        expected_instructions,
        "expected explicit override after resume to rebase base instructions"
    );
    let developer_texts = request.message_input_texts("developer");
    assert!(
        !developer_texts
            .iter()
            .any(|text| text.contains("<model_switch>")),
        "did not expect additive model switch message after explicit rebase"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_model_switch_with_thread_name_persists_across_resume() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config.model = Some("gpt-5.2-codex".to_string());
    });
    let initial = builder.build(&server).await?;
    let codex = Arc::clone(&initial.codex);
    let home = Arc::clone(&initial.home);
    let rollout_path = initial
        .session_configured
        .rollout_path
        .clone()
        .expect("rollout path");
    let thread_name = "nero-thread-name-model-switch-rebase";
    let switched_model = "gpt-5.1-codex-max";

    codex
        .submit(Op::SetThreadName {
            name: thread_name.to_string(),
        })
        .await?;
    wait_for_event(&codex, |event| {
        matches!(
            event,
            EventMsg::ThreadNameUpdated(updated)
                if updated.thread_name.as_deref() == Some(thread_name)
        )
    })
    .await;

    let initial_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-initial"),
            ev_assistant_message("msg-initial", "initial turn complete"),
            ev_completed("resp-initial"),
        ]),
    )
    .await;
    codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "initial rollout turn".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
        })
        .await?;
    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;
    let _ = initial_mock.single_request();

    codex
        .submit(Op::OverrideTurnContext {
            cwd: None,
            approval_policy: None,
            approvals_reviewer: None,
            sandbox_policy: None,
            windows_sandbox_level: None,
            model: Some(switched_model.to_string()),
            effort: None,
            summary: None,
            service_tier: None,
            collaboration_mode: None,
            personality: None,
            nero_auto_runtime: None,
        })
        .await?;

    let queue_path = home
        .path()
        .join("log")
        .join("model-switch-base-rebase-queue.jsonl");
    let queue_content = {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let mut queued = None;
        while tokio::time::Instant::now() <= deadline {
            match tokio::fs::read_to_string(&queue_path).await {
                Ok(value) => {
                    queued = Some(value);
                    break;
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(err) => return Err(err.into()),
            }
        }
        queued.expect("expected queue file to appear after explicit model switch")
    };
    assert!(
        queue_content.contains(thread_name),
        "expected queued rebase entry keyed by thread_name before resume"
    );

    let resumed_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-resume"),
            ev_assistant_message("msg-1", "resumed turn"),
            ev_completed("resp-resume"),
        ]),
    )
    .await;

    let mut resume_builder = test_codex().with_config(|config| {
        config.model = Some("gpt-5.2-codex".to_string());
    });
    let resumed = resume_builder
        .resume(&server, home, rollout_path.clone())
        .await?;

    resumed
        .codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "turn after resume".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
        })
        .await?;
    wait_for_event(&resumed.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let request = resumed_mock.single_request();
    let expected_instructions =
        codex_core::test_support::construct_model_info_offline(switched_model, &resumed.config)
            .get_model_instructions(resumed.config.personality);
    assert_eq!(
        request.instructions_text(),
        expected_instructions,
        "expected resumed turn to use rebased base instructions from queued thread_name switch"
    );
    let developer_texts = request.message_input_texts("developer");
    assert!(
        !developer_texts
            .iter()
            .any(|text| text.contains("<model_switch>")),
        "did not expect additive model switch message after queued rebase is applied"
    );

    assert!(
        !queue_path.exists(),
        "expected queue file to be consumed for single-thread test case"
    );

    let rollout_content = tokio::fs::read_to_string(&rollout_path).await?;
    let first_line = rollout_content
        .lines()
        .next()
        .expect("expected rollout to contain session_meta line");
    let first_line_json: Value = serde_json::from_str(first_line)?;
    let persisted_base_instructions = first_line_json
        .get("payload")
        .and_then(|value| value.get("base_instructions"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        persisted_base_instructions, expected_instructions,
        "expected rollout session_meta.base_instructions to be rewritten on queued rebase consume"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_model_switch_rebase_does_not_override_explicit_base_instructions_on_resume()
-> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config.model = Some("gpt-5.2-codex".to_string());
    });
    let initial = builder.build(&server).await?;
    let codex = Arc::clone(&initial.codex);
    let home = Arc::clone(&initial.home);
    let rollout_path = initial
        .session_configured
        .rollout_path
        .clone()
        .expect("rollout path");
    let thread_name = "nero-thread-name-explicit-base-override";
    let switched_model = "gpt-5.1-codex-max";
    let explicit_override = "explicit base instructions should win on resume";

    codex
        .submit(Op::SetThreadName {
            name: thread_name.to_string(),
        })
        .await?;
    wait_for_event(&codex, |event| {
        matches!(
            event,
            EventMsg::ThreadNameUpdated(updated)
                if updated.thread_name.as_deref() == Some(thread_name)
        )
    })
    .await;

    let initial_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-initial"),
            ev_assistant_message("msg-initial", "initial turn complete"),
            ev_completed("resp-initial"),
        ]),
    )
    .await;
    codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "initial rollout turn".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
        })
        .await?;
    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;
    let _ = initial_mock.single_request();

    codex
        .submit(Op::OverrideTurnContext {
            cwd: None,
            approval_policy: None,
            approvals_reviewer: None,
            sandbox_policy: None,
            windows_sandbox_level: None,
            model: Some(switched_model.to_string()),
            effort: None,
            summary: None,
            service_tier: None,
            collaboration_mode: None,
            personality: None,
            nero_auto_runtime: None,
        })
        .await?;

    let queue_path = home
        .path()
        .join("log")
        .join("model-switch-base-rebase-queue.jsonl");
    let queue_content = {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let mut queued = None;
        while tokio::time::Instant::now() <= deadline {
            match tokio::fs::read_to_string(&queue_path).await {
                Ok(value) => {
                    queued = Some(value);
                    break;
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(err) => return Err(err.into()),
            }
        }
        queued.expect("expected queue file to appear after explicit model switch")
    };
    assert!(
        queue_content.contains(thread_name),
        "expected queued rebase entry keyed by thread_name before resume"
    );

    let resumed_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-resume"),
            ev_assistant_message("msg-resume", "resumed turn"),
            ev_completed("resp-resume"),
        ]),
    )
    .await;
    let mut resume_builder = test_codex().with_config(|config| {
        config.model = Some("gpt-5.2-codex".to_string());
        config.base_instructions = Some(explicit_override.to_string());
    });
    let resumed = resume_builder.resume(&server, home, rollout_path).await?;

    resumed
        .codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "turn after resume".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
        })
        .await?;
    wait_for_event(&resumed.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let request = resumed_mock.single_request();
    assert_eq!(
        request.instructions_text(),
        explicit_override,
        "expected explicit base instructions override to remain authoritative on resume"
    );
    let developer_texts = request.message_input_texts("developer");
    assert!(
        !developer_texts
            .iter()
            .any(|text| text.contains("<model_switch>")),
        "did not expect additive model switch message after explicit base instructions override"
    );
    assert!(
        !queue_path.exists(),
        "expected queue file to be consumed even when explicit base instructions override is present"
    );

    Ok(())
}

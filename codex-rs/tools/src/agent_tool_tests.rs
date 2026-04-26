use super::*;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use pretty_assertions::assert_eq;
use serde_json::json;

fn model_preset(id: &str, show_in_picker: bool) -> ModelPreset {
    ModelPreset {
        id: id.to_string(),
        model: format!("{id}-model"),
        display_name: format!("{id} display"),
        description: format!("{id} description"),
        default_reasoning_effort: ReasoningEffort::Medium,
        supported_reasoning_efforts: vec![ReasoningEffortPreset {
            effort: ReasoningEffort::Medium,
            description: "Balanced".to_string(),
        }],
        supports_personality: false,
        is_default: false,
        upgrade: None,
        show_in_picker,
        availability_nux: None,
        supported_in_api: true,
        input_modalities: Vec::new(),
    }
}

#[test]
fn spawn_agent_tool_v2_requires_task_name_and_lists_visible_models() {
    let tool = create_spawn_agent_tool_v2(SpawnAgentToolOptions {
        available_models: &[
            model_preset("visible", /*show_in_picker*/ true),
            model_preset("hidden", /*show_in_picker*/ false),
        ],
        agent_type_description: "role help".to_string(),
    });

    let ToolSpec::Function(ResponsesApiTool {
        description,
        parameters,
        output_schema,
        ..
    }) = tool
    else {
        panic!("spawn_agent should be a function tool");
    };
    let parameters_json = serde_json::to_value(&parameters).expect("spawn_agent parameters");
    let JsonSchema::Object {
        properties,
        required,
        ..
    } = parameters
    else {
        panic!("spawn_agent should use object params");
    };
    assert!(description.contains("visible display (`visible-model`)"));
    assert!(!description.contains("hidden display (`hidden-model`)"));
    assert!(properties.contains_key("task_name"));
    assert!(!properties.contains_key("fork_context"));
    assert!(!properties.contains_key("context_inheritance"));
    assert_eq!(
        properties.get("agent_type"),
        Some(&JsonSchema::String {
            enum_values: None,
            description: Some("role help".to_string()),
        })
    );
    assert_eq!(required, Some(vec!["task_name".to_string()]));
    assert_eq!(
        parameters_json["properties"]["delegation_report"]["type"],
        json!("object")
    );
    assert_eq!(
        parameters_json["properties"]["delegation_report"]["required"],
        json!([
            "general_task_type",
            "task_difficulty_1_10",
            "brief_completeness_1_10",
            "task_self_sufficiency_1_10",
            "expected_duration_minutes",
            "why_this_agent",
            "expected_output_shape",
            "files_or_scope",
            "risks_or_unknowns"
        ])
    );
    assert_eq!(
        parameters_json["properties"]["delegation_report"]["additionalProperties"],
        json!(false)
    );
    assert_eq!(
        parameters_json["properties"]["delegation_report"]["properties"]["orchestration_context"]["type"],
        json!("object")
    );
    assert_eq!(
        parameters_json["properties"]["delegation_report"]["properties"]["orchestration_context"]["additionalProperties"],
        json!(false)
    );
    let output_schema = output_schema.expect("spawn_agent output schema");
    assert_eq!(
        output_schema["required"],
        json!([
            "agent_id",
            "task_name",
            "nickname",
            "delegation_report",
            "context_inheritance_requested",
            "context_inheritance_effective",
            "context_inheritance_telemetry"
        ])
    );
    assert_eq!(
        output_schema["properties"]["delegation_report"]["type"],
        json!(["object", "null"])
    );
    assert_eq!(
        output_schema["properties"]["delegation_report"]["required"],
        json!([
            "general_task_type",
            "task_difficulty_1_10",
            "brief_completeness_1_10",
            "task_self_sufficiency_1_10",
            "expected_duration_minutes",
            "why_this_agent",
            "expected_output_shape",
            "files_or_scope",
            "risks_or_unknowns"
        ])
    );
    assert_eq!(
        output_schema["properties"]["delegation_report"]["properties"]["expected_duration_minutes"]
            ["minimum"],
        json!(1)
    );
    assert_eq!(
        output_schema["properties"]["delegation_report"]["properties"]["orchestration_context"]["type"],
        json!(["object", "null"])
    );
    assert_eq!(
        output_schema["properties"]["delegation_report"]["properties"]["orchestration_context"]["properties"]
            ["action_type"]["type"],
        json!(["string", "null"])
    );
    assert_eq!(
        output_schema["properties"]["delegation_report"]["properties"]["orchestration_context"]["properties"]
            ["campaign_id"]["type"],
        json!(["string", "null"])
    );
    assert_eq!(
        output_schema["properties"]["context_inheritance_effective"]["enum"],
        json!([
            "off",
            "exact",
            "bounded_full",
            "bounded_trimmed",
            "bounded_suppressed"
        ])
    );
    assert_eq!(
        output_schema["properties"]["context_inheritance_telemetry"]["properties"]["suppression_reason"]
            ["enum"],
        json!([
            null,
            "invalid_parent_spawn_pairing",
            "missing_budget_proxy",
            "budget_exceeded"
        ])
    );
}

#[test]
fn spawn_agent_tool_v1_omits_context_inheritance_inputs() {
    let tool = create_spawn_agent_tool_v1(SpawnAgentToolOptions {
        available_models: &[model_preset("visible", /*show_in_picker*/ true)],
        agent_type_description: "role help".to_string(),
    });

    let ToolSpec::Function(ResponsesApiTool {
        parameters,
        output_schema,
        ..
    }) = tool
    else {
        panic!("spawn_agent should be a function tool");
    };
    let parameters_json = serde_json::to_value(&parameters).expect("spawn_agent parameters");
    let JsonSchema::Object { properties, .. } = parameters else {
        panic!("spawn_agent should use object params");
    };

    assert!(!properties.contains_key("task_name"));
    assert!(!properties.contains_key("fork_context"));
    assert!(!properties.contains_key("context_inheritance"));
    assert_eq!(
        parameters_json["properties"]["delegation_report"]["type"],
        json!("object")
    );
    assert_eq!(
        parameters_json["properties"]["delegation_report"]["required"],
        json!([
            "general_task_type",
            "task_difficulty_1_10",
            "brief_completeness_1_10",
            "task_self_sufficiency_1_10",
            "expected_duration_minutes",
            "why_this_agent",
            "expected_output_shape",
            "files_or_scope",
            "risks_or_unknowns"
        ])
    );
    assert_eq!(
        parameters_json["properties"]["delegation_report"]["properties"]["orchestration_context"]["type"],
        json!("object")
    );
    let output_schema = output_schema.expect("spawn_agent output schema");
    assert_eq!(
        output_schema["required"],
        json!([
            "agent_id",
            "nickname",
            "delegation_report",
            "context_inheritance_requested",
            "context_inheritance_effective",
            "context_inheritance_telemetry"
        ])
    );
    assert_eq!(
        output_schema["properties"]["delegation_report"]["type"],
        json!(["object", "null"])
    );
}

#[test]
fn spawn_agent_tool_v2_can_require_delegation_report() {
    let ToolSpec::Function(ResponsesApiTool { parameters, .. }) =
        create_spawn_agent_tool_v2_with_requirements(
            SpawnAgentToolOptions {
                available_models: &[model_preset("visible", /*show_in_picker*/ true)],
                agent_type_description: "role help".to_string(),
            },
            SpawnAgentToolRequirements {
                delegation_report_required: true,
                delegation_orchestration_context_required: false,
            },
        )
    else {
        panic!("spawn_agent should be a function tool");
    };
    let JsonSchema::Object { required, .. } = parameters else {
        panic!("spawn_agent should use object params");
    };
    assert_eq!(
        required,
        Some(vec![
            "task_name".to_string(),
            "delegation_report".to_string()
        ])
    );
}

#[test]
fn spawn_agent_tool_v1_can_require_delegation_report() {
    let ToolSpec::Function(ResponsesApiTool { parameters, .. }) =
        create_spawn_agent_tool_v1_with_requirements(
            SpawnAgentToolOptions {
                available_models: &[model_preset("visible", /*show_in_picker*/ true)],
                agent_type_description: "role help".to_string(),
            },
            SpawnAgentToolRequirements {
                delegation_report_required: true,
                delegation_orchestration_context_required: false,
            },
        )
    else {
        panic!("spawn_agent should be a function tool");
    };
    let JsonSchema::Object { required, .. } = parameters else {
        panic!("spawn_agent should use object params");
    };
    assert_eq!(required, Some(vec!["delegation_report".to_string()]));
}

#[test]
fn spawn_agent_tool_v2_can_require_delegation_orchestration_context() {
    let ToolSpec::Function(ResponsesApiTool { parameters, .. }) =
        create_spawn_agent_tool_v2_with_requirements(
            SpawnAgentToolOptions {
                available_models: &[model_preset("visible", /*show_in_picker*/ true)],
                agent_type_description: "role help".to_string(),
            },
            SpawnAgentToolRequirements {
                delegation_report_required: false,
                delegation_orchestration_context_required: true,
            },
        )
    else {
        panic!("spawn_agent should be a function tool");
    };
    let JsonSchema::Object {
        properties,
        required,
        ..
    } = parameters
    else {
        panic!("spawn_agent should use object params");
    };
    assert_eq!(
        required,
        Some(vec![
            "task_name".to_string(),
            "delegation_report".to_string()
        ])
    );
    let Some(JsonSchema::Object {
        required: Some(report_required),
        ..
    }) = properties.get("delegation_report")
    else {
        panic!("delegation_report should include required fields");
    };
    assert!(report_required.contains(&"orchestration_context".to_string()));
}

#[test]
fn spawn_agent_tool_v1_can_require_delegation_orchestration_context() {
    let ToolSpec::Function(ResponsesApiTool { parameters, .. }) =
        create_spawn_agent_tool_v1_with_requirements(
            SpawnAgentToolOptions {
                available_models: &[model_preset("visible", /*show_in_picker*/ true)],
                agent_type_description: "role help".to_string(),
            },
            SpawnAgentToolRequirements {
                delegation_report_required: false,
                delegation_orchestration_context_required: true,
            },
        )
    else {
        panic!("spawn_agent should be a function tool");
    };
    let JsonSchema::Object {
        properties,
        required,
        ..
    } = parameters
    else {
        panic!("spawn_agent should use object params");
    };
    assert_eq!(required, Some(vec!["delegation_report".to_string()]));
    let Some(JsonSchema::Object {
        required: Some(report_required),
        ..
    }) = properties.get("delegation_report")
    else {
        panic!("delegation_report should include required fields");
    };
    assert!(report_required.contains(&"orchestration_context".to_string()));
}

#[test]
fn send_input_tool_includes_optional_delegation_report() {
    let ToolSpec::Function(ResponsesApiTool { parameters, .. }) = create_send_input_tool_v1()
    else {
        panic!("send_input should be a function tool");
    };
    let JsonSchema::Object {
        properties,
        required,
        ..
    } = parameters
    else {
        panic!("send_input should use object params");
    };

    assert!(properties.contains_key("target"));
    assert!(properties.contains_key("delegation_report"));
    assert_eq!(required, Some(vec!["target".to_string()]));
}

#[test]
fn send_message_tool_requires_items_and_uses_submission_output() {
    let ToolSpec::Function(ResponsesApiTool {
        parameters,
        output_schema,
        ..
    }) = create_send_message_tool()
    else {
        panic!("send_message should be a function tool");
    };
    let JsonSchema::Object {
        properties,
        required,
        ..
    } = parameters
    else {
        panic!("send_message should use object params");
    };
    assert!(properties.contains_key("target"));
    assert!(properties.contains_key("items"));
    assert!(properties.contains_key("delegation_report"));
    assert!(!properties.contains_key("message"));
    assert_eq!(
        required,
        Some(vec!["target".to_string(), "items".to_string()])
    );
    assert_eq!(
        output_schema.expect("send_message output schema")["required"],
        json!(["submission_id"])
    );
}

#[test]
fn assign_task_tool_includes_optional_delegation_report() {
    let ToolSpec::Function(ResponsesApiTool { parameters, .. }) = create_assign_task_tool() else {
        panic!("assign_task should be a function tool");
    };
    let JsonSchema::Object {
        properties,
        required,
        ..
    } = parameters
    else {
        panic!("assign_task should use object params");
    };

    assert!(properties.contains_key("delegation_report"));
    assert_eq!(
        required,
        Some(vec!["target".to_string(), "items".to_string()])
    );
}

#[test]
fn wait_agent_tool_v2_uses_task_targets_and_summary_output() {
    let ToolSpec::Function(ResponsesApiTool {
        description: tool_description,
        parameters,
        output_schema,
        ..
    }) = create_wait_agent_tool_v2(WaitAgentTimeoutOptions {
        default_timeout_ms: 30_000,
        min_timeout_ms: 10_000,
        max_timeout_ms: 3_600_000,
    })
    else {
        panic!("wait_agent should be a function tool");
    };
    let JsonSchema::Object { properties, .. } = parameters else {
        panic!("wait_agent should use object params");
    };
    let Some(JsonSchema::Array {
        description: Some(description),
        ..
    }) = properties.get("targets")
    else {
        panic!("wait_agent should define targets array");
    };
    assert!(tool_description.contains("activity"));
    assert!(tool_description.contains("does not wait for all listed targets"));
    assert!(description.contains("canonical task names"));
    assert!(tool_description.contains("Runtime may extend the requested timeout"));
    let output_schema = output_schema.expect("wait output schema");
    assert_eq!(
        output_schema["properties"]["message"]["description"],
        json!("Brief wait summary without the agent's final content.")
    );
    assert_eq!(
        output_schema["required"],
        json!(["message", "pending", "timed_out", "wait_outcome"])
    );
    assert_eq!(
        output_schema["properties"]["pending"]["items"]["required"],
        json!(["id", "state"])
    );
    assert_eq!(
        output_schema["properties"]["wait_outcome"]["enum"],
        json!([
            "completion_already_available",
            "completion_observed",
            "activity_observed",
            "listen_window_ended"
        ])
    );
}

#[test]
fn close_agent_tool_v2_exposes_safe_close_mode() {
    let ToolSpec::Function(ResponsesApiTool {
        description,
        parameters,
        output_schema,
        ..
    }) = create_close_agent_tool_v2()
    else {
        panic!("close_agent should be a function tool");
    };
    let JsonSchema::Object {
        properties,
        required,
        ..
    } = parameters
    else {
        panic!("close_agent should use object params");
    };

    assert!(description.contains("safe_close"));
    assert!(description.contains("force_cancel"));
    assert_eq!(required, Some(vec!["target".to_string()]));
    assert_eq!(
        properties.get("mode"),
        Some(&JsonSchema::String {
            enum_values: Some(vec!["safe_close".to_string(), "force_cancel".to_string()]),
            description: Some(
                "Optional close mode. safe_close is the default and only closes already-finished agents; force_cancel intentionally terminates running work."
                    .to_string(),
            ),
        })
    );
    assert_eq!(
        output_schema.expect("close_agent output schema")["required"],
        json!(["previous_status"])
    );
}

#[test]
fn list_agents_tool_includes_path_prefix_and_agent_fields() {
    let ToolSpec::Function(ResponsesApiTool {
        parameters,
        output_schema,
        ..
    }) = create_list_agents_tool()
    else {
        panic!("list_agents should be a function tool");
    };
    let JsonSchema::Object { properties, .. } = parameters else {
        panic!("list_agents should use object params");
    };
    assert!(properties.contains_key("path_prefix"));
    assert_eq!(
        output_schema.expect("list_agents output schema")["properties"]["agents"]["items"]["required"],
        json!(["agent_name", "agent_status", "last_task_message"])
    );
}

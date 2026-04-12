use crate::JsonSchema;
use crate::ResponsesApiTool;
use crate::ToolSpec;
use codex_protocol::openai_models::ModelPreset;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct SpawnAgentToolOptions<'a> {
    pub available_models: &'a [ModelPreset],
    pub agent_type_description: String,
    pub require_delegation_report: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitAgentTimeoutOptions {
    pub default_timeout_ms: i64,
    pub min_timeout_ms: i64,
    pub max_timeout_ms: i64,
}

pub fn create_spawn_agent_tool_v1(options: SpawnAgentToolOptions<'_>) -> ToolSpec {
    let available_models_description = spawn_agent_models_description(options.available_models);
    let return_value_description =
        "Returns the spawned agent id plus the user-facing nickname when available.";
    let properties = spawn_agent_common_properties(&options.agent_type_description);

    ToolSpec::Function(ResponsesApiTool {
        name: "spawn_agent".to_string(),
        description: spawn_agent_tool_description(
            &available_models_description,
            return_value_description,
        ),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::Object {
            properties,
            required: options
                .require_delegation_report
                .then(|| vec!["delegation_report".to_string()]),
            additional_properties: Some(false.into()),
        },
        output_schema: Some(spawn_agent_output_schema_v1()),
    })
}

pub fn create_spawn_agent_tool_v2(options: SpawnAgentToolOptions<'_>) -> ToolSpec {
    let available_models_description = spawn_agent_models_description(options.available_models);
    let return_value_description = "Returns the canonical task name for the spawned agent, plus the user-facing nickname when available.";
    let mut properties = spawn_agent_common_properties(&options.agent_type_description);
    properties.insert(
        "task_name".to_string(),
        JsonSchema::String {
            enum_values: None,
            description: Some(
                "Task name for the new agent. Use lowercase letters, digits, and underscores."
                    .to_string(),
            ),
        },
    );

    ToolSpec::Function(ResponsesApiTool {
        name: "spawn_agent".to_string(),
        description: spawn_agent_tool_description(
            &available_models_description,
            return_value_description,
        ),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::Object {
            properties,
            required: Some(if options.require_delegation_report {
                vec!["task_name".to_string(), "delegation_report".to_string()]
            } else {
                vec!["task_name".to_string()]
            }),
            additional_properties: Some(false.into()),
        },
        output_schema: Some(spawn_agent_output_schema_v2()),
    })
}

pub fn create_send_input_tool_v1() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "target".to_string(),
            JsonSchema::String {
                enum_values: None,
                description: Some("Agent id to message (from spawn_agent).".to_string()),
            },
        ),
        (
            "message".to_string(),
            JsonSchema::String {
                enum_values: None,
                description: Some(
                    "Legacy plain-text message to send to the agent. Use either message or items."
                        .to_string(),
                ),
            },
        ),
        ("items".to_string(), create_collab_input_items_schema()),
        (
            "interrupt".to_string(),
            JsonSchema::Boolean {
                description: Some(
                    "When true, stop the agent's current task and handle this immediately. When false (default), queue this message."
                        .to_string(),
                ),
            },
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "send_input".to_string(),
        description: "Send a message to an existing agent. Use interrupt=true to redirect work immediately. You should reuse the agent by send_input if you believe your assigned task is highly dependent on the context of a previous task."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::Object {
            properties,
            required: Some(vec!["target".to_string()]),
            additional_properties: Some(false.into()),
        },
        output_schema: Some(send_input_output_schema()),
    })
}

pub fn create_send_message_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "target".to_string(),
            JsonSchema::String {
                enum_values: None,
                description: Some(
                    "Agent id or canonical task name to message (from spawn_agent).".to_string(),
                ),
            },
        ),
        ("items".to_string(), create_collab_input_items_schema()),
        (
            "interrupt".to_string(),
            JsonSchema::Boolean {
                description: Some(
                    "When true, stop the agent's current task and handle this immediately. When false (default), queue this message."
                        .to_string(),
                ),
            },
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "send_message".to_string(),
        description: "Add a message to an existing agent without triggering a new turn. Use interrupt=true to stop the current task first. In MultiAgentV2, this tool currently supports text content only."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::Object {
            properties,
            required: Some(vec!["target".to_string(), "items".to_string()]),
            additional_properties: Some(false.into()),
        },
        output_schema: Some(send_input_output_schema()),
    })
}

pub fn create_assign_task_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "target".to_string(),
            JsonSchema::String {
                enum_values: None,
                description: Some(
                    "Agent id or canonical task name to message (from spawn_agent).".to_string(),
                ),
            },
        ),
        ("items".to_string(), create_collab_input_items_schema()),
        (
            "interrupt".to_string(),
            JsonSchema::Boolean {
                description: Some(
                    "When true, stop the agent's current task and handle this immediately. When false (default), queue this message."
                        .to_string(),
                ),
            },
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "assign_task".to_string(),
        description: "Add a message to an existing agent and trigger a turn in the target. Use interrupt=true to redirect work immediately. In MultiAgentV2, this tool currently supports text content only."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::Object {
            properties,
            required: Some(vec!["target".to_string(), "items".to_string()]),
            additional_properties: Some(false.into()),
        },
        output_schema: Some(send_input_output_schema()),
    })
}

pub fn create_resume_agent_tool() -> ToolSpec {
    let properties = BTreeMap::from([(
        "id".to_string(),
        JsonSchema::String {
            enum_values: None,
            description: Some("Agent id to resume.".to_string()),
        },
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: "resume_agent".to_string(),
        description:
            "Resume a previously closed agent by id so it can receive send_input and wait_agent calls."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::Object {
            properties,
            required: Some(vec!["id".to_string()]),
            additional_properties: Some(false.into()),
        },
        output_schema: Some(resume_agent_output_schema()),
    })
}

pub fn create_wait_agent_tool_v1(options: WaitAgentTimeoutOptions) -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name: "wait_agent".to_string(),
        description: "Wait for agents to reach a final status. Completed statuses may include the agent's final message. Returns empty status when timed out. Once the agent reaches a final status, a notification message will be received containing the same completed status."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: wait_agent_tool_parameters_v1(options),
        output_schema: Some(wait_output_schema_v1()),
    })
}

pub fn create_wait_agent_tool_v2(options: WaitAgentTimeoutOptions) -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name: "wait_agent".to_string(),
        description: "Wait for agents to reach a final status. Returns a brief wait summary instead of the agent's final content. Returns a timeout summary when no agent reaches a final status before the deadline."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: wait_agent_tool_parameters_v2(options),
        output_schema: Some(wait_output_schema_v2()),
    })
}

pub fn create_list_agents_tool() -> ToolSpec {
    let properties = BTreeMap::from([(
        "path_prefix".to_string(),
        JsonSchema::String {
                enum_values: None,
            description: Some(
                "Optional task-path prefix. Accepts the same relative or absolute task-path syntax as other MultiAgentV2 agent targets."
                    .to_string(),
            ),
        },
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: "list_agents".to_string(),
        description:
            "List live agents in the current root thread tree. Optionally filter by task-path prefix."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::Object {
            properties,
            required: None,
            additional_properties: Some(false.into()),
        },
        output_schema: Some(list_agents_output_schema()),
    })
}

pub fn create_close_agent_tool_v1() -> ToolSpec {
    let properties = BTreeMap::from([(
        "target".to_string(),
        JsonSchema::String {
            enum_values: None,
            description: Some("Agent id to close (from spawn_agent).".to_string()),
        },
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: "close_agent".to_string(),
        description: "Close an agent and any open descendants when they are no longer needed, and return the target agent's previous status before shutdown was requested. Don't keep agents open for too long if they are not needed anymore.".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::Object {
            properties,
            required: Some(vec!["target".to_string()]),
            additional_properties: Some(false.into()),
        },
        output_schema: Some(close_agent_output_schema()),
    })
}

pub fn create_close_agent_tool_v2() -> ToolSpec {
    let properties = BTreeMap::from([(
        "target".to_string(),
        JsonSchema::String {
            enum_values: None,
            description: Some(
                "Agent id or canonical task name to close (from spawn_agent).".to_string(),
            ),
        },
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: "close_agent".to_string(),
        description: "Close an agent and any open descendants when they are no longer needed, and return the target agent's previous status before shutdown was requested. Don't keep agents open for too long if they are not needed anymore.".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::Object {
            properties,
            required: Some(vec!["target".to_string()]),
            additional_properties: Some(false.into()),
        },
        output_schema: Some(close_agent_output_schema()),
    })
}

fn agent_status_output_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "string",
                "enum": ["pending_init", "running", "shutdown", "not_found"]
            },
            {
                "type": "object",
                "properties": {
                    "completed": {
                        "type": ["string", "null"]
                    }
                },
                "required": ["completed"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "errored": {
                        "type": "string"
                    }
                },
                "required": ["errored"],
                "additionalProperties": false
            }
        ]
    })
}

fn spawn_agent_output_schema_v1() -> Value {
    json!({
        "type": "object",
        "properties": {
            "agent_id": {
                "type": "string",
                "description": "Thread identifier for the spawned agent."
            },
            "nickname": {
                "type": ["string", "null"],
                "description": "User-facing nickname for the spawned agent when available."
            },
            "delegation_report": delegation_report_output_schema(),
            "context_inheritance_requested": {
                "type": "string",
                "enum": ["off", "exact", "bounded"],
                "description": "Requested context inheritance mode for this spawn."
            },
            "context_inheritance_effective": {
                "type": "string",
                "enum": ["off", "exact", "bounded_full", "bounded_trimmed", "bounded_suppressed"],
                "description": "Effective context inheritance mode used for this spawn."
            },
            "context_inheritance_telemetry": {
                "type": ["object", "null"],
                "description": "Runtime budgeting telemetry for the effective inheritance decision.",
                "properties": {
                    "parent_replay_safe_turn_count": {"type": ["integer", "null"]},
                    "shipped_replay_safe_turn_count": {"type": ["integer", "null"]},
                    "estimated_shipped_tokens": {"type": ["integer", "null"]},
                    "usable_context_budget_tokens": {"type": ["integer", "null"]},
                    "suppression_reason": {
                        "type": ["string", "null"],
                        "enum": [null, "invalid_parent_spawn_pairing", "missing_budget_proxy", "budget_exceeded"]
                    }
                },
                "required": [
                    "parent_replay_safe_turn_count",
                    "shipped_replay_safe_turn_count",
                    "estimated_shipped_tokens",
                    "usable_context_budget_tokens",
                    "suppression_reason"
                ],
                "additionalProperties": false
            }
        },
        "required": ["agent_id", "nickname", "delegation_report", "context_inheritance_requested", "context_inheritance_effective", "context_inheritance_telemetry"],
        "additionalProperties": false
    })
}

fn spawn_agent_output_schema_v2() -> Value {
    json!({
        "type": "object",
        "properties": {
            "agent_id": {
                "type": ["string", "null"],
                "description": "Legacy thread identifier for the spawned agent."
            },
            "task_name": {
                "type": "string",
                "description": "Canonical task name for the spawned agent."
            },
            "nickname": {
                "type": ["string", "null"],
                "description": "User-facing nickname for the spawned agent when available."
            },
            "delegation_report": delegation_report_output_schema(),
            "context_inheritance_requested": {
                "type": "string",
                "enum": ["off", "exact", "bounded"],
                "description": "Requested context inheritance mode for this spawn."
            },
            "context_inheritance_effective": {
                "type": "string",
                "enum": ["off", "exact", "bounded_full", "bounded_trimmed", "bounded_suppressed"],
                "description": "Effective context inheritance mode used for this spawn."
            },
            "context_inheritance_telemetry": {
                "type": ["object", "null"],
                "description": "Runtime budgeting telemetry for the effective inheritance decision.",
                "properties": {
                    "parent_replay_safe_turn_count": {"type": ["integer", "null"]},
                    "shipped_replay_safe_turn_count": {"type": ["integer", "null"]},
                    "estimated_shipped_tokens": {"type": ["integer", "null"]},
                    "usable_context_budget_tokens": {"type": ["integer", "null"]},
                    "suppression_reason": {
                        "type": ["string", "null"],
                        "enum": [null, "invalid_parent_spawn_pairing", "missing_budget_proxy", "budget_exceeded"]
                    }
                },
                "required": [
                    "parent_replay_safe_turn_count",
                    "shipped_replay_safe_turn_count",
                    "estimated_shipped_tokens",
                    "usable_context_budget_tokens",
                    "suppression_reason"
                ],
                "additionalProperties": false
            }
        },
        "required": ["agent_id", "task_name", "nickname", "delegation_report", "context_inheritance_requested", "context_inheritance_effective", "context_inheritance_telemetry"],
        "additionalProperties": false
    })
}

fn send_input_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "submission_id": {
                "type": "string",
                "description": "Identifier for the queued input submission."
            }
        },
        "required": ["submission_id"],
        "additionalProperties": false
    })
}

fn list_agents_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "agents": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "agent_name": {
                            "type": "string",
                            "description": "Canonical task name for the agent when available, otherwise the agent id."
                        },
                        "agent_status": {
                            "description": "Last known status of the agent.",
                            "allOf": [agent_status_output_schema()]
                        },
                        "last_task_message": {
                            "type": ["string", "null"],
                            "description": "Most recent user or inter-agent instruction received by the agent, when available."
                        }
                    },
                    "required": ["agent_name", "agent_status", "last_task_message"],
                    "additionalProperties": false
                },
                "description": "Live agents visible in the current root thread tree."
            }
        },
        "required": ["agents"],
        "additionalProperties": false
    })
}

fn resume_agent_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "status": agent_status_output_schema()
        },
        "required": ["status"],
        "additionalProperties": false
    })
}

fn wait_output_schema_v1() -> Value {
    json!({
        "type": "object",
        "properties": {
            "status": {
                "type": "object",
                "description": "Final statuses keyed by agent id.",
                "additionalProperties": agent_status_output_schema()
            },
            "timed_out": {
                "type": "boolean",
                "description": "Whether the wait call returned due to timeout before any agent reached a final status."
            }
        },
        "required": ["status", "timed_out"],
        "additionalProperties": false
    })
}

fn wait_output_schema_v2() -> Value {
    json!({
        "type": "object",
        "properties": {
            "message": {
                "type": "string",
                "description": "Brief wait summary without the agent's final content."
            },
            "timed_out": {
                "type": "boolean",
                "description": "Whether the wait call returned due to timeout before any agent reached a final status."
            }
        },
        "required": ["message", "timed_out"],
        "additionalProperties": false
    })
}

fn close_agent_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "previous_status": {
                "description": "The agent status observed before shutdown was requested.",
                "allOf": [agent_status_output_schema()]
            }
        },
        "required": ["previous_status"],
        "additionalProperties": false
    })
}

fn delegation_report_properties() -> BTreeMap<String, JsonSchema> {
    BTreeMap::from([
        (
            "general_task_type".to_string(),
            JsonSchema::String {
                enum_values: None,
                description: Some("High-level type of the delegated work (non-empty).".to_string()),
            },
        ),
        (
            "task_difficulty_1_10".to_string(),
            JsonSchema::Integer {
                description: Some("Integer task difficulty on a 1 to 10 scale (1-10).".to_string()),
                minimum: Some(1.0),
                maximum: Some(10.0),
            },
        ),
        (
            "brief_completeness_1_10".to_string(),
            JsonSchema::Integer {
                description: Some(
                    "Integer brief completeness on a 1 to 10 scale (1-10).".to_string(),
                ),
                minimum: Some(1.0),
                maximum: Some(10.0),
            },
        ),
        (
            "task_self_sufficiency_1_10".to_string(),
            JsonSchema::Integer {
                description: Some(
                    "Integer task self-sufficiency on a 1 to 10 scale (1-10).".to_string(),
                ),
                minimum: Some(1.0),
                maximum: Some(10.0),
            },
        ),
        (
            "expected_duration_minutes".to_string(),
            JsonSchema::Integer {
                description: Some("Expected duration in whole minutes, minimum 1.".to_string()),
                minimum: Some(1.0),
                maximum: None,
            },
        ),
        (
            "why_this_agent".to_string(),
            JsonSchema::String {
                enum_values: None,
                description: Some("Why this agent should handle the task (non-empty).".to_string()),
            },
        ),
        (
            "expected_output_shape".to_string(),
            JsonSchema::String {
                enum_values: None,
                description: Some(
                    "Expected shape of the delivered output (non-empty).".to_string(),
                ),
            },
        ),
        (
            "files_or_scope".to_string(),
            JsonSchema::String {
                enum_values: None,
                description: Some("Files or scope the task should cover (non-empty).".to_string()),
            },
        ),
        (
            "risks_or_unknowns".to_string(),
            JsonSchema::String {
                enum_values: None,
                description: Some("Known risks or open questions (non-empty).".to_string()),
            },
        ),
    ])
}

fn delegation_report_input_schema() -> JsonSchema {
    JsonSchema::Object {
        properties: delegation_report_properties(),
        required: Some(vec![
            "general_task_type".to_string(),
            "task_difficulty_1_10".to_string(),
            "brief_completeness_1_10".to_string(),
            "task_self_sufficiency_1_10".to_string(),
            "expected_duration_minutes".to_string(),
            "why_this_agent".to_string(),
            "expected_output_shape".to_string(),
            "files_or_scope".to_string(),
            "risks_or_unknowns".to_string(),
        ]),
        additional_properties: Some(false.into()),
    }
}

fn delegation_report_output_schema() -> Value {
    json!({
        "type": ["object", "null"],
        "description": "Optional delegation report echoed back from spawn_agent.",
        "properties": {
            "general_task_type": {
                "type": "string",
                "description": "High-level type of the delegated work."
            },
            "task_difficulty_1_10": {
                "type": "integer",
                "minimum": 1,
                "maximum": 10,
                "description": "Task difficulty on a 1 to 10 scale."
            },
            "brief_completeness_1_10": {
                "type": "integer",
                "minimum": 1,
                "maximum": 10,
                "description": "How complete the brief is on a 1 to 10 scale."
            },
            "task_self_sufficiency_1_10": {
                "type": "integer",
                "minimum": 1,
                "maximum": 10,
                "description": "How self-sufficient the task is on a 1 to 10 scale."
            },
            "expected_duration_minutes": {
                "type": "integer",
                "minimum": 1,
                "description": "Expected duration in whole minutes."
            },
            "why_this_agent": {
                "type": "string",
                "description": "Why this agent should handle the task."
            },
            "expected_output_shape": {
                "type": "string",
                "description": "Expected shape of the delivered output."
            },
            "files_or_scope": {
                "type": "string",
                "description": "Files or scope the task should cover."
            },
            "risks_or_unknowns": {
                "type": "string",
                "description": "Known risks or open questions."
            }
        },
        "required": [
            "general_task_type",
            "task_difficulty_1_10",
            "brief_completeness_1_10",
            "task_self_sufficiency_1_10",
            "expected_duration_minutes",
            "why_this_agent",
            "expected_output_shape",
            "files_or_scope",
            "risks_or_unknowns"
        ],
        "additionalProperties": false
    })
}

fn create_collab_input_items_schema() -> JsonSchema {
    let properties = BTreeMap::from([
        (
            "type".to_string(),
            JsonSchema::String {
                enum_values: None,
                description: Some(
                    "Input item type: text, image, local_image, skill, or mention.".to_string(),
                ),
            },
        ),
        (
            "text".to_string(),
            JsonSchema::String {
                enum_values: None,
                description: Some("Text content when type is text.".to_string()),
            },
        ),
        (
            "image_url".to_string(),
            JsonSchema::String {
                enum_values: None,
                description: Some("Image URL when type is image.".to_string()),
            },
        ),
        (
            "path".to_string(),
            JsonSchema::String {
                enum_values: None,
                description: Some(
                    "Path when type is local_image/skill, or structured mention target such as app://<connector-id> or plugin://<plugin-name>@<marketplace-name> when type is mention."
                        .to_string(),
                ),
            },
        ),
        (
            "name".to_string(),
            JsonSchema::String {
                enum_values: None,
                description: Some("Display name when type is skill or mention.".to_string()),
            },
        ),
    ]);

    JsonSchema::Array {
        items: Box::new(JsonSchema::Object {
            properties,
            required: None,
            additional_properties: Some(false.into()),
        }),
        description: Some(
            "Structured input items. Use this to pass explicit mentions (for example app:// connector paths)."
                .to_string(),
        ),
    }
}

fn spawn_agent_common_properties(agent_type_description: &str) -> BTreeMap<String, JsonSchema> {
    BTreeMap::from([
        (
            "message".to_string(),
            JsonSchema::String {
                enum_values: None,
                description: Some(
                    "Initial plain-text task for the new agent. Use either message or items."
                        .to_string(),
                ),
            },
        ),
        ("items".to_string(), create_collab_input_items_schema()),
        (
            "agent_type".to_string(),
            JsonSchema::String {
                enum_values: None,
                description: Some(agent_type_description.to_string()),
            },
        ),
        (
            "model".to_string(),
            JsonSchema::String {
                enum_values: None,
                description: Some(
                    "Optional model override for the new agent. Replaces the inherited model."
                        .to_string(),
                ),
            },
        ),
        (
            "reasoning_effort".to_string(),
            JsonSchema::String {
                enum_values: None,
                description: Some(
                    "Optional reasoning effort override for the new agent. Replaces the inherited reasoning effort."
                        .to_string(),
                ),
            },
        ),
        (
            "delegation_report".to_string(),
            delegation_report_input_schema(),
        ),
    ])
}

fn spawn_agent_tool_description(
    available_models_description: &str,
    return_value_description: &str,
) -> String {
    format!(
        r#"
        Only use `spawn_agent` if and only if the user explicitly asks for sub-agents, delegation, or parallel agent work.
        Requests for depth, thoroughness, research, investigation, or detailed codebase analysis do not count as permission to spawn.
        Agent-role guidance below only helps choose which agent to use after spawning is already authorized; it never authorizes spawning by itself.
        Spawn a sub-agent for a well-scoped task. {return_value_description} This spawn_agent tool provides you access to smaller but more efficient sub-agents. A mini model can solve many tasks faster than the main model. You should follow the rules and guidelines below to use this tool.

{available_models_description}
### When to delegate vs. do the subtask yourself
- First, quickly analyze the overall user task and form a succinct high-level plan. Identify which tasks are immediate blockers on the critical path, and which tasks are sidecar tasks that are needed but can run in parallel without blocking the next local step. As part of that plan, explicitly decide what immediate task you should do locally right now. Do this planning step before delegating to agents so you do not hand off the immediate blocking task to a submodel and then waste time waiting on it.
- Use the smaller subagent when a subtask is easy enough for it to handle and can run in parallel with your local work. Prefer delegating concrete, bounded sidecar tasks that materially advance the main task without blocking your immediate next local step.
- Do not delegate urgent blocking work when your immediate next step depends on that result. If the very next action is blocked on that task, the main rollout should usually do it locally to keep the critical path moving.
- Keep work local when the subtask is too difficult to delegate well and when it is tightly coupled, urgent, or likely to block your immediate next step.

### Designing delegated subtasks
- Subtasks must be concrete, well-defined, complete, clear, and self-sufficient.
- Delegated subtasks must materially advance the main task.
- Do not duplicate work between the main rollout and delegated subtasks.
- Avoid issuing multiple delegate calls on the same unresolved thread unless the new delegated task is genuinely different and necessary.
- Narrow the delegated ask to the concrete output you need next.
- For coding tasks, prefer delegating concrete code-change worker subtasks over read-only explorer analysis when the subagent can make a bounded patch in a clear write scope.
- When delegating coding work, instruct the submodel to edit files directly in its forked workspace and list the file paths it changed in the final answer.
- For code-edit subtasks, decompose work so each delegated task has a disjoint write set.

### After you delegate
- Call wait_agent very sparingly. Only call wait_agent when you need the result immediately for the next critical-path step and you are blocked until it returns.
- Do not redo delegated subagent tasks yourself; focus on integrating results or tackling non-overlapping work.
- While the subagent is running in the background, do meaningful non-overlapping work immediately.
- Do not repeatedly wait by reflex.
- When a delegated coding task returns, quickly review the uploaded changes, then integrate or refine them.

### Parallel delegation patterns
- Run multiple independent information-seeking subtasks in parallel when you have distinct questions that can be answered independently.
- Split implementation into disjoint codebase slices and spawn multiple agents for them in parallel when the write scopes do not overlap.
- Delegate verification only when it can run in parallel with ongoing implementation and is likely to catch a concrete risk before final integration.
- The key is to find opportunities to spawn multiple independent subtasks in parallel within the same round, while ensuring each subtask is well-defined, self-contained, and materially advances the main task."#
    )
}

fn spawn_agent_models_description(models: &[ModelPreset]) -> String {
    let visible_models: Vec<&ModelPreset> =
        models.iter().filter(|model| model.show_in_picker).collect();
    if visible_models.is_empty() {
        return "No picker-visible models are currently loaded.".to_string();
    }

    visible_models
        .into_iter()
        .map(|model| {
            let efforts = model
                .supported_reasoning_efforts
                .iter()
                .map(|preset| format!("{} ({})", preset.effort, preset.description))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "- {} (`{}`): {} Default reasoning effort: {}. Supported reasoning efforts: {}.",
                model.display_name,
                model.model,
                model.description,
                model.default_reasoning_effort,
                efforts
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn wait_agent_tool_parameters_v1(options: WaitAgentTimeoutOptions) -> JsonSchema {
    let properties = BTreeMap::from([
        (
            "targets".to_string(),
            JsonSchema::Array {
                items: Box::new(JsonSchema::String {
                    enum_values: None,
                    description: None,
                }),
                description: Some(
                    "Agent ids to wait on. Pass multiple ids to wait for whichever finishes first."
                        .to_string(),
                ),
            },
        ),
        (
            "timeout_ms".to_string(),
            JsonSchema::Number {
                description: Some(format!(
                    "Optional timeout in milliseconds. Defaults to {}, min {}, max {}. Prefer longer waits (minutes) to avoid busy polling.",
                    options.default_timeout_ms, options.min_timeout_ms, options.max_timeout_ms,
                )),
            },
        ),
    ]);

    JsonSchema::Object {
        properties,
        required: Some(vec!["targets".to_string()]),
        additional_properties: Some(false.into()),
    }
}

fn wait_agent_tool_parameters_v2(options: WaitAgentTimeoutOptions) -> JsonSchema {
    let properties = BTreeMap::from([
        (
            "targets".to_string(),
            JsonSchema::Array {
                items: Box::new(JsonSchema::String {
                enum_values: None, description: None }),
                description: Some(
                    "Agent ids or canonical task names to wait on. Pass multiple targets to wait for whichever finishes first."
                        .to_string(),
                ),
            },
        ),
        (
            "timeout_ms".to_string(),
            JsonSchema::Number {
                description: Some(format!(
                    "Optional timeout in milliseconds. Defaults to {}, min {}, max {}. Prefer longer waits (minutes) to avoid busy polling.",
                    options.default_timeout_ms, options.min_timeout_ms, options.max_timeout_ms,
                )),
            },
        ),
    ]);

    JsonSchema::Object {
        properties,
        required: Some(vec!["targets".to_string()]),
        additional_properties: Some(false.into()),
    }
}

#[cfg(test)]
#[path = "agent_tool_tests.rs"]
mod tests;

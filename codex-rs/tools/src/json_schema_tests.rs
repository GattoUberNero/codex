use super::AdditionalProperties;
use super::JsonSchema;
use super::parse_tool_input_schema;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;

#[test]
fn parse_tool_input_schema_coerces_boolean_schemas() {
    let schema = parse_tool_input_schema(&serde_json::json!(true)).expect("parse schema");

    assert_eq!(
        schema,
        JsonSchema::String {
            enum_values: None,
            description: None
        }
    );
}

#[test]
fn parse_tool_input_schema_infers_object_shape_and_defaults_properties() {
    let schema = parse_tool_input_schema(&serde_json::json!({
        "properties": {
            "query": {"description": "search query"}
        }
    }))
    .expect("parse schema");

    assert_eq!(
        schema,
        JsonSchema::Object {
            properties: BTreeMap::from([(
                "query".to_string(),
                JsonSchema::String {
                    enum_values: None,
                    description: Some("search query".to_string()),
                },
            )]),
            required: None,
            additional_properties: None,
        }
    );
}

#[test]
fn parse_tool_input_schema_accepts_integer_bounds_with_integral_floats() {
    let schema = parse_tool_input_schema(&serde_json::json!({
        "type": "integer",
        "minimum": 1.0,
        "maximum": 10.0
    }))
    .expect("parse schema");

    assert_eq!(
        schema,
        JsonSchema::Integer {
            description: None,
            minimum: Some(1.0),
            maximum: Some(10.0)
        }
    );
}

#[test]
fn parse_tool_input_schema_normalizes_integer_and_missing_array_items() {
    let schema = parse_tool_input_schema(&serde_json::json!({
        "type": "object",
        "properties": {
            "page": {"type": "integer"},
            "tags": {"type": "array"}
        }
    }))
    .expect("parse schema");

    assert_eq!(
        schema,
        JsonSchema::Object {
            properties: BTreeMap::from([
                (
                    "page".to_string(),
                    JsonSchema::Integer {
                        description: None,
                        minimum: None,
                        maximum: None
                    },
                ),
                (
                    "tags".to_string(),
                    JsonSchema::Array {
                        items: Box::new(JsonSchema::String {
                            enum_values: None,
                            description: None
                        }),
                        min_items: None,
                        description: None,
                    },
                ),
            ]),
            required: None,
            additional_properties: None,
        }
    );
}

#[test]
fn parse_tool_input_schema_sanitizes_additional_properties_schema() {
    let schema = parse_tool_input_schema(&serde_json::json!({
        "type": "object",
        "additionalProperties": {
            "required": ["value"],
            "properties": {
                "value": {"anyOf": [{"type": "string"}, {"type": "number"}]}
            }
        }
    }))
    .expect("parse schema");

    assert_eq!(
        schema,
        JsonSchema::Object {
            properties: BTreeMap::new(),
            required: None,
            additional_properties: Some(AdditionalProperties::Schema(Box::new(
                JsonSchema::Object {
                    properties: BTreeMap::from([(
                        "value".to_string(),
                        JsonSchema::String {
                            enum_values: None,
                            description: None
                        },
                    )]),
                    required: Some(vec!["value".to_string()]),
                    additional_properties: None,
                },
            ))),
        }
    );
}

#[test]
fn parse_tool_input_schema_preserves_array_min_items() {
    let schema = parse_tool_input_schema(&serde_json::json!({
        "type": "object",
        "properties": {
            "requests": {
                "type": "array",
                "minItems": 1,
                "items": { "type": "string" }
            }
        },
        "required": ["requests"],
        "additionalProperties": false
    }))
    .expect("parse schema");

    assert_eq!(
        schema,
        JsonSchema::Object {
            properties: BTreeMap::from([(
                "requests".to_string(),
                JsonSchema::Array {
                    items: Box::new(JsonSchema::String {
                        enum_values: None,
                        description: None,
                    }),
                    min_items: Some(1),
                    description: None,
                },
            )]),
            required: Some(vec!["requests".to_string()]),
            additional_properties: Some(false.into()),
        }
    );
}

#[test]
fn parse_tool_input_schema_preserves_string_enum_values() {
    let schema = parse_tool_input_schema(&serde_json::json!({
        "type": "string",
        "enum": ["off", "exact", "bounded"]
    }))
    .expect("parse schema");

    assert_eq!(
        schema,
        JsonSchema::String {
            description: None,
            enum_values: Some(vec![
                "off".to_string(),
                "exact".to_string(),
                "bounded".to_string(),
            ]),
        }
    );
}

#[test]
fn parse_tool_input_schema_drops_non_string_enum_when_type_is_inferred() {
    let schema = parse_tool_input_schema(&serde_json::json!({
        "enum": [1, 2, 3]
    }))
    .expect("parse schema");

    assert_eq!(
        schema,
        JsonSchema::String {
            description: None,
            enum_values: None,
        }
    );
}

#[test]
fn parse_tool_input_schema_drops_non_string_enum_for_explicit_string_type() {
    let schema = parse_tool_input_schema(&serde_json::json!({
        "type": "string",
        "enum": ["off", 7, null]
    }))
    .expect("parse schema");

    assert_eq!(
        schema,
        JsonSchema::String {
            description: None,
            enum_values: None,
        }
    );
}

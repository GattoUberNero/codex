use super::*;
use crate::JsonSchema;
use crate::ToolSpec;
use crate::parse_tool_input_schema;
use codex_nero_file_reader::output_schema_json;
use pretty_assertions::assert_eq;

#[test]
fn multi_file_reader_tool_matches_expected_name_and_output_schema() {
    let ToolSpec::Function(tool) = create_multi_file_reader_tool() else {
        panic!("multi_file_reader should use a function tool spec");
    };

    assert_eq!(tool.name, "multi_file_reader");
    assert!(tool.description.contains("Primary tool for reading file"));
    assert!(tool.description.contains("Batch related file reads"));
    assert!(tool.description.contains("Use `full`"));
    assert!(tool.description.contains("output budget"));
    assert_eq!(tool.output_schema, Some(output_schema_json()));

    let JsonSchema::Object {
        properties,
        required,
        additional_properties,
    } = tool.parameters
    else {
        panic!("multi_file_reader parameters should be an object schema");
    };

    assert_eq!(properties.contains_key("requests"), true);
    assert_eq!(required, Some(vec!["requests".to_string()]));
    assert_eq!(additional_properties, Some(false.into()));
    match properties.get("requests") {
        Some(JsonSchema::Array {
            items,
            min_items: Some(1),
            description: Some(description),
            ..
        }) => {
            assert!(description.contains("Batch related file reads"));
            let JsonSchema::Object {
                properties,
                required,
                additional_properties,
            } = items.as_ref()
            else {
                panic!("expected requests items to be object schema, got {items:?}");
            };
            assert_eq!(
                required,
                &Some(vec!["path".to_string(), "mode".to_string()])
            );
            assert_eq!(additional_properties, &Some(false.into()));
            assert_eq!(properties.contains_key("path"), true);
            assert_eq!(properties.contains_key("mode"), true);

            match properties.get("path") {
                Some(JsonSchema::String {
                    description: Some(description),
                    ..
                }) => assert!(description.contains("same reasoning step")),
                other => panic!("expected path string description, got {other:?}"),
            }
            match properties.get("mode") {
                Some(JsonSchema::String {
                    description: Some(description),
                    ..
                }) => {
                    assert!(description.contains("output budget"));
                    assert!(description.contains("use `lines`"));
                }
                other => panic!("expected mode string description, got {other:?}"),
            }
            match properties.get("start_line") {
                Some(JsonSchema::Integer {
                    description: Some(description),
                    ..
                }) => assert!(description.contains("surrounding context")),
                other => panic!("expected start_line integer description, got {other:?}"),
            }
            match properties.get("end_line") {
                Some(JsonSchema::Integer {
                    description: Some(description),
                    ..
                }) => assert!(description.contains("many small ranges")),
                other => panic!("expected end_line integer description, got {other:?}"),
            }
        }
        other => panic!("expected requests array minItems=1, got {other:?}"),
    }
}

#[test]
fn multi_file_reader_raw_schema_preserves_model_guidance_descriptions() {
    let JsonSchema::Object { properties, .. } =
        parse_tool_input_schema(&codex_nero_file_reader::input_schema_json())
            .expect("parse multi_file_reader schema")
    else {
        panic!("multi_file_reader parameters should be an object schema");
    };

    let Some(JsonSchema::Array {
        description: Some(description),
        items,
        ..
    }) = properties.get("requests")
    else {
        panic!("requests should be an array with a description");
    };
    assert!(description.contains("Batch related file reads"));

    let JsonSchema::Object { properties, .. } = items.as_ref() else {
        panic!("requests items should be an object schema");
    };
    assert!(matches!(
        properties.get("mode"),
        Some(JsonSchema::String {
            description: Some(description),
            ..
        }) if description.contains("output budget")
    ));
}

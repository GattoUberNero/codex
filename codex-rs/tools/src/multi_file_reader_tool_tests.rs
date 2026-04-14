use super::*;
use crate::JsonSchema;
use crate::ToolSpec;
use codex_nero_file_reader::output_schema_json;
use pretty_assertions::assert_eq;

#[test]
fn multi_file_reader_tool_matches_expected_name_and_output_schema() {
    let ToolSpec::Function(tool) = create_multi_file_reader_tool() else {
        panic!("multi_file_reader should use a function tool spec");
    };

    assert_eq!(tool.name, "multi_file_reader");
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
            ..
        }) => {
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
        }
        other => panic!("expected requests array minItems=1, got {other:?}"),
    }
}

use crate::ResponsesApiTool;
use crate::ToolSpec;
use crate::parse_tool_input_schema;
use codex_nero_file_reader::TOOL_NAME;
use codex_nero_file_reader::input_schema_json;
use codex_nero_file_reader::output_schema_json;

pub fn create_multi_file_reader_tool() -> ToolSpec {
    let parameters = parse_tool_input_schema(&input_schema_json())
        .unwrap_or_else(|error| panic!("multi_file_reader input schema must be valid: {error}"));
    ToolSpec::Function(ResponsesApiTool {
        name: TOOL_NAME.to_string(),
        description: "Reads multiple files or line ranges in one call. Relative paths are resolved against the current turn cwd."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters,
        output_schema: Some(output_schema_json()),
    })
}

#[cfg(test)]
#[path = "multi_file_reader_tool_tests.rs"]
mod tests;

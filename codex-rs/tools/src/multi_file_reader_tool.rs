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
        description: concat!(
            "Primary tool for reading file contents when available. Batch related file reads in ",
            "one call instead of splitting them across repeated shell commands. Use `full` when ",
            "you need most of a file and expect the batch to stay within the output budget; use ",
            "`lines` for large files or targeted sections. Avoid reading the same file in many ",
            "small chunks unless there is a clear reason. Relative paths are resolved against ",
            "the current turn cwd."
        )
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

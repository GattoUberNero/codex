use serde_json::Value;
use serde_json::json;

pub const TOOL_NAME: &str = "multi_file_reader";
pub const RESPONSE_SCHEMA: &str = "codex.nero.file_reader.response.v1";

pub fn input_schema_json() -> Value {
    json!({
      "type": "object",
      "properties": {
        "requests": {
          "type": "array",
          "minItems": 1,
          "description": "Batch related file reads in one call. Prefer fewer, larger reads over repeated small reads when the total output stays relevant.",
          "items": {
            "type": "object",
            "properties": {
              "request_id": {
                "type": "string",
                "description": "Optional stable ID used in the response for this request item."
              },
              "path": {
                "type": "string",
                "description": "Absolute path or relative path (resolved against current turn cwd). Prefer grouping paths that are needed for the same reasoning step into one request batch."
              },
              "mode": {
                "type": "string",
                "enum": ["full", "lines"],
                "description": "Read mode: use `full` when you need most of a file and expect the batch to stay within the output budget; use `lines` for large files or precise sections. `lines` reads an inclusive line range."
              },
              "start_line": {
                "type": "integer",
                "minimum": 1,
                "description": "Required for `mode: lines`. Choose ranges large enough to preserve surrounding context."
              },
              "end_line": {
                "type": "integer",
                "minimum": 1,
                "description": "Required for `mode: lines`; must be >= start_line. Avoid splitting one nearby area into many small ranges without a clear reason."
              }
            },
            "required": ["path", "mode"],
            "additionalProperties": false
          }
        }
      },
      "required": ["requests"],
      "additionalProperties": false
    })
}

pub fn output_schema_json() -> Value {
    json!({
      "type": "object",
      "properties": {
        "schema": { "type": "string" },
        "ok": { "type": "boolean" },
        "summary": {
          "type": "object",
          "properties": {
            "total_requests": { "type": "integer" },
            "success_count": { "type": "integer" },
            "error_count": { "type": "integer" }
          },
          "required": ["total_requests", "success_count", "error_count"],
          "additionalProperties": false
        },
        "results": {
          "type": "array",
          "items": {
            "type": "object",
            "properties": {
              "request_id": { "type": "string" },
              "status": { "type": "string", "enum": ["success", "error"] },
              "path": { "type": "string" },
              "mode": { "type": "string" },
              "start_line": { "type": "integer" },
              "end_line": { "type": "integer" },
              "line_count": { "type": "integer" },
              "content": { "type": "string" },
              "error_code": { "type": "string" },
              "message": { "type": "string" },
              "hint": { "type": "string" }
            },
            "required": ["request_id", "status", "path", "mode"],
            "additionalProperties": false
          }
        },
        "human_readable_summary": { "type": "string" },
        "global_error": {
          "type": "object",
          "properties": {
            "error_code": { "type": "string" },
            "message": { "type": "string" },
            "hint": { "type": "string" }
          },
          "required": ["error_code", "message", "hint"],
          "additionalProperties": false
        }
      },
      "required": ["schema", "ok", "summary", "results", "human_readable_summary"],
      "additionalProperties": false
    })
}

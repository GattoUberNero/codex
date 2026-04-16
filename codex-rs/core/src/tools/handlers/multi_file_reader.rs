use async_trait::async_trait;
use codex_exec_server::ExecutorFileSystem;
use codex_nero_file_reader::MultiFileReaderFs;
use codex_nero_file_reader::MultiFileReaderResponse;
use codex_nero_file_reader::ReaderConfig;
use codex_nero_file_reader::ReaderFileMetadata;
use codex_nero_file_reader::ResponseItem;
use codex_nero_file_reader::build_text_output;
use codex_nero_file_reader::execute_multi_file_reader_json;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::MultiFileReaderEntry;
use codex_protocol::protocol::MultiFileReaderItemStatus;
use codex_protocol::protocol::MultiFileReaderSummary;
use codex_protocol::protocol::MultiFileReaderToolCallEvent;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde_json::Value as JsonValue;
use std::io;
use std::path::Path;
use std::sync::Arc;

use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct MultiFileReaderHandler;

#[async_trait]
impl ToolHandler for MultiFileReaderHandler {
    type Output = MultiFileReaderToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            call_id,
            payload,
            ..
        } = invocation;
        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "multi_file_reader handler received unsupported payload".to_string(),
                ));
            }
        };

        let fs = CoreMultiFileReaderFs {
            inner: turn.environment.get_filesystem(),
        };
        let response = execute_multi_file_reader_json(
            &arguments,
            &ReaderConfig::default(),
            turn.cwd.as_path(),
            &fs,
        )
        .await;
        session
            .send_event(
                turn.as_ref(),
                EventMsg::MultiFileReaderToolCall(MultiFileReaderToolCallEvent {
                    call_id,
                    summary: MultiFileReaderSummary {
                        total_requests: response.summary.total_requests,
                        success_count: response.summary.success_count,
                        error_count: response.summary.error_count,
                        total_lines: response
                            .results
                            .iter()
                            .filter_map(|item| item.line_count)
                            .sum(),
                    },
                    entries: response
                        .results
                        .iter()
                        .map(response_item_to_event_entry)
                        .collect(),
                }),
            )
            .await;
        Ok(MultiFileReaderToolOutput { response })
    }
}

struct CoreMultiFileReaderFs {
    inner: Arc<dyn ExecutorFileSystem>,
}

#[async_trait]
impl MultiFileReaderFs for CoreMultiFileReaderFs {
    async fn get_metadata(&self, path: &Path) -> io::Result<ReaderFileMetadata> {
        let absolute = absolute_path(path)?;
        let metadata = self.inner.get_metadata(&absolute).await?;
        Ok(ReaderFileMetadata {
            is_file: metadata.is_file,
        })
    }

    async fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        let absolute = absolute_path(path)?;
        self.inner.read_file(&absolute).await
    }
}

fn absolute_path(path: &Path) -> io::Result<AbsolutePathBuf> {
    AbsolutePathBuf::from_absolute_path(path).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path must be absolute, got `{}`: {error}", path.display()),
        )
    })
}

fn response_item_to_event_entry(item: &ResponseItem) -> MultiFileReaderEntry {
    MultiFileReaderEntry {
        path: item.path.clone(),
        mode: item.mode.clone(),
        start_line: item.start_line,
        end_line: item.end_line,
        line_count: item.line_count,
        status: match item.status {
            codex_nero_file_reader::ItemStatus::Success => MultiFileReaderItemStatus::Success,
            codex_nero_file_reader::ItemStatus::Error => MultiFileReaderItemStatus::Error,
        },
        error_code: item.error_code.clone(),
    }
}

pub struct MultiFileReaderToolOutput {
    response: MultiFileReaderResponse,
}

impl ToolOutput for MultiFileReaderToolOutput {
    fn log_preview(&self) -> String {
        build_text_output(&self.response)
    }

    fn success_for_logging(&self) -> bool {
        self.response.ok
    }

    fn to_response_item(&self, call_id: &str, _payload: &ToolPayload) -> ResponseInputItem {
        let output = FunctionCallOutputPayload {
            body: FunctionCallOutputBody::ContentItems(vec![
                FunctionCallOutputContentItem::InputText {
                    text: build_text_output(&self.response),
                },
            ]),
            success: Some(self.response.ok),
        };

        ResponseInputItem::FunctionCallOutput {
            call_id: call_id.to_string(),
            output,
        }
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        serde_json::to_value(&self.response).unwrap_or_else(|error| {
            JsonValue::String(format!("failed to serialize response: {error}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_nero_file_reader::ResponseSummary;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn code_mode_result_returns_structured_response() {
        let output = MultiFileReaderToolOutput {
            response: MultiFileReaderResponse {
                schema: codex_nero_file_reader::RESPONSE_SCHEMA,
                ok: true,
                summary: ResponseSummary {
                    total_requests: 0,
                    success_count: 0,
                    error_count: 0,
                },
                results: vec![],
                human_readable_summary: "Requests: 0 total | 0 success | 0 error".to_string(),
                global_error: None,
            },
        };

        assert_eq!(
            output.code_mode_result(&ToolPayload::Function {
                arguments: "{}".to_string(),
            }),
            json!({
                "schema": codex_nero_file_reader::RESPONSE_SCHEMA,
                "ok": true,
                "summary": {
                    "total_requests": 0,
                    "success_count": 0,
                    "error_count": 0
                },
                "results": [],
                "human_readable_summary": "Requests: 0 total | 0 success | 0 error"
            })
        );
    }
}

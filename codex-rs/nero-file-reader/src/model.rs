use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MultiFileReaderInput {
    pub requests: Vec<ReadRequestInput>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReadRequestInput {
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub start_line: Option<usize>,
    #[serde(default)]
    pub end_line: Option<usize>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Success,
    Error,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MultiFileReaderResponse {
    pub schema: &'static str,
    pub ok: bool,
    pub summary: ResponseSummary,
    pub results: Vec<ResponseItem>,
    pub human_readable_summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_error: Option<ResponseError>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResponseSummary {
    pub total_requests: usize,
    pub success_count: usize,
    pub error_count: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResponseItem {
    pub request_id: String,
    pub status: ItemStatus,
    pub path: String,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResponseError {
    pub error_code: String,
    pub message: String,
    pub hint: String,
}

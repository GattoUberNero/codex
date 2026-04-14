use crate::fs::MultiFileReaderFs;
use crate::model::ItemStatus;
use crate::model::MultiFileReaderInput;
use crate::model::MultiFileReaderResponse;
use crate::model::ReadRequestInput;
use crate::model::ResponseError;
use crate::model::ResponseItem;
use crate::model::ResponseSummary;
use crate::schema::RESPONSE_SCHEMA;
use std::io;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

pub const DEFAULT_MAX_TOTAL_LINES: usize = 12_000;
pub const DEFAULT_MAX_REQUESTS: usize = 20;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReaderConfig {
    pub max_total_lines: usize,
    pub max_requests: usize,
}

impl Default for ReaderConfig {
    fn default() -> Self {
        Self {
            max_total_lines: DEFAULT_MAX_TOTAL_LINES,
            max_requests: DEFAULT_MAX_REQUESTS,
        }
    }
}

pub async fn execute_multi_file_reader_json<F: MultiFileReaderFs + ?Sized>(
    arguments: &str,
    config: &ReaderConfig,
    cwd: &Path,
    fs: &F,
) -> MultiFileReaderResponse {
    match parse_input(arguments) {
        Ok(input) => execute_requests(input, config, cwd, fs).await,
        Err(global_error) => {
            let summary = ResponseSummary {
                total_requests: 0,
                success_count: 0,
                error_count: 1,
            };
            MultiFileReaderResponse {
                schema: RESPONSE_SCHEMA,
                ok: false,
                summary: summary.clone(),
                results: vec![],
                human_readable_summary: human_readable_summary(&summary),
                global_error: Some(global_error),
            }
        }
    }
}

fn parse_input(arguments: &str) -> Result<MultiFileReaderInput, ResponseError> {
    let input: MultiFileReaderInput =
        serde_json::from_str(arguments).map_err(|error| ResponseError {
            error_code: "invalid_input".to_string(),
            message: format!("Failed to parse tool input: {error}"),
            hint: "Provide valid JSON with non-empty `requests`.".to_string(),
        })?;

    if input.requests.is_empty() {
        return Err(ResponseError {
            error_code: "invalid_input".to_string(),
            message: "Input `requests` must contain at least one item.".to_string(),
            hint: "Provide one or more read requests in `requests[]`.".to_string(),
        });
    }

    Ok(input)
}

async fn execute_requests<F: MultiFileReaderFs + ?Sized>(
    input: MultiFileReaderInput,
    config: &ReaderConfig,
    cwd: &Path,
    fs: &F,
) -> MultiFileReaderResponse {
    let mut line_budget = config.max_total_lines;
    let mut results = Vec::with_capacity(input.requests.len());

    for (index, request) in input.requests.iter().enumerate() {
        let request_id = request_id_for(request, index);
        if index >= config.max_requests {
            results.push(error_item(
                request_id,
                request.path.clone().unwrap_or_else(|| "<missing-path>".to_string()),
                request.mode.clone().unwrap_or_else(|| "unknown".to_string()),
                "too_many_requests_for_batch_item_handling",
                "Request could not be fulfilled because this call exceeded the configured per-batch request limit.",
                "Split the batch into smaller calls or lower the number of request items.",
            ));
            continue;
        }
        results.push(process_request(request, request_id, cwd, fs, &mut line_budget).await);
    }

    let summary = summarize_results(&results);
    MultiFileReaderResponse {
        schema: RESPONSE_SCHEMA,
        ok: true,
        summary: summary.clone(),
        results,
        human_readable_summary: human_readable_summary(&summary),
        global_error: None,
    }
}

async fn process_request<F: MultiFileReaderFs + ?Sized>(
    request: &ReadRequestInput,
    request_id: String,
    cwd: &Path,
    fs: &F,
    line_budget: &mut usize,
) -> ResponseItem {
    let mode = request
        .mode
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    let raw_path = match request.path.as_deref().map(str::trim) {
        Some(path) if !path.is_empty() => path,
        _ => {
            return error_item(
                request_id,
                "<missing-path>".to_string(),
                mode,
                "invalid_path",
                "Missing or empty `path`.",
                "Provide a non-empty path for each request item.",
            );
        }
    };

    let normalized_path = normalize_absolute_path(raw_path, cwd);
    let path_text = normalized_path.to_string_lossy().to_string();

    let metadata = match fs.get_metadata(normalized_path.as_path()).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return error_item(
                request_id,
                path_text,
                mode,
                "file_not_found",
                "File does not exist.",
                "Check whether the path is correct or whether the file was moved or deleted.",
            );
        }
        Err(error) => {
            return error_item(
                request_id,
                path_text,
                mode,
                "internal_error",
                &format!("Failed to read file metadata: {error}"),
                "Retry the request or verify filesystem permissions.",
            );
        }
    };

    if !metadata.is_file {
        return error_item(
            request_id,
            path_text,
            mode,
            "not_a_file",
            "Path exists but is not a regular file.",
            "Provide a path to a regular file.",
        );
    }

    let file_bytes = match fs.read_file(normalized_path.as_path()).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return error_item(
                request_id,
                path_text,
                mode,
                "file_not_found",
                "File does not exist.",
                "Check whether the path is correct or whether the file was moved or deleted.",
            );
        }
        Err(error) => {
            return error_item(
                request_id,
                path_text,
                mode,
                "internal_error",
                &format!("Failed to read file contents: {error}"),
                "Retry the request or verify filesystem permissions.",
            );
        }
    };

    let file_contents = String::from_utf8_lossy(&file_bytes);

    let lines = split_preserving_blank_lines(file_contents.as_ref());
    let mode_lower = mode.to_ascii_lowercase();
    match mode_lower.as_str() {
        "full" => {
            if lines.len() > *line_budget {
                return error_item(
                    request_id,
                    path_text,
                    "full".to_string(),
                    "line_budget_exceeded",
                    "Request could not be fulfilled because the batch line budget was exhausted.",
                    "Split the batch into fewer items or reduce the requested ranges.",
                );
            }
            *line_budget -= lines.len();
            let entries = lines
                .iter()
                .enumerate()
                .map(|(index, line)| (index + 1, *line))
                .collect::<Vec<_>>();

            success_item(
                request_id,
                path_text,
                "full".to_string(),
                if lines.is_empty() { 0 } else { 1 },
                lines.len(),
                entries.len(),
                format_numbered_lines(&entries),
            )
        }
        "lines" => {
            let start_line = match request.start_line {
                Some(value) => value,
                None => {
                    return error_item(
                        request_id,
                        path_text,
                        "lines".to_string(),
                        "start_line_missing",
                        "Missing `start_line` for lines mode.",
                        "Provide `start_line >= 1`.",
                    );
                }
            };
            let end_line = match request.end_line {
                Some(value) => value,
                None => {
                    return error_item(
                        request_id,
                        path_text,
                        "lines".to_string(),
                        "end_line_missing",
                        "Missing `end_line` for lines mode.",
                        "Provide `end_line >= start_line`.",
                    );
                }
            };

            if start_line == 0 || end_line == 0 || end_line < start_line {
                return error_item(
                    request_id,
                    path_text,
                    "lines".to_string(),
                    "invalid_line_range",
                    "Requested line range is invalid: end_line must be greater than or equal to start_line.",
                    "Provide start_line >= 1 and end_line >= start_line.",
                );
            }

            let selected = (start_line..=end_line)
                .filter_map(|line_number| {
                    lines
                        .get(line_number.saturating_sub(1))
                        .map(|line| (line_number, *line))
                })
                .collect::<Vec<_>>();

            if selected.len() > *line_budget {
                return error_item(
                    request_id,
                    path_text,
                    "lines".to_string(),
                    "line_budget_exceeded",
                    "Request could not be fulfilled because the batch line budget was exhausted.",
                    "Split the batch into fewer items or reduce the requested ranges.",
                );
            }

            *line_budget -= selected.len();
            success_item(
                request_id,
                path_text,
                "lines".to_string(),
                start_line,
                end_line,
                selected.len(),
                format_numbered_lines(&selected),
            )
        }
        _ => error_item(
            request_id,
            path_text,
            mode,
            "invalid_mode",
            "Invalid mode value. Supported modes are `full` and `lines`.",
            "Provide mode as `full` or `lines`.",
        ),
    }
}

fn request_id_for(request: &ReadRequestInput, index: usize) -> String {
    match request.request_id.as_deref().map(str::trim) {
        Some(value) if !value.is_empty() => value.to_string(),
        _ => format!("generated-{index}"),
    }
}

fn error_item(
    request_id: String,
    path: String,
    mode: String,
    error_code: &str,
    message: &str,
    hint: &str,
) -> ResponseItem {
    ResponseItem {
        request_id,
        status: ItemStatus::Error,
        path,
        mode,
        start_line: None,
        end_line: None,
        line_count: None,
        content: None,
        error_code: Some(error_code.to_string()),
        message: Some(message.to_string()),
        hint: Some(hint.to_string()),
    }
}

fn success_item(
    request_id: String,
    path: String,
    mode: String,
    start_line: usize,
    end_line: usize,
    line_count: usize,
    content: String,
) -> ResponseItem {
    ResponseItem {
        request_id,
        status: ItemStatus::Success,
        path,
        mode,
        start_line: Some(start_line),
        end_line: Some(end_line),
        line_count: Some(line_count),
        content: Some(content),
        error_code: None,
        message: None,
        hint: None,
    }
}

fn split_preserving_blank_lines(contents: &str) -> Vec<&str> {
    let mut lines = contents.split('\n').collect::<Vec<_>>();
    if contents.ends_with('\n') {
        let _ = lines.pop();
    }
    for line in &mut lines {
        *line = line.strip_suffix('\r').unwrap_or(line);
    }
    lines
}

fn format_numbered_lines(lines: &[(usize, &str)]) -> String {
    lines
        .iter()
        .map(|(line_number, line)| format!("{line_number:>6}\t{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn summarize_results(results: &[ResponseItem]) -> ResponseSummary {
    let success_count = results
        .iter()
        .filter(|item| item.status == ItemStatus::Success)
        .count();
    let error_count = results
        .iter()
        .filter(|item| item.status == ItemStatus::Error)
        .count();

    ResponseSummary {
        total_requests: results.len(),
        success_count,
        error_count,
    }
}

fn human_readable_summary(summary: &ResponseSummary) -> String {
    format!(
        "Requests: {} total | {} success | {} error",
        summary.total_requests, summary.success_count, summary.error_count
    )
}

fn normalize_absolute_path(raw_path: &str, cwd: &Path) -> PathBuf {
    let joined = {
        let input_path = Path::new(raw_path);
        if input_path.is_absolute() {
            input_path.to_path_buf()
        } else {
            cwd.join(input_path)
        }
    };

    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

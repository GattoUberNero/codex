use crate::model::ItemStatus;
use crate::model::MultiFileReaderResponse;

pub fn build_text_output(response: &MultiFileReaderResponse) -> String {
    let mut out = String::new();
    out.push_str(&response.human_readable_summary);

    if let Some(global_error) = &response.global_error {
        out.push_str("\n\n=== GLOBAL ERROR ===\n");
        out.push_str(&format!("ERROR: {}\n", global_error.error_code));
        out.push_str(&format!("MESSAGE: {}\n", global_error.message));
        out.push_str(&format!("HINT: {}", global_error.hint));
        return out;
    }

    for result in &response.results {
        out.push_str("\n\n");
        match result.status {
            ItemStatus::Success => {
                out.push_str(&format!(
                    "=== REQUEST {} | success ===\n",
                    result.request_id
                ));
                out.push_str(&format!("PATH: {}\n", result.path));
                out.push_str(&format!("MODE: {}\n", result.mode));
                if let (Some(start_line), Some(end_line)) = (result.start_line, result.end_line) {
                    out.push_str(&format!("LINES: {start_line}-{end_line}\n"));
                }
                out.push('\n');
                if let Some(content) = &result.content {
                    out.push_str(content);
                }
            }
            ItemStatus::Error => {
                out.push_str(&format!("=== REQUEST {} | error ===\n", result.request_id));
                out.push_str(&format!("PATH: {}\n", result.path));
                out.push_str(&format!("MODE: {}\n", result.mode));
                out.push_str(&format!(
                    "ERROR: {}\n",
                    result.error_code.as_deref().unwrap_or("internal_error")
                ));
                out.push_str(&format!(
                    "MESSAGE: {}\n",
                    result
                        .message
                        .as_deref()
                        .unwrap_or("An unknown error occurred.")
                ));
                out.push_str(&format!(
                    "HINT: {}",
                    result
                        .hint
                        .as_deref()
                        .unwrap_or("Retry the request with valid parameters.")
                ));
            }
        }
    }

    out
}

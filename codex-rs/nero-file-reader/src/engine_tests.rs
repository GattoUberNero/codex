use crate::engine::ReaderConfig;
use crate::execute_multi_file_reader_json;
use crate::fs::LocalMultiFileReaderFs;
use crate::model::ItemStatus;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::tempdir;

#[tokio::test]
async fn reads_full_file_and_generates_request_id() {
    let root = tempdir().expect("tempdir");
    let file_path = root.path().join("doc.txt");
    tokio::fs::write(&file_path, "line one\n\nline three\n")
        .await
        .expect("write test file");

    let input = json!({
        "requests": [{
            "path": file_path,
            "mode": "full"
        }]
    })
    .to_string();

    let response = execute_multi_file_reader_json(
        &input,
        &ReaderConfig::default(),
        root.path(),
        &LocalMultiFileReaderFs,
    )
    .await;

    assert_eq!(response.ok, true);
    assert_eq!(response.summary.total_requests, 1);
    assert_eq!(response.summary.success_count, 1);
    assert_eq!(response.summary.error_count, 0);
    assert_eq!(response.results.len(), 1);
    assert_eq!(response.results[0].request_id, "generated-0");
    assert_eq!(response.results[0].status, ItemStatus::Success);
    assert_eq!(
        response.results[0].content.as_deref(),
        Some("     1\tline one\n     2\t\n     3\tline three")
    );
}

#[tokio::test]
async fn returns_global_error_for_invalid_json() {
    let root = tempdir().expect("tempdir");
    let response = execute_multi_file_reader_json(
        "{",
        &ReaderConfig::default(),
        root.path(),
        &LocalMultiFileReaderFs,
    )
    .await;

    assert_eq!(response.ok, false);
    assert_eq!(response.summary.total_requests, 0);
    assert_eq!(response.summary.success_count, 0);
    assert_eq!(response.summary.error_count, 1);
    assert_eq!(
        response
            .global_error
            .as_ref()
            .map(|error| error.error_code.as_str()),
        Some("invalid_input")
    );
}

#[tokio::test]
async fn lines_mode_requires_start_line() {
    let root = tempdir().expect("tempdir");
    let file_path = root.path().join("doc.txt");
    tokio::fs::write(&file_path, "a\nb\nc\n")
        .await
        .expect("write test file");

    let input = json!({
        "requests": [{
            "request_id": "r1",
            "path": file_path,
            "mode": "lines",
            "end_line": 2
        }]
    })
    .to_string();

    let response = execute_multi_file_reader_json(
        &input,
        &ReaderConfig::default(),
        root.path(),
        &LocalMultiFileReaderFs,
    )
    .await;

    assert_eq!(response.ok, true);
    assert_eq!(response.summary.success_count, 0);
    assert_eq!(response.summary.error_count, 1);
    assert_eq!(response.results[0].request_id, "r1");
    assert_eq!(response.results[0].status, ItemStatus::Error);
    assert_eq!(
        response.results[0].error_code.as_deref(),
        Some("start_line_missing")
    );
}

#[tokio::test]
async fn resolves_relative_paths_against_cwd() {
    let root = tempdir().expect("tempdir");
    let nested = root.path().join("nested");
    tokio::fs::create_dir(&nested).await.expect("create nested");
    tokio::fs::write(nested.join("doc.txt"), "x\ny\n")
        .await
        .expect("write test file");

    let input = json!({
        "requests": [{
            "path": "nested/doc.txt",
            "mode": "lines",
            "start_line": 2,
            "end_line": 2
        }]
    })
    .to_string();

    let response = execute_multi_file_reader_json(
        &input,
        &ReaderConfig::default(),
        root.path(),
        &LocalMultiFileReaderFs,
    )
    .await;

    assert_eq!(response.ok, true);
    assert_eq!(response.summary.success_count, 1);
    assert_eq!(response.results[0].status, ItemStatus::Success);
    assert_eq!(response.results[0].line_count, Some(1));
    assert_eq!(response.results[0].content.as_deref(), Some("     2\ty"));
}

#[tokio::test]
async fn enforces_batch_request_limit() {
    let root = tempdir().expect("tempdir");
    let file_path = root.path().join("doc.txt");
    tokio::fs::write(&file_path, "a\n")
        .await
        .expect("write test file");
    let config = ReaderConfig {
        max_total_lines: 100,
        max_requests: 1,
    };

    let input = json!({
        "requests": [
            {"path": file_path, "mode": "full"},
            {"path": file_path, "mode": "full"}
        ]
    })
    .to_string();

    let response =
        execute_multi_file_reader_json(&input, &config, root.path(), &LocalMultiFileReaderFs).await;

    assert_eq!(response.ok, true);
    assert_eq!(response.summary.total_requests, 2);
    assert_eq!(response.summary.success_count, 1);
    assert_eq!(response.summary.error_count, 1);
    assert_eq!(
        response.results[1].error_code.as_deref(),
        Some("too_many_requests_for_batch_item_handling")
    );
}

#[tokio::test]
async fn enforces_line_budget() {
    let root = tempdir().expect("tempdir");
    let file_path = root.path().join("doc.txt");
    tokio::fs::write(&file_path, "a\nb\n")
        .await
        .expect("write test file");
    let config = ReaderConfig {
        max_total_lines: 1,
        max_requests: 20,
    };

    let input = json!({
        "requests": [
            {"path": file_path, "mode": "full"}
        ]
    })
    .to_string();

    let response =
        execute_multi_file_reader_json(&input, &config, root.path(), &LocalMultiFileReaderFs).await;

    assert_eq!(response.ok, true);
    assert_eq!(response.summary.success_count, 0);
    assert_eq!(response.summary.error_count, 1);
    assert_eq!(
        response.results[0].error_code.as_deref(),
        Some("line_budget_exceeded")
    );
}

#[tokio::test]
async fn normalizes_crlf_in_rendered_output() {
    let root = tempdir().expect("tempdir");
    let file_path = root.path().join("doc.txt");
    tokio::fs::write(&file_path, b"a\r\nb\r\n")
        .await
        .expect("write test file");

    let input = json!({
        "requests": [{
            "path": file_path,
            "mode": "full"
        }]
    })
    .to_string();

    let response = execute_multi_file_reader_json(
        &input,
        &ReaderConfig::default(),
        root.path(),
        &LocalMultiFileReaderFs,
    )
    .await;

    assert_eq!(response.summary.success_count, 1);
    assert_eq!(
        response.results[0].content.as_deref(),
        Some("     1\ta\n     2\tb")
    );
}

#[tokio::test]
async fn decodes_invalid_utf8_lossily_instead_of_failing_request() {
    let root = tempdir().expect("tempdir");
    let file_path = root.path().join("doc.txt");
    tokio::fs::write(&file_path, b"ok\nbad:\xff\n")
        .await
        .expect("write test file");

    let input = json!({
        "requests": [{
            "path": file_path,
            "mode": "lines",
            "start_line": 1,
            "end_line": 2
        }]
    })
    .to_string();

    let response = execute_multi_file_reader_json(
        &input,
        &ReaderConfig::default(),
        root.path(),
        &LocalMultiFileReaderFs,
    )
    .await;

    assert_eq!(response.summary.success_count, 1);
    assert_eq!(response.summary.error_count, 0);
    assert_eq!(
        response.results[0].content.as_deref(),
        Some("     1\tok\n     2\tbad:�")
    );
}

#[tokio::test]
async fn rejects_unknown_top_level_fields() {
    let root = tempdir().expect("tempdir");
    let input = json!({
        "requests": [],
        "unexpected": true
    })
    .to_string();

    let response = execute_multi_file_reader_json(
        &input,
        &ReaderConfig::default(),
        root.path(),
        &LocalMultiFileReaderFs,
    )
    .await;

    assert_eq!(response.ok, false);
    assert_eq!(
        response
            .global_error
            .as_ref()
            .map(|error| error.error_code.as_str()),
        Some("invalid_input")
    );
}

#[tokio::test]
async fn rejects_unknown_request_fields() {
    let root = tempdir().expect("tempdir");
    let file_path = root.path().join("doc.txt");
    tokio::fs::write(&file_path, "line one\n")
        .await
        .expect("write test file");

    let input = json!({
        "requests": [{
            "path": file_path,
            "mode": "full",
            "unexpected": true
        }]
    })
    .to_string();

    let response = execute_multi_file_reader_json(
        &input,
        &ReaderConfig::default(),
        root.path(),
        &LocalMultiFileReaderFs,
    )
    .await;

    assert_eq!(response.ok, false);
    assert_eq!(
        response
            .global_error
            .as_ref()
            .map(|error| error.error_code.as_str()),
        Some("invalid_input")
    );
}

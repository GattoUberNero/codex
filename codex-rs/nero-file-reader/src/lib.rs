#![deny(clippy::print_stdout, clippy::print_stderr)]

mod engine;
mod fs;
mod model;
mod render;
mod schema;

pub use engine::ReaderConfig;
pub use engine::execute_multi_file_reader_json;
pub use fs::LocalMultiFileReaderFs;
pub use fs::MultiFileReaderFs;
pub use fs::ReaderFileMetadata;
pub use model::ItemStatus;
pub use model::MultiFileReaderInput;
pub use model::MultiFileReaderResponse;
pub use model::ReadRequestInput;
pub use model::ResponseError;
pub use model::ResponseItem;
pub use model::ResponseSummary;
pub use render::build_text_output;
pub use schema::RESPONSE_SCHEMA;
pub use schema::TOOL_NAME;
pub use schema::input_schema_json;
pub use schema::output_schema_json;

#[cfg(test)]
mod engine_tests;

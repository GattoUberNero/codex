use async_trait::async_trait;
use std::io;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReaderFileMetadata {
    pub is_file: bool,
}

#[async_trait]
pub trait MultiFileReaderFs: Send + Sync {
    async fn get_metadata(&self, path: &Path) -> io::Result<ReaderFileMetadata>;

    async fn read_file(&self, path: &Path) -> io::Result<Vec<u8>>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LocalMultiFileReaderFs;

#[async_trait]
impl MultiFileReaderFs for LocalMultiFileReaderFs {
    async fn get_metadata(&self, path: &Path) -> io::Result<ReaderFileMetadata> {
        let metadata = tokio::fs::metadata(path).await?;
        Ok(ReaderFileMetadata {
            is_file: metadata.is_file(),
        })
    }

    async fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        tokio::fs::read(path).await
    }
}

//! Read-only review of the current Git index and working tree.
mod document;
mod model;
mod patch;
mod process;
mod repository;

pub use document::{TextDocument, read_document};
pub use model::{
    CommittedComparison, Comparison, DiffDocument, DiffRow, FileChange, GitPath, ReflogEntry,
    ReflogList, ReflogRequest, RowKind, View,
};
pub use process::CancelToken;
pub use repository::Repository;

pub const MAX_SOURCE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_PATCH_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_DIFF_ROWS: usize = 100_000;
pub const REFLOG_PAGE_SIZE: usize = 15;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Git is unavailable. Install Git and ensure it is on PATH: {0}")]
    GitUnavailable(std::io::Error),
    #[error("Git operation failed: {0}")]
    Git(String),
    #[error("Operation cancelled")]
    Cancelled,
    #[error("Git output exceeds the automatic loading limit")]
    OutputLimit,
    #[error("Invalid Git data: {0}")]
    InvalidData(String),
    #[error("{0}")]
    Document(String),
    #[error("Invalid repository-relative path")]
    InvalidPath,
    #[error("File access failed: {0}")]
    Io(#[from] std::io::Error),
}

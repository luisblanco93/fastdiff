//! Lightweight launcher, local IPC, and WSL helper bridge.
mod args;
mod backend;
mod frame;
mod helper;
mod ipc;
mod launch;
mod pipe;

pub use args::{Command, HELP, OpenRequest, OpenTarget, WslTarget, parse};
pub use backend::{Backend, HelperClient};
pub use frame::{MAX_MESSAGE_BYTES, read_frame, write_frame};
pub use helper::{
    HelperRequest, HelperResponse, RequestEnvelope, ResponseEnvelope, run_helper, run_stdio_helper,
    run_stdio_helper_target,
};
pub use ipc::{IpcServer, forward, forward_at};
pub use launch::launch;

pub const PROTOCOL_VERSION: u32 = 5;
pub const READY_MESSAGE: &str = "FASTDIFF_ACCEPTED";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    InvalidArgument(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid protocol message: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Protocol(String),
    #[error("{0}")]
    Backend(String),
    #[error("Operation cancelled")]
    Cancelled,
}
pub type Result<T> = std::result::Result<T, Error>;

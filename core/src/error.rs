//! Public error type for the Dropwire engine.

use thiserror::Error;

use crate::progress::ErrorCode;

/// Errors surfaced across the `irohcore` boundary.
///
/// Internals use `anyhow` freely; everything that crosses the public API is
/// normalized into this enum so callers (the Tauri shell) never see iroh-blobs
/// error types.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid ticket: {0}")]
    InvalidTicket(String),

    /// Couldn't reach the sender (offline, unreachable, or the link expired).
    /// The technical detail is kept for logs; the message is the plain one.
    #[error("Could not reach the sender. They may be offline, or the code may have expired.")]
    Unreachable(String),

    #[error("transfer not found: {0}")]
    NotFound(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl CoreError {
    /// The [`ErrorCode`] a transfer that ended with this error reports.
    pub fn code(&self) -> ErrorCode {
        match self {
            CoreError::Io(e) => crate::fail::io_reason(e).0,
            CoreError::Unreachable(_) => ErrorCode::Unreachable,
            CoreError::InvalidTicket(_) | CoreError::NotFound(_) | CoreError::Other(_) => {
                ErrorCode::Other
            }
        }
    }
}

/// Convenience alias used throughout the public API.
pub type Result<T> = std::result::Result<T, CoreError>;

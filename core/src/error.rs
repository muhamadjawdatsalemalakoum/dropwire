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

    /// The code could not be read at all, usually because it was cut off when
    /// it was copied. Holds what was given.
    #[error("That code is not valid. It may have been cut off, so copy the whole code again.")]
    InvalidTicket(String),

    /// Couldn't reach the sender (offline, unreachable, or the link expired).
    /// The technical detail is kept for logs; the message is the plain one.
    #[error("Could not reach the sender. They may be offline, or the code may have expired.")]
    Unreachable(String),

    /// The sender refused this device: a code works for one device, and
    /// another one already used it, or the sender stopped sharing it.
    #[error(
        "This code was already used by another device, or the sender stopped sharing it. \
         Ask the sender for a new code."
    )]
    AlreadyClaimed,

    /// The folder chosen for a receive cannot be used. Checked before anything
    /// is downloaded.
    #[error("Cannot save to {path}: {reason}. Choose another folder.")]
    Destination { path: String, reason: String },

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
            CoreError::AlreadyClaimed => ErrorCode::AlreadyClaimed,
            CoreError::InvalidTicket(_)
            | CoreError::Destination { .. }
            | CoreError::NotFound(_)
            | CoreError::Other(_) => ErrorCode::Other,
        }
    }

    /// A stable name for this kind of error, for callers that show different
    /// help for each: `invalidTicket` for a code that cannot be read,
    /// `destination` for a folder that cannot be saved to, or one of the
    /// [`ErrorCode`] names (`unreachable`, `alreadyClaimed`, ..., `other`).
    pub fn kind(&self) -> &'static str {
        match self {
            CoreError::InvalidTicket(_) => "invalidTicket",
            CoreError::Destination { .. } => "destination",
            other => other.code().as_str(),
        }
    }
}

/// Convenience alias used throughout the public API.
pub type Result<T> = std::result::Result<T, CoreError>;

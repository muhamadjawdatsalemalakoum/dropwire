//! Public error type for the Dropwire engine.

use thiserror::Error;

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

    #[error("transfer not found: {0}")]
    NotFound(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Convenience alias used throughout the public API.
pub type Result<T> = std::result::Result<T, CoreError>;

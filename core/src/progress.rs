//! The progress/event vocabulary the rest of the app sees.
//!
//! These types are deliberately free of any iroh-blobs types so the UI layer
//! depends only on `irohcore`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Opaque identifier for one transfer (send or receive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TransferId(pub Uuid);

impl TransferId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TransferId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TransferId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for TransferId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

/// Direction of a transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    Send,
    Receive,
}

/// Whether the active connection is direct (peer-to-peer) or via the relay.
/// Surfaced in the UI as the "direct vs relayed" badge and used to reason about
/// bandwidth cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Route {
    Direct,
    Relayed,
    Unknown,
}

/// Final statistics for a completed transfer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferStats {
    pub bytes: u64,
    pub seconds: f64,
    /// Receive only: files and top-level folders saved under a different name
    /// than the one they were sent with, because the name was already taken in
    /// the destination (nothing on disk is ever replaced), collided with another
    /// name in the same transfer, or is not allowed on Windows. A renamed
    /// top-level folder is listed once, not per file inside it. Empty for sends
    /// and when nothing was renamed; lists at most 1000 entries.
    #[serde(default)]
    pub renamed: Vec<RenamedFile>,
}

/// One entry of [`TransferStats::renamed`]. Both paths are relative to the
/// destination folder and use forward slashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenamedFile {
    /// The name as the sender sent it.
    pub name: String,
    /// The name it was saved under.
    pub saved_as: String,
}

/// One file in a [`TransferPreview`]: its name and byte size. Both are committed
/// by the ticket's BLAKE3 hash, so they are facts the sender cannot fake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePreview {
    pub name: String,
    pub size: u64,
}

/// What a transfer contains, learned from the sender *before* downloading any
/// file content: the file list, count, total size, and the connection route.
/// This is what powers "preview before you accept".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferPreview {
    pub files: Vec<FilePreview>,
    pub file_count: usize,
    pub total_bytes: u64,
    pub route: Route,
}

/// What kind of failure ended a transfer (see [`Progress::Error`]), so the app
/// can decide what to offer without reading the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    /// The sender could not be reached: offline, or the code has expired.
    Unreachable,
    /// The sender refused this device: another device already used the code,
    /// or the sender stopped sharing it.
    AlreadyClaimed,
    /// A file or folder is no longer there.
    NotFound,
    /// The system refused access to a file or folder, or the drive is
    /// read-only.
    PermissionDenied,
    /// The disk is full.
    DiskFull,
    /// A file is open in another app.
    FileInUse,
    /// Anything else. The message says what happened.
    #[default]
    Other,
}

impl ErrorCode {
    /// The name this code is serialized as.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Unreachable => "unreachable",
            ErrorCode::AlreadyClaimed => "alreadyClaimed",
            ErrorCode::NotFound => "notFound",
            ErrorCode::PermissionDenied => "permissionDenied",
            ErrorCode::DiskFull => "diskFull",
            ErrorCode::FileInUse => "fileInUse",
            ErrorCode::Other => "other",
        }
    }
}

/// Progress events emitted on a transfer's [`ProgressStream`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Progress {
    /// Sender: hashing/importing the chosen path into the local store.
    Importing {
        id: TransferId,
        done: u64,
        total: u64,
    },
    /// Sender: content imported, ticket minted, now serving.
    Ready { id: TransferId, ticket: String },
    /// Sender: a receiver connected.
    PeerJoined { id: TransferId },
    /// Receiver (and, later, sender): bytes are moving.
    Transferring {
        id: TransferId,
        offset: u64,
        total: u64,
        route: Route,
    },
    /// Transfer completed successfully.
    Done {
        id: TransferId,
        stats: TransferStats,
    },
    /// Transfer failed. `message` is a plain sentence meant for the screen;
    /// `code` says what kind of failure it was, for choosing what to offer next.
    Error {
        id: TransferId,
        #[serde(default)]
        code: ErrorCode,
        message: String,
    },
    /// Transfer was cancelled by the user.
    Cancelled { id: TransferId },
}

impl Progress {
    /// The transfer this event belongs to.
    pub fn id(&self) -> TransferId {
        match self {
            Progress::Importing { id, .. }
            | Progress::Ready { id, .. }
            | Progress::PeerJoined { id, .. }
            | Progress::Transferring { id, .. }
            | Progress::Done { id, .. }
            | Progress::Error { id, .. }
            | Progress::Cancelled { id, .. } => *id,
        }
    }
}

/// A stream of [`Progress`] events for one transfer. Implements
/// [`futures_lite::Stream`] (and `tokio_stream::Stream`), so the shell can
/// `.next().await` it.
pub type ProgressStream = tokio_stream::wrappers::ReceiverStream<Progress>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_names_match_their_serialized_form() {
        for code in [
            ErrorCode::Unreachable,
            ErrorCode::AlreadyClaimed,
            ErrorCode::NotFound,
            ErrorCode::PermissionDenied,
            ErrorCode::DiskFull,
            ErrorCode::FileInUse,
            ErrorCode::Other,
        ] {
            let json = serde_json::to_value(code).unwrap();
            assert_eq!(json, serde_json::Value::String(code.as_str().into()));
        }
    }

    #[test]
    fn an_error_without_a_code_still_reads() {
        let id = TransferId::new();
        let json = serde_json::json!({ "kind": "error", "id": id, "message": "x" });
        let Progress::Error { code, .. } = serde_json::from_value(json).unwrap() else {
            panic!("expected an error event");
        };
        assert_eq!(code, ErrorCode::Other);
    }
}

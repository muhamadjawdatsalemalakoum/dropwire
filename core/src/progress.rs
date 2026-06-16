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
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TransferStats {
    pub bytes: u64,
    pub seconds: f64,
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
    /// Transfer failed.
    Error { id: TransferId, message: String },
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

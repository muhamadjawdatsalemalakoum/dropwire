//! Local, on-disk catalog of transfers.
//!
//! Privacy-first: this lives only on the user's machine (`transfers.json` in the
//! data dir). It exists so the UI can show a transfer list and offer "resume"
//! after a crash/restart. Nothing here is ever sent anywhere.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::progress::{Direction, TransferId};

/// Status of a catalog entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Active,
    Done,
    Error,
    Cancelled,
    /// Started but not finished (e.g. app closed mid-transfer) — resumable.
    Interrupted,
}

/// One transfer in the local catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferRecord {
    pub id: TransferId,
    pub direction: Direction,
    /// Display name (file or folder name).
    pub name: String,
    /// The ticket string (lets a receive be resumed).
    pub ticket: String,
    /// Hex of the content hash.
    pub hash: String,
    /// Destination directory for receives.
    pub dest: Option<String>,
    /// Source path for sends (lets a send be re-shared from history).
    #[serde(default)]
    pub source: Option<String>,
    pub total_bytes: u64,
    pub transferred: u64,
    pub status: Status,
    pub created_at: u64,
    pub updated_at: u64,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The persisted catalog.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Catalog {
    #[serde(default)]
    entries: BTreeMap<String, TransferRecord>,
    #[serde(skip)]
    path: PathBuf,
}

impl Catalog {
    /// Load the catalog from disk, or start empty if absent/corrupt.
    pub fn load(path: PathBuf) -> Self {
        let mut cat = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Catalog>(&bytes).unwrap_or_default(),
            Err(_) => Catalog::default(),
        };
        cat.path = path;
        cat
    }

    /// Insert or update a record, then persist (best-effort).
    pub fn upsert(&mut self, mut rec: TransferRecord) {
        rec.updated_at = now_secs();
        self.entries.insert(rec.id.to_string(), rec);
        self.save();
    }

    /// Update the status (and optionally transferred bytes) of an entry.
    pub fn set_status(&mut self, id: TransferId, status: Status, transferred: Option<u64>) {
        if let Some(rec) = self.entries.get_mut(&id.to_string()) {
            rec.status = status;
            if let Some(t) = transferred {
                rec.transferred = t;
            }
            rec.updated_at = now_secs();
        }
        self.save();
    }

    #[allow(dead_code)] // used by the shell layer (resume-by-id); kept on the API surface
    pub fn get(&self, id: TransferId) -> Option<TransferRecord> {
        self.entries.get(&id.to_string()).cloned()
    }

    /// All records, newest first.
    pub fn list(&self) -> Vec<TransferRecord> {
        let mut v: Vec<_> = self.entries.values().cloned().collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        v
    }

    /// On startup, mark any still-"active" entries as interrupted (the process
    /// clearly didn't finish them).
    pub fn mark_stale_interrupted(&mut self) {
        let mut changed = false;
        for rec in self.entries.values_mut() {
            if rec.status == Status::Active {
                rec.status = Status::Interrupted;
                changed = true;
            }
        }
        if changed {
            self.save();
        }
    }

    /// Build a fresh record stamped with the current time.
    #[allow(clippy::too_many_arguments)] // a flat record constructor; a struct would not aid clarity
    pub fn new_record(
        id: TransferId,
        direction: Direction,
        name: String,
        ticket: String,
        hash: String,
        dest: Option<String>,
        source: Option<String>,
        total_bytes: u64,
    ) -> TransferRecord {
        let now = now_secs();
        TransferRecord {
            id,
            direction,
            name,
            ticket,
            hash,
            dest,
            source,
            total_bytes,
            transferred: 0,
            status: Status::Active,
            created_at: now,
            updated_at: now,
        }
    }

    fn save(&self) {
        if self.path.as_os_str().is_empty() {
            return;
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            // Atomic-ish write: tmp then rename.
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, &json).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}

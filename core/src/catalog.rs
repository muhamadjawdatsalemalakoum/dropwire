//! Local, on-disk catalog of transfers.
//!
//! Privacy-first: this lives only on the user's machine (`transfers.json` in the
//! data dir). It exists so the UI can show a transfer list and offer "resume"
//! after a crash/restart. Nothing here is ever sent anywhere.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

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
    /// Number of files in the transfer (0 for records created before this
    /// field existed). Surfaced in nearby-offer summaries.
    #[serde(default)]
    pub file_count: usize,
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

/// The persisted catalog: `{"entries": {"<id>": <record>, ...}}` on disk.
#[derive(Debug, Default)]
pub struct Catalog {
    entries: BTreeMap<String, TransferRecord>,
    /// Records this version cannot read (written by a newer version, or
    /// damaged), kept exactly as found so saving never drops them.
    unreadable: Map<String, Value>,
    /// Where the catalog is saved. Empty means "do not save" (tests, or a file
    /// that exists but could not be read, which must not be overwritten).
    path: PathBuf,
}

impl Catalog {
    /// Load the catalog from disk. A missing file starts an empty history. One
    /// record that cannot be read is set aside, not the whole history. A file
    /// that is not a catalog at all is copied to `transfers.json.bad` before
    /// starting empty, so nothing is lost for good.
    pub fn load(path: PathBuf) -> Self {
        let mut cat = Catalog::default();
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                cat.path = path;
                return cat;
            }
            Err(e) => {
                // It is there but unreadable right now. Saving over it would
                // erase every record it holds, so leave it alone this session.
                tracing::warn!(
                    "could not read {}: {e}; history will not be saved until restart",
                    path.display()
                );
                return cat;
            }
        };
        match catalog_entries(&bytes) {
            Some(raw) => {
                for (key, value) in raw {
                    match serde_json::from_value::<TransferRecord>(value.clone()) {
                        Ok(rec) => {
                            cat.entries.insert(key, rec);
                        }
                        Err(e) => {
                            tracing::warn!(
                                "keeping history record {key} as is, it could not be read: {e}"
                            );
                            cat.unreadable.insert(key, value);
                        }
                    }
                }
            }
            None => {
                let backup = backup_path(&path);
                match std::fs::copy(&path, &backup) {
                    Ok(_) => tracing::warn!(
                        "{} is damaged; kept a copy at {} and started a new history",
                        path.display(),
                        backup.display()
                    ),
                    Err(e) => {
                        tracing::warn!(
                            "{} is damaged and could not be copied aside ({e}); leaving it untouched",
                            path.display()
                        );
                        return cat;
                    }
                }
            }
        }
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
            self.save();
        }
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

    /// Forget every finished record. In-flight transfers are kept: clearing the
    /// list must never orphan something the UI is still driving. Returns the ids
    /// of the receives that were forgotten, so the data they kept for a resume
    /// can be let go too.
    pub fn clear_finished(&mut self) -> Vec<TransferId> {
        let removed = self
            .entries
            .values()
            .filter(|r| r.status != Status::Active && r.direction == Direction::Receive)
            .map(|r| r.id)
            .collect();
        self.entries.retain(|_, r| r.status == Status::Active);
        // Records this version could not read are not shown, so the user cannot
        // tell them apart; clearing history clears them too.
        self.unreadable.clear();
        self.save();
        removed
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
        file_count: usize,
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
            file_count,
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
        let mut entries = self.unreadable.clone();
        for (key, rec) in &self.entries {
            if let Ok(value) = serde_json::to_value(rec) {
                entries.insert(key.clone(), value);
            }
        }
        let mut doc = Map::new();
        doc.insert("entries".into(), Value::Object(entries));
        let Ok(json) = serde_json::to_vec_pretty(&Value::Object(doc)) else {
            return;
        };
        // Write a temporary file in full and flush it to disk before it
        // replaces the old one, so a crash or power cut mid-save leaves either
        // the old history or the new one, never half of a file.
        let tmp = self.path.with_extension("json.tmp");
        let written = std::fs::File::create(&tmp).and_then(|mut f| {
            f.write_all(&json)?;
            f.sync_all()
        });
        match written.and_then(|()| std::fs::rename(&tmp, &self.path)) {
            Ok(()) => {}
            Err(e) => {
                tracing::warn!("could not save {}: {e}", self.path.display());
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }
}

/// The records of a catalog file, keyed by id. `None` when the file is not a
/// catalog at all (not JSON, or `entries` is not a map).
fn catalog_entries(bytes: &[u8]) -> Option<Map<String, Value>> {
    let Value::Object(mut doc) = serde_json::from_slice::<Value>(bytes).ok()? else {
        return None;
    };
    match doc.remove("entries") {
        None => Some(Map::new()),
        Some(Value::Object(entries)) => Some(entries),
        Some(_) => None,
    }
}

/// Where a damaged catalog is copied before a new one is started. An earlier
/// copy is never replaced.
fn backup_path(path: &Path) -> PathBuf {
    let first = path.with_extension("json.bad");
    if !first.exists() {
        return first;
    }
    path.with_extension(format!("json.{}.bad", now_secs()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str) -> TransferRecord {
        Catalog::new_record(
            TransferId::new(),
            Direction::Receive,
            name.into(),
            "ticket".into(),
            "hash".into(),
            Some("/dest".into()),
            None,
            1,
            10,
        )
    }

    #[test]
    fn a_record_it_cannot_read_does_not_cost_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transfers.json");
        let good = record("good");
        let good_json = serde_json::to_value(&good).unwrap();
        let mut odd = serde_json::to_value(record("from a newer version")).unwrap();
        odd["status"] = Value::String("paused".into());
        let odd_id = odd["id"].as_str().unwrap().to_string();
        let file = serde_json::json!({
            "entries": { good.id.to_string(): good_json, odd_id.clone(): odd.clone() }
        });
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();

        let mut cat = Catalog::load(path.clone());
        let listed = cat.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "good");

        // Saving keeps the record it could not read, exactly as it was.
        cat.upsert(record("new"));
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["entries"][&odd_id], odd);
        assert_eq!(saved["entries"].as_object().unwrap().len(), 3);
        assert_eq!(Catalog::load(path).list().len(), 2);
    }

    #[test]
    fn a_damaged_file_is_copied_aside_before_starting_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transfers.json");
        std::fs::write(&path, b"{\"entries\": {\"abc\": ").unwrap();

        let mut cat = Catalog::load(path.clone());
        assert!(cat.list().is_empty());
        let backup = dir.path().join("transfers.json.bad");
        assert_eq!(
            std::fs::read(&backup).unwrap(),
            b"{\"entries\": {\"abc\": ".to_vec()
        );

        // A second damaged file does not replace the first copy.
        cat.upsert(record("after"));
        std::fs::write(&path, b"not json").unwrap();
        let _ = Catalog::load(path);
        assert_eq!(
            std::fs::read(&backup).unwrap(),
            b"{\"entries\": {\"abc\": ".to_vec()
        );
        let copies = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".bad"))
            .count();
        assert_eq!(copies, 2);
    }

    #[test]
    fn records_from_older_versions_still_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transfers.json");
        let id = TransferId::new();
        // No source, file_count or any later field.
        let file = serde_json::json!({ "entries": { id.to_string(): {
            "id": id, "direction": "receive", "name": "old", "ticket": "t",
            "hash": "h", "dest": "/d", "total_bytes": 5, "transferred": 0,
            "status": "interrupted", "created_at": 1, "updated_at": 2
        }}});
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();

        let cat = Catalog::load(path);
        let rec = cat.get(id).expect("the old record loads");
        assert_eq!(rec.status, Status::Interrupted);
        assert_eq!(rec.file_count, 0);
    }

    #[test]
    fn updating_an_unknown_transfer_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transfers.json");
        let mut cat = Catalog::load(path.clone());
        cat.set_status(TransferId::new(), Status::Error, None);
        assert!(!path.exists());
    }
}

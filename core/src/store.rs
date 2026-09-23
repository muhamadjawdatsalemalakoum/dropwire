//! Blob store setup, and what the store keeps.
//!
//! Received content lands in the store first and is then copied out to the
//! folder the receiver chose. The store must not keep that second copy once it
//! is no longer needed, so garbage collection is on, and a receive holds its
//! data with a named tag only while the data can still be used to resume:
//!
//! - while the receive runs, and after it fails or is interrupted (so Resume
//!   and a retry reuse what already arrived);
//! - not after every file is saved, not after the user cancels, and not once
//!   the record is cleared from history.
//!
//! Sends hold their imported content with temp tags for as long as they serve
//! it. Sent files are imported by reference, and the collector only ever
//! deletes files inside the store's own directory (it removes the database
//! rows of a referenced entry and leaves the file alone), so the originals are
//! never touched.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use iroh_blobs::api::Store;
use iroh_blobs::store::fs::options::Options;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::store::GcConfig;
use iroh_blobs::{Hash, HashAndFormat};
use n0_future::StreamExt;

use crate::catalog::{Status, TransferRecord};
use crate::error::Result;
use crate::progress::{Direction, TransferId};

/// The ALPN advertised/dialed for the blobs protocol.
///
/// Centralized so a future rename in iroh-blobs is a one-line change. (`sendme`
/// uses `iroh_blobs::protocol::ALPN`; the README uses `iroh_blobs::ALPN` — they
/// are the same re-export.)
pub const BLOBS_ALPN: &[u8] = iroh_blobs::ALPN;

/// How often unreferenced content is collected. Short enough that the copy of
/// a finished receive is gone soon after its files are saved.
const GC_INTERVAL: Duration = Duration::from_secs(60);

#[cfg(feature = "test-utils")]
static GC_INTERVAL_OVERRIDE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Test-only: collect garbage this often in every engine started afterwards in
/// this process, so tests can watch the store shrink.
#[cfg(feature = "test-utils")]
#[doc(hidden)]
pub fn set_gc_interval_for_tests(interval: Duration) {
    GC_INTERVAL_OVERRIDE_MS.store(
        interval.as_millis() as u64,
        std::sync::atomic::Ordering::Relaxed,
    );
}

fn gc_interval() -> Duration {
    #[cfg(feature = "test-utils")]
    {
        let ms = GC_INTERVAL_OVERRIDE_MS.load(std::sync::atomic::Ordering::Relaxed);
        if ms > 0 {
            return Duration::from_millis(ms);
        }
    }
    GC_INTERVAL
}

/// Open (or create) the persistent on-disk BLAKE3 store.
///
/// Persistence is what makes resume work across app restarts — interrupted
/// transfers keep their partial data here (unlike `MemStore`).
pub async fn open(blobs_dir: &Path) -> Result<FsStore> {
    std::fs::create_dir_all(blobs_dir)?;
    // Same database path `FsStore::load` uses, so existing stores open as before.
    let mut options = Options::new(blobs_dir);
    options.gc = Some(GcConfig {
        interval: gc_interval(),
        add_protected: None,
    });
    let store = FsStore::load_with_opts(blobs_dir.join("blobs.db"), options)
        .await
        .context("open blob store")?;
    Ok(store)
}

/// Tags that hold a receive's data are named after the transfer.
const RECEIVE_TAG_PREFIX: &str = "recv/";

fn receive_tag(id: TransferId) -> String {
    format!("{RECEIVE_TAG_PREFIX}{id}")
}

/// Keep a receive's content (the collection and every file in it, whole or
/// partial) out of garbage collection until [`release_receive`].
pub(crate) async fn hold_receive(store: &Store, id: TransferId, hash: Hash) -> anyhow::Result<()> {
    store
        .tags()
        .set(receive_tag(id), HashAndFormat::hash_seq(hash))
        .await
        .context("hold received data")?;
    Ok(())
}

/// Let the next collection reclaim a receive's content (unless another tag
/// still holds it). Best effort: a failure only means the data stays a while.
pub(crate) async fn release_receive(store: &Store, id: TransferId) {
    if let Err(e) = store.tags().delete(receive_tag(id)).await {
        tracing::warn!("could not release data of receive {id}: {e}");
    }
}

/// Whether a record's partial data is still worth keeping: a receive that can
/// be resumed or retried, and that no later receive of the same content has
/// finished since.
fn keeps_data(rec: &TransferRecord, all: &[TransferRecord]) -> bool {
    rec.direction == Direction::Receive
        && matches!(
            rec.status,
            Status::Active | Status::Interrupted | Status::Error
        )
        && !all.iter().any(|other| {
            other.direction == Direction::Receive
                && other.status == Status::Done
                && other.hash == rec.hash
                && other.updated_at >= rec.created_at
        })
}

/// A receive of `hash` just finished and saved everything: the data other,
/// no-longer-running receives of the same content were holding for a resume
/// is not needed any more. Running receives keep theirs.
pub(crate) async fn release_superseded(store: &Store, records: &[TransferRecord], hash: &str) {
    for rec in records {
        if rec.direction == Direction::Receive && rec.hash == hash && rec.status != Status::Active {
            release_receive(store, rec.id).await;
        }
    }
}

/// On startup, make the receive tags match the history: keep (or, for records
/// written by versions that kept no tags, create) one for every receive that
/// can still be resumed, and drop the rest. Runs before any transfer starts and
/// long before the first collection.
pub(crate) async fn reconcile_receive_tags(store: &Store, records: &[TransferRecord]) {
    let keep: Vec<&TransferRecord> = records.iter().filter(|r| keeps_data(r, records)).collect();
    let keep_names: HashSet<String> = keep.iter().map(|r| receive_tag(r.id)).collect();

    let mut stale = Vec::new();
    match store.tags().list_prefix(RECEIVE_TAG_PREFIX).await {
        Ok(mut tags) => {
            while let Some(tag) = tags.next().await {
                let Ok(tag) = tag else { continue };
                let name = String::from_utf8_lossy(tag.name.as_ref()).to_string();
                if !keep_names.contains(&name) {
                    stale.push(name);
                }
            }
        }
        Err(e) => tracing::warn!("could not list receive tags: {e}"),
    }
    for name in stale {
        if let Err(e) = store.tags().delete(&name).await {
            tracing::warn!("could not drop stale tag {name}: {e}");
        }
    }

    for rec in keep {
        let Ok(hash) = rec.hash.parse::<Hash>() else {
            continue;
        };
        if let Err(e) = hold_receive(store, rec.id, hash).await {
            tracing::warn!("could not keep data of receive {}: {e:#}", rec.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;

    fn record(status: Status, hash: Hash, created: u64, updated: u64) -> TransferRecord {
        let mut rec = Catalog::new_record(
            TransferId::new(),
            Direction::Receive,
            "x".into(),
            "ticket".into(),
            hash.to_string(),
            Some("/tmp".into()),
            None,
            1,
            10,
        );
        rec.status = status;
        rec.created_at = created;
        rec.updated_at = updated;
        rec
    }

    async fn tag_names(store: &Store) -> Vec<String> {
        let mut out = Vec::new();
        let mut tags = store.tags().list().await.unwrap();
        while let Some(tag) = tags.next().await {
            out.push(String::from_utf8_lossy(tag.unwrap().name.as_ref()).to_string());
        }
        out.sort();
        out
    }

    #[tokio::test]
    async fn startup_keeps_only_resumable_receives() {
        let store = iroh_blobs::store::mem::MemStore::new();
        let (h1, h2, h3, h4) = (
            Hash::new(b"one"),
            Hash::new(b"two"),
            Hash::new(b"three"),
            Hash::new(b"four"),
        );
        let interrupted = record(Status::Interrupted, h1, 10, 20);
        let done = record(Status::Done, h2, 10, 20);
        let failed = record(Status::Error, h3, 10, 20);
        let cancelled = record(Status::Cancelled, h4, 10, 20);
        // A failed receive of content that a later receive then finished.
        let superseded = record(Status::Error, h2, 5, 6);
        let mut send = record(Status::Interrupted, h4, 10, 20);
        send.direction = Direction::Send;
        let records = vec![
            interrupted.clone(),
            done.clone(),
            failed.clone(),
            cancelled.clone(),
            superseded.clone(),
            send.clone(),
        ];

        // Leftovers: tags for records that should not keep data, one for a
        // record that no longer exists, and an unrelated tag.
        for rec in [&done, &cancelled, &superseded] {
            hold_receive(&store, rec.id, h2).await.unwrap();
        }
        hold_receive(&store, TransferId::new(), h1).await.unwrap();
        store
            .tags()
            .set("other", HashAndFormat::raw(h1))
            .await
            .unwrap();

        reconcile_receive_tags(&store, &records).await;

        let mut want = vec![
            receive_tag(interrupted.id),
            receive_tag(failed.id),
            "other".to_string(),
        ];
        want.sort();
        assert_eq!(tag_names(&store).await, want);
    }

    #[tokio::test]
    async fn a_finished_receive_releases_older_attempts_but_not_running_ones() {
        let store = iroh_blobs::store::mem::MemStore::new();
        let h = Hash::new(b"content");
        let old = record(Status::Interrupted, h, 1, 2);
        let running = record(Status::Active, h, 3, 3);
        let other = record(Status::Error, Hash::new(b"else"), 1, 2);
        for rec in [&old, &running, &other] {
            hold_receive(&store, rec.id, h).await.unwrap();
        }
        let records = vec![old.clone(), running.clone(), other.clone()];

        release_superseded(&store, &records, &h.to_string()).await;

        let mut want = vec![receive_tag(running.id), receive_tag(other.id)];
        want.sort();
        assert_eq!(tag_names(&store).await, want);
    }
}

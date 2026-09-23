//! Blob store setup.

use std::path::Path;

use anyhow::Context;
use iroh_blobs::store::fs::FsStore;

use crate::error::Result;

/// The ALPN advertised/dialed for the blobs protocol.
///
/// Centralized so a future rename in iroh-blobs is a one-line change. (`sendme`
/// uses `iroh_blobs::protocol::ALPN`; the README uses `iroh_blobs::ALPN` — they
/// are the same re-export.)
pub const BLOBS_ALPN: &[u8] = iroh_blobs::ALPN;

/// Open (or create) the persistent on-disk BLAKE3 store.
///
/// Persistence is what makes resume work across app restarts — interrupted
/// transfers keep their partial data here (unlike `MemStore`).
pub async fn open(blobs_dir: &Path) -> Result<FsStore> {
    std::fs::create_dir_all(blobs_dir)?;
    // FsStore::load keeps its database at `<root>/blobs.db`.
    ensure_db_usable(&blobs_dir.join("blobs.db"))?;
    let store = FsStore::load(blobs_dir).await.context("open blob store")?;
    Ok(store)
}

/// Fail fast when the store's database cannot be opened, above all when another
/// process (another copy of Dropwire) already holds it.
///
/// `FsStore::load` does not report that as an error: when opening the database
/// fails, iroh-blobs drops its private runtime from inside one of that
/// runtime's own worker threads, which never completes, so the load hangs for
/// good. Checking the file and its lock first turns the common causes into an
/// ordinary error the caller can show.
// File locking is std since Rust 1.89; iroh 1.0 already requires 1.91.
#[allow(clippy::incompatible_msrv)]
fn ensure_db_usable(db: &Path) -> Result<()> {
    let file = match std::fs::OpenOptions::new().read(true).write(true).open(db) {
        Ok(file) => file,
        // First run: the store creates it.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(anyhow::Error::new(e)
                .context("open blob store database")
                .into())
        }
    };
    match file.try_lock() {
        // Dropping the file releases the lock again straight away.
        Ok(()) => Ok(()),
        Err(std::fs::TryLockError::WouldBlock) => Err(anyhow::anyhow!(
            "blob store is already open in another process (is Dropwire already running?)"
        )
        .into()),
        // Locking unsupported here: leave the decision to the store itself.
        Err(std::fs::TryLockError::Error(_)) => Ok(()),
    }
}

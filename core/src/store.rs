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
    let store = FsStore::load(blobs_dir).await.context("open blob store")?;
    Ok(store)
}

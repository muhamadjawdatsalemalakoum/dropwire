//! Receiving: parse a ticket, resume/download, export to disk.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use iroh_blobs::api::remote::GetProgressItem;
use iroh_blobs::format::collection::Collection;
use iroh_blobs::get::request::get_hash_seq_and_sizes;
use iroh_blobs::protocol::{ChunkRanges, GetRequest};
use iroh_blobs::ticket::BlobTicket;
use iroh_blobs::HashAndFormat;
use n0_future::StreamExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::catalog::{Catalog, Status};
use crate::error::{CoreError, Result};
use crate::export;
use crate::progress::{
    Direction, FilePreview, Progress, ProgressStream, Route, TransferId, TransferPreview,
    TransferStats,
};
use crate::store::{self, BLOBS_ALPN};
use crate::Core;

/// How long to wait to connect to the sender before declaring it unreachable
/// (offline or expired link), instead of hanging indefinitely. Local-only mode
/// has no relay/DHT, so a reachable peer connects near-instantly — fail fast there.
fn connect_timeout(infra: &crate::Infra) -> Duration {
    match infra {
        crate::Infra::LocalOnly => Duration::from_secs(3),
        _ => Duration::from_secs(15),
    }
}

/// Most files one transfer may list. Far above any real folder send, and low
/// enough that a crafted code cannot make preview or receive walk (and probe)
/// millions of entries.
const MAX_FILES: usize = 100_000;

/// Largest names blob we agree to fetch and parse. Real ones hold a short path
/// per file, so even a full 100 000-file transfer stays well under this.
const MAX_META_BYTES: u64 = 16 * 1024 * 1024;

/// Byte cap for the collection's hash list: one 32-byte hash per file plus the
/// names blob. Anything larger is refused before a single size is probed.
const MAX_HASH_SEQ_BYTES: u64 = (MAX_FILES as u64 + 1) * 32;

/// What the receiver is told when a transfer is over the limits above.
const TOO_LARGE: &str = "this transfer is too large to open: it lists more than 100000 files, \
     or its list of names is bigger than 16 MB";

/// Fetch the collection's hash list and every child's verified size, refusing
/// transfers over the file-count limit before any size is probed.
async fn fetch_sizes(
    conn: &iroh::endpoint::Connection,
    hash: &iroh_blobs::Hash,
) -> Result<Vec<u64>> {
    match get_hash_seq_and_sizes(conn, hash, MAX_HASH_SEQ_BYTES, None).await {
        Ok((_hash_seq, sizes)) => {
            check_manifest(&sizes, MAX_FILES, MAX_META_BYTES)?;
            Ok(sizes.to_vec())
        }
        // Raised for a hash list over `MAX_HASH_SEQ_BYTES` (or one that is not a
        // whole number of hashes): either way, not something we will open.
        Err(iroh_blobs::get::GetError::BadRequest { .. }) => {
            Err(CoreError::Other(anyhow!(TOO_LARGE)))
        }
        Err(e) => Err(CoreError::Other(
            anyhow::Error::new(e).context("fetch sizes"),
        )),
    }
}

/// Bounds on the sender-controlled manifest: `sizes[0]` is the names blob and
/// `sizes[1..]` are the files. Runs before the names blob is fetched.
fn check_manifest(sizes: &[u64], max_files: usize, max_meta: u64) -> Result<()> {
    let Some(&meta) = sizes.first() else {
        return Err(CoreError::Other(anyhow!(
            "this transfer's file list is empty or damaged"
        )));
    };
    if meta > max_meta || sizes.len() - 1 > max_files {
        return Err(CoreError::Other(anyhow!(TOO_LARGE)));
    }
    Ok(())
}

impl Core {
    /// Download the content referenced by `ticket` into `dest`, resuming from any
    /// partial data already in the local store.
    pub async fn receive(
        &self,
        ticket: String,
        dest: PathBuf,
    ) -> Result<(TransferId, ProgressStream)> {
        self.spawn_receive(ticket, dest, None).await
    }

    /// Like [`Core::receive`], but downloads only the files at the given indices
    /// (0-based into the transfer's file list, as reported by [`Core::inspect`]).
    /// Unselected files are neither fetched over the network nor written to disk.
    pub async fn receive_selected(
        &self,
        ticket: String,
        dest: PathBuf,
        selected: Vec<usize>,
    ) -> Result<(TransferId, ProgressStream)> {
        self.spawn_receive(ticket, dest, Some(selected)).await
    }

    async fn spawn_receive(
        &self,
        ticket: String,
        dest: PathBuf,
        selected: Option<Vec<usize>>,
    ) -> Result<(TransferId, ProgressStream)> {
        // Parse up front so the caller gets a clean error synchronously.
        let parsed: BlobTicket = ticket
            .parse()
            .map_err(|_| CoreError::InvalidTicket(ticket.clone()))?;

        let id = TransferId::new();
        let (tx, rx) = mpsc::channel(64);
        let token = CancellationToken::new();
        self.inner.active.lock().await.insert(id, token.clone());

        let core = self.clone();
        let tx_err = tx.clone();
        tokio::spawn(async move {
            if let Err(e) =
                run_receive(core.clone(), id, parsed, ticket, dest, selected, tx, token).await
            {
                let _ = tx_err
                    .send(Progress::Error {
                        id,
                        message: e.to_string(),
                    })
                    .await;
                core.inner
                    .catalog
                    .lock()
                    .await
                    .set_status(id, Status::Error, None);
            }
            core.inner.active.lock().await.remove(&id);
        });

        Ok((id, ReceiverStream::new(rx)))
    }

    /// Connect to the sender and fetch ONLY the transfer's metadata — the file
    /// names, per-file sizes, count, total, and connection route — *without*
    /// downloading any file content. This is the "preview before you accept"
    /// step: the receiver sees exactly what's being sent and decides before a
    /// single payload byte lands.
    ///
    /// Cheap and content-free: it fetches the collection's HashSeq (with each
    /// child's verified size) and the small metadata blob holding the names. The
    /// names and sizes are committed by the ticket's BLAKE3 hash, so they cannot
    /// be faked. Requires the sender to be online (it is a live connection).
    pub async fn inspect(&self, ticket: String) -> Result<TransferPreview> {
        let parsed: BlobTicket = ticket
            .parse()
            .map_err(|_| CoreError::InvalidTicket(ticket.clone()))?;
        let endpoint = self.inner.router.endpoint();
        let hash = parsed.hash();

        let conn = match tokio::time::timeout(
            connect_timeout(&self.inner.config.infra),
            endpoint.connect(parsed.addr().clone(), BLOBS_ALPN),
        )
        .await
        {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => return Err(CoreError::Unreachable(e.to_string())),
            Err(_) => {
                return Err(CoreError::Unreachable(
                    "timed out — the sender may be offline or the link may have expired".into(),
                ))
            }
        };

        let store = &self.inner.store;

        // Per-file verified sizes (last chunk only) — no bodies. The collection's
        // HashSeq is [metadata, file0, file1, …], so sizes[0] is the metadata blob
        // and sizes[1..] are the file sizes. Oversized manifests stop here, before
        // the names blob is fetched.
        let sizes = fetch_sizes(&conn, &hash).await?;
        let route = detect_route(&conn);

        // Keep what this fetches until the names are read. Nothing holds it after
        // that, so an abandoned preview leaves nothing behind once the store's
        // collector runs.
        let _hold = store
            .tags()
            .temp_tag(HashAndFormat::hash_seq(hash))
            .await
            .context("hold preview data")?;

        // Fetch ONLY the collection structure into the store — the HashSeq root and
        // the names/metadata blob (offset 0 + child 0), never any file content — then
        // read the names locally. (A standalone get of the metadata blob by hash is
        // not served, since it is reachable only through the HashSeq.)
        let request = GetRequest::builder()
            .root(ChunkRanges::all())
            .child(0, ChunkRanges::all())
            .build(hash);
        let mut stream = store.remote().execute_get(conn, request).stream();
        while let Some(item) = stream.next().await {
            match item {
                GetProgressItem::Done(_) => break,
                GetProgressItem::Error(e) => {
                    return Err(CoreError::Other(anyhow!("fetch metadata: {e}")))
                }
                GetProgressItem::Progress(_) => {}
            }
        }

        let collection = Collection::load(hash, store.as_ref())
            .await
            .context("load collection metadata")?;
        let files: Vec<FilePreview> = collection
            .iter()
            .enumerate()
            .map(|(i, (name, _hash))| FilePreview {
                name: name.clone(),
                size: sizes.get(i + 1).copied().unwrap_or(0),
            })
            .collect();
        let total_bytes = files.iter().map(|f| f.size).sum();

        Ok(TransferPreview {
            file_count: files.len(),
            total_bytes,
            files,
            route,
        })
    }
}

#[allow(clippy::too_many_arguments)] // internal plumbing; bundling would not aid clarity
async fn run_receive(
    core: Core,
    id: TransferId,
    ticket: BlobTicket,
    ticket_str: String,
    dest: PathBuf,
    selected: Option<Vec<usize>>,
    tx: mpsc::Sender<Progress>,
    token: CancellationToken,
) -> anyhow::Result<()> {
    // The summary's duration covers the whole receive: connecting, downloading
    // and saving.
    let started = Instant::now();
    let store = &core.inner.store;
    let endpoint = core.inner.router.endpoint();
    let hf = ticket.hash_and_format();
    let hash = ticket.hash();

    // Connect to the provider (bounded, so an offline sender fails cleanly).
    let conn = match tokio::time::timeout(
        connect_timeout(&core.inner.config.infra),
        endpoint.connect(ticket.addr().clone(), BLOBS_ALPN),
    )
    .await
    {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            return Err(anyhow!(
                "can't reach the sender (offline or link expired): {e}"
            ))
        }
        Err(_) => {
            return Err(anyhow!(
                "can't reach the sender — they may be offline or the link expired"
            ))
        }
    };
    // Track the connection path; relay→direct can upgrade after hole-punch, so we
    // watch it live and the badge reflects the *current* path during the transfer.
    let route_state = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(route_u8(
        detect_route(&conn),
    )));
    let _route_watch = {
        let rc = conn.clone();
        let rs = route_state.clone();
        n0_future::task::AbortOnDropHandle::new(tokio::spawn(async move {
            let mut paths = rc.paths_stream();
            while let Some(snapshot) = paths.next().await {
                rs.store(
                    route_u8(route_from_paths(&snapshot)),
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
        }))
    };

    // Total size for the progress bar (provider advertises sizes up front).
    // Oversized manifests are refused here, before anything is fetched.
    let sizes = fetch_sizes(&conn, &hash).await?;
    // The chosen files as a lookup table (one flag per file), so the export
    // loop checks membership in O(1). Out-of-range and repeated indices drop out.
    let wanted: Option<Vec<bool>> = selected.map(|idx| {
        let mut flags = vec![false; sizes.len() - 1];
        for i in idx {
            if let Some(f) = flags.get_mut(i) {
                *f = true;
            }
        }
        flags
    });
    let is_wanted = |i: usize| {
        wanted
            .as_ref()
            .is_none_or(|w| w.get(i).copied().unwrap_or(false))
    };
    // Total bytes to fetch: the whole transfer, or just the selected files.
    // `sizes[0]` is the list of names, not a file, so it is left out, the same
    // as in the preview.
    let total: u64 = match &wanted {
        None => sizes.iter().skip(1).sum(),
        Some(_) => (0..sizes.len() - 1)
            .filter(|&i| is_wanted(i))
            .map(|i| sizes[i + 1])
            .sum(),
    };

    // Record (active).
    {
        let name = dest
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "download".to_string());
        let mut cat = core.inner.catalog.lock().await;
        cat.upsert(Catalog::new_record(
            id,
            Direction::Receive,
            name,
            ticket_str,
            hash.to_string(),
            Some(dest.to_string_lossy().to_string()),
            None,
            0, // file count is learned from the preview; not needed for resume
            total,
        ));
    }

    // Hold everything this receive fetches, and whatever an earlier attempt left,
    // until it is saved (or the receive is cancelled or cleared from history).
    store::hold_receive(store, id, hash).await?;

    // What this receive needs: everything, or the collection's structure (its
    // hash list and names) plus the chosen files.
    let needed = match &wanted {
        None => GetRequest::from(hf),
        Some(_) => {
            let mut b = GetRequest::builder()
                .root(ChunkRanges::all())
                .child(0, ChunkRanges::all());
            for i in (0..sizes.len() - 1).filter(|&i| is_wanted(i)) {
                b = b.child((i as u64) + 1, ChunkRanges::all());
            }
            b.build(hash)
        }
    };
    // Resume: ask only for the parts not stored yet, for a selection as much as
    // for a whole transfer. When everything needed is here, nothing is fetched.
    let local = store
        .remote()
        .local_for_request(needed)
        .await
        .context("inspect local store")?;
    if !local.is_complete() {
        let get = store.remote().execute_get(conn, local.missing());
        let mut stream = get.stream();
        // Throttle UI progress to ~12/s: blob progress can fire per-chunk.
        let mut last_emit = Instant::now() - Duration::from_millis(200);
        loop {
            tokio::select! {
                _ = token.cancelled() => return finish_cancelled(&core, id, &tx).await,
                item = stream.next() => match item {
                    Some(GetProgressItem::Progress(offset)) => {
                        if last_emit.elapsed() >= Duration::from_millis(80) {
                            last_emit = Instant::now();
                            let route = u8_route(route_state.load(std::sync::atomic::Ordering::Relaxed));
                            let _ = tx.send(Progress::Transferring { id, offset, total, route }).await;
                        }
                    }
                    Some(GetProgressItem::Done(_stats)) => break,
                    Some(GetProgressItem::Error(e)) => return Err(anyhow!("download failed: {e}")),
                    None => break,
                }
            }
        }
    }

    // Export the collection tree to `dest`.
    let collection = Collection::load(hash, store.as_ref())
        .await
        .context("load collection")?;
    // Where each file goes. Nothing already on disk is replaced: taken names
    // get a " (n)" suffix, decided once per top-level name so a folder lands
    // together, and content already saved by an earlier receive of this same
    // transfer is left as it is. This touches the disk (and may hash existing
    // files), so it runs off the async threads.
    let dest = std::path::absolute(&dest).unwrap_or(dest);
    std::fs::create_dir_all(&dest)?;
    let wanted_files: Vec<export::Wanted> = collection
        .iter()
        .enumerate()
        .filter(|(i, _)| is_wanted(*i))
        .map(|(i, (name, child_hash))| export::Wanted {
            index: i,
            name: name.clone(),
            hash: *child_hash,
            size: sizes.get(i + 1).copied().unwrap_or(0),
        })
        .collect();
    let attempted = wanted_files.len();
    let plan = {
        let dest = dest.clone();
        tokio::task::spawn_blocking(move || export::plan(&dest, wanted_files))
            .await
            .context("plan file names")?
    };

    // One file that cannot be written (a permission problem, a full disk, a path
    // too long for this system) must not cost the receiver every file after it:
    // save what can be saved, then report exactly what could not.
    let mut failed = plan.failed;
    let mut renamed = plan.renamed;
    let transfer = id.to_string();
    for file in plan.files.iter().filter(|f| !f.present) {
        if token.is_cancelled() {
            return finish_cancelled(&core, id, &tx).await;
        }
        match export::save(store, &dest, file, &transfer, &token).await {
            Ok(export::Saved::Written { renamed: late }) => {
                if let Some(late) = late {
                    if renamed.len() < export::MAX_RENAMED_REPORTED {
                        renamed.push(late);
                    }
                }
            }
            Ok(export::Saved::Cancelled) => return finish_cancelled(&core, id, &tx).await,
            Err(why) => failed.push((file.name.clone(), why)),
        }
    }
    if !failed.is_empty() {
        return Err(anyhow!(export::describe_failures(&failed, attempted)));
    }

    let stats = TransferStats {
        bytes: total,
        seconds: started.elapsed().as_secs_f64(),
        renamed,
    };
    let records = {
        let mut cat = core.inner.catalog.lock().await;
        cat.set_status(id, Status::Done, Some(total));
        cat.list()
    };
    // Every file is saved, so the store's copy is no longer needed: neither this
    // receive's nor one an earlier, unfinished attempt kept for a resume.
    store::release_superseded(store, &records, &hash.to_string()).await;
    let _ = tx.send(Progress::Done { id, stats }).await;
    Ok(())
}

/// End a receive the user cancelled. What it downloaded is let go: a cancel
/// should free the space, and a new receive of the same code starts over.
async fn finish_cancelled(
    core: &Core,
    id: TransferId,
    tx: &mpsc::Sender<Progress>,
) -> anyhow::Result<()> {
    core.inner
        .catalog
        .lock()
        .await
        .set_status(id, Status::Cancelled, None);
    store::release_receive(&core.inner.store, id).await;
    let _ = tx.send(Progress::Cancelled { id }).await;
    Ok(())
}

/// Map a connection's selected path to a route for the UI badge.
fn route_from_paths(paths: &iroh::endpoint::PathList<'_>) -> Route {
    if let Some(p) = paths.iter().find(|p| p.is_selected()) {
        return if p.is_relay() {
            Route::Relayed
        } else if p.is_ip() {
            Route::Direct
        } else {
            Route::Unknown
        };
    }
    if paths.iter().any(|p| p.is_ip()) {
        Route::Direct
    } else if paths.iter().any(|p| p.is_relay()) {
        Route::Relayed
    } else {
        Route::Unknown
    }
}

fn detect_route(conn: &iroh::endpoint::Connection) -> Route {
    route_from_paths(&conn.paths())
}

fn route_u8(r: Route) -> u8 {
    match r {
        Route::Direct => 1,
        Route::Relayed => 2,
        Route::Unknown => 0,
    }
}

fn u8_route(v: u8) -> Route {
    match v {
        1 => Route::Direct,
        2 => Route::Relayed,
        _ => Route::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_within_limits_is_accepted() {
        assert!(check_manifest(&[100, 1, 2, 3], 3, 100).is_ok());
    }

    #[test]
    fn manifest_with_too_many_files_is_refused() {
        let err = check_manifest(&[10, 1, 2, 3, 4], 3, 100).unwrap_err();
        assert!(err.to_string().contains("too large"), "{err}");
    }

    #[test]
    fn manifest_with_oversized_names_is_refused() {
        let err = check_manifest(&[101, 1], 3, 100).unwrap_err();
        assert!(err.to_string().contains("too large"), "{err}");
    }

    #[test]
    fn empty_manifest_is_refused() {
        assert!(check_manifest(&[], 3, 100).is_err());
    }
}

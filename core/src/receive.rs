//! Receiving: parse a ticket, resume/download, export to disk.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use iroh_blobs::api::blobs::{ExportMode, ExportOptions};
use iroh_blobs::api::remote::GetProgressItem;
use iroh_blobs::format::collection::Collection;
use iroh_blobs::get::request::get_hash_seq_and_sizes;
use iroh_blobs::protocol::{ChunkRanges, GetRequest};
use iroh_blobs::ticket::BlobTicket;
use n0_future::StreamExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::catalog::{Catalog, Status};
use crate::error::{CoreError, Result};
use crate::progress::{
    Direction, FilePreview, Progress, ProgressStream, Route, TransferId, TransferPreview,
    TransferStats,
};
use crate::store::BLOBS_ALPN;
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
        // and sizes[1..] are the file sizes.
        let (_hash_seq, sizes) = get_hash_seq_and_sizes(&conn, &hash, 1024 * 1024 * 32, None)
            .await
            .context("fetch sizes")?;
        let route = detect_route(&conn);

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
    let (_hash_seq, sizes) = get_hash_seq_and_sizes(&conn, &hash, 1024 * 1024 * 32, None)
        .await
        .context("fetch sizes")?;
    // Total bytes to fetch: the whole transfer, or just the selected files.
    let total: u64 = match &selected {
        None => sizes.iter().sum(),
        Some(idx) => idx.iter().filter_map(|&i| sizes.get(i + 1)).copied().sum(),
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
            total,
        ));
    }

    // Resume: for a full download, request only what's missing; for a selective
    // download, request the chosen files (plus the collection structure: root + meta).
    let local = store
        .remote()
        .local(hf)
        .await
        .context("inspect local store")?;
    let request = match &selected {
        None => local.missing(),
        Some(idx) => {
            let mut b = GetRequest::builder()
                .root(ChunkRanges::all())
                .child(0, ChunkRanges::all());
            for &i in idx {
                b = b.child((i as u64) + 1, ChunkRanges::all());
            }
            b.build(hash)
        }
    };
    if selected.is_some() || !local.is_complete() {
        let get = store.remote().execute_get(conn, request);
        let mut stream = get.stream();
        // Throttle UI progress to ~12/s: blob progress can fire per-chunk.
        let mut last_emit = Instant::now() - Duration::from_millis(200);
        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    core.inner.catalog.lock().await.set_status(id, Status::Cancelled, None);
                    let _ = tx.send(Progress::Cancelled { id }).await;
                    return Ok(());
                }
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
    let started = Instant::now();
    let collection = Collection::load(hash, store.as_ref())
        .await
        .context("load collection")?;
    std::fs::create_dir_all(&dest)?;
    for (i, (name, child_hash)) in collection.iter().enumerate() {
        if let Some(idx) = &selected {
            if !idx.contains(&i) {
                continue;
            }
        }
        let target = dest.join(sanitize_rel(name));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // VERIFY (ARCHITECTURE.md §13): ExportProgress completion — `.await` vs
        // draining `.stream()` of ExportProgressItem on 0.103.
        store
            .export_with_opts(ExportOptions {
                hash: *child_hash,
                target,
                mode: ExportMode::Copy,
            })
            .await
            .with_context(|| format!("export {name}"))?;
    }

    let stats = TransferStats {
        bytes: total,
        seconds: started.elapsed().as_secs_f64(),
    };
    core.inner
        .catalog
        .lock()
        .await
        .set_status(id, Status::Done, Some(total));
    let _ = tx.send(Progress::Done { id, stats }).await;
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

/// Prevent path traversal when writing received files: drop any `..`, absolute
/// roots, or drive prefixes; keep only normal path components.
fn sanitize_rel(name: &str) -> PathBuf {
    use std::path::Component;
    let raw = PathBuf::from(name.replace('\\', "/"));
    let mut out = PathBuf::new();
    for comp in raw.components() {
        if let Component::Normal(c) = comp {
            out.push(c);
        }
    }
    if out.as_os_str().is_empty() {
        out.push("file");
    }
    out
}

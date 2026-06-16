//! Sending: import a path, bundle it, serve it, hand back a ticket.

use std::path::{Path, PathBuf};

use anyhow::Context;
use iroh_blobs::api::blobs::{AddPathOptions, ImportMode};
use iroh_blobs::api::TempTag;
use iroh_blobs::format::collection::Collection;
use iroh_blobs::provider::events::{AbortReason, ProviderMessage, RequestUpdate};
use iroh_blobs::ticket::BlobTicket;
use iroh_blobs::{BlobFormat, Hash};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::catalog::{Catalog, Status};
use crate::error::Result;
use crate::progress::{Direction, Progress, ProgressStream, Route, TransferId, TransferStats};
use crate::Core;

impl Core {
    /// Import `path` (a file or folder), start serving it, and stream progress.
    /// The key event is `Progress::Ready { ticket }` — the string to share.
    pub async fn send(&self, path: PathBuf) -> Result<(TransferId, ProgressStream)> {
        let id = TransferId::new();
        let (tx, rx) = mpsc::channel(64);
        let token = CancellationToken::new();
        self.inner.active.lock().await.insert(id, token.clone());

        let core = self.clone();
        let tx_err = tx.clone();
        tokio::spawn(async move {
            if let Err(e) = run_send(core.clone(), id, path, tx, token).await {
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
}

async fn run_send(
    core: Core,
    id: TransferId,
    path: PathBuf,
    tx: mpsc::Sender<Progress>,
    token: CancellationToken,
) -> anyhow::Result<()> {
    let store = &core.inner.store;

    let display_name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "transfer".to_string());

    // 1. Enumerate files (single file -> one entry; directory -> recursive).
    let files = collect_files(&path)?;
    let total: u64 = files.iter().map(|(_, p)| file_len(p)).sum();

    // 2. Import each file, holding the TempTags so nothing is GC'd while serving.
    let mut tags: Vec<TempTag> = Vec::with_capacity(files.len());
    let mut entries: Vec<(String, Hash)> = Vec::with_capacity(files.len());
    let mut imported = 0u64;
    for (name, p) in files {
        if token.is_cancelled() {
            let _ = tx.send(Progress::Cancelled { id }).await;
            return Ok(());
        }
        // VERIFY (ARCHITECTURE.md §13): AddProgress::temp_tag() on iroh-blobs 0.103.
        let tt = store
            .add_path_with_opts(AddPathOptions {
                path: p.clone(),
                mode: ImportMode::TryReference,
                format: BlobFormat::Raw,
            })
            .temp_tag()
            .await
            .with_context(|| format!("import {}", p.display()))?;
        entries.push((name, tt.hash()));
        tags.push(tt);
        imported += file_len(&p);
        let _ = tx
            .send(Progress::Importing {
                id,
                done: imported,
                total,
            })
            .await;
    }

    // 3. Bundle into a Collection (a HashSeq) — uniform for single file or folder.
    let collection: Collection = entries.into_iter().collect();
    let collection_tag = collection.store(store).await.context("store collection")?;
    let hash = collection_tag.hash();

    // 4. Mint the ticket from our endpoint address. For relay-backed modes, wait
    //    (time-boxed) for a relay handshake so the address is reachable; skip in
    //    local-only mode where there is no relay (online() would never resolve).
    let endpoint = core.inner.router.endpoint();
    if !matches!(core.inner.config.infra, crate::Infra::LocalOnly) {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(10), endpoint.online()).await;
    }
    let addr = endpoint.addr();
    let ticket = BlobTicket::new(addr, hash, BlobFormat::HashSeq);
    let ticket_str = ticket.to_string();

    // 5. Register for provider events on this hash, record, and announce.
    let hash_key = hash.to_string();
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<ProviderEvent>();
    core.inner
        .serving
        .lock()
        .await
        .insert(hash_key.clone(), ev_tx);
    {
        let mut cat = core.inner.catalog.lock().await;
        cat.upsert(Catalog::new_record(
            id,
            Direction::Send,
            display_name,
            ticket_str.clone(),
            hash_key.clone(),
            None,
            Some(path.to_string_lossy().to_string()),
            total,
        ));
    }
    let _ = tx
        .send(Progress::Ready {
            id,
            ticket: ticket_str,
        })
        .await;

    // 6. Serve, surfacing sender-side progress from provider events, until the user
    //    cancels. Holding `tags` + `collection_tag` keeps the content alive.
    let mut completed = false;
    loop {
        tokio::select! {
            _ = token.cancelled() => break,
            ev = ev_rx.recv() => match ev {
                Some(ProviderEvent::PeerJoined) => {
                    let _ = tx.send(Progress::PeerJoined { id }).await;
                }
                Some(ProviderEvent::Progress { offset, total: t }) => {
                    let total = if t > 0 { t } else { total };
                    let _ = tx
                        .send(Progress::Transferring { id, offset, total, route: Route::Unknown })
                        .await;
                }
                Some(ProviderEvent::Done { bytes, seconds }) => {
                    completed = true;
                    core.inner.catalog.lock().await.set_status(id, Status::Done, Some(bytes));
                    let _ = tx.send(Progress::Done { id, stats: TransferStats { bytes, seconds } }).await;
                    // keep serving — another receiver may still fetch — until cancelled.
                }
                Some(ProviderEvent::Aborted) => { /* a receiver aborted; keep serving */ }
                None => break,
            }
        }
    }

    core.inner.serving.lock().await.remove(&hash_key);
    core.inner.bound.lock().await.remove(&hash_key);
    drop(collection_tag);
    drop(tags);
    if !completed {
        core.inner
            .catalog
            .lock()
            .await
            .set_status(id, Status::Cancelled, None);
    }
    let _ = tx.send(Progress::Cancelled { id }).await;
    Ok(())
}

/// File length, tolerating missing metadata.
fn file_len(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// Enumerate files to send, with forward-slash relative names. A directory keeps
/// its top-level name so the receiver recreates the tree.
fn collect_files(path: &Path) -> anyhow::Result<Vec<(String, PathBuf)>> {
    use walkdir::WalkDir;

    let meta = std::fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if meta.is_file() {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .context("file has no name")?;
        return Ok(vec![(name, path.to_path_buf())]);
    }

    let base = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut out = Vec::new();
    for entry in WalkDir::new(path).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry.path().strip_prefix(path).unwrap_or(entry.path());
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let name = if base.is_empty() {
            rel_str
        } else {
            format!("{base}/{rel_str}")
        };
        out.push((name, entry.path().to_path_buf()));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Sender-side events distilled from iroh-blobs provider events, routed per hash.
pub(crate) enum ProviderEvent {
    PeerJoined,
    Progress { offset: u64, total: u64 },
    Done { bytes: u64, seconds: f64 },
    Aborted,
}

/// Consume the global provider-event stream from the blobs server and route each
/// served-request's progress to the matching in-flight `send` (by content hash).
pub(crate) async fn consume_provider_events(core: Core, mut rx: mpsc::Receiver<ProviderMessage>) {
    while let Some(msg) = rx.recv().await {
        match msg {
            // Learn which device each connection belongs to, for one-to-one gating.
            ProviderMessage::ClientConnectedNotify(m) => {
                if let Some(eid) = m.endpoint_id {
                    core.inner.conns.lock().await.insert(m.connection_id, eid);
                }
            }
            ProviderMessage::ConnectionClosed(m) => {
                core.inner.conns.lock().await.remove(&m.connection_id);
            }
            // A peer requested our content by hash. Gate it (one-to-one), then, if
            // allowed, route per-request progress to the matching send.
            ProviderMessage::GetRequestReceived(m) => {
                let hash_key = m.request.hash.to_string();
                let conn_id = m.connection_id;
                let endpoint = core.inner.conns.lock().await.get(&conn_id).copied();
                if !approve_one_to_one(&core, &hash_key, endpoint).await {
                    let _ = m.tx.send(Err(AbortReason::Permission)).await;
                    continue;
                }
                let _ = m.tx.send(Ok(())).await;

                let sub = core.inner.serving.lock().await.get(&hash_key).cloned();
                if let Some(tx) = sub {
                    let _ = tx.send(ProviderEvent::PeerJoined);
                    let mut stream = m.rx; // irpc receiver of per-request RequestUpdate
                    tokio::spawn(async move {
                        let mut total = 0u64;
                        // Throttle UI progress to ~12/s (provider progress is per-chunk).
                        let mut last =
                            std::time::Instant::now() - std::time::Duration::from_millis(200);
                        while let Ok(Some(update)) = stream.recv().await {
                            match update {
                                RequestUpdate::Started(s) => total = s.size,
                                RequestUpdate::Progress(p) => {
                                    if last.elapsed() >= std::time::Duration::from_millis(80) {
                                        last = std::time::Instant::now();
                                        let _ = tx.send(ProviderEvent::Progress {
                                            offset: p.end_offset,
                                            total,
                                        });
                                    }
                                }
                                RequestUpdate::Completed(c) => {
                                    let _ = tx.send(ProviderEvent::Done {
                                        bytes: c.stats.payload_bytes_sent,
                                        seconds: c.stats.duration.as_secs_f64(),
                                    });
                                    break;
                                }
                                RequestUpdate::Aborted(_) => {
                                    let _ = tx.send(ProviderEvent::Aborted);
                                    break;
                                }
                            }
                        }
                    });
                }
            }
            _ => {}
        }
    }
}

/// One-to-one enforcement: a ticket is served to the FIRST device that requests it
/// (whether it peeks via `inspect` or downloads); the same device may reconnect
/// (preview → accept, or resume), but a different device is denied. Defaults to
/// ALLOW whenever the peer is unknown or we are not the sender for this hash, so a
/// normal single-receiver transfer is never blocked or hung.
async fn approve_one_to_one(
    core: &Core,
    hash_key: &str,
    endpoint: Option<iroh::EndpointId>,
) -> bool {
    let Some(eid) = endpoint else {
        return true;
    };
    if !core.inner.serving.lock().await.contains_key(hash_key) {
        return true;
    }
    let mut bound = core.inner.bound.lock().await;
    match bound.get(hash_key) {
        None => {
            bound.insert(hash_key.to_string(), eid);
            true
        }
        Some(existing) => *existing == eid,
    }
}

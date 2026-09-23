//! Sending: import a path, bundle it, serve it, hand back a ticket.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use futures_lite::StreamExt;
use iroh_blobs::api::blobs::{AddPathOptions, AddProgressItem, ImportMode};
use iroh_blobs::api::TempTag;
use iroh_blobs::format::collection::Collection;
use iroh_blobs::protocol::{ChunkRanges, ChunkRangesExt, ChunkRangesSeq};
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

/// Why a send of content that is already live was refused. Shown on the new
/// card as-is.
const ALREADY_SHARING: &str =
    "This is already being shared. Use its code, or stop sharing it first.";

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
    let (files, skipped) = collect_files(&path)?;
    let total: u64 = files.iter().map(|(_, p)| file_len(p)).sum();

    // 2. Import each file, holding the TempTags so nothing is GC'd while serving.
    let mut tags: Vec<TempTag> = Vec::with_capacity(files.len());
    let mut entries: Vec<(String, Hash)> = Vec::with_capacity(files.len());
    // Each file's size, in collection order, so a request can be sized.
    let mut sizes: Vec<u64> = Vec::with_capacity(files.len());
    let mut imported = 0u64;
    // Numbers on the card from the start, not a bare "Preparing...".
    let _ = tx.send(Progress::Importing { id, done: 0, total }).await;
    for (name, p) in files {
        let Some((tt, len)) = import_file(store, &p, &token, &tx, id, imported, total).await?
        else {
            let _ = tx.send(Progress::Cancelled { id }).await;
            return Ok(());
        };
        entries.push((name, tt.hash()));
        tags.push(tt);
        sizes.push(len);
        imported += len;
        let _ = tx
            .send(Progress::Importing {
                id,
                done: imported,
                total,
            })
            .await;
    }

    // What was actually imported, in case a file changed size since the listing.
    let total = imported;

    // 3. Bundle into a Collection (a HashSeq) — uniform for single file or folder.
    let files_count = entries.len();
    let collection: Collection = entries.into_iter().collect();
    let collection_tag = collection.store(store).await.context("store collection")?;
    let hash = collection_tag.hash();

    // 4. Mint the ticket from our endpoint address. For relay-backed modes, wait
    //    (time-boxed) for a relay handshake so the address is reachable; skip in
    //    local-only mode where there is no relay (online() would never resolve).
    //    A cancel cuts the wait short (and is caught just below).
    let endpoint = core.inner.router.endpoint();
    if !matches!(core.inner.config.infra, crate::Infra::LocalOnly) {
        tokio::select! {
            _ = token.cancelled() => {}
            _ = tokio::time::timeout(Duration::from_secs(10), endpoint.online()) => {}
        }
    }
    let addr = endpoint.addr();
    let ticket = BlobTicket::new(addr, hash, BlobFormat::HashSeq);
    let ticket_str = ticket.to_string();

    // Cancelled while importing or waiting for the relay: stop before minting a
    // live code, and before taking over another send of the same content.
    if token.is_cancelled() {
        let _ = tx.send(Progress::Cancelled { id }).await;
        return Ok(());
    }

    // 5. Register for provider events on this hash, record, and announce.
    //    The same files always make the same hash, so this content may already
    //    be live under another send. Check and claim under one lock.
    let hash_key = hash.to_string();
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<ProviderEvent>();
    let delivered = Arc::new(AtomicBool::new(false));
    {
        let mut serving = core.inner.serving.lock().await;
        if let Some(live) = serving.get(&hash_key) {
            // Not delivered yet: its code is the one to use. (The binding is
            // per hash, so two live sends of it could not go to two people.)
            if !live.delivered.load(Ordering::Acquire) {
                anyhow::bail!(ALREADY_SHARING);
            }
            // Delivered: this send takes over, and the old one stops.
            live.token.cancel();
        }
        serving.insert(
            hash_key.clone(),
            Serving {
                id,
                events: ev_tx,
                token: token.clone(),
                delivered: delivered.clone(),
                sizes: sizes.into(),
            },
        );
        // A new share starts unbound: the first device to use it takes it.
        core.inner.bound.lock().await.remove(&hash_key);
    }
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
            files_count,
            total,
        ));
    }
    let _ = tx
        .send(Progress::Ready {
            id,
            ticket: ticket_str,
            skipped,
        })
        .await;

    // 6. Serve, surfacing sender-side progress from provider events, until the user
    //    cancels. Holding `tags` + `collection_tag` keeps the content alive.
    let mut completed = false;
    // Downloads of file content being served right now. Every one that joins
    // ends with exactly one Done or Aborted. (A preview never joins.)
    let mut in_flight = 0usize;
    // Whether "previewing" is what the card says now, so the two requests of
    // a preview (or a download's size check) report it once.
    let mut previewing = false;
    loop {
        tokio::select! {
            // Once cancelled, report nothing more but the Cancelled below.
            biased;
            _ = token.cancelled() => break,
            ev = ev_rx.recv() => match ev {
                Some(ProviderEvent::Previewing) => {
                    // A size check while content is already moving is no news.
                    if in_flight == 0 && !previewing {
                        previewing = true;
                        let _ = tx.send(Progress::Previewing { id }).await;
                    }
                }
                Some(ProviderEvent::PeerJoined) => {
                    in_flight += 1;
                    previewing = false;
                    let _ = tx.send(Progress::PeerJoined { id }).await;
                }
                Some(ProviderEvent::Progress { offset, total: t }) => {
                    let total = if t > 0 { t } else { total };
                    let offset = offset.min(total);
                    let _ = tx
                        .send(Progress::Transferring { id, offset, total, route: Route::Unknown })
                        .await;
                }
                Some(ProviderEvent::Done { bytes, body, seconds }) => {
                    in_flight = in_flight.saturating_sub(1);
                    completed = true;
                    delivered.store(true, Ordering::Release);
                    // Record how much of the transfer the receiver now holds
                    // (all of it, unless it chose some files), not the bytes
                    // this one request moved: a resumed download moves less.
                    core.inner
                        .catalog
                        .lock()
                        .await
                        .set_status(id, Status::Done, Some(body.min(total)));
                    let _ = tx.send(Progress::Done { id, stats: TransferStats { bytes, seconds } }).await;
                    // Keep serving until cancelled: the same device may come
                    // back for it (the one-to-one gate still applies).
                }
                Some(ProviderEvent::Aborted) => {
                    in_flight = in_flight.saturating_sub(1);
                    // The receiver cancelled or dropped, and nothing else is
                    // being served: say so rather than sit on "Sending...".
                    // Keep serving so the same device can come back and resume.
                    if in_flight == 0 {
                        let _ = tx.send(Progress::PeerLeft { id }).await;
                    }
                }
                Some(ProviderEvent::Declined) => {
                    previewing = false;
                    let _ = tx.send(Progress::Declined { id }).await;
                }
                None => break,
            }
        }
    }

    // Stop serving, but only if this send still owns the hash: a newer send
    // of the same content may have taken it over, binding and all.
    {
        let mut serving = core.inner.serving.lock().await;
        if serving.get(&hash_key).is_some_and(|s| s.id == id) {
            serving.remove(&hash_key);
            core.inner.bound.lock().await.remove(&hash_key);
        }
    }
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

/// Import one file into the store (by reference where it can), reporting
/// progress while it is hashed: `before` bytes of the send's `total` are
/// already done. Returns the tag that keeps it alive and its size, or `None`
/// if the send was cancelled first. Dropping the import's progress stream
/// makes the store stop hashing at its next progress report, so a cancel does
/// not sit through the rest of a large file.
async fn import_file(
    store: &iroh_blobs::store::fs::FsStore,
    path: &Path,
    token: &CancellationToken,
    tx: &mpsc::Sender<Progress>,
    id: TransferId,
    before: u64,
    total: u64,
) -> anyhow::Result<Option<(TempTag, u64)>> {
    let mut items = store
        .add_path_with_opts(AddPathOptions {
            path: path.to_path_buf(),
            mode: ImportMode::TryReference,
            format: BlobFormat::Raw,
        })
        .stream()
        .await;
    let mut size = None;
    // A few updates a second is plenty, and `try_send` never holds up hashing.
    let mut last = Instant::now();
    loop {
        let item = tokio::select! {
            biased;
            _ = token.cancelled() => return Ok(None),
            item = items.next() => item,
        };
        match item {
            Some(AddProgressItem::Size(n)) => size = Some(n),
            // By reference there is no copy: hashing is the whole import.
            Some(AddProgressItem::OutboardProgress(offset)) => {
                if last.elapsed() >= Duration::from_millis(100) {
                    last = Instant::now();
                    let _ = tx.try_send(Progress::Importing {
                        id,
                        done: before + offset,
                        total,
                    });
                }
            }
            Some(AddProgressItem::Done(tt)) => {
                let len = size.unwrap_or_else(|| file_len(path));
                return Ok(Some((tt, len)));
            }
            Some(AddProgressItem::Error(e)) => {
                return Err(anyhow::Error::from(e).context(format!("import {}", path.display())))
            }
            Some(AddProgressItem::CopyProgress(_) | AddProgressItem::CopyDone) => {}
            None => anyhow::bail!("import {} ended unexpectedly", path.display()),
        }
    }
}

/// Why a folder send was refused before any code was made. Shown as-is.
const EMPTY_FOLDER: &str = "This folder has no files to send.";
const ONLY_OUTSIDE_LINKS: &str =
    "This folder only has links to things outside it, so there is nothing to send.";

/// Enumerate files to send, with forward-slash relative names. A directory keeps
/// its top-level name so the receiver recreates the tree. Also returns how many
/// links in a folder were left out.
///
/// A link inside a folder is sent (as the file it points to, under the link's
/// name) only when it points to a file inside that same folder, so a link can
/// never carry something from elsewhere on the disk along with it. Links to
/// anything else (outside the folder, to a folder, or to nothing) are left out
/// and counted, so the sender can be told. Links to folders are never followed,
/// so there are no loops. A folder with nothing to send is refused.
fn collect_files(path: &Path) -> anyhow::Result<(Vec<(String, PathBuf)>, usize)> {
    use walkdir::WalkDir;

    let meta = std::fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if meta.is_file() {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .context("file has no name")?;
        return Ok((vec![(name, path.to_path_buf())], 0));
    }

    let base = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let root = std::fs::canonicalize(path).with_context(|| format!("open {}", path.display()))?;
    let mut out = Vec::new();
    let mut skipped = 0usize;
    for entry in WalkDir::new(path).follow_links(false) {
        let entry = entry?;
        let ft = entry.file_type();
        let file = if ft.is_file() {
            entry.path().to_path_buf()
        } else if ft.is_dir() {
            continue;
        } else if entry.path_is_symlink() {
            match std::fs::canonicalize(entry.path()) {
                Ok(target) if target.starts_with(&root) && target.is_file() => target,
                _ => {
                    skipped += 1;
                    continue;
                }
            }
        } else {
            // Not a file at all (a socket, a device): nothing to send.
            continue;
        };
        let rel = entry.path().strip_prefix(path).unwrap_or(entry.path());
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let name = if base.is_empty() {
            rel_str
        } else {
            format!("{base}/{rel_str}")
        };
        out.push((name, file));
    }
    if out.is_empty() {
        anyhow::bail!(if skipped > 0 {
            ONLY_OUTSIDE_LINKS
        } else {
            EMPTY_FOLDER
        });
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok((out, skipped))
}

/// A live send's entry in `serving`: the gate's allow-list, and where provider
/// events for that hash go.
#[derive(Clone)]
pub(crate) struct Serving {
    /// The send that owns this hash. Only it may remove the entry.
    pub(crate) id: TransferId,
    /// Provider events for this hash are routed here.
    pub(crate) events: mpsc::UnboundedSender<ProviderEvent>,
    /// The owning send's token. Requests in flight stop when it fires, and a
    /// newer send of the same content fires it to take over.
    pub(crate) token: CancellationToken,
    /// Set once a download of the send's content finished (a preview never
    /// counts).
    pub(crate) delivered: Arc<AtomicBool>,
    /// Each file's size, in collection order (file `i` is request offset
    /// `i + 2`). Tells a download from a preview, and sizes a download.
    pub(crate) sizes: Arc<[u64]>,
}

/// Sender-side events distilled from iroh-blobs provider events, routed per hash.
pub(crate) enum ProviderEvent {
    /// A request for no file content: the receiver's preview or size check.
    Previewing,
    /// A download of file content started.
    PeerJoined,
    Progress {
        offset: u64,
        total: u64,
    },
    /// A download finished. `bytes` went over the wire; `body` is the full
    /// size of the files it asked for.
    Done {
        bytes: u64,
        body: u64,
        seconds: f64,
    },
    Aborted,
    /// The bound device declined from the preview (see [`consume_declines`]).
    Declined,
}

/// How many bytes of file content a request asks for, or `None` if it asks
/// for none. A collection's hash sequence is `[names, file0, file1, ...]`, so
/// request offset 0 is the root, 1 the names blob, and `i + 2` file `i`.
///
/// The receiver's preview asks for the root and the names only, and its size
/// check adds each file's last chunk (the proof of the file's size). Neither
/// is a delivery. Anything more of a file is: the whole of it, or the ranges a
/// resumed download is still missing. The sum is of the requested files' full
/// sizes.
fn requested_body(ranges: &ChunkRangesSeq, sizes: &[u64]) -> Option<u64> {
    let size_only = ChunkRanges::last_chunk();
    let mut body = None;
    // Zipped with `sizes` to stay bounded: a size check's ranges repeat
    // forever, and only real files count.
    for (r, size) in ranges.iter_infinite().skip(2).zip(sizes) {
        if !r.is_empty() && *r != size_only {
            *body.get_or_insert(0) += size;
        }
    }
    body
}

/// Consume the global provider-event stream from the blobs server and route each
/// served-request's progress to the matching in-flight `send` (by content hash).
///
/// Ordering: iroh-blobs 0.103 awaits a connection's `ClientConnectedNotify` on
/// this channel before it accepts any stream on that connection, and this loop
/// handles messages one at a time in order. So a connection's peer id is always
/// in `conns` before any of its requests reach the gate below.
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
                let Some(route) = approve_one_to_one(&core, &hash_key, endpoint).await else {
                    let _ = m.tx.send(Err(AbortReason::Permission)).await;
                    continue;
                };
                // Only a download of file content is a delivery: it alone
                // joins, reports progress, and ends in Done or Aborted. A
                // preview or size check says "previewing" and nothing else.
                let body = requested_body(&m.request.ranges, &route.sizes);
                let _ = m.tx.send(Ok(())).await;

                let Serving {
                    events: tx, token, ..
                } = route;
                let _ = tx.send(match body {
                    Some(_) => ProviderEvent::PeerJoined,
                    None => ProviderEvent::Previewing,
                });
                // Drain the per-request updates even for a preview: dropping
                // this receiver early makes the provider abort the request.
                let mut stream = m.rx;
                tokio::spawn(async move {
                    let mut completed = false;
                    // One running offset for the whole request, against the
                    // full size of the files it asks for (`body`), so the bar
                    // fills once instead of once per file. The provider
                    // reports offsets within the blob it is sending; `before`
                    // adds up the files already finished, and `current` is
                    // the size of the file being sent (None for the root and
                    // the names blob, which are not file bytes).
                    let mut before = 0u64;
                    let mut current: Option<u64> = None;
                    // Throttle UI progress to ~12/s (provider progress is per-chunk).
                    let mut last =
                        std::time::Instant::now() - std::time::Duration::from_millis(200);
                    loop {
                        let update = tokio::select! {
                            // The send ended (cancelled, dismissed, replaced).
                            // Returning drops `stream`, and the provider aborts
                            // this request at its next write, so a transfer in
                            // flight stops too instead of running to the end.
                            _ = token.cancelled() => break,
                            update = stream.recv() => update,
                        };
                        let Ok(Some(update)) = update else { break };
                        let Some(body) = body else {
                            continue;
                        };
                        match update {
                            RequestUpdate::Started(s) => {
                                // Blobs go out in offset order: 0 is the root,
                                // 1 the names, 2 and up the files.
                                before += current.take().unwrap_or(0);
                                if s.index >= 2 {
                                    current = Some(s.size);
                                }
                            }
                            RequestUpdate::Progress(p) => {
                                if current.is_some()
                                    && last.elapsed() >= std::time::Duration::from_millis(80)
                                {
                                    last = std::time::Instant::now();
                                    let _ = tx.send(ProviderEvent::Progress {
                                        offset: before + p.end_offset,
                                        total: body,
                                    });
                                }
                            }
                            RequestUpdate::Completed(c) => {
                                completed = true;
                                let _ = tx.send(ProviderEvent::Done {
                                    bytes: c.stats.payload_bytes_sent,
                                    body,
                                    seconds: c.stats.duration.as_secs_f64(),
                                });
                                break;
                            }
                            RequestUpdate::Aborted(_) => break,
                        }
                    }
                    // Anything short of completion counts as the receiver
                    // leaving, including an update stream that just closed.
                    // (Not when the send itself ended: that is no news. And
                    // not for a preview, which never joined.)
                    if body.is_some() && !completed && !token.is_cancelled() {
                        let _ = tx.send(ProviderEvent::Aborted);
                    }
                });
            }
            // Dropwire only ever fetches a live send's root with a plain GET, so
            // every other request kind is refused outright. In iroh-blobs 0.103
            // the `get` mask governs all of them, so they arrive here as
            // intercepts; answering keeps the refusal explicit rather than relying
            // on a dropped reply. Push matters most: it would write into our store.
            ProviderMessage::GetManyRequestReceived(m) => {
                let _ = m.tx.send(Err(AbortReason::Permission)).await;
            }
            ProviderMessage::PushRequestReceived(m) => {
                let _ = m.tx.send(Err(AbortReason::Permission)).await;
            }
            ProviderMessage::ObserveRequestReceived(m) => {
                let _ = m.tx.send(Err(AbortReason::Permission)).await;
            }
            _ => {}
        }
    }
}

/// The one-to-one gate, deny by default. Only the root hash of a LIVE send (one
/// with an entry in `serving`) is served, and only to the device it is bound
/// to. The first device to request it (a preview or a download) takes the
/// binding; that same device may come back (preview, then accept, or resume).
///
/// Everything else is refused: an unknown peer, a child blob fetched on its
/// own, content this device received, and any send that has ended (cancelled,
/// dismissed, or from before a restart; a restarted sender must Resend).
///
/// On approval, returns where that request's progress should be routed. The
/// check and the binding happen under the `serving` lock, so a send tearing
/// down at the same moment can never be bound or served after it ends.
async fn approve_one_to_one(
    core: &Core,
    hash_key: &str,
    endpoint: Option<iroh::EndpointId>,
) -> Option<Serving> {
    let eid = endpoint?;
    let serving = core.inner.serving.lock().await;
    let entry = serving.get(hash_key)?;
    let mut bound = core.inner.bound.lock().await;
    match bound.get(hash_key) {
        None => {
            bound.insert(hash_key.to_string(), eid);
        }
        Some(existing) if *existing == eid => {}
        Some(_) => return None,
    }
    Some(entry.clone())
}

/// Act on declines from receivers' previews (routed from the control channel
/// with the sender's authenticated id). Only the device a code is bound to
/// can decline it, so no one else can release someone's binding. Declining
/// releases the binding and tells the send; the send keeps serving, so its
/// code can go to someone else. A decline after that device downloaded the
/// content is ignored.
pub(crate) async fn consume_declines(
    core: Core,
    mut rx: mpsc::UnboundedReceiver<(iroh::EndpointId, Option<String>)>,
) {
    while let Some((remote, hash)) = rx.recv().await {
        let serving = core.inner.serving.lock().await;
        let mut bound = core.inner.bound.lock().await;
        let declined = match hash {
            Some(h) => (bound.get(&h) == Some(&remote)).then_some(h),
            // An older peer's decline names no code. Act only when exactly one
            // live send is bound to that device, so there is no guessing.
            None => {
                let mut theirs = bound
                    .iter()
                    .filter(|(h, eid)| **eid == remote && serving.contains_key(*h))
                    .map(|(h, _)| h.clone());
                match (theirs.next(), theirs.next()) {
                    (Some(h), None) => Some(h),
                    _ => None,
                }
            }
        };
        let Some(h) = declined else { continue };
        let Some(entry) = serving.get(&h) else {
            continue;
        };
        // Too late once the device has downloaded it: releasing the code now
        // would let a second device have what the first one already got.
        if entry.delivered.load(Ordering::Acquire) {
            continue;
        }
        bound.remove(&h);
        let _ = entry.events.send(ProviderEvent::Declined);
    }
}

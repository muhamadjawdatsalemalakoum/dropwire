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
        self.send_many(vec![path]).await
    }

    /// Like [`Core::send`], for several files and folders at once: one code
    /// for all of them. Each keeps its own top-level name; when two share one,
    /// the later gets a number ("photo.jpg", "photo (2).jpg") so nothing is
    /// overwritten on the receiving side. The same path given twice is sent
    /// once.
    pub async fn send_many(&self, paths: Vec<PathBuf>) -> Result<(TransferId, ProgressStream)> {
        if paths.is_empty() {
            return Err(anyhow::anyhow!("nothing was chosen to send").into());
        }
        let id = TransferId::new();
        let (tx, rx) = mpsc::channel(64);
        let token = CancellationToken::new();
        self.inner.active.lock().await.insert(id, token.clone());

        let core = self.clone();
        let tx_err = tx.clone();
        tokio::spawn(async move {
            if let Err(e) = run_send(core.clone(), id, paths, tx, token).await {
                tracing::warn!("send {id} failed: {e:#}");
                let (code, message) = crate::fail::describe(&e);
                let _ = tx_err.send(Progress::Error { id, code, message }).await;
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
    paths: Vec<PathBuf>,
    tx: mpsc::Sender<Progress>,
    token: CancellationToken,
) -> anyhow::Result<()> {
    let store = &core.inner.store;

    // 1. Enumerate files (single file -> one entry; directory -> recursive).
    let Listing {
        files,
        skipped,
        display_name,
        roots,
    } = list_paths(&paths)?;
    let total: u64 = files.iter().map(|(_, p)| file_len(p)).sum();

    // 2. Import each file, holding the TempTags so nothing is GC'd while serving.
    let mut tags: Vec<TempTag> = Vec::with_capacity(files.len());
    let mut entries: Vec<(String, Hash)> = Vec::with_capacity(files.len());
    // Each file as it was when shared, in collection order: to size requests,
    // and to notice a file that is edited while its code is out.
    let mut sent: Vec<SentFile> = Vec::with_capacity(files.len());
    let mut imported = 0u64;
    // Numbers on the card from the start, not a bare "Preparing...".
    let _ = tx.send(Progress::Importing { id, done: 0, total }).await;
    for (name, p) in files {
        // Taken before hashing, so an edit made while hashing shows up too.
        let modified = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
        let Some((tt, len)) = import_file(store, &p, &token, &tx, id, imported, total).await?
        else {
            let _ = tx.send(Progress::Cancelled { id }).await;
            return Ok(());
        };
        entries.push((name.clone(), tt.hash()));
        tags.push(tt);
        sent.push(SentFile {
            name,
            path: p,
            size: len,
            modified,
        });
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
    let collection_tag = collection
        .store(store)
        .await
        .context("could not prepare the transfer")?;
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
                files: sent.into(),
                denied: Default::default(),
            },
        );
        // A new share starts unbound: the first device to use it takes it.
        core.inner.bound.lock().await.remove(&hash_key);
    }
    {
        // `source` stays the one chosen path, for a send of one thing (and
        // for older builds reading this history). `sources` lists everything
        // chosen, so a send of several things can be sent again as a whole.
        let sources: Vec<String> = roots
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        let source = match sources.as_slice() {
            [one] => Some(one.clone()),
            _ => None,
        };
        let mut rec = Catalog::new_record(
            id,
            Direction::Send,
            display_name,
            ticket_str.clone(),
            hash_key.clone(),
            None,
            source,
            files_count,
            total,
        );
        rec.sources = sources;
        core.inner.catalog.lock().await.upsert(rec);
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
    // Set when a file was edited or removed after it was shared: the send
    // stops, since its code can no longer be honoured.
    let mut changed: Option<String> = None;
    loop {
        tokio::select! {
            // Once cancelled, report nothing more but the Cancelled below.
            biased;
            _ = token.cancelled() => break,
            ev = ev_rx.recv() => match ev {
                Some(ProviderEvent::Changed { name }) => {
                    changed = Some(name);
                    break;
                }
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
                    let _ = tx.send(Progress::Done { id, stats: TransferStats { bytes, seconds, ..Default::default() } }).await;
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

    // A changed file ends the send like a cancel does, downloads in flight
    // included: what they would send no longer matches the code.
    if changed.is_some() {
        token.cancel();
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
    if let Some(name) = changed {
        // A delivered send stays "done" in history; only its code stopped.
        if !completed {
            core.inner
                .catalog
                .lock()
                .await
                .set_status(id, Status::Error, None);
        }
        let _ = tx
            .send(Progress::Error {
                id,
                code: crate::progress::ErrorCode::Other,
                message: format!(
                    "{name} was changed after it was shared, so this code no longer works. \
                     Share it again to send it as it is now."
                ),
            })
            .await;
        return Ok(());
    }
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
                return Err(
                    anyhow::Error::from(e).context(format!("could not read {}", path.display()))
                )
            }
            Some(AddProgressItem::CopyProgress(_) | AddProgressItem::CopyDone) => {}
            None => anyhow::bail!("could not read {}: it ended unexpectedly", path.display()),
        }
    }
}

/// Why a send was refused before any code was made, when what was chosen
/// holds no files. Shown as-is.
const EMPTY_FOLDER: &str = "This folder has no files to send.";
const ONLY_OUTSIDE_LINKS: &str =
    "This folder only has links to things outside it, so there is nothing to send.";
const EMPTY_FOLDERS: &str = "These folders have no files to send.";
const ONLY_OUTSIDE_LINKS_MANY: &str =
    "These folders only have links to things outside them, so there is nothing to send.";

/// Everything chosen for one send, ready to import.
struct Listing {
    /// Every file, named as the receiver will see it, in collection order.
    files: Vec<(String, PathBuf)>,
    /// Links left out (see [`collect_files`]).
    skipped: usize,
    /// The name for the card and history: the one thing chosen, or
    /// "<first> and N more".
    display_name: String,
    /// The distinct chosen paths, in order.
    roots: Vec<PathBuf>,
}

/// List the files of every chosen path, one after the other. Each keeps its
/// own top-level name (a file's name, a folder's name); when one clashes with
/// an earlier one, ignoring case since most desktop file systems do, it is
/// numbered ("photo (2).jpg", "pics (2)") so no file overwrites another on the
/// receiving side. The same path chosen twice is listed once.
fn list_paths(paths: &[PathBuf]) -> anyhow::Result<Listing> {
    use std::collections::{HashMap, HashSet};

    let mut files = Vec::new();
    let mut skipped = 0usize;
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut seen = HashSet::new();
    let mut taken: HashSet<String> = HashSet::new();
    let mut first_name = None;
    for path in paths {
        let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        if !seen.insert(key) {
            continue;
        }
        let (mut listed, left_out) = collect_files(path)?;
        skipped += left_out;

        // This path's top-level names, renamed where an earlier path has one.
        let mut renamed: HashMap<String, String> = HashMap::new();
        for (name, _) in &listed {
            let (top, rest) = split_top(name);
            if !renamed.contains_key(top) {
                let unique = unique_name(top, rest.is_some(), &mut taken);
                renamed.insert(top.to_string(), unique);
            }
        }
        for (name, _) in &mut listed {
            let (top, rest) = split_top(name);
            let new_top = &renamed[top];
            if new_top != top {
                *name = match rest {
                    Some(rest) => format!("{new_top}/{rest}"),
                    None => new_top.clone(),
                };
            }
        }

        if first_name.is_none() {
            first_name = Some(
                path.file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "transfer".to_string()),
            );
        }
        files.extend(listed);
        roots.push(path.clone());
    }

    if files.is_empty() {
        anyhow::bail!(match (roots.len() > 1, skipped > 0) {
            (false, false) => EMPTY_FOLDER,
            (false, true) => ONLY_OUTSIDE_LINKS,
            (true, false) => EMPTY_FOLDERS,
            (true, true) => ONLY_OUTSIDE_LINKS_MANY,
        });
    }
    let first = first_name.unwrap_or_else(|| "transfer".to_string());
    let display_name = match roots.len() {
        0 | 1 => first,
        n => format!("{first} and {} more", n - 1),
    };
    Ok(Listing {
        files,
        skipped,
        display_name,
        roots,
    })
}

/// A listed name's top-level part, and the rest of it if there is more.
fn split_top(name: &str) -> (&str, Option<&str>) {
    match name.split_once('/') {
        Some((top, rest)) => (top, Some(rest)),
        None => (name, None),
    }
}

/// `name`, or the first free "name (2)", "name (3)"... that nothing in `taken`
/// has (ignoring case), which is then taken. A file keeps its extension last:
/// "photo (2).jpg".
fn unique_name(name: &str, is_dir: bool, taken: &mut std::collections::HashSet<String>) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && !is_dir => name.split_at(i),
        _ => (name, ""),
    };
    let mut candidate = name.to_string();
    let mut n = 1;
    while !taken.insert(candidate.to_lowercase()) {
        n += 1;
        candidate = format!("{stem} ({n}){ext}");
    }
    candidate
}

/// Enumerate the files of one chosen path, with forward-slash relative names.
/// A directory keeps its top-level name so the receiver recreates the tree.
/// Also returns how many links in a folder were left out.
///
/// A link inside a folder is sent (as the file it points to, under the link's
/// name) only when it points to a file inside that same folder, so a link can
/// never carry something from elsewhere on the disk along with it. Links to
/// anything else (outside the folder, to a folder, or to nothing) are left out
/// and counted, so the sender can be told. Links to folders are never followed,
/// so there are no loops. An empty folder lists nothing ([`list_paths`] refuses
/// a send with nothing in it).
fn collect_files(path: &Path) -> anyhow::Result<(Vec<(String, PathBuf)>, usize)> {
    use walkdir::WalkDir;

    let meta =
        std::fs::metadata(path).with_context(|| format!("could not open {}", path.display()))?;
    if meta.is_file() {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .with_context(|| format!("{} has no file name", path.display()))?;
        return Ok((vec![(name, path.to_path_buf())], 0));
    }

    let base = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let root = std::fs::canonicalize(path)
        .with_context(|| format!("could not open {}", path.display()))?;
    let mut out = Vec::new();
    let mut skipped = 0usize;
    for entry in WalkDir::new(path).follow_links(false) {
        let entry =
            entry.with_context(|| format!("could not read the folder {}", path.display()))?;
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
    /// The send's files as they were when shared, in collection order (file
    /// `i` is request offset `i + 2`). Tells a download from a preview, sizes
    /// a download, and shows whether a file was edited since.
    pub(crate) files: Arc<[SentFile]>,
    /// Devices this send was offered to (nearby) that did not take it. The
    /// code went out with the offer, so they hold it, but the gate refuses
    /// them. A new offer to one of them lets it back in.
    pub(crate) denied: std::collections::HashSet<iroh::EndpointId>,
}

/// One file of a live send, as it was when it was shared.
pub(crate) struct SentFile {
    /// Its name in the transfer (for messages).
    pub(crate) name: String,
    /// Where it is on disk. Files over 16 KiB are sent straight from here
    /// (they are shared by reference, not copied into the store).
    pub(crate) path: PathBuf,
    pub(crate) size: u64,
    /// Its modification time when shared, where the platform has one.
    pub(crate) modified: Option<std::time::SystemTime>,
}

impl SentFile {
    /// Whether the file on disk is no longer the one that was shared: edited,
    /// replaced or removed. Its bytes would no longer match the code, so the
    /// receiver's check would fail on them (or worse, on a file changed back
    /// and forth, pass on some and not others).
    fn changed(&self) -> bool {
        match std::fs::metadata(&self.path) {
            Ok(m) => m.len() != self.size || m.modified().ok() != self.modified,
            Err(_) => true,
        }
    }
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
    /// A file (by its name in the transfer) was edited or removed since it was
    /// shared, and a request for it was refused.
    Changed {
        name: String,
    },
}

/// What one request asks of a send's files. A collection's hash sequence is
/// `[names, file0, file1, ...]`, so request offset 0 is the root, 1 the names
/// blob, and `i + 2` file `i`.
struct Asked {
    /// The files it reads anything of, even just the last chunk.
    touched: Vec<usize>,
    /// How many bytes of file content it asks for, or `None` if it asks for
    /// none. The receiver's preview asks for the root and the names only, and
    /// its size check adds each file's last chunk (the proof of the file's
    /// size). Neither is a delivery. Anything more of a file is: the whole of
    /// it, or the ranges a resumed download is still missing. The sum is of the
    /// requested files' full sizes.
    body: Option<u64>,
}

fn asked(ranges: &ChunkRangesSeq, files: &[SentFile]) -> Asked {
    let size_only = ChunkRanges::last_chunk();
    let mut touched = Vec::new();
    let mut body = None;
    // Zipped with `files` to stay bounded: a size check's ranges repeat
    // forever, and only real files count.
    for (i, (r, file)) in ranges.iter_infinite().skip(2).zip(files).enumerate() {
        if r.is_empty() {
            continue;
        }
        touched.push(i);
        if *r != size_only {
            *body.get_or_insert(0) += file.size;
        }
    }
    Asked { touched, body }
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
                let Asked { touched, body } = asked(&m.request.ranges, &route.files);

                // Files over 16 KiB are served from where they sit on disk. One
                // edited since it was shared would go out as bytes that do not
                // match the code: refuse, and end the send with a reason.
                let files = route.files.clone();
                let changed = tokio::task::spawn_blocking(move || {
                    touched
                        .into_iter()
                        .find(|&i| files[i].changed())
                        .map(|i| files[i].name.clone())
                })
                .await
                .ok()
                .flatten();
                if let Some(name) = changed {
                    let _ = m.tx.send(Err(AbortReason::Permission)).await;
                    let _ = route.events.send(ProviderEvent::Changed { name });
                    continue;
                }

                // Only a download of file content is a delivery: it alone
                // joins, reports progress, and ends in Done or Aborted. A
                // preview or size check says "previewing" and nothing else.
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
/// own, content this device received, any send that has ended (cancelled,
/// dismissed, or from before a restart; a restarted sender must Resend), and
/// a device that turned down a nearby offer of the send.
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
    if entry.denied.contains(&eid) {
        return None;
    }
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

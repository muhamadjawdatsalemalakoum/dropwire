//! Browser adapter for Dropwire's existing collection tickets and blob ALPN.
use std::collections::HashSet;
use std::{cell::RefCell, rc::Rc};
mod disk;
use anyhow::{anyhow, bail, Context, Result};
use iroh::endpoint::{presets, Connection};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, TransportAddr};
use iroh_blobs::api::{Store, TempTag};
use iroh_blobs::format::collection::Collection;
use iroh_blobs::get::request::get_hash_seq_and_sizes;
use iroh_blobs::protocol::{ChunkRanges, GetRequest};
use iroh_blobs::ticket::BlobTicket;
use iroh_blobs::{BlobFormat, Hash};
use js_sys::{Function, Uint8Array};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wasm_bindgen::prelude::*;

const LIMIT: u64 = 9_007_199_254_740_991;
const FILES: usize = 100_000;
const META: u64 = 8 * 1024 * 1024;
const CTRL: &[u8] = b"dropwire/ctrl/1";

#[derive(Default, Debug)]
struct Gate {
    root: Option<Hash>,
    bound: Option<EndpointId>,
    closed: bool,
    phase: String,
    bytes: u64,
    total: u64,
}
impl Gate {
    fn allow(&mut self, root: Hash, peer: EndpointId) -> bool {
        if self.closed || self.root != Some(root) || self.bound.is_some_and(|bound| bound != peer) {
            return false;
        }
        self.bound = Some(peer);
        if !matches!(self.phase.as_str(), "done" | "transferring") {
            self.phase = "previewing".into();
        }
        true
    }
    fn decline(&mut self, peer: EndpointId, root: &str) {
        if self.bound == Some(peer)
            && self.root.map(|hash| hash.to_string()).as_deref() == Some(root)
            && matches!(
                self.phase.as_str(),
                "previewing" | "waiting" | "declined" | "interrupted"
            )
        {
            self.bound = None;
            self.phase = "declined".into();
        }
    }
}

#[derive(Debug, Clone)]
struct Control(Arc<Mutex<Gate>>);
impl ProtocolHandler for Control {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        let result = n0_future::time::timeout(Duration::from_secs(10), async {
            let (mut tx, mut rx) = conn.accept_bi().await?;
            let data = rx.read_to_end(4096).await.map_err(AcceptError::from_err)?;
            if let Ok(frame) = serde_json::from_slice::<serde_json::Value>(&data) {
                let mut gate = self.0.lock().unwrap();
                if frame["kind"] == "decline" {
                    if let Some(root) = frame["hash"].as_str() {
                        gate.decline(conn.remote_id(), root);
                    }
                }
            }
            tx.write_all(&data).await.map_err(AcceptError::from_err)?;
            tx.finish()?;
            let _ = tx.stopped().await;
            Ok::<_, AcceptError>(())
        })
        .await;
        match result {
            Ok(r) => r,
            Err(_) => Ok(()),
        }
    }
}

#[derive(Clone, Serialize)]
struct FileInfo {
    name: String,
    size: u64,
    hash: String,
}
struct Preview {
    ticket: BlobTicket,
    files: Vec<FileInfo>,
}

#[wasm_bindgen]
pub struct BrowserNode {
    router: Router,
    store: Store,
    gate: Arc<Mutex<Gate>>,
    tags: Vec<TempTag>,
    imported: Vec<(String, Hash)>,
    imported_size: u64,
    preview: Option<Preview>,
    provided: disk::Blobs,
    inspected_code: Option<String>,
}

fn js_error(e: impl std::fmt::Display) -> JsError {
    JsError::new(&e.to_string())
}

fn parse_ticket(input: &str) -> Result<BlobTicket> {
    if input.len() > 8192 {
        bail!("This transfer code is too long.");
    }
    let ticket: BlobTicket = input
        .trim()
        .parse()
        .map_err(|_| anyhow!("This transfer code is invalid or incomplete."))?;
    if ticket.format() != BlobFormat::HashSeq {
        bail!("Use a Dropwire file-transfer code.");
    }
    Ok(ticket)
}

fn browser_address(addr: &EndpointAddr) -> Result<EndpointAddr> {
    let mut out = EndpointAddr::new(addr.id);
    for transport in &addr.addrs {
        if let TransportAddr::Relay(relay) = transport {
            let mut url = url_without_dot(&relay.to_string())?;
            // File links may only dial secure relays in this browser adapter.
            if !url.starts_with("https://") {
                bail!("This code uses an insecure relay. Use the desktop app.");
            }
            if !url.ends_with('/') {
                url.push('/');
            }
            out = out.with_relay_url(url.parse()?);
        }
    }
    Ok(out)
}

fn url_without_dot(value: &str) -> Result<String> {
    // Upstream default relay hostnames are FQDNs; Safari rejects their dotted form.
    Ok(value.replace(".iroh.link./", ".iroh.link/"))
}

#[wasm_bindgen]
pub struct CancelHandle {
    endpoint: Endpoint,
    gate: Arc<Mutex<Gate>>,
}
#[wasm_bindgen]
impl CancelHandle {
    pub async fn cancel(&self) {
        self.gate.lock().unwrap().closed = true;
        self.endpoint.close().await;
    }
}

async fn connect(endpoint: &Endpoint, ticket: &BlobTicket, alpn: &[u8]) -> Result<Connection> {
    let addr = browser_address(ticket.addr())?;
    n0_future::time::timeout(Duration::from_secs(20), endpoint.connect(addr, alpn))
        .await
        .map_err(|_| {
            anyhow!("The sender is unavailable. Keep both apps or tabs open, then retry.")
        })?
        .map_err(|_| anyhow!("Cannot reach the sender. Check the code and your connection."))
}

#[wasm_bindgen]
impl BrowserNode {
    pub fn cancel_handle(&self) -> CancelHandle {
        CancelHandle {
            endpoint: self.router.endpoint().clone(),
            gate: self.gate.clone(),
        }
    }

    pub fn qr_svg(&self, link: String) -> Result<String, JsError> {
        if link.len() > 8192 {
            return Err(js_error("The link is too long for a QR code."));
        }
        let code = qrcode::QrCode::new(link.as_bytes()).map_err(js_error)?;
        Ok(code
            .render::<qrcode::render::svg::Color>()
            .min_dimensions(240, 240)
            .build())
    }
    pub async fn spawn(relays: String) -> Result<BrowserNode, JsError> {
        console_error_panic_hook::set_once();
        let urls: Vec<String> = serde_json::from_str(&relays).map_err(js_error)?;
        if urls.is_empty()
            || urls.len() > 8
            || urls
                .iter()
                .any(|url| !url.starts_with("https://") || url.len() > 2048)
        {
            return Err(js_error(
                "Secure browser relay configuration is unavailable. Use the desktop app.",
            ));
        }
        let map = RelayMap::try_from_iter(urls.iter().map(String::as_str)).map_err(js_error)?;
        let endpoint = Endpoint::builder(presets::N0)
            .relay_mode(RelayMode::Custom(map))
            .bind()
            .await
            .map_err(js_error)?;
        let mem = iroh_blobs::store::mem::MemStore::default();
        let store = mem.as_ref().clone();
        let gate = Arc::new(Mutex::new(Gate {
            phase: "idle".into(),
            ..Default::default()
        }));
        let provided = Rc::new(RefCell::new(Vec::new()));
        let provider = disk::Provider::new(gate.clone(), provided.clone());
        let router = Router::builder(endpoint)
            .accept(iroh_blobs::ALPN, provider)
            .accept(CTRL, Control(gate.clone()))
            .spawn();
        Ok(Self {
            router,
            store,
            gate,
            tags: Vec::new(),
            imported: Vec::new(),
            imported_size: 0,
            preview: None,
            provided,
            inspected_code: None,
        })
    }

    pub fn status(&self) -> String {
        let g = self.gate.lock().unwrap();
        serde_json::json!({"phase": g.phase, "bytes": g.bytes, "total": g.total}).to_string()
    }

    pub async fn add_file(
        &mut self,
        name: String,
        size: f64,
        read: Function,
        out_read: Function,
        out_write: Function,
    ) -> Result<(), JsError> {
        if self.gate.lock().unwrap().root.is_some() || self.preview.is_some() {
            return Err(js_error("Stop the current transfer first."));
        }
        if !size.is_finite() || size < 0.0 || size.fract() != 0.0 || size > LIMIT as f64 {
            return Err(js_error(
                "This file size cannot be represented safely by your browser.",
            ));
        }
        let size = size as u64;
        if self.imported.len() >= FILES
            || self
                .imported_size
                .checked_add(size)
                .filter(|&n| n <= LIMIT)
                .is_none()
        {
            return Err(js_error("This file list exceeds the browser's metadata safety bound. Split the folder or use the desktop app."));
        }
        if name.is_empty() || name.len() > 1024 || name.chars().any(char::is_control) {
            return Err(js_error("This file name is invalid."));
        }
        let (hash, blob) = disk::prepare(read, size, out_read, out_write)
            .await
            .map_err(js_error)?;
        self.imported.push((name, hash));
        self.provided.borrow_mut().push(blob);
        self.imported_size += size;
        Ok(())
    }

    pub async fn share(&mut self) -> Result<String, JsError> {
        if self.gate.lock().unwrap().closed {
            return Err(js_error("Transfer cancelled."));
        }
        if self.imported.is_empty() {
            return Err(js_error("Choose at least one file."));
        }
        if !self.tags.is_empty() || self.provided.borrow().len() != self.imported.len() {
            return Err(js_error(
                "Stop this session before creating another transfer.",
            ));
        }
        let collection: Collection = self.imported.clone().into_iter().collect();
        let metadata: Vec<_> = collection.to_blobs().collect();
        if metadata.iter().any(|b| b.len() as u64 > META) {
            return Err(js_error(
                "This file list is too large. Split the folder or use the desktop app.",
            ));
        }
        {
            let mut blobs = self.provided.borrow_mut();
            blobs.insert(0, disk::Blob::memory(metadata[0].clone()));
            blobs.insert(0, disk::Blob::memory(metadata[1].clone()));
        }
        let root = collection.store(&self.store).await.map_err(js_error)?;
        let hash = root.hash();
        self.tags.push(root);
        n0_future::time::timeout(Duration::from_secs(20), self.router.endpoint().online())
            .await
            .map_err(|_| {
                js_error("Cannot connect to an encrypted relay. Check your network and retry.")
            })?;
        let addr = self.router.endpoint().addr();
        if !addr
            .addrs
            .iter()
            .any(|a| matches!(a, TransportAddr::Relay(_)))
        {
            return Err(js_error("No browser-compatible relay is available."));
        }
        {
            let mut g = self.gate.lock().unwrap();
            g.root = Some(hash);
            g.total = self.imported_size;
            g.phase = "waiting".into();
        }
        Ok(BlobTicket::new(addr, hash, BlobFormat::HashSeq).to_string())
    }

    pub async fn inspect(&mut self, input: String) -> Result<String, JsError> {
        if !self.imported.is_empty() {
            return Err(js_error("Stop sharing before receiving."));
        }
        let ticket = parse_ticket(&input).map_err(js_error)?;
        if self
            .inspected_code
            .as_ref()
            .is_some_and(|code| code != &input)
        {
            return Err(js_error(
                "Stop this session before previewing a different transfer.",
            ));
        }
        if let Some(preview) = &self.preview {
            return Ok(serde_json::json!({"files": preview.files, "route": "encrypted-relay", "total": preview.files.iter().map(|f| f.size).sum::<u64>()}).to_string());
        }
        self.inspected_code = Some(input);
        let conn = connect(self.router.endpoint(), &ticket, iroh_blobs::ALPN)
            .await
            .map_err(js_error)?;
        let (seq, sizes) = n0_future::time::timeout(Duration::from_secs(30), get_hash_seq_and_sizes(&conn, &ticket.hash(), ((FILES + 1) * 32) as u64, None)).await
            .map_err(|_| js_error("The sender stopped responding. Retry the preview."))?.map_err(|_| js_error("The transfer could not be verified. The sender may have stopped, the code may already be claimed, or its file list may be invalid."))?;
        if sizes.is_empty()
            || sizes.len() > FILES + 1
            || sizes[0] > META
            || sizes[1..]
                .iter()
                .try_fold(0u64, |a, &b| a.checked_add(b))
                .filter(|&n| n <= LIMIT)
                .is_none()
        {
            return Err(js_error("This transfer exceeds the browser metadata safety bound. Split the folder or use the desktop app."));
        }
        if seq.len() != sizes.len() {
            return Err(js_error("The file list is damaged."));
        }
        let metadata_hash = seq
            .get(0)
            .ok_or_else(|| js_error("The file list is empty."))?;
        let metadata = fetch_metadata(conn, ticket.hash(), metadata_hash, sizes[0])
            .await
            .map_err(js_error)?;
        let root = self
            .store
            .blobs()
            .add_bytes_with_opts((seq.into_inner(), BlobFormat::HashSeq))
            .temp_tag()
            .await
            .map_err(js_error)?;
        let meta = self
            .store
            .blobs()
            .add_bytes(metadata)
            .temp_tag()
            .await
            .map_err(js_error)?;
        self.tags = vec![root, meta];
        let collection = Collection::load(ticket.hash(), &self.store)
            .await
            .map_err(js_error)?;
        if collection.is_empty() || collection.len() + 1 != sizes.len() {
            return Err(js_error("The file list does not match its verified sizes."));
        }
        let files: Vec<_> = collection
            .iter()
            .enumerate()
            .map(|(i, (name, hash))| FileInfo {
                name: name.clone(),
                hash: hash.to_string(),
                size: sizes[i + 1],
            })
            .collect();
        if files.iter().any(|f| {
            f.name.is_empty() || f.name.len() > 1024 || f.name.chars().any(char::is_control)
        }) {
            return Err(js_error("The sender supplied an invalid file name."));
        }
        let result = serde_json::json!({"files": files, "route": "encrypted-relay", "total": sizes[1..].iter().sum::<u64>()}).to_string();
        self.preview = Some(Preview { ticket, files });
        Ok(result)
    }

    pub async fn receive(
        &mut self,
        selected: String,
        write: Function,
        progress: Function,
    ) -> Result<(), JsError> {
        use bao_tree::io::BaoContentItem;
        use iroh_blobs::get::fsm;
        let indices: Vec<usize> = serde_json::from_str(&selected).map_err(|_| js_error("Transfer interrupted or failed integrity verification. The source may have changed. No completed file was saved; keep both endpoints open and retry."))?;
        let preview = self
            .preview
            .as_ref()
            .ok_or_else(|| js_error("Preview the transfer first."))?;
        let unique: HashSet<_> = indices.into_iter().collect();
        if unique.is_empty() || unique.iter().any(|&i| i >= preview.files.len()) {
            return Err(js_error("Choose valid files from the preview."));
        }
        let total: u64 = unique.iter().map(|&i| preview.files[i].size).sum();
        let mut request = GetRequest::builder();
        for i in &unique {
            request = request.child((*i + 1) as u64, ChunkRanges::all());
        }
        let conn = connect(self.router.endpoint(), &preview.ticket, iroh_blobs::ALPN)
            .await
            .map_err(|_| js_error("Transfer interrupted or failed integrity verification. The source may have changed. No completed file was saved; keep both endpoints open and retry."))?;
        let connected = n0_future::time::timeout(Duration::from_secs(60), async {
            let initial = fsm::start(
                conn,
                request.build(preview.ticket.hash()),
                Default::default(),
            )
            .next()
            .await
            .map_err(anyhow::Error::from)?;
            initial.next().await.map_err(anyhow::Error::from)
        })
        .await
        .map_err(|_| js_error("The sender stopped responding."))?
        .map_err(|_| js_error("Transfer interrupted or failed integrity verification. The source may have changed. No completed file was saved; keep both endpoints open and retry."))?;
        let fsm::ConnectedNext::StartChild(mut start) = connected else {
            return Err(js_error("Invalid file response."));
        };
        let mut done = 0;
        loop {
            let index = (start.offset() as usize)
                .checked_sub(2)
                .ok_or_else(|| js_error("Invalid file response."))?;
            if !unique.contains(&index) {
                return Err(js_error("The sender returned an unselected file."));
            }
            let file = preview
                .files
                .get(index)
                .ok_or_else(|| js_error("Invalid file response."))?;
            let (mut content, actual_size) = n0_future::time::timeout(
                Duration::from_secs(60),
                start
                    .next(file.hash.parse::<Hash>().map_err(|_| js_error("Transfer interrupted or failed integrity verification. The source may have changed. No completed file was saved; keep both endpoints open and retry."))?)
                    .next(),
            )
            .await
            .map_err(|_| js_error("The sender stopped responding."))?
            .map_err(|_| js_error("Transfer interrupted or failed integrity verification. The source may have changed. No completed file was saved; keep both endpoints open and retry."))?;
            if actual_size != file.size {
                return Err(js_error("The file size changed. Nothing was saved."));
            }
            let end = loop {
                match n0_future::time::timeout(Duration::from_secs(60), content.next())
                    .await
                    .map_err(|_| {
                        js_error("The sender stopped responding. Retry on a working connection.")
                    })? {
                    fsm::BlobContentNext::More((next, item)) => {
                        content = next;
                        if let BaoContentItem::Leaf(leaf) = item.map_err(|_| js_error("Transfer interrupted or failed integrity verification. The source may have changed. No completed file was saved; keep both endpoints open and retry."))? {
                            let value = write
                                .call3(
                                    &JsValue::NULL,
                                    &JsValue::from_f64(index as f64),
                                    &JsValue::from_f64(leaf.offset as f64),
                                    &Uint8Array::from(leaf.data.as_ref()),
                                )
                                .map_err(|_| js_error("Browser storage is unavailable."))?;
                            wasm_bindgen_futures::JsFuture::from(js_sys::Promise::resolve(&value)).await.map_err(|_| js_error("Browser storage is unavailable or full. Nothing was saved."))?;
                            done += leaf.data.len() as u64;
                            let _ = progress.call2(
                                &JsValue::NULL,
                                &JsValue::from_f64(done as f64),
                                &JsValue::from_f64(total as f64),
                            );
                        }
                    }
                    fsm::BlobContentNext::Done(end) => break end,
                }
            };
            match end.next() {
                fsm::EndBlobNext::MoreChildren(next) => {
                    start = next;
                }
                fsm::EndBlobNext::Closing(end) => {
                    n0_future::time::timeout(Duration::from_secs(60), end.next())
                        .await
                        .map_err(|_| js_error("The sender stopped responding."))?
                        .map_err(|_| js_error("Transfer interrupted or failed integrity verification. The source may have changed. No completed file was saved; keep both endpoints open and retry."))?;
                    break;
                }
            }
        }
        if done != total {
            return Err(js_error(
                "Integrity verification did not finish. Nothing was saved.",
            ));
        }
        Ok(())
    }

    pub async fn decline(&self) -> Result<(), JsError> {
        let preview = self
            .preview
            .as_ref()
            .ok_or_else(|| js_error("No transfer to decline."))?;
        let conn = connect(self.router.endpoint(), &preview.ticket, CTRL)
            .await
            .map_err(js_error)?;
        let frame = serde_json::json!({"kind":"decline", "hash":preview.ticket.hash().to_string()})
            .to_string();
        n0_future::time::timeout(Duration::from_secs(10), async {
            let (mut tx, mut rx) = conn.open_bi().await?;
            tx.write_all(frame.as_bytes()).await?;
            tx.finish()?;
            rx.read_to_end(4096).await?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .map_err(|_| js_error("Could not notify the sender. Your download has not started."))?
        .map_err(js_error)?;
        Ok(())
    }

    pub async fn close(&mut self) {
        self.gate.lock().unwrap().closed = true;
        self.router.endpoint().close().await;
        let _ = self.router.shutdown().await;
        self.tags.clear();
        self.imported.clear();
        self.preview = None;
        self.provided.borrow_mut().clear();
        let _ = self.store.shutdown().await;
    }
}

// Validate the wire size before creating or writing to any metadata store.
async fn fetch_metadata(
    conn: Connection,
    root: Hash,
    expected_hash: Hash,
    expected_size: u64,
) -> Result<bytes::Bytes> {
    use iroh_blobs::get::fsm;
    let operation = async {
        let request = GetRequest::builder()
            .child(0, ChunkRanges::all())
            .build(root);
        let connected = fsm::start(conn, request, Default::default())
            .next()
            .await?
            .next()
            .await?;
        let fsm::ConnectedNext::StartChild(start) = connected else {
            bail!("Invalid metadata response.");
        };
        let (mut content, actual_size) = start.next(expected_hash).next().await?;
        if !metadata_size_allowed(actual_size, expected_size) {
            bail!("The metadata size is invalid. Nothing was saved.");
        }
        let mut data = Vec::with_capacity(actual_size as usize);
        let end = loop {
            match content.next().await {
                fsm::BlobContentNext::More((next, item)) => {
                    content = next;
                    if let bao_tree::io::BaoContentItem::Leaf(leaf) = item? {
                        if leaf.offset != data.len() as u64
                            || data.len() as u64 + leaf.data.len() as u64 > expected_size
                        {
                            bail!("Invalid metadata range.");
                        }
                        data.extend_from_slice(&leaf.data);
                    }
                }
                fsm::BlobContentNext::Done(end) => break end,
            }
        };
        let fsm::EndBlobNext::Closing(end) = end.next() else {
            bail!("Unexpected metadata response.");
        };
        end.next().await?;
        if data.len() as u64 != expected_size {
            bail!("Incomplete metadata response.");
        }
        Ok(bytes::Bytes::from(data))
    };
    n0_future::time::timeout(Duration::from_secs(60), operation)
        .await
        .context("The sender stopped responding to the preview.")?
}
fn metadata_size_allowed(actual: u64, expected: u64) -> bool {
    expected <= META && actual == expected
}

#[cfg(test)]
mod tests {
    use super::*;
    fn peer(byte: u8) -> EndpointId {
        iroh::SecretKey::from_bytes(&[byte; 32]).public()
    }
    #[test]
    fn recipient_binding_survives_wrong_root_peer_and_active_decline() {
        let root = Hash::from([7; 32]);
        let mut gate = Gate {
            root: Some(root),
            phase: "waiting".into(),
            ..Default::default()
        };
        assert!(!gate.allow(Hash::from([8; 32]), peer(1)));
        assert!(gate.bound.is_none());
        assert!(gate.allow(root, peer(1)));
        assert!(!gate.allow(root, peer(2)));
        gate.decline(peer(2), &root.to_string());
        assert_eq!(gate.bound, Some(peer(1)));
        gate.phase = "transferring".into();
        gate.decline(peer(1), &root.to_string());
        assert_eq!(gate.bound, Some(peer(1)));
        gate.closed = true;
        assert!(!gate.allow(root, peer(1)));
    }
    #[test]
    fn decline_before_acceptance_releases_the_recipient() {
        let root = Hash::from([7; 32]);
        let mut gate = Gate {
            root: Some(root),
            phase: "waiting".into(),
            ..Default::default()
        };
        assert!(gate.allow(root, peer(1)));
        gate.decline(peer(1), &root.to_string());
        assert!(gate.bound.is_none());
        assert!(gate.allow(root, peer(2)));
    }
    #[test]
    fn oversized_or_changed_metadata_is_rejected_before_storage() {
        assert!(metadata_size_allowed(0, 0));
        assert!(metadata_size_allowed(META, META));
        assert!(!metadata_size_allowed(u64::MAX, 1));
        assert!(!metadata_size_allowed(META + 1, META + 1));
        assert!(!metadata_size_allowed(8, 9));
    }
    #[test]
    fn only_bounded_collection_tickets_are_accepted() {
        let addr = EndpointAddr::new(peer(1));
        let root = Hash::from([7; 32]);
        let raw = BlobTicket::new(addr.clone(), root, BlobFormat::Raw).to_string();
        assert!(parse_ticket(&raw).is_err());
        assert!(parse_ticket(&"b".repeat(8193)).is_err());
        assert!(parse_ticket("blobmalformed").is_err());
        let collection = BlobTicket::new(addr, root, BlobFormat::HashSeq).to_string();
        assert!(parse_ticket(&format!(" {collection} ")).is_ok());
    }
}

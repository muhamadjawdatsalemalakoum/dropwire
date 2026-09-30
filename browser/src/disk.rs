//! Bounded readers and verified streaming over the existing blob wire protocol.
use bao_tree::{
    io::{
        fsm::{self, CreateOutboard},
        outboard::{PostOrderOutboard, PreOrderMemOutboard},
    },
    BaoTree,
};
use bytes::Bytes;
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use iroh_blobs::{
    protocol::{ChunkRanges, ChunkRangesExt, Request},
    provider::{events::EventSender, StreamPair},
    Hash,
};
use iroh_io::{AsyncSliceReader, AsyncSliceWriter};
use js_sys::{Function, Promise, Uint8Array};
use std::{
    cell::RefCell,
    io,
    rc::Rc,
    sync::{Arc, Mutex},
};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

const WINDOW: usize = 1024 * 1024;
fn error() -> io::Error {
    io::Error::other("Browser storage is unavailable or full. No completed file was saved.")
}
pub async fn call(function: &Function, offset: u64, bytes: &[u8]) -> io::Result<()> {
    let promise = function
        .call2(
            &JsValue::NULL,
            &JsValue::from_f64(offset as f64),
            &Uint8Array::from(bytes),
        )
        .map_err(|_| error())?;
    JsFuture::from(Promise::resolve(&promise))
        .await
        .map_err(|_| error())?;
    Ok(())
}

#[derive(Clone)]
pub struct Reader {
    pub read: Function,
    pub len: u64,
    start: u64,
    cache: Bytes,
}
impl Reader {
    pub fn new(read: Function, len: u64) -> Self {
        Self {
            read,
            len,
            start: 0,
            cache: Bytes::new(),
        }
    }
}
impl AsyncSliceReader for Reader {
    async fn read_at(&mut self, offset: u64, len: usize) -> io::Result<Bytes> {
        // Convert only the bounded read length; WASM usize is 32-bit even for >4 GiB files.
        let len = (len as u64).min(self.len.saturating_sub(offset)) as usize;
        if len > WINDOW {
            return Err(error());
        }
        if offset >= self.start && offset + len as u64 <= self.start + self.cache.len() as u64 {
            return Ok(self
                .cache
                .slice((offset - self.start) as usize..(offset - self.start) as usize + len));
        }
        let amount = (WINDOW as u64).min(self.len.saturating_sub(offset));
        let promise = self
            .read
            .call2(
                &JsValue::NULL,
                &JsValue::from_f64(offset as f64),
                &JsValue::from_f64(amount as f64),
            )
            .map_err(|_| error())?;
        let value = JsFuture::from(Promise::resolve(&promise))
            .await
            .map_err(|_| error())?;
        let data = Uint8Array::new(&value);
        if data.length() as u64 != amount {
            return Err(error());
        }
        self.cache = Bytes::from(data.to_vec());
        self.start = offset;
        Ok(self.cache.slice(..len))
    }
    async fn size(&mut self) -> io::Result<u64> {
        Ok(self.len)
    }
}

pub struct Writer {
    pub write: Function,
    buffer: Vec<u8>,
    start: u64,
}
impl Writer {
    pub fn new(write: Function) -> Self {
        Self {
            write,
            buffer: Vec::with_capacity(WINDOW),
            start: 0,
        }
    }
    async fn flush(&mut self) -> io::Result<()> {
        if !self.buffer.is_empty() {
            call(&self.write, self.start, &self.buffer).await?;
            self.start += self.buffer.len() as u64;
            self.buffer.clear();
        }
        Ok(())
    }
}
impl AsyncSliceWriter for Writer {
    async fn write_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()> {
        if offset != self.start + self.buffer.len() as u64
            || self.buffer.len() + data.len() > WINDOW
        {
            self.flush().await?;
            self.start = offset;
        }
        if data.len() > WINDOW {
            return call(&self.write, offset, data).await;
        }
        self.buffer.extend_from_slice(data);
        Ok(())
    }
    async fn write_bytes_at(&mut self, offset: u64, data: Bytes) -> io::Result<()> {
        self.write_at(offset, &data).await
    }
    async fn set_len(&mut self, _len: u64) -> io::Result<()> {
        Ok(())
    }
    async fn sync(&mut self) -> io::Result<()> {
        self.flush().await
    }
}

#[derive(Clone)]
pub enum Blob {
    Memory {
        data: Bytes,
        outboard: PreOrderMemOutboard,
    },
    File {
        data: Reader,
        outboard: PostOrderOutboard<Reader>,
    },
}
impl Blob {
    pub fn memory(data: Bytes) -> Self {
        let outboard = PreOrderMemOutboard::create(&data, iroh_blobs::store::IROH_BLOCK_SIZE);
        Self::Memory { data, outboard }
    }
    pub fn size(&self) -> u64 {
        match self {
            Self::Memory { data, .. } => data.len() as u64,
            Self::File { data, .. } => data.len,
        }
    }
}
pub type Blobs = Rc<RefCell<Vec<Blob>>>;
pub async fn prepare(
    read: Function,
    size: u64,
    out_read: Function,
    out_write: Function,
) -> io::Result<(Hash, Blob)> {
    let data = Reader::new(read, size);
    let tree = BaoTree::new(size, iroh_blobs::store::IROH_BLOCK_SIZE);
    let mut outboard = PostOrderOutboard {
        tree,
        root: bao_tree::blake3::hash(&[]),
        data: Writer::new(out_write),
    };
    outboard.init_from(io::Cursor::new(data.clone())).await?;
    let hash = outboard.root;
    let out_len = tree.outboard_size();
    Ok((
        hash.into(),
        Blob::File {
            data,
            outboard: PostOrderOutboard {
                tree,
                root: hash,
                data: Reader::new(out_read, out_len),
            },
        },
    ))
}

struct Job {
    pair: StreamPair,
    request: iroh_blobs::protocol::GetRequest,
    _permit: tokio::sync::OwnedSemaphorePermit,
}
#[derive(Debug, Clone)]
pub struct Provider {
    tx: tokio::sync::mpsc::Sender<Job>,
    gate: Arc<Mutex<super::Gate>>,
    permits: Arc<tokio::sync::Semaphore>,
    connections: Arc<tokio::sync::Semaphore>,
}
impl Provider {
    pub fn new(gate: Arc<Mutex<super::Gate>>, blobs: Blobs) -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Job>(2);
        let state = gate.clone();
        wasm_bindgen_futures::spawn_local(async move {
            while let Some(job) = rx.recv().await {
                let blobs = blobs.clone();
                let gate = state.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let body = job
                        .request
                        .ranges
                        .iter_infinite()
                        .skip(2)
                        .take(blobs.borrow().len().saturating_sub(2))
                        .any(|r| !r.is_empty() && *r != ChunkRanges::last_chunk());
                    if body {
                        gate.lock().unwrap().phase = "transferring".into();
                    }
                    let result = serve(job, blobs, gate.clone()).await;
                    if body {
                        gate.lock().unwrap().phase = if result.is_ok() {
                            "done"
                        } else {
                            "interrupted"
                        }
                        .into();
                    }
                });
            }
        });
        Self {
            tx,
            gate,
            permits: Arc::new(tokio::sync::Semaphore::new(2)),
            connections: Arc::new(tokio::sync::Semaphore::new(8)),
        }
    }
}
impl ProtocolHandler for Provider {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let Ok(_connection) = self.connections.clone().try_acquire_owned() else {
            conn.close(1u32.into(), b"busy");
            return Ok(());
        };
        loop {
            let mut pair = match StreamPair::accept(&conn, EventSender::default()).await {
                Ok(p) => p,
                Err(_) => return Ok(()),
            };
            let request = match n0_future::time::timeout(
                std::time::Duration::from_secs(15),
                pair.read_request(),
            )
            .await
            {
                Ok(Ok(Request::Get(r))) => r,
                _ => {
                    conn.close(1u32.into(), b"invalid request");
                    return Ok(());
                }
            };
            // Idle connections must not retain the bounded work queue's permits.
            let Ok(permit) = self.permits.clone().try_acquire_owned() else {
                conn.close(1u32.into(), b"busy");
                return Ok(());
            };
            let allowed = {
                self.gate
                    .lock()
                    .unwrap()
                    .allow(request.hash, conn.remote_id())
            };
            if !allowed {
                conn.close(1u32.into(), b"permission denied");
                return Ok(());
            }
            if self
                .tx
                .send(Job {
                    pair,
                    request,
                    _permit: permit,
                })
                .await
                .is_err()
            {
                return Ok(());
            }
        }
    }
}
async fn serve(job: Job, blobs: Blobs, gate: Arc<Mutex<super::Gate>>) -> anyhow::Result<()> {
    let tracker = job.pair.get_request(|| job.request.clone()).await?;
    let mut writer = job.pair.into_writer(tracker).await?.inner;
    let count = blobs.borrow().len();
    let mut total = 0;
    for (index, ranges) in job.request.ranges.iter_non_empty_infinite() {
        if index as usize >= count {
            break;
        }
        if gate.lock().unwrap().closed {
            anyhow::bail!("cancelled");
        }
        let blob = blobs.borrow()[index as usize].clone();
        writer.write_all(&blob.size().to_le_bytes()).await?;
        match blob {
            Blob::Memory { data, outboard } => {
                fsm::encode_ranges_validated(
                    data,
                    outboard,
                    ranges,
                    &mut NetworkWriter(&mut writer),
                )
                .await?
            }
            Blob::File { data, outboard } => {
                if *ranges != ChunkRanges::last_chunk() {
                    total += data.len;
                    gate.lock().unwrap().total = total;
                }
                fsm::encode_ranges_validated(
                    data,
                    outboard,
                    ranges,
                    &mut NetworkWriter(&mut writer),
                )
                .await?;
            }
        }
    }
    writer.finish()?;
    if writer.stopped().await?.is_some() {
        anyhow::bail!("recipient stopped receiving");
    }
    gate.lock().unwrap().bytes = total;
    Ok(())
}
struct NetworkWriter<'a>(&'a mut iroh::endpoint::SendStream);
impl iroh_io::AsyncStreamWriter for NetworkWriter<'_> {
    async fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.0.write_all(data).await.map_err(io::Error::other)
    }
    async fn write_bytes(&mut self, data: Bytes) -> io::Result<()> {
        self.0.write_chunk(data).await.map_err(io::Error::other)
    }
    async fn sync(&mut self) -> io::Result<()> {
        Ok(())
    }
}

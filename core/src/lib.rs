//! # irohcore — the Dropwire transfer engine
//!
//! This is the **only** crate in Dropwire that depends on `iroh` / `iroh-blobs`.
//! Everything above this boundary (the Tauri shell, the UI) speaks the types in
//! this crate — [`Core`], [`Progress`], [`CoreConfig`] — and never touches an
//! iroh-blobs type directly. That containment is deliberate: iroh-blobs is
//! pre-1.0 and mid-rewrite, so when it changes we fix one crate, not six apps
//! (see `ARCHITECTURE.md` §4).
//!
//! ## Shape
//! - [`Core::start`] builds one long-lived endpoint + blob store + serving router.
//! - [`Core::send`] imports a path and hands back a shareable ticket.
//! - [`Core::receive`] downloads a ticket's content, resuming if interrupted.
//! - Both return a [`ProgressStream`] of [`Progress`] events.

mod catalog;
mod config;
mod endpoint;
mod error;
mod identity;
mod progress;
mod receive;
mod send;
mod store;

use std::collections::HashMap;
use std::sync::Arc;

use iroh::protocol::Router;
use iroh_blobs::provider::events::{
    ConnectMode, EventMask, EventSender, ProviderMessage, RequestMode,
};
use iroh_blobs::store::fs::FsStore;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

pub use catalog::{Status, TransferRecord};
pub use config::{CoreConfig, Infra};
pub use error::{CoreError, Result};
pub use progress::{
    Direction, FilePreview, Progress, ProgressStream, Route, TransferId, TransferPreview,
    TransferStats,
};

use catalog::Catalog;

/// A handle to the running Dropwire engine. Cheap to clone (internally `Arc`).
#[derive(Clone)]
pub struct Core {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub(crate) store: FsStore,
    pub(crate) router: Router,
    #[allow(dead_code)] // retained for future use (LAN mode, reconfig)
    pub(crate) config: CoreConfig,
    pub(crate) catalog: Mutex<Catalog>,
    pub(crate) active: Mutex<HashMap<TransferId, CancellationToken>>,
    /// In-flight sends, keyed by content hash (hex), so provider events can be
    /// routed to the right transfer's progress stream.
    pub(crate) serving: Mutex<HashMap<String, mpsc::UnboundedSender<send::ProviderEvent>>>,
    /// Live connections: `connection_id` → the peer's `EndpointId`. Populated from
    /// provider connect events so a get request can be attributed to a device.
    pub(crate) conns: Mutex<HashMap<u64, iroh::EndpointId>>,
    /// One-to-one binding: content hash (hex) → the first approved receiver's
    /// `EndpointId`. The ticket is served to that one device; others are denied.
    pub(crate) bound: Mutex<HashMap<String, iroh::EndpointId>>,
}

impl Core {
    /// Start the engine: load/create identity, bind the endpoint, open the blob
    /// store, and spin up the always-on serving router.
    pub async fn start(config: CoreConfig) -> Result<Core> {
        std::fs::create_dir_all(&config.data_dir)?;

        let secret = identity::load_or_create(&config.data_dir.join("node.key"))?;
        let endpoint = endpoint::build(secret, &config.infra).await?;
        let store = store::open(&config.data_dir.join("blobs")).await?;

        // One always-on blobs server with provider events: any blob in the store is
        // served by hash, and the global event stream lets us surface sender-side
        // progress (peer connected → bytes sent → done) per transfer.
        let (ev_tx, ev_rx) = mpsc::channel::<ProviderMessage>(64);
        let events = EventSender::new(
            ev_tx,
            EventMask {
                connected: ConnectMode::Notify,
                // InterceptLog = we can allow/deny each request before bytes flow
                // (one-to-one enforcement) AND still get per-request progress.
                get: RequestMode::InterceptLog,
                ..EventMask::DEFAULT
            },
        );
        let blobs = iroh_blobs::BlobsProtocol::new(&store, Some(events));
        let router = Router::builder(endpoint)
            .accept(store::BLOBS_ALPN, blobs)
            .spawn();

        let mut catalog = Catalog::load(config.data_dir.join("transfers.json"));
        catalog.mark_stale_interrupted();

        let inner = Arc::new(Inner {
            store,
            router,
            config,
            catalog: Mutex::new(catalog),
            active: Mutex::new(HashMap::new()),
            serving: Mutex::new(HashMap::new()),
            conns: Mutex::new(HashMap::new()),
            bound: Mutex::new(HashMap::new()),
        });
        let core = Core { inner };
        tokio::spawn(send::consume_provider_events(core.clone(), ev_rx));
        Ok(core)
    }

    /// This device's stable public identity (`EndpointId`), as a string.
    pub fn endpoint_id(&self) -> String {
        self.inner.router.endpoint().id().to_string()
    }

    /// Cancel an in-flight transfer (no-op if it already finished).
    pub async fn cancel(&self, id: TransferId) {
        if let Some(tok) = self.inner.active.lock().await.get(&id) {
            tok.cancel();
        }
    }

    /// The local transfer history (newest first).
    pub async fn transfers(&self) -> Vec<TransferRecord> {
        self.inner.catalog.lock().await.list()
    }

    /// Gracefully shut down the engine.
    pub async fn shutdown(self) -> Result<()> {
        let _ = self.inner.router.shutdown().await;
        // VERIFY (ARCHITECTURE.md §13): FsStore::shutdown() shape on 0.103.
        let _ = self.inner.store.shutdown().await;
        Ok(())
    }
}

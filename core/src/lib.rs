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
mod control;
mod discover;
mod endpoint;
mod error;
mod export;
mod fail;
mod identity;
mod offer;
mod progress;
mod receive;
mod send;
mod store;

use std::collections::HashMap;
use std::sync::Arc;

use iroh::protocol::Router;
use iroh_blobs::provider::events::{
    ConnectMode, EventMask, EventSender, ObserveMode, ProviderMessage, RequestMode,
};
use iroh_blobs::store::fs::FsStore;
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio_util::sync::CancellationToken;

pub use catalog::{Status, TransferRecord};
pub use config::{CoreConfig, Infra};
pub use control::CtrlMsg;
pub use discover::NearbyDevice;
pub use error::{CoreError, Result};
pub use offer::{IncomingOffer, OfferUpdate, OfferWithdrawn, WithdrawReason};
pub use progress::RenamedFile;
pub use progress::{
    Direction, ErrorCode, FilePreview, Progress, ProgressStream, Route, TransferId,
    TransferPreview, TransferStats,
};
#[cfg(feature = "test-utils")]
pub use store::set_gc_interval_for_tests;

use catalog::Catalog;
use discover::NearbyState;
use offer::ConsentCtx;

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
    /// Live sends, keyed by content hash (hex). This is the allow-list of the
    /// one-to-one gate: only these roots are served. Also routes provider
    /// events to the right transfer's progress stream.
    pub(crate) serving: Mutex<HashMap<String, send::Serving>>,
    /// Live connections: `connection_id` → the peer's `EndpointId`. Populated from
    /// provider connect events so a get request can be attributed to a device.
    pub(crate) conns: Mutex<HashMap<u64, iroh::EndpointId>>,
    /// One-to-one binding: content hash (hex) → the first approved receiver's
    /// `EndpointId`. The ticket is served to that one device; others are denied.
    /// Lock order: `serving` before `bound` wherever both are held.
    pub(crate) bound: Mutex<HashMap<String, iroh::EndpointId>>,
    /// Broadcast of control messages received from peers (see [`control`]).
    pub(crate) ctrl_tx: broadcast::Sender<control::CtrlMsg>,
    /// Nearby (mDNS) discovery session: advertisement + live peer table.
    pub(crate) nearby: Mutex<NearbyState>,
    /// UDP port our mDNS announcement points at (the engine's QUIC socket).
    pub(crate) nearby_port: u16,
    /// Two-sided consent state for nearby transfers (see [`offer`]).
    pub(crate) consent: ConsentCtx,
    /// Offers this device sent that have no answer yet, by offer id.
    pub(crate) outgoing_offers: std::sync::Mutex<HashMap<String, offer::OutgoingOffer>>,
}

impl Core {
    /// Start the engine: load/create identity, bind the endpoint, open the blob
    /// store, and spin up the always-on serving router.
    pub async fn start(config: CoreConfig) -> Result<Core> {
        std::fs::create_dir_all(&config.data_dir)?;

        let secret = identity::load_or_create(&config.data_dir.join("node.key"))?;
        let endpoint = endpoint::build(secret, &config.infra).await?;
        let store = store::open(&config.data_dir.join("blobs")).await?;

        // One always-on blobs server with provider events. Every request passes
        // through the one-to-one gate in `send::consume_provider_events`, which
        // serves only the root of a live send to its bound device; the global
        // event stream also surfaces sender-side progress per transfer.
        let (ev_tx, ev_rx) = mpsc::channel::<ProviderMessage>(64);
        let events = EventSender::new(
            ev_tx,
            EventMask {
                connected: ConnectMode::Notify,
                // InterceptLog = we can allow/deny each request before bytes flow
                // (one-to-one enforcement) AND still get per-request progress.
                get: RequestMode::InterceptLog,
                // Never used by Dropwire. iroh-blobs 0.103 routes these through
                // the `get` mode above (and the gate refuses them); these
                // settings keep them shut if a later version honors them.
                get_many: RequestMode::Disabled,
                push: RequestMode::Disabled,
                observe: ObserveMode::Intercept,
                ..EventMask::DEFAULT
            },
        );
        let blobs = iroh_blobs::BlobsProtocol::new(&store, Some(events));

        // Control plane (presence/chat + nearby consent frames).
        let (ctrl_tx, _) = broadcast::channel(64);
        let (offer_tx, _) = broadcast::channel(64);
        let (withdrawn_tx, _) = broadcast::channel(64);
        let (decline_tx, decline_rx) = mpsc::unbounded_channel();

        // Nearby discovery session. The mDNS SRV record points at this
        // endpoint's real QUIC port so peers can dial straight over the LAN.
        let self_eid = endpoint.id().to_string();
        let nearby = NearbyState::new(self_eid, discover::default_device_name());
        let nearby_port = endpoint
            .bound_sockets()
            .iter()
            .find_map(|s| match s {
                std::net::SocketAddr::V4(v4) => Some(v4.port()),
                _ => None,
            })
            .unwrap_or(0);

        let consent = offer::ConsentCtx {
            ctrl_tx: ctrl_tx.clone(),
            offer_tx,
            incoming_offers: Arc::new(std::sync::Mutex::new(HashMap::new())),
            nearby_running: nearby.running_flag(),
            nearby_peers: nearby.peer_table(),
            verdict_waiters: Arc::new(std::sync::Mutex::new(HashMap::new())),
            decline_tx,
            withdrawn_tx,
        };

        let router = Router::builder(endpoint)
            .accept(store::BLOBS_ALPN, blobs)
            .accept(
                control::CTRL_ALPN,
                control::Ctrl {
                    core_ctx: consent.clone(),
                },
            )
            .spawn();

        let mut catalog = Catalog::load(config.data_dir.join("transfers.json"));
        catalog.mark_stale_interrupted();
        store::reconcile_receive_tags(&store, &catalog.list()).await;

        let inner = Arc::new(Inner {
            store,
            router,
            config,
            catalog: Mutex::new(catalog),
            active: Mutex::new(HashMap::new()),
            serving: Mutex::new(HashMap::new()),
            conns: Mutex::new(HashMap::new()),
            bound: Mutex::new(HashMap::new()),
            ctrl_tx,
            nearby: Mutex::new(nearby),
            nearby_port,
            consent,
            outgoing_offers: std::sync::Mutex::new(HashMap::new()),
        });
        let core = Core { inner };
        tokio::spawn(send::consume_provider_events(core.clone(), ev_rx));
        tokio::spawn(send::consume_declines(core.clone(), decline_rx));
        Ok(core)
    }

    /// This device's stable public identity (`EndpointId`), as a string.
    pub fn endpoint_id(&self) -> String {
        self.inner.router.endpoint().id().to_string()
    }

    /// The short human-checkable fingerprint of THIS device (for pairing UIs).
    pub fn fingerprint(&self) -> String {
        discover::NearbyDevice::fingerprint_for(&self.endpoint_id())
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

    /// Clear finished history. Records live only on this device, so this is the
    /// whole delete story: there is nothing on a server to remove as well.
    pub async fn clear_transfers(&self) {
        let removed = self.inner.catalog.lock().await.clear_finished();
        for id in removed {
            store::release_receive(&self.inner.store, id).await;
        }
    }

    /// Gracefully shut down the engine.
    pub async fn shutdown(self) -> Result<()> {
        self.stop_nearby().await;
        let _ = self.inner.router.shutdown().await;
        // VERIFY (ARCHITECTURE.md §13): FsStore::shutdown() shape on 0.103.
        let _ = self.inner.store.shutdown().await;
        Ok(())
    }
}

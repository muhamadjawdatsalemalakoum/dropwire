//! Two-sided consent for nearby transfers: offers, accept/decline, handoff.
//!
//! Flow (mirrors Bluetooth pairing etiquette):
//!
//! ```text
//!   sender                                receiver
//!   ──────                                ────────
//!   offer_nearby(eid) ── Offer{ticket} ─► ► IncomingOffer event (UI modal)
//!   Waiting…           ◄══ echo verdict ══ respond_offer(accept|decline)
//!   Accepted → receiver downloads via the normal blobs path (the sender's
//!   one-to-one gate was bound to this neighbor at offer time).
//! ```
//!
//! The verdict returns on the sender's *own* outgoing connection (the control
//! handler echoes the frame back), so no dial-back address is ever needed. The
//! ticket inside the offer commits to the manifest (names/sizes/hashes) via
//! BLAKE3, so what the receiver confirms is exactly what arrives.
//!
//! Sender rules: the caller names the send; a send already going to another
//! device is never offered; one offer per send at a time. An offer that is not
//! taken (declined, withdrawn, unreachable) leaves the send and its code
//! running and only releases the binding it made; a device that may hold the
//! code from the offer is refused by the gate from then on. Ending the send,
//! or [`Core::cancel_offer`], takes the offer back and closes its connection.
//!
//! Receiver rules: an offer is shown only while Nearby is on, from a device
//! seen on the local network, whose code names that same device; one waiting
//! offer per device, a few in all. One that ends unanswered (taken back,
//! expired, replaced) is reported on [`Core::subscribe_offer_withdrawals`].

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use iroh::endpoint::{Connection, VarInt};
use iroh::{Endpoint, EndpointAddr, EndpointId, TransportAddr};
use iroh_blobs::ticket::BlobTicket;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::catalog::Status;
use crate::control::CTRL_ALPN;
use crate::discover::{parse_eid, NearbyDevice, PeerTable};
use crate::error::{CoreError, Result};
use crate::progress::{Direction, TransferId};
use crate::{Core, CtrlMsg};

/// How long the sender waits for the receiver's answer before giving up.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(120);
/// Connect timeout for both sides of the consent handshake.
const CONSENT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Offers waiting for an answer here, at most, from all devices together.
/// Each device has at most one (a newer one replaces it). More are declined
/// unseen, so no one can stack up dialogs.
const MAX_PENDING_OFFERS: usize = 4;
/// Longest sender-written labels shown in the dialog, in characters.
const MAX_DEVICE_NAME: usize = 64;
const MAX_TITLE: usize = 200;

/// Why an offer was refused before it went out. Shown to the sender as-is.
const SEND_ENDED: &str = "This send has ended. Start a new one to offer it.";
const SEND_TAKEN: &str =
    "This send is already going to another device. Start a new send to offer it to someone else.";
const OFFER_PENDING: &str =
    "This send is already offered and waiting for an answer. Cancel that offer first.";

/// Why an answer to an incoming offer went nowhere. Shown as-is.
const OFFER_GONE: &str = "This offer is no longer open. They may have cancelled it, or it expired.";

/// QUIC close code on an offer's connection when the sender takes it back.
const WITHDRAWN_CODE: u32 = 1;

/// Anything exchangeable over the control channel: the original presence
/// frames plus the nearby-consent trio. One enum keeps parsing single-sourced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum Frame {
    // ---- presence / chat (public [`CtrlMsg`] vocabulary) ----
    Hello,
    Ack,
    /// Receiver to sender: no thanks, from the preview. `hash` names the code
    /// being declined (hex content hash). Older peers send a bare
    /// `{"kind":"decline"}`, which still parses (as `None`); older senders
    /// ignore the extra field.
    Decline {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
    },
    Chat {
        text: String,
    },
    // ---- nearby consent ----
    /// Sender → receiver: "may I send you this?"
    ///
    /// No fingerprint travels in the frame: the receiver derives it from the
    /// TLS-authenticated remote id, so a sender cannot claim someone else's
    /// pairing code. (`device_name`/`title`/counts remain sender-authored hints
    /// — the receiver's verified preview, not these fields, gates the download.)
    /// The ticket must name the sender itself (the authenticated id); an offer
    /// whose code points at any other device is declined unseen.
    Offer {
        offer_id: String,
        ticket: String,
        device_name: String,
        title: String,
        file_count: usize,
        total_bytes: u64,
    },
    /// Receiver → sender on the offer's own connection: yes.
    OfferAccept {
        offer_id: String,
    },
    /// Receiver → sender on the offer's own connection: no. `unseen` when the
    /// receiving engine turned it down without showing it to anyone (Nearby
    /// off, the sender not seen nearby, too many offers waiting): that device
    /// never held the code. Older peers send no `unseen` (read as false).
    OfferDecline {
        offer_id: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        unseen: bool,
    },
}

impl From<&CtrlMsg> for Frame {
    fn from(m: &CtrlMsg) -> Self {
        match m {
            CtrlMsg::Hello => Frame::Hello,
            CtrlMsg::Ack => Frame::Ack,
            CtrlMsg::Decline => Frame::Decline { hash: None },
            CtrlMsg::Chat { text } => Frame::Chat { text: text.clone() },
        }
    }
}

/// An offer received from a nearby device (surfaced to the UI for consent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingOffer {
    pub offer_id: String,
    /// Hex endpoint id of the offering device (authenticated by the handshake).
    pub from_endpoint_id: String,
    /// Sender-authored display name (a claim — pair it with the fingerprint).
    pub device_name: String,
    /// Human-checkable fingerprint of the sender's identity.
    pub fingerprint: String,
    /// The transfer ticket — held until acceptance, then handed to receive.
    pub ticket: String,
    /// Transfer name (file/folder name).
    pub title: String,
    pub file_count: usize,
    pub total_bytes: u64,
    /// How the offer reached us (LAN socket / relay) — the natural route for
    /// our answer. Filled by the engine from the incoming connection; not
    /// authored by the sender.
    #[serde(default)]
    pub(crate) reply_transport: Option<EndpointReplyTransport>,
}

/// Serializable stand-in for the transport an offer arrived on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndpointReplyTransport {
    /// Direct IP socket (the normal LAN case).
    Ip(std::net::SocketAddr),
    /// Via a relay URL.
    Relay(String),
}

/// Status updates for an outgoing offer, streamed back to the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OfferUpdate {
    /// Delivered; the other side hasn't answered yet.
    Waiting,
    /// They accepted — the transfer can proceed.
    Accepted,
    /// They declined. `unseen`: their device turned it down without showing
    /// it (Nearby off there, this device not seen on their network, or too
    /// many offers waiting), so no one there saw the offer or its code.
    Declined {
        #[serde(default)]
        unseen: bool,
    },
    /// Couldn't deliver / timed out / they went away.
    Failed { reason: String },
    /// This device took the offer back ([`Core::cancel_offer`], or its send
    /// ended) before they answered. Their dialog closes.
    Withdrawn,
}

/// An offer that was showing (or waiting to show) on this device ended
/// without an answer from the user here, so its dialog should close.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfferWithdrawn {
    /// The [`IncomingOffer::offer_id`] it was surfaced with.
    pub offer_id: String,
    pub reason: WithdrawReason,
}

/// Why an incoming offer ended without an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WithdrawReason {
    /// The sender took it back, cancelled its send, or went away.
    Cancelled,
    /// Nobody answered in time.
    Expired,
    /// The same device sent a newer offer, which takes its place.
    Replaced,
}

/// An offer this device sent that has no answer yet.
pub(crate) struct OutgoingOffer {
    /// The send it offers.
    pub(crate) transfer: TransferId,
    /// Fired to take the offer back: by [`Core::cancel_offer`], or when its
    /// send ends (it is a child of the send's token).
    pub(crate) token: CancellationToken,
}

/// The slice of engine state the control-ALPN handler needs to route frames.
/// Kept small (Arc'd pieces only) so it can be built before the router exists.
#[derive(Clone, Debug)]
pub(crate) struct ConsentCtx {
    pub(crate) ctrl_tx: broadcast::Sender<CtrlMsg>,
    pub(crate) offer_tx: broadcast::Sender<IncomingOffer>,
    pub(crate) incoming_offers: Arc<StdMutex<HashMap<String, IncomingOffer>>>,
    /// Shared "nearby sharing is ON" flag. When it is off, incoming offers are
    /// declined without ever reaching the user — the UI promise that the device
    /// is "invisible" while off must actually hold at the consent layer, not
    /// just for mDNS advertising.
    pub(crate) nearby_running: Arc<std::sync::atomic::AtomicBool>,
    /// The live mDNS peer table: devices this one currently sees on the local
    /// network. Only they can raise an offer dialog here.
    pub(crate) nearby_peers: PeerTable,
    /// Verdict wait-list: offer_id → a oneshot the UI's answer is sent down.
    /// The control handler parks the sender's offer connection here until the
    /// local user responds, then the verdict travels back in-band.
    pub(crate) verdict_waiters: Arc<StdMutex<HashMap<String, mpsc::UnboundedSender<Frame>>>>,
    /// Declines received from peers: (authenticated sender of the frame, the
    /// code's hash if it named one). The engine acts on them in
    /// `send::consume_declines`, which can reach the serving state.
    pub(crate) decline_tx: mpsc::UnboundedSender<(EndpointId, Option<String>)>,
    /// Surfaced offers that ended without the local user's answer.
    pub(crate) withdrawn_tx: broadcast::Sender<OfferWithdrawn>,
}

impl ConsentCtx {
    /// Park `tx` as the answer channel for `offer_id`. False, and nothing
    /// changes, if that id is already waiting.
    pub(crate) fn add_verdict_waiter(
        &self,
        offer_id: &str,
        tx: mpsc::UnboundedSender<Frame>,
    ) -> bool {
        use std::collections::hash_map::Entry;
        match self
            .verdict_waiters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(offer_id.to_string())
        {
            Entry::Occupied(_) => false,
            Entry::Vacant(slot) => {
                slot.insert(tx);
                true
            }
        }
    }

    /// Deliver the local user's verdict to the parked connection, if any.
    pub(crate) fn resolve_verdict(&self, offer_id: &str, frame: Frame) -> bool {
        let waiter = self
            .verdict_waiters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(offer_id);
        match waiter {
            Some(tx) => tx.send(frame).is_ok(),
            None => false,
        }
    }

    /// Forget an offer that ended without the local user's answer: the sender
    /// withdrew it (or went away), or nobody answered in time. Clears both
    /// maps so the pending-offer list can't grow without bound and a late
    /// `respond_offer` finds nothing to accept (reported as ended). If it
    /// was still waiting for the user, the UI is told, to close its dialog.
    pub(crate) fn retire_offer(&self, offer_id: &str, reason: WithdrawReason) {
        self.verdict_waiters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(offer_id);
        let pending = self
            .incoming_offers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(offer_id)
            .is_some();
        if pending {
            let _ = self.withdrawn_tx.send(OfferWithdrawn {
                offer_id: offer_id.to_string(),
                reason,
            });
        }
    }
}

impl Core {
    /// Start advertising + browsing on the local network. Idempotent.
    pub async fn start_nearby(&self) -> Result<()> {
        let mut state = self.inner.nearby.lock().await;
        if state.is_running() {
            return Ok(()); // already running
        }
        state.start(self.inner.nearby_port)?;
        tracing::info!(eid = %self.endpoint_id(), "nearby discovery started");
        Ok(())
    }

    /// Stop advertising + browsing (peers see us leave within their TTL).
    pub async fn stop_nearby(&self) {
        self.inner.nearby.lock().await.stop();
    }

    /// Live snapshot of nearby Dropwire devices (excluding ourselves).
    pub async fn nearby_devices(&self) -> Vec<NearbyDevice> {
        self.inner.nearby.lock().await.list()
    }

    /// This device's display name as advertised to others.
    pub async fn device_name(&self) -> String {
        self.inner.nearby.lock().await.device_name.clone()
    }

    /// Change the name nearby devices see. The name is tidied first (control
    /// characters and invisible marks removed, spaces collapsed, trimmed; the
    /// result is what [`Self::device_name`] returns). An empty name, or one
    /// over 40 characters, is refused with a message to show as it is, and
    /// nothing changes. Takes effect immediately: a live advertisement is
    /// re-registered under the new name, or kept as it was if that fails.
    pub async fn set_device_name(&self, name: String) -> Result<()> {
        let port = self.inner.nearby_port;
        self.inner.nearby.lock().await.rename(&name, port)
    }

    /// Subscribe to offers arriving from nearby devices.
    pub fn subscribe_offers(&self) -> broadcast::Receiver<IncomingOffer> {
        self.inner.consent.offer_tx.subscribe()
    }

    /// Subscribe to incoming offers that ended before the user here answered
    /// them (the sender took one back, or it expired), so their dialogs can
    /// close.
    pub fn subscribe_offer_withdrawals(&self) -> broadcast::Receiver<OfferWithdrawn> {
        self.inner.consent.withdrawn_tx.subscribe()
    }

    /// Take back an offer this device sent (by the id [`Self::offer_nearby`]
    /// returned) before it is answered. The other device's dialog closes, the
    /// offer ends as [`OfferUpdate::Withdrawn`], and the send goes on. An
    /// unknown or finished offer is ignored.
    pub fn cancel_offer(&self, offer_id: &str) {
        let outgoing = self
            .inner
            .outgoing_offers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(offer) = outgoing.get(offer_id) {
            offer.token.cancel();
        }
    }

    /// Offer the send `id` (one the caller started, now sharing its code) to
    /// the nearby device `eid_hex`.
    ///
    /// Returns a stream of [`OfferUpdate`]s ending in Accepted/Declined/Failed.
    pub async fn offer_nearby(
        &self,
        eid_hex: String,
        id: TransferId,
    ) -> Result<(String, ReceiverStream<OfferUpdate>)> {
        self.offer_nearby_dial(eid_hex, id, None).await
    }

    /// Like [`Self::offer_nearby`] with an explicit dial-address hint — used
    /// when the peer's transport was learned out-of-band (BLE bootstrap, or
    /// hermetic tests). Dial priority: mDNS LAN socket → hint → engine lookup.
    pub async fn offer_nearby_dial(
        &self,
        eid_hex: String,
        id: TransferId,
        addr_hint: Option<EndpointAddr>,
    ) -> Result<(String, ReceiverStream<OfferUpdate>)> {
        let peer = parse_eid(&eid_hex)?;

        // The send the caller named, never a guess: several can be live at
        // once. Its code must already be out (`Ready` makes it Active).
        let record = self
            .inner
            .catalog
            .lock()
            .await
            .get(id)
            .ok_or_else(|| CoreError::NotFound(id.to_string()))?;
        if record.direction != Direction::Send {
            return Err(CoreError::Other(anyhow::anyhow!(
                "Only something you are sending can be offered."
            )));
        }
        if record.status != Status::Active || record.ticket.is_empty() {
            return Err(CoreError::Other(anyhow::anyhow!(SEND_ENDED)));
        }

        // Dial every LAN socket announced for this id, plus the hint (else
        // the engine looks the id up). Another host can announce this id with
        // its own address, but the handshake proves which one is the device:
        // a false address can never redirect the offer, and the real one is
        // still tried.
        let lan = {
            let state = self.inner.nearby.lock().await;
            state.peer_sockets(&eid_hex)
        };
        let hint: Vec<TransportAddr> = addr_hint
            .map(|a| a.addrs.into_iter().collect())
            .unwrap_or_default();
        let dial_addr =
            EndpointAddr::from_parts(peer, lan.into_iter().map(TransportAddr::Ip).chain(hint));

        let device_name = self.device_name().await;
        let frame = Frame::Offer {
            offer_id: Uuid::new_v4().to_string(),
            ticket: record.ticket.clone(),
            device_name,
            title: record.name.clone(),
            file_count: record.file_count,
            total_bytes: record.total_bytes,
        };
        let offer_id = match &frame {
            Frame::Offer { offer_id, .. } => offer_id.clone(),
            _ => unreachable!("just built an Offer"),
        };

        // Bind the one-to-one gate to this neighbor NOW so no third party can
        // pull the content between consent and download. A send that already
        // has a receiver (someone used its code, or took an earlier offer) is
        // never handed to a second device: that would take it from the first
        // mid-transfer. Checked and bound under one lock, `serving` first.
        // `already_ours`: this neighbor held the binding before this offer,
        // so the offer's outcome must leave it alone. One offer per send at a
        // time, so an offer's outcome is only ever its own. The offer's token
        // is a child of the send's: ending the send takes the offer back.
        let (already_ours, withdraw) = {
            let mut serving = self.inner.serving.lock().await;
            let Some(entry) = serving.get_mut(&record.hash).filter(|s| s.id == id) else {
                return Err(CoreError::Other(anyhow::anyhow!(SEND_ENDED)));
            };
            let mut bound = self.inner.bound.lock().await;
            let mut outgoing = self
                .inner
                .outgoing_offers
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if outgoing.values().any(|o| o.transfer == id) {
                return Err(CoreError::Other(anyhow::anyhow!(OFFER_PENDING)));
            }
            let already_ours = match bound.get(&record.hash) {
                Some(other) if *other != peer => {
                    return Err(CoreError::Other(anyhow::anyhow!(SEND_TAKEN)));
                }
                Some(_) => true,
                None => {
                    bound.insert(record.hash.clone(), peer);
                    false
                }
            };
            // Offered again after turning it down: the sender chose them anew.
            entry.denied.remove(&peer);
            let withdraw = entry.token.child_token();
            outgoing.insert(
                offer_id.clone(),
                OutgoingOffer {
                    transfer: id,
                    token: withdraw.clone(),
                },
            );
            (already_ours, withdraw)
        };

        let (upd_tx, upd_rx) = mpsc::channel(8);
        let endpoint = self.inner.router.endpoint().clone();
        let core = self.clone();
        let hash_key = record.hash.clone();
        let offer_peer = peer;
        let task_offer_id = offer_id.clone();
        tokio::spawn(async move {
            let _ = upd_tx.send(OfferUpdate::Waiting).await;

            let mut reached = false;
            let mut update =
                match deliver_offer(&endpoint, dial_addr, frame, &withdraw, &mut reached).await {
                    Ok(u) => u,
                    Err(reason) => OfferUpdate::Failed { reason },
                };
            // Taken back while their yes was on its way: this side's word
            // stands, so the send is not reported as accepted after a Cancel.
            if update == OfferUpdate::Accepted && withdraw.is_cancelled() {
                update = OfferUpdate::Withdrawn;
            }

            // Not taken (declined, withdrawn, or it never got an answer): the
            // send goes on, so its code and other offers still work. A
            // neighbor that was already this send's receiver keeps it whatever
            // it says to a repeat offer.
            if !already_ours && update != OfferUpdate::Accepted {
                // Their device may hold the code only if the offer got there
                // and was not turned down unseen.
                let may_hold = reached && !matches!(update, OfferUpdate::Declined { unseen: true });
                release_offer(&core, id, &hash_key, offer_peer, may_hold).await;
            }
            core.inner
                .outgoing_offers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&task_offer_id);
            let _ = upd_tx.send(update).await;
        });

        Ok((offer_id, ReceiverStream::new(upd_rx)))
    }

    /// Respond to an incoming offer (receiver side). The verdict is delivered
    /// in-band on the sender's still-open offer connection (it parks waiting
    /// for exactly this), so no second connection or dial-back is needed.
    pub async fn respond_offer(&self, offer_id: String, accept: bool) -> Result<()> {
        // Gone already: answered, taken back by the sender, or expired.
        let open = self
            .inner
            .consent
            .incoming_offers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&offer_id);
        if !open {
            return Err(CoreError::Other(anyhow::anyhow!(OFFER_GONE)));
        }

        let frame = if accept {
            Frame::OfferAccept {
                offer_id: offer_id.clone(),
            }
        } else {
            Frame::OfferDecline {
                offer_id: offer_id.clone(),
                unseen: false,
            }
        };

        // Wake the parked control-connection task with the verdict.
        let delivered = self.inner.consent.resolve_verdict(&offer_id, frame);

        // Whether or not it was delivered, this offer is now spent — drop it so
        // it can't be answered twice and the map can't leak.
        self.inner
            .consent
            .incoming_offers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&offer_id);

        if !delivered {
            // The offering connection is already gone (it timed out after
            // ANSWER_WAIT, or the sender left). Report that so the UI shows
            // the offer has ended instead of starting a download the sender
            // has already abandoned.
            tracing::warn!(%offer_id, "offer connection already gone");
            return Err(CoreError::Other(anyhow::anyhow!(OFFER_GONE)));
        }
        Ok(())
    }

    /// TEST-ONLY: this endpoint's dial address, so hermetic tests can hand
    /// one core another's address directly (same shape a ticket carries).
    ///
    /// Built from the bound sockets on loopback rather than `endpoint.addr()`:
    /// right after bind the endpoint may not have gathered its interface
    /// addresses yet (slower on machines with many adapters), and with no relay
    /// or discovery in local-only mode an address-less dial just times out.
    #[cfg(feature = "test-utils")]
    pub fn test_dial_addr(&self) -> iroh::EndpointAddr {
        use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
        let endpoint = self.inner.router.endpoint();
        let loopback = endpoint.bound_sockets().into_iter().map(|s| match s {
            SocketAddr::V4(v4) => SocketAddr::from((Ipv4Addr::LOCALHOST, v4.port())),
            SocketAddr::V6(v6) => SocketAddr::from((Ipv6Addr::LOCALHOST, v6.port())),
        });
        EndpointAddr::from_parts(endpoint.id(), loopback.map(TransportAddr::Ip))
    }

    /// TEST-ONLY: make this device see `eid_hex` on the local network, as if
    /// mDNS had found it, so its offers pass the visibility gate. Hermetic
    /// tests have no multicast.
    #[cfg(feature = "test-utils")]
    pub fn test_see_nearby_peer(&self, eid_hex: &str) {
        let seen = crate::discover::Announcement {
            instance: format!("test-{eid_hex}"),
            eid: eid_hex.to_string(),
            name: "Test device".to_string(),
            os: None,
            socks: Vec::new(),
        };
        crate::discover::apply(
            &self.inner.consent.nearby_peers,
            &crate::discover::Change::Seen(seen),
        );
    }

    /// TEST-ONLY: whether this process is browsing the local network for
    /// nearby devices (it should only while some session has sharing on).
    #[cfg(feature = "test-utils")]
    pub fn test_nearby_browsing(&self) -> bool {
        crate::discover::browsing()
    }

    /// TEST-ONLY: flip the "nearby sharing on" flag that gates incoming offers,
    /// without standing up the real mDNS daemon. Production sets this via
    /// [`Core::start_nearby`] / [`Core::stop_nearby`].
    #[cfg(feature = "test-utils")]
    pub fn test_set_nearby_running(&self, on: bool) {
        self.inner
            .consent
            .nearby_running
            .store(on, std::sync::atomic::Ordering::Relaxed);
    }
}

/// An offer of send `id` to `peer` was not taken. Free its code for someone
/// else, and when `peer` may hold the code (it travels inside the offer),
/// refuse `peer` from now on. Both happen under the `serving` lock the gate
/// takes, so the code is never open to `peer` in between. The send itself
/// goes on.
async fn release_offer(core: &Core, id: TransferId, hash: &str, peer: EndpointId, may_hold: bool) {
    let mut serving = core.inner.serving.lock().await;
    // The send ended, or a newer send of the same files took over: not ours.
    let Some(entry) = serving.get_mut(hash).filter(|s| s.id == id) else {
        return;
    };
    // Too late once it was downloaded: the code stays with the device that
    // has the files, as it does when a preview is declined after that.
    if entry.delivered.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    if may_hold {
        entry.denied.insert(peer);
    }
    let mut bound = core.inner.bound.lock().await;
    if bound.get(hash) == Some(&peer) {
        bound.remove(hash);
    }
}

/// Deliver the offer and read the verdict off our own connection (echoed).
/// `reached` is set once a connection to the neighbor is up: from then on it
/// may hold the code the offer carries. When `withdraw` fires first, the
/// connection is closed with [`WITHDRAWN_CODE`], which the other side sees at
/// once and closes its dialog.
async fn deliver_offer(
    endpoint: &Endpoint,
    dial_addr: EndpointAddr,
    frame: Frame,
    withdraw: &CancellationToken,
    reached: &mut bool,
) -> std::result::Result<OfferUpdate, String> {
    let conn = tokio::select! {
        biased;
        _ = withdraw.cancelled() => return Ok(OfferUpdate::Withdrawn),
        conn = tokio::time::timeout(
            CONSENT_CONNECT_TIMEOUT,
            endpoint.connect(dial_addr, CTRL_ALPN),
        ) => conn
            .map_err(|_| "neighbor unreachable".to_string())?
            .map_err(|e| format!("connect failed: {e}"))?,
    };
    *reached = true;

    let answer = tokio::select! {
        biased;
        _ = withdraw.cancelled() => {
            conn.close(VarInt::from_u32(WITHDRAWN_CODE), b"withdrawn");
            return Ok(OfferUpdate::Withdrawn);
        }
        answer = exchange(&conn, &frame) => answer?,
    };

    match serde_json::from_slice::<Frame>(&answer) {
        Ok(Frame::OfferAccept { .. }) => Ok(OfferUpdate::Accepted),
        Ok(Frame::OfferDecline { unseen, .. }) => Ok(OfferUpdate::Declined { unseen }),
        _ => Err("unexpected answer".into()),
    }
}

/// Send the offer frame on a new stream and wait for the echoed verdict.
async fn exchange(conn: &Connection, frame: &Frame) -> std::result::Result<Vec<u8>, String> {
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| format!("stream open failed: {e}"))?;
    let bytes = serde_json::to_vec(frame).map_err(|e| e.to_string())?;
    send.write_all(&bytes).await.map_err(|e| e.to_string())?;
    send.finish().map_err(|e| e.to_string())?;

    tokio::time::timeout(ANSWER_TIMEOUT, recv.read_to_end(64 * 1024))
        .await
        .map_err(|_| "they didn't answer in time".to_string())?
        .map_err(|e| format!("read failed: {e}"))
}

/// How long the receiver's parked offer connection waits for the local user's
/// answer before declining on their behalf. Generous: a human has to read a
/// dialog. The SENDER's wait is [`ANSWER_TIMEOUT`] (slightly longer, so the
/// sender always sees the receiver's self-decline rather than its own).
pub(crate) const ANSWER_WAIT: Duration = Duration::from_secs(115);

/// Surface an inbound OFFER to the UI (validation + visibility gate). The
/// control handler has already parked the connection on the verdict wait-list.
pub(crate) fn route_offer(
    ctx: &ConsentCtx,
    remote: EndpointId,
    via: Option<EndpointReplyTransport>,
    bytes: &[u8],
) {
    let Ok(Frame::Offer {
        offer_id,
        ticket,
        device_name,
        title,
        file_count,
        total_bytes,
        ..
    }) = serde_json::from_slice::<Frame>(bytes)
    else {
        return;
    };

    // Nearby sharing off ⇒ invisible. The control ALPN is always registered (it
    // also carries presence + the receive-by-code decline), so a peer that
    // knows our endpoint id can still open a connection even when the user has
    // turned Nearby off — including over the relay/WAN, since every ticket we
    // ever shared embeds this id. Decline such offers immediately: the user is
    // never shown a dialog (the "invisible while off" promise holds) and the
    // sender gets a prompt "no" instead of a 115s hang.
    let running = ctx
        .nearby_running
        .load(std::sync::atomic::Ordering::Relaxed);
    if !running {
        decline_unseen(ctx, offer_id);
        return;
    }

    // Nearby means nearby: only a device this one currently sees on the local
    // network (by its authenticated id) can raise a dialog. Anyone else who
    // learned our id, from a code we shared or over the relay, is declined
    // unseen, however it names itself.
    let visible = ctx
        .nearby_peers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&remote.to_string());
    if !visible {
        tracing::debug!(%remote, "offer from a device not seen nearby");
        decline_unseen(ctx, offer_id);
        return;
    }

    // Validate the ticket up front so we never surface junk.
    let Ok(parsed) = BlobTicket::from_str(&ticket) else {
        // Unknown ticket shape → decline immediately so the sender isn't left
        // waiting on a dialog that will never appear.
        decline_unseen(ctx, offer_id);
        return;
    };

    // The code must be the sender's own. The dialog shows who offered it (the
    // authenticated `remote` and its fingerprint), and accepting fetches from
    // whoever the code names: a code naming some other device would have the
    // user check one device and download from another. Declined unseen.
    if parsed.addr().id != remote {
        tracing::warn!(%remote, named = %parsed.addr().id, "offer's code names another device");
        decline_unseen(ctx, offer_id);
        return;
    }

    let offer = IncomingOffer {
        reply_transport: via,
        offer_id,
        from_endpoint_id: remote.to_string(),
        device_name: label(&device_name, MAX_DEVICE_NAME),
        // Derive the pairing fingerprint from the TLS-AUTHENTICATED remote id,
        // never from a sender-supplied field: an impostor cannot then present a
        // victim's pairing code. This is the value the user compares aloud.
        fingerprint: NearbyDevice::fingerprint_for(&remote.to_string()),
        ticket: parsed.to_string(),
        title: label(&title, MAX_TITLE),
        file_count,
        total_bytes,
    };

    // One waiting offer per device: a newer one replaces the older (from a
    // sender that restarted, say). And a few at most in all. Checked and
    // stored under one lock.
    let (replaced, room) = {
        let mut pending = ctx
            .incoming_offers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let replaced: Vec<String> = pending
            .values()
            .filter(|o| o.from_endpoint_id == offer.from_endpoint_id)
            .map(|o| o.offer_id.clone())
            .collect();
        for id in &replaced {
            pending.remove(id);
        }
        let room = pending.len() < MAX_PENDING_OFFERS;
        if room {
            pending.insert(offer.offer_id.clone(), offer.clone());
        }
        (replaced, room)
    };
    for offer_id in replaced {
        // It was on screen, so it counts as declined by the user.
        let frame = Frame::OfferDecline {
            offer_id: offer_id.clone(),
            unseen: false,
        };
        ctx.resolve_verdict(&offer_id, frame);
        let _ = ctx.withdrawn_tx.send(OfferWithdrawn {
            offer_id,
            reason: WithdrawReason::Replaced,
        });
    }
    if !room {
        tracing::debug!(%remote, "too many offers waiting; declining");
        decline_unseen(ctx, offer.offer_id);
        return;
    }
    let _ = ctx.offer_tx.send(offer);
}

/// Answer an offer "no" without it ever being shown here.
fn decline_unseen(ctx: &ConsentCtx, offer_id: String) {
    let frame = Frame::OfferDecline {
        offer_id: offer_id.clone(),
        unseen: true,
    };
    ctx.resolve_verdict(&offer_id, frame);
}

/// A sender-written label for the dialog: no control characters, trimmed, and
/// at most `max` characters.
fn label(s: &str, max: usize) -> String {
    let clean: String = s.chars().filter(|c| !c.is_control()).collect();
    clean.trim().chars().take(max).collect()
}

/// Route any other inbound control frame (presence/chat) to the broadcast bus.
/// Verdict frames arriving unsolicited are ignored (the sender learns its
/// verdict in-band on its own outgoing offer connection). `remote` is the
/// TLS-authenticated id of the peer that sent the frame.
pub(crate) fn route_other(ctx: &ConsentCtx, remote: EndpointId, frame: Frame) {
    match frame {
        Frame::Hello => {
            let _ = ctx.ctrl_tx.send(CtrlMsg::Hello);
        }
        Frame::Ack => {
            let _ = ctx.ctrl_tx.send(CtrlMsg::Ack);
        }
        Frame::Decline { hash } => {
            let _ = ctx.decline_tx.send((remote, hash));
            let _ = ctx.ctrl_tx.send(CtrlMsg::Decline);
        }
        Frame::Chat { text } => {
            let _ = ctx.ctrl_tx.send(CtrlMsg::Chat { text });
        }
        Frame::OfferAccept { .. } | Frame::OfferDecline { .. } | Frame::Offer { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decline_frame_stays_wire_compatible() {
        // An older peer's bare decline still parses.
        let old: Frame = serde_json::from_str(r#"{"kind":"decline"}"#).unwrap();
        assert_eq!(old, Frame::Decline { hash: None });

        // The new frame names the code and round-trips.
        let new = Frame::Decline {
            hash: Some("ab12".into()),
        };
        let json = serde_json::to_string(&new).unwrap();
        assert_eq!(json, r#"{"kind":"decline","hash":"ab12"}"#);
        assert_eq!(serde_json::from_str::<Frame>(&json).unwrap(), new);

        // Without a hash it goes out exactly as older builds sent it.
        let bare = serde_json::to_string(&Frame::Decline { hash: None }).unwrap();
        assert_eq!(bare, r#"{"kind":"decline"}"#);

        // An older sender, whose decline had no fields (the same shape as the
        // public CtrlMsg), still reads the new frame.
        assert_eq!(
            serde_json::from_str::<CtrlMsg>(&json).unwrap(),
            CtrlMsg::Decline
        );
    }

    #[test]
    fn offer_decline_stays_wire_compatible() {
        // An older receiver's answer carries no `unseen`: read as seen.
        let old: Frame = serde_json::from_str(r#"{"kind":"offerDecline","offer_id":"o"}"#).unwrap();
        assert_eq!(
            old,
            Frame::OfferDecline {
                offer_id: "o".into(),
                unseen: false
            }
        );
        // A user's "no" goes out exactly as older builds sent it.
        let seen = Frame::OfferDecline {
            offer_id: "o".into(),
            unseen: false,
        };
        assert_eq!(
            serde_json::to_string(&seen).unwrap(),
            r#"{"kind":"offerDecline","offer_id":"o"}"#
        );
        // An unseen one round-trips, and older senders ignore the field.
        let unseen = Frame::OfferDecline {
            offer_id: "o".into(),
            unseen: true,
        };
        let json = serde_json::to_string(&unseen).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"offerDecline","offer_id":"o","unseen":true}"#
        );
        assert_eq!(serde_json::from_str::<Frame>(&json).unwrap(), unseen);
    }
}

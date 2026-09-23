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

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use iroh::{Endpoint, EndpointAddr, EndpointId, TransportAddr};
use iroh_blobs::ticket::BlobTicket;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::catalog::Status;
use crate::control::CTRL_ALPN;
use crate::discover::{parse_eid, NearbyDevice};
use crate::error::{CoreError, Result};
use crate::progress::{Direction, TransferId};
use crate::{Core, CtrlMsg};

/// How long the sender waits for the receiver's answer before giving up.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(120);
/// Connect timeout for both sides of the consent handshake.
const CONSENT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Why an offer was refused before it went out. Shown to the sender as-is.
const SEND_ENDED: &str = "This send has ended. Start a new one to offer it.";
const SEND_TAKEN: &str =
    "This send is already going to another device. Start a new send to offer it to someone else.";

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
    /// Receiver → sender on the offer's own connection: no.
    OfferDecline {
        offer_id: String,
    },
}

impl Frame {
    /// The offer id carried by consent frames (empty for presence frames).
    pub(crate) fn offer_id_str(&self) -> String {
        match self {
            Frame::Offer { offer_id, .. }
            | Frame::OfferAccept { offer_id }
            | Frame::OfferDecline { offer_id } => offer_id.clone(),
            _ => String::new(),
        }
    }
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
    /// They declined.
    Declined,
    /// Couldn't deliver / timed out / they went away.
    Failed { reason: String },
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
    /// Verdict wait-list: offer_id → a oneshot the UI's answer is sent down.
    /// The control handler parks the sender's offer connection here until the
    /// local user responds, then the verdict travels back in-band.
    pub(crate) verdict_waiters: Arc<StdMutex<HashMap<String, mpsc::UnboundedSender<Frame>>>>,
    /// Declines received from peers: (authenticated sender of the frame, the
    /// code's hash if it named one). The engine acts on them in
    /// `send::consume_declines`, which can reach the serving state.
    pub(crate) decline_tx: mpsc::UnboundedSender<(EndpointId, Option<String>)>,
}

impl ConsentCtx {
    /// Park `tx` as the answer channel for `offer_id` (replaces any waiter).
    pub(crate) fn add_verdict_waiter(&self, offer_id: &str, tx: mpsc::UnboundedSender<Frame>) {
        self.verdict_waiters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(offer_id.to_string(), tx);
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

    /// Forget an offer that lapsed unanswered (its parked connection timed out).
    /// Clears both maps so the pending-offer list can't grow without bound and a
    /// late `respond_offer` finds nothing to accept (reported as expired).
    pub(crate) fn expire_offer(&self, offer_id: &str) {
        self.verdict_waiters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(offer_id);
        self.incoming_offers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(offer_id);
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

    /// Change the name nearby devices see. Takes effect immediately: a live
    /// advertisement is re-registered under the new name.
    pub async fn set_device_name(&self, name: String) -> Result<()> {
        let port = self.inner.nearby_port;
        self.inner.nearby.lock().await.rename(name, port)
    }

    /// Subscribe to offers arriving from nearby devices.
    pub fn subscribe_offers(&self) -> broadcast::Receiver<IncomingOffer> {
        self.inner.consent.offer_tx.subscribe()
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

        // Dial priority: mDNS LAN socket → explicit hint → engine lookup.
        let lan = {
            let state = self.inner.nearby.lock().await;
            state.peer_socket(&eid_hex)
        };
        let dial_addr = match lan {
            Some(sock) => EndpointAddr::from_parts(peer, [TransportAddr::Ip(sock)]).with_addrs(
                addr_hint
                    .as_ref()
                    .map(|a| a.addrs.iter().cloned())
                    .unwrap_or_default(),
            ),
            None => match addr_hint {
                Some(a) => EndpointAddr::from_parts(peer, a.addrs.iter().cloned()),
                None => EndpointAddr::from_parts(peer, []),
            },
        };

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
        // so the offer's outcome must leave it alone.
        let already_ours = {
            let mut serving = self.inner.serving.lock().await;
            let Some(entry) = serving.get_mut(&record.hash).filter(|s| s.id == id) else {
                return Err(CoreError::Other(anyhow::anyhow!(SEND_ENDED)));
            };
            let mut bound = self.inner.bound.lock().await;
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
            already_ours
        };

        let (upd_tx, upd_rx) = mpsc::channel(8);
        let endpoint = self.inner.router.endpoint().clone();
        let core = self.clone();
        let hash_key = record.hash.clone();
        let offer_peer = peer;
        tokio::spawn(async move {
            let _ = upd_tx.send(OfferUpdate::Waiting).await;

            let mut reached = false;
            let update = match deliver_offer(&endpoint, dial_addr, frame, &mut reached).await {
                Ok(u) => u,
                Err(reason) => OfferUpdate::Failed { reason },
            };

            // Not taken (declined, or it never got an answer): the send goes
            // on, so its code and other offers still work. A neighbor that was
            // already this send's receiver keeps it whatever it says to a
            // repeat offer.
            if !already_ours && update != OfferUpdate::Accepted {
                release_offer(&core, id, &hash_key, offer_peer, reached).await;
            }
            let _ = upd_tx.send(update).await;
        });

        Ok((offer_id, ReceiverStream::new(upd_rx)))
    }

    /// Respond to an incoming offer (receiver side). The verdict is delivered
    /// in-band on the sender's still-open offer connection (it parks waiting
    /// for exactly this), so no second connection or dial-back is needed.
    pub async fn respond_offer(&self, offer_id: String, accept: bool) -> Result<()> {
        let offer = self
            .inner
            .consent
            .incoming_offers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&offer_id)
            .cloned()
            .ok_or_else(|| CoreError::NotFound(offer_id.clone()))?;
        let _ = offer; // validated above; the frame carries only the id

        let frame = if accept {
            Frame::OfferAccept {
                offer_id: offer_id.clone(),
            }
        } else {
            Frame::OfferDecline {
                offer_id: offer_id.clone(),
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
            // "this offer expired" instead of starting a download the sender
            // has already abandoned.
            tracing::warn!(%offer_id, "offer connection already gone");
            return Err(CoreError::Other(anyhow::anyhow!(
                "this offer expired before you answered"
            )));
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
/// else, and when the offer may have reached `peer` (the code travels inside
/// it), refuse `peer` from now on. Both happen under the `serving` lock the
/// gate takes, so the code is never open to `peer` in between. The send itself
/// goes on.
async fn release_offer(core: &Core, id: TransferId, hash: &str, peer: EndpointId, reached: bool) {
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
    if reached {
        entry.denied.insert(peer);
    }
    let mut bound = core.inner.bound.lock().await;
    if bound.get(hash) == Some(&peer) {
        bound.remove(hash);
    }
}

/// Deliver the offer and read the verdict off our own connection (echoed).
/// `reached` is set once a connection to the neighbor is up: from then on it
/// may hold the code the offer carries.
async fn deliver_offer(
    endpoint: &Endpoint,
    dial_addr: EndpointAddr,
    frame: Frame,
    reached: &mut bool,
) -> std::result::Result<OfferUpdate, String> {
    let conn = tokio::time::timeout(
        CONSENT_CONNECT_TIMEOUT,
        endpoint.connect(dial_addr, CTRL_ALPN),
    )
    .await
    .map_err(|_| "neighbor unreachable".to_string())?
    .map_err(|e| format!("connect failed: {e}"))?;
    *reached = true;

    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| format!("stream open failed: {e}"))?;
    let bytes = serde_json::to_vec(&frame).map_err(|e| e.to_string())?;
    send.write_all(&bytes).await.map_err(|e| e.to_string())?;
    send.finish().map_err(|e| e.to_string())?;

    let answer = tokio::time::timeout(ANSWER_TIMEOUT, recv.read_to_end(64 * 1024))
        .await
        .map_err(|_| "they didn't answer in time".to_string())?
        .map_err(|e| format!("read failed: {e}"))?;

    match serde_json::from_slice::<Frame>(&answer) {
        Ok(Frame::OfferAccept { .. }) => Ok(OfferUpdate::Accepted),
        Ok(Frame::OfferDecline { .. }) => Ok(OfferUpdate::Declined),
        _ => Err("unexpected answer".into()),
    }
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
        let frame = Frame::OfferDecline { offer_id };
        ctx.resolve_verdict(&frame.offer_id_str(), frame);
        return;
    }

    // Validate the ticket up front so we never surface junk.
    let Ok(parsed) = BlobTicket::from_str(&ticket) else {
        // Unknown ticket shape → decline immediately so the sender isn't left
        // waiting on a dialog that will never appear.
        let frame = Frame::OfferDecline { offer_id };
        ctx.resolve_verdict(&frame.offer_id_str(), frame);
        return;
    };

    let offer = IncomingOffer {
        reply_transport: via,
        offer_id,
        from_endpoint_id: remote.to_string(),
        device_name,
        // Derive the pairing fingerprint from the TLS-AUTHENTICATED remote id,
        // never from a sender-supplied field: an impostor cannot then present a
        // victim's pairing code. This is the value the user compares aloud.
        fingerprint: NearbyDevice::fingerprint_for(&remote.to_string()),
        ticket: parsed.to_string(),
        title,
        file_count,
        total_bytes,
    };
    ctx.incoming_offers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(offer.offer_id.clone(), offer.clone());
    let _ = ctx.offer_tx.send(offer);
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
}

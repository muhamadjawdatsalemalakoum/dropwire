//! A tiny two-way control channel between the two peers, on its own ALPN
//! alongside the blob transfer. Because Dropwire is peer-to-peer and runs on the
//! users' own machines, this channel is **free** — no server, no per-message cost.
//!
//! It carries small out-of-band signals (presence, an instant decline, a short
//! chat message) and the nearby-consent handshake (see [`crate::offer`]). It is
//! purely additive: a second ALPN registered on the same endpoint, so it never
//! touches the file-transfer path.
//!
//! Every frame gets an immediate echo-ack: the handler replies with the *same*
//! frame it received, which doubles as (a) the transport-level ack for
//! presence/chat and (b) the consent verdict carrier for offers — the sender
//! reads its answer off its own outgoing connection, so no dial-back is needed.

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh_blobs::ticket::BlobTicket;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

use crate::error::{CoreError, Result};
use crate::{offer, Core};
/// ALPN for Dropwire's control protocol (distinct from the blobs ALPN).
pub(crate) const CTRL_ALPN: &[u8] = b"dropwire/ctrl/1";

/// Control frames are tiny JSON messages; cap the read to a sane size.
const MAX_FRAME: usize = 64 * 1024;

/// A control-plane message exchanged out-of-band from the file transfer.
/// (Public vocabulary; consent frames live in [`crate::offer::Frame`].)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CtrlMsg {
    /// Presence ping ("I'm here").
    Hello,
    /// The receiver declined the transfer, so the sender hears "no" instantly
    /// instead of waiting for a timeout.
    Decline,
    /// Acknowledge / accept.
    Ack,
    /// A short chat message between the two humans, alongside the transfer.
    Chat { text: String },
}

/// Protocol handler for incoming control connections. Each received frame is
/// routed (offers → consent state, presence → broadcast) and echoed back.
#[derive(Debug, Clone)]
pub(crate) struct Ctrl {
    pub(crate) core_ctx: offer::ConsentCtx,
}

impl ProtocolHandler for Ctrl {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let bytes = recv
            .read_to_end(MAX_FRAME)
            .await
            .map_err(AcceptError::from_err)?;

        // The dialer authenticated itself via the QUIC/TLS handshake — this is
        // the *proven* identity of the other endpoint, not a self-claimed field.
        let remote = connection.remote_id();
        // Remember how they reached us (surfaced with the offer for display).
        let paths = connection.paths();
        let via = paths
            .iter()
            .find(|p| p.is_selected())
            .or_else(|| paths.iter().next())
            .and_then(|p| match p.remote_addr() {
                iroh::TransportAddr::Ip(sock) => Some(offer::EndpointReplyTransport::Ip(*sock)),
                iroh::TransportAddr::Relay(url) => {
                    Some(offer::EndpointReplyTransport::Relay(url.to_string()))
                }
                _ => None,
            });
        drop(paths);

        match serde_json::from_slice::<offer::Frame>(&bytes) {
            Ok(offer::Frame::Offer { offer_id, .. }) => {
                // Register the answer channel BEFORE surfacing the offer, so a
                // fast accept can never race the wait below.
                let (verdict_tx, mut verdict_rx) = mpsc::unbounded_channel();
                self.core_ctx.add_verdict_waiter(&offer_id, verdict_tx);
                offer::route_offer(&self.core_ctx, remote, via, &bytes);
                // Park this connection until the local user answers (or the
                // wait times out — an unanswered offer declines itself).
                let verdict =
                    match tokio::time::timeout(offer::ANSWER_WAIT, verdict_rx.recv()).await {
                        Ok(Some(frame)) => Some(frame),
                        _ => None, // timeout, waiter dropped, or channel gone
                    };
                let answer = verdict.unwrap_or(offer::Frame::OfferDecline {
                    offer_id: offer_id.clone(),
                });
                if let Ok(echo) = serde_json::to_vec(&answer) {
                    let _ = send.write_all(&echo).await;
                }
                let _ = send.finish();
                let _ = connection.closed().await;
            }
            Ok(other_frame) => {
                offer::route_other(&self.core_ctx, other_frame.clone());
                if let Ok(echo) = serde_json::to_vec(&other_frame) {
                    let _ = send.write_all(&echo).await;
                }
                let _ = send.finish();
                // Brief hold so the echo flushes before the QUIC close.
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(5), connection.closed())
                        .await;
            }
            Err(_) => {
                // Legacy/plain presence frames (older peers, direct tests).
                if let Ok(msg) = serde_json::from_slice::<CtrlMsg>(&bytes) {
                    if let Ok(echo) = serde_json::to_vec(&msg) {
                        let _ = send.write_all(&echo).await;
                    }
                    let _ = self.core_ctx.ctrl_tx.send(msg);
                }
                let _ = send.finish();
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(5), connection.closed())
                        .await;
            }
        }
        Ok(())
    }
}

impl Core {
    /// Subscribe to control messages this device receives from peers.
    pub fn subscribe_control(&self) -> broadcast::Receiver<CtrlMsg> {
        self.inner.ctrl_tx.subscribe()
    }

    /// Send a one-shot control message to the peer that issued `ticket` (the
    /// sender). Dials the control ALPN on the same endpoint and waits for the ack.
    pub async fn send_control(&self, ticket: String, msg: CtrlMsg) -> Result<()> {
        let parsed: BlobTicket = ticket
            .parse()
            .map_err(|_| CoreError::InvalidTicket(ticket.clone()))?;
        self.dial_ctrl(parsed.addr().clone(), msg).await
    }

    /// TEST-ONLY: send a control message to an explicit address (hermetic
    /// tests wire two loopback endpoints directly).
    #[cfg(feature = "test-utils")]
    pub async fn send_control_to(&self, addr: iroh::EndpointAddr, msg: CtrlMsg) -> Result<()> {
        self.dial_ctrl(addr, msg).await
    }

    /// Dial the control ALPN, deliver `msg`, wait for the echo-ack.
    async fn dial_ctrl(&self, addr: iroh::EndpointAddr, msg: CtrlMsg) -> Result<()> {
        let endpoint = self.inner.router.endpoint();
        let conn = endpoint
            .connect(addr, CTRL_ALPN)
            .await
            .map_err(|e| CoreError::Other(anyhow::anyhow!("control connect: {e}")))?;
        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .map_err(|e| CoreError::Other(anyhow::anyhow!("control stream: {e}")))?;
        let bytes = serde_json::to_vec(&msg)
            .map_err(|e| CoreError::Other(anyhow::anyhow!("encode: {e}")))?;
        send.write_all(&bytes)
            .await
            .map_err(|e| CoreError::Other(anyhow::anyhow!("control send: {e}")))?;
        send.finish()
            .map_err(|e| CoreError::Other(anyhow::anyhow!("control finish: {e}")))?;
        // Wait for the peer's ack (and clean stream close) before returning.
        let _ = recv.read_to_end(64 * 1024).await;
        Ok(())
    }
}

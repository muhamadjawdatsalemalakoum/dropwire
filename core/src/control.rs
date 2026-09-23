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

use std::time::Duration;

use iroh::endpoint::{Connection, SendStream};
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

/// How long a peer has to open its stream and send its one frame. One that
/// stalls is dropped rather than holding a connection and a task.
const STREAM_WAIT: Duration = Duration::from_secs(10);

/// How long the reply gets to flush before the connection is let go, so a
/// peer that never reads it (or never closes) cannot hold it open.
const CLOSE_GRACE: Duration = Duration::from_secs(5);

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
        let opened = tokio::time::timeout(STREAM_WAIT, async {
            let (send, mut recv) = connection.accept_bi().await?;
            let bytes = recv
                .read_to_end(MAX_FRAME)
                .await
                .map_err(AcceptError::from_err)?;
            Ok::<_, AcceptError>((send, bytes))
        })
        .await;
        // Too slow: dropping the connection closes it.
        let Ok(opened) = opened else {
            return Ok(());
        };
        let (mut send, bytes) = opened?;

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
                // An offer id that is already waiting (sent twice, or not
                // theirs to use) is refused, leaving the first one alone.
                if !self.core_ctx.add_verdict_waiter(&offer_id, verdict_tx) {
                    let decline = offer::Frame::OfferDecline { offer_id };
                    reply(&connection, &mut send, serde_json::to_vec(&decline).ok()).await;
                    return Ok(());
                }
                offer::route_offer(&self.core_ctx, remote, via, &bytes);
                // Park this connection until the local user answers, the
                // sender goes away, or the wait times out (an unanswered offer
                // declines itself).
                let verdict = tokio::select! {
                    verdict = verdict_rx.recv() => verdict,
                    // The sender took the offer back, cancelled its send, or
                    // left. Retire the offer now so its dialog closes and a
                    // late Accept is reported as ended, not sent nowhere.
                    _ = connection.closed() => {
                        self.core_ctx
                            .retire_offer(&offer_id, offer::WithdrawReason::Cancelled);
                        return Ok(());
                    }
                    _ = tokio::time::sleep(offer::ANSWER_WAIT) => {
                        // Unanswered: forget the offer so neither map leaks
                        // and a stale Accept is reported as ended rather than
                        // silently starting a doomed download.
                        self.core_ctx
                            .retire_offer(&offer_id, offer::WithdrawReason::Expired);
                        None
                    }
                };
                let answer = verdict.unwrap_or(offer::Frame::OfferDecline {
                    offer_id: offer_id.clone(),
                });
                reply(&connection, &mut send, serde_json::to_vec(&answer).ok()).await;
            }
            Ok(other_frame) => {
                offer::route_other(&self.core_ctx, remote, other_frame.clone());
                reply(
                    &connection,
                    &mut send,
                    serde_json::to_vec(&other_frame).ok(),
                )
                .await;
            }
            Err(_) => {
                // Legacy/plain presence frames (older peers, direct tests).
                let msg = serde_json::from_slice::<CtrlMsg>(&bytes).ok();
                let echo = msg.as_ref().and_then(|m| serde_json::to_vec(m).ok());
                if let Some(msg) = msg {
                    let _ = self.core_ctx.ctrl_tx.send(msg);
                }
                reply(&connection, &mut send, echo).await;
            }
        }
        Ok(())
    }
}

/// Send `echo` (if any) back on the frame's stream, then hold briefly so it
/// flushes before the QUIC close. Bounded by [`CLOSE_GRACE`] as a whole.
async fn reply(connection: &Connection, send: &mut SendStream, echo: Option<Vec<u8>>) {
    let _ = tokio::time::timeout(CLOSE_GRACE, async {
        if let Some(echo) = echo {
            let _ = send.write_all(&echo).await;
        }
        let _ = send.finish();
        connection.closed().await
    })
    .await;
}

impl Core {
    /// Subscribe to control messages this device receives from peers.
    pub fn subscribe_control(&self) -> broadcast::Receiver<CtrlMsg> {
        self.inner.ctrl_tx.subscribe()
    }

    /// Send a one-shot control message to the peer that issued `ticket` (the
    /// sender). Dials the control ALPN on the same endpoint and waits for the ack.
    /// A [`CtrlMsg::Decline`] goes out as [`Core::decline`] sends it.
    pub async fn send_control(&self, ticket: String, msg: CtrlMsg) -> Result<()> {
        if msg == CtrlMsg::Decline {
            return self.decline(ticket).await;
        }
        let parsed: BlobTicket = ticket
            .parse()
            .map_err(|_| CoreError::InvalidTicket(ticket.clone()))?;
        self.dial_ctrl(parsed.addr().clone(), &offer::Frame::from(&msg))
            .await
    }

    /// Decline the code in `ticket` after previewing it. The sender is told
    /// right away, and if this device is the one the code is bound to, the
    /// binding is released so the sender's code can go to someone else.
    pub async fn decline(&self, ticket: String) -> Result<()> {
        let parsed: BlobTicket = ticket
            .parse()
            .map_err(|_| CoreError::InvalidTicket(ticket.clone()))?;
        let frame = offer::Frame::Decline {
            hash: Some(parsed.hash().to_string()),
        };
        self.dial_ctrl(parsed.addr().clone(), &frame).await
    }

    /// TEST-ONLY: send a control message to an explicit address (hermetic
    /// tests wire two loopback endpoints directly).
    #[cfg(feature = "test-utils")]
    pub async fn send_control_to(&self, addr: iroh::EndpointAddr, msg: CtrlMsg) -> Result<()> {
        self.dial_ctrl(addr, &offer::Frame::from(&msg)).await
    }

    /// Dial the control ALPN, deliver `frame`, wait for the echo-ack.
    async fn dial_ctrl(&self, addr: iroh::EndpointAddr, frame: &offer::Frame) -> Result<()> {
        let endpoint = self.inner.router.endpoint();
        let conn = endpoint
            .connect(addr, CTRL_ALPN)
            .await
            .map_err(|e| CoreError::Other(anyhow::anyhow!("control connect: {e}")))?;
        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .map_err(|e| CoreError::Other(anyhow::anyhow!("control stream: {e}")))?;
        let bytes = serde_json::to_vec(frame)
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

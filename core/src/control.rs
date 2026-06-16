//! A tiny two-way control channel between the two peers, on its own ALPN
//! alongside the blob transfer. Because Dropwire is peer-to-peer and runs on the
//! users' own machines, this channel is **free** — no server, no per-message cost.
//!
//! It carries small out-of-band signals (presence, an instant decline, a short
//! chat message) that the pull-based blob protocol can't express on its own. It is
//! purely additive: a second ALPN registered on the same endpoint, so it never
//! touches the file-transfer path.

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh_blobs::ticket::BlobTicket;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::error::{CoreError, Result};
use crate::Core;

/// ALPN for Dropwire's control protocol (distinct from the blobs ALPN).
pub(crate) const CTRL_ALPN: &[u8] = b"dropwire/ctrl/1";

/// Control frames are tiny JSON messages; cap the read to a sane size.
const MAX_FRAME: usize = 64 * 1024;

/// A control-plane message exchanged out-of-band from the file transfer.
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

/// Protocol handler for incoming control connections. Each received message is
/// published on the Core's broadcast channel.
#[derive(Debug, Clone)]
pub(crate) struct Ctrl {
    pub(crate) tx: broadcast::Sender<CtrlMsg>,
}

impl ProtocolHandler for Ctrl {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let bytes = recv
            .read_to_end(MAX_FRAME)
            .await
            .map_err(AcceptError::from_err)?;
        if let Ok(msg) = serde_json::from_slice::<CtrlMsg>(&bytes) {
            // Broadcast to any subscriber (the UI). Ignored if there are none.
            let _ = self.tx.send(msg);
        }
        // Best-effort ack so the sender's round-trip can complete cleanly.
        let _ = send.write_all(b"ok").await;
        let _ = send.finish();
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
        let endpoint = self.inner.router.endpoint();
        let conn = endpoint
            .connect(parsed.addr().clone(), CTRL_ALPN)
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
        let _ = recv.read_to_end(64).await;
        Ok(())
    }
}

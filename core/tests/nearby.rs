//! End-to-end tests for nearby discovery + two-sided consent.
//!
//! These run hermetically (LocalOnly infra, no mDNS daemon): the consent
//! handshake is exercised over the real control ALPN between two in-process
//! endpoints, which is exactly what the mDNS path bootstraps in production.
//! The mDNS advertisement/browse loop itself is exercised in `nearby_mdns.rs`
//! (ignored by default: it needs a live multicast-capable network).

mod common;

use std::time::Duration;

use common::{local_core, make_payload, wait_done, wait_ready};
use irohcore::{CtrlMsg, IncomingOffer, OfferUpdate};
use tokio_stream::StreamExt;

/// Sender offers; receiver accepts; the file lands. The full two-sided flow.
#[tokio::test]
async fn nearby_offer_accept_transfers() {
    let dir = tempdir::dir();
    let dir2 = tempdir::dir();
    let sender = local_core(dir.path()).await;
    let receiver = local_core(dir2.path()).await;

    // Receiver subscribes BEFORE the offer is sent (no missed broadcasts).
    let mut offers = receiver.subscribe_offers();

    // Sender picks a file and waits for the ticket.
    let src = dir.path().join("album.zip");
    std::fs::write(&src, make_payload(256 * 1024)).unwrap();
    let (_send_id, mut send_stream) = sender.send(src.clone()).await.unwrap();
    wait_ready(&mut send_stream).await;

    // Sender offers directly to the receiver's endpoint id, with the
    // receiver's address as an explicit hint (production gets both from
    // mDNS; hermetic tests wire the loopback address in directly).
    let receiver_eid = receiver.endpoint_id();
    let (_offer_id, mut updates) = sender
        .offer_nearby_dial(receiver_eid, Some(receiver.test_dial_addr()))
        .await
        .expect("offer");

    // Receiver sees the offer with honest metadata + a fingerprint.
    let offer: IncomingOffer = tokio::time::timeout(Duration::from_secs(15), offers.recv())
        .await
        .expect("no offer arrived")
        .expect("offer channel closed");
    assert_eq!(offer.title, "album.zip");
    assert_eq!(offer.file_count, 1);
    assert_eq!(offer.total_bytes, 256 * 1024);
    assert_eq!(offer.from_endpoint_id, sender.endpoint_id());
    assert!(!offer.fingerprint.is_empty());

    // Receiver ACCEPTS.
    receiver
        .respond_offer(offer.offer_id.clone(), true)
        .await
        .expect("respond accept");

    // Sender sees the acceptance (skipping the initial "waiting" update).
    let update = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match updates.next().await {
                Some(OfferUpdate::Waiting) => continue,
                other => return other.expect("offer stream ended"),
            }
        }
    })
    .await
    .expect("no verdict in time");
    assert_eq!(update, OfferUpdate::Accepted);

    // Receiver downloads via the offered ticket (normal blobs path).
    let dest = dir2.path().join("out");
    let (_rx_id, mut rx_stream) = receiver.receive(offer.ticket, dest).await.unwrap();
    wait_done(&mut rx_stream).await;

    let got = std::fs::read(dir2.path().join("out").join("album.zip")).unwrap();
    assert_eq!(got, make_payload(256 * 1024));
}

/// The security property: no bytes move without the receiver's consent.
/// The receiver declines; the sender learns instantly (not via timeout) and
/// the receiver downloads nothing.
#[tokio::test]
async fn nearby_offer_decline_blocks_transfer() {
    let dir = tempdir::dir();
    let dir2 = tempdir::dir();
    let sender = local_core(dir.path()).await;
    let receiver = local_core(dir2.path()).await;
    let mut offers = receiver.subscribe_offers();

    let src = dir.path().join("secret.txt");
    std::fs::write(&src, make_payload(64 * 1024)).unwrap();
    let (_id, mut send_stream) = sender.send(src).await.unwrap();
    let _ticket = wait_ready(&mut send_stream).await;

    let (_oid, mut updates) = sender
        .offer_nearby_dial(receiver.endpoint_id(), Some(receiver.test_dial_addr()))
        .await
        .expect("offer");

    let offer = tokio::time::timeout(Duration::from_secs(15), offers.recv())
        .await
        .unwrap()
        .unwrap();

    // Receiver DECLINES.
    receiver
        .respond_offer(offer.offer_id, false)
        .await
        .expect("respond decline");

    // Sender hears "no" promptly (skipping the initial "waiting" update).
    let update = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match updates.next().await {
                Some(OfferUpdate::Waiting) => continue,
                other => return other.expect("offer stream ended"),
            }
        }
    })
    .await
    .expect("no verdict in time");
    assert_eq!(update, OfferUpdate::Declined);

    // The receiver never accepted, so no download destination may exist and
    // none of the offered files may appear anywhere on disk. (Engine
    // bookkeeping like node.key/transfers.json in its own data dir is fine.)
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !dir2.path().join("out").exists(),
        "no download dir expected"
    );
    let stray = std::fs::read_dir(dir2.path())
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".txt") || n.ends_with(".zip"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert!(stray.is_empty(), "declined transfer must not write files");
}

/// Presence frames still flow through the shared control channel (the same
/// ALPN carries consent), using an explicit dial address.
#[tokio::test]
async fn control_channel_presence_still_works() {
    let dir = tempdir::dir();
    let dir2 = tempdir::dir();
    let a = local_core(dir.path()).await;
    let b = local_core(dir2.path()).await;
    let mut ctrl = b.subscribe_control();

    a.send_control_to(b.test_dial_addr(), CtrlMsg::Hello)
        .await
        .expect("hello");

    let msg = tokio::time::timeout(Duration::from_secs(10), ctrl.recv())
        .await
        .expect("no ctrl message")
        .expect("ctrl channel closed");
    assert_eq!(msg, CtrlMsg::Hello);
}

/// Offering with no active send fails cleanly instead of hanging.
#[tokio::test]
async fn offer_without_active_send_errors() {
    let dir = tempdir::dir();
    let dir2 = tempdir::dir();
    let sender = local_core(dir.path()).await;
    let receiver = local_core(dir2.path()).await;

    let err = sender
        .offer_nearby(receiver.endpoint_id())
        .await
        .expect_err("must fail without an active send");
    assert!(err.to_string().contains("no active send"));
}

/// A tiny helper producing a unique temp dir per call (no external dev-dep).
mod tempdir {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static N: AtomicU64 = AtomicU64::new(0);

    pub struct TempDir(PathBuf);
    impl TempDir {
        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub fn dir() -> TempDir {
        let n = N.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let p = std::env::temp_dir().join(format!("dw-nearby-{pid}-{n}"));
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
}

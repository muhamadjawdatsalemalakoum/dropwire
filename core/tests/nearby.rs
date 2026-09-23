//! End-to-end tests for nearby discovery + two-sided consent.
//!
//! These run hermetically (LocalOnly infra, no mDNS daemon): the consent
//! handshake is exercised over the real control ALPN between two in-process
//! endpoints, which is exactly what the mDNS path bootstraps in production.
//! The mDNS advertisement/browse loop itself is exercised in `nearby_mdns.rs`
//! (ignored by default: it needs a live multicast-capable network).

mod common;

use std::time::Duration;

use common::{drain_for, local_core, make_payload, wait_done, wait_ready};
use iroh_blobs::ticket::BlobTicket;
use irohcore::{CoreError, CtrlMsg, IncomingOffer, OfferUpdate, Progress, TransferId};
use tokio::sync::broadcast;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

/// The next offer to surface on the receiver.
async fn next_offer(offers: &mut broadcast::Receiver<IncomingOffer>) -> IncomingOffer {
    tokio::time::timeout(Duration::from_secs(15), offers.recv())
        .await
        .expect("no offer arrived")
        .expect("offer channel closed")
}

/// The sender's verdict on an offer, skipping the initial "waiting" update.
async fn next_verdict(updates: &mut ReceiverStream<OfferUpdate>) -> OfferUpdate {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match updates.next().await {
                Some(OfferUpdate::Waiting) => continue,
                other => return other.expect("offer stream ended"),
            }
        }
    })
    .await
    .expect("no verdict in time")
}

/// Drive a send stream until it reports Cancelled.
async fn wait_cancelled(stream: &mut irohcore::ProgressStream) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(ev) = stream.next().await {
            if let Progress::Cancelled { .. } = ev {
                return;
            }
        }
        panic!("send stream ended before Cancelled");
    })
    .await
    .expect("timed out waiting for Cancelled");
}

/// The content hash a code names.
fn hash_of(ticket: &str) -> String {
    ticket.parse::<BlobTicket>().unwrap().hash().to_string()
}

/// The request reached the sender and its one-to-one gate refused it:
/// iroh-blobs resets a refused request's stream with ERR_PERMISSION (1).
fn assert_refused<T: std::fmt::Debug>(res: Result<T, CoreError>, what: &str) {
    const NEEDLE: &str = "reset by peer: error 1";
    match res {
        Ok(v) => panic!("{what}: expected a refusal, got {v:?}"),
        Err(e) => {
            let chain = format!("{e:#}");
            let refused = chain
                .match_indices(NEEDLE)
                .any(|(i, m)| !chain[i + m.len()..].starts_with(|c: char| c.is_ascii_digit()));
            assert!(refused, "{what}: expected the gate to refuse, got: {chain}");
        }
    }
}

/// Sender offers; receiver accepts; the file lands. The full two-sided flow.
#[tokio::test]
async fn nearby_offer_accept_transfers() {
    let dir = tempdir::dir();
    let dir2 = tempdir::dir();
    let sender = local_core(dir.path()).await;
    let receiver = local_core(dir2.path()).await;
    // Incoming offers are gated on nearby being ON (invisible while off); enable
    // it here without standing up a real mDNS daemon.
    receiver.test_set_nearby_running(true);

    // Receiver subscribes BEFORE the offer is sent (no missed broadcasts).
    let mut offers = receiver.subscribe_offers();

    // Sender picks a file and waits for the ticket.
    let src = dir.path().join("album.zip");
    std::fs::write(&src, make_payload(256 * 1024)).unwrap();
    let (send_id, mut send_stream) = sender.send(src.clone()).await.unwrap();
    wait_ready(&mut send_stream).await;

    // Sender offers directly to the receiver's endpoint id, with the
    // receiver's address as an explicit hint (production gets both from
    // mDNS; hermetic tests wire the loopback address in directly).
    let receiver_eid = receiver.endpoint_id();
    let (_offer_id, mut updates) = sender
        .offer_nearby_dial(receiver_eid, send_id, Some(receiver.test_dial_addr()))
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
    receiver.test_set_nearby_running(true);
    let mut offers = receiver.subscribe_offers();

    let src = dir.path().join("secret.txt");
    std::fs::write(&src, make_payload(64 * 1024)).unwrap();
    let (id, mut send_stream) = sender.send(src).await.unwrap();
    let _ticket = wait_ready(&mut send_stream).await;

    let (_oid, mut updates) = sender
        .offer_nearby_dial(receiver.endpoint_id(), id, Some(receiver.test_dial_addr()))
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

/// With nearby sharing OFF the device is invisible: an incoming offer is
/// declined at the consent layer and never surfaces to the user, even though
/// the control ALPN is reachable (a peer that knows our id can still dial).
#[tokio::test]
async fn offer_while_nearby_off_is_declined_silently() {
    let dir = tempdir::dir();
    let dir2 = tempdir::dir();
    let sender = local_core(dir.path()).await;
    let receiver = local_core(dir2.path()).await;
    // Receiver leaves nearby OFF (the default) — do NOT enable it.
    let mut offers = receiver.subscribe_offers();

    let src = dir.path().join("thing.bin");
    std::fs::write(&src, make_payload(32 * 1024)).unwrap();
    let (id, mut send_stream) = sender.send(src).await.unwrap();
    wait_ready(&mut send_stream).await;

    let (_oid, mut updates) = sender
        .offer_nearby_dial(receiver.endpoint_id(), id, Some(receiver.test_dial_addr()))
        .await
        .expect("offer");

    // Sender is told "no" promptly …
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

    // … and the receiver was never shown anything.
    let surfaced = tokio::time::timeout(Duration::from_millis(500), offers.recv()).await;
    assert!(
        surfaced.is_err(),
        "an offer must not surface while nearby is off"
    );
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

/// Offering a send that does not exist, or one that has ended, fails cleanly
/// instead of hanging or offering something else.
#[tokio::test]
async fn offer_without_live_send_errors() {
    let dir = tempdir::dir();
    let dir2 = tempdir::dir();
    let sender = local_core(dir.path()).await;
    let receiver = local_core(dir2.path()).await;

    let err = sender
        .offer_nearby(receiver.endpoint_id(), TransferId::new())
        .await
        .expect_err("must fail for an unknown send");
    assert!(matches!(err, CoreError::NotFound(_)), "got {err}");

    // A send that was cancelled is not offered either.
    let src = dir.path().join("gone.txt");
    std::fs::write(&src, make_payload(4 * 1024)).unwrap();
    let (id, mut send_stream) = sender.send(src).await.unwrap();
    wait_ready(&mut send_stream).await;
    sender.cancel(id).await;
    wait_cancelled(&mut send_stream).await;
    let err = sender
        .offer_nearby(receiver.endpoint_id(), id)
        .await
        .expect_err("must fail for an ended send");
    assert!(err.to_string().contains("has ended"), "got {err}");
}

/// With two sends live, the one the caller names is the one offered, even
/// when the other is newer (the engine used to pick the newest itself).
#[tokio::test]
async fn offer_names_the_send_it_offers() {
    let dir = tempdir::dir();
    let dir2 = tempdir::dir();
    let sender = local_core(dir.path()).await;
    let receiver = local_core(dir2.path()).await;
    receiver.test_set_nearby_running(true);
    let mut offers = receiver.subscribe_offers();

    let older = dir.path().join("older.txt");
    std::fs::write(&older, make_payload(8 * 1024)).unwrap();
    let (older_id, mut older_stream) = sender.send(older).await.unwrap();
    let older_ticket = wait_ready(&mut older_stream).await;

    let newer = dir.path().join("newer.txt");
    std::fs::write(&newer, make_payload(12 * 1024)).unwrap();
    let (_newer_id, mut newer_stream) = sender.send(newer).await.unwrap();
    let newer_ticket = wait_ready(&mut newer_stream).await;

    let (_oid, mut updates) = sender
        .offer_nearby_dial(
            receiver.endpoint_id(),
            older_id,
            Some(receiver.test_dial_addr()),
        )
        .await
        .expect("offer");
    let offer = next_offer(&mut offers).await;
    assert_eq!(offer.title, "older.txt");
    assert_eq!(offer.total_bytes, 8 * 1024);
    assert_eq!(hash_of(&offer.ticket), hash_of(&older_ticket));
    assert_ne!(hash_of(&offer.ticket), hash_of(&newer_ticket));

    receiver
        .respond_offer(offer.offer_id, true)
        .await
        .expect("accept");
    assert_eq!(next_verdict(&mut updates).await, OfferUpdate::Accepted);
}

/// A send whose code is already in use is never offered to a second device:
/// that would take it from the first one mid-transfer. A repeat offer to the
/// device that has it is fine, and declining that offer does not cost it the
/// code.
#[tokio::test]
async fn offer_never_takes_a_send_from_its_receiver() {
    let dir = tempdir::dir();
    let d1 = tempdir::dir();
    let d2 = tempdir::dir();
    let sender = local_core(dir.path()).await;
    let r1 = local_core(d1.path()).await;
    let r2 = local_core(d2.path()).await;
    r1.test_set_nearby_running(true);
    r2.test_set_nearby_running(true);
    let mut r1_offers = r1.subscribe_offers();
    let mut r2_offers = r2.subscribe_offers();

    let src = dir.path().join("report.pdf");
    std::fs::write(&src, make_payload(96 * 1024)).unwrap();
    let (id, mut send_stream) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut send_stream).await;

    // r1 opens the code's preview, so the code is now r1's.
    r1.inspect(ticket.clone()).await.expect("r1 previews");

    let err = sender
        .offer_nearby_dial(r2.endpoint_id(), id, Some(r2.test_dial_addr()))
        .await
        .expect_err("must not hand r1's send to r2");
    assert!(
        err.to_string().contains("already going to another device"),
        "got {err}"
    );
    let surfaced = tokio::time::timeout(Duration::from_millis(300), r2_offers.recv()).await;
    assert!(surfaced.is_err(), "a refused offer must not go out");

    // Offering it to r1 itself is allowed; r1 says no to that offer.
    let (_oid, mut updates) = sender
        .offer_nearby_dial(r1.endpoint_id(), id, Some(r1.test_dial_addr()))
        .await
        .expect("offer to r1");
    let offer = next_offer(&mut r1_offers).await;
    r1.respond_offer(offer.offer_id, false).await.unwrap();
    assert_eq!(next_verdict(&mut updates).await, OfferUpdate::Declined);

    // r1 still gets the files with the code, and the send never stopped.
    let dest = d1.path().join("out");
    let (_rid, mut rx) = r1.receive(ticket, dest.clone()).await.unwrap();
    wait_done(&mut rx).await;
    assert_eq!(
        std::fs::read(dest.join("report.pdf")).unwrap(),
        make_payload(96 * 1024)
    );
    let seen = drain_for(&mut send_stream, Duration::from_millis(300)).await;
    assert!(
        !seen.iter().any(|e| matches!(e, Progress::Cancelled { .. })),
        "the send must keep going: {seen:?}"
    );
}

/// Declining an offer does not end the send: its code still works for
/// someone else. The device that declined holds the code (it came with the
/// offer), but the sender refuses it from the moment it says no.
#[tokio::test]
async fn declined_offer_keeps_the_send_and_shuts_the_decliner_out() {
    let dir = tempdir::dir();
    let bob_dir = tempdir::dir();
    let carol_dir = tempdir::dir();
    let sender = local_core(dir.path()).await;
    let bob = local_core(bob_dir.path()).await;
    let carol = local_core(carol_dir.path()).await;
    bob.test_set_nearby_running(true);
    let mut offers = bob.subscribe_offers();

    let src = dir.path().join("plans.pdf");
    std::fs::write(&src, make_payload(80 * 1024)).unwrap();
    let (id, mut send_stream) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut send_stream).await;

    let (_oid, mut updates) = sender
        .offer_nearby_dial(bob.endpoint_id(), id, Some(bob.test_dial_addr()))
        .await
        .expect("offer");
    let offer = next_offer(&mut offers).await;
    bob.respond_offer(offer.offer_id.clone(), false)
        .await
        .unwrap();
    assert_eq!(next_verdict(&mut updates).await, OfferUpdate::Declined);

    // Bob cannot use the code the offer carried.
    assert_refused(bob.inspect(offer.ticket.clone()).await, "bob's preview");

    // The send is still going, and its code works for Carol.
    let dest = carol_dir.path().join("out");
    let (_rid, mut rx) = carol.receive(ticket, dest.clone()).await.unwrap();
    wait_done(&mut rx).await;
    assert_eq!(
        std::fs::read(dest.join("plans.pdf")).unwrap(),
        make_payload(80 * 1024)
    );
    let seen = drain_for(&mut send_stream, Duration::from_millis(300)).await;
    assert!(
        !seen.iter().any(|e| matches!(e, Progress::Cancelled { .. })),
        "a declined offer must not cancel the send: {seen:?}"
    );

    // Bob is still refused after Carol took it (and would be if she had not).
    assert_refused(bob.inspect(offer.ticket).await, "bob after carol");
}

/// An offer that never reaches the device fails without ending the send, and
/// without holding the code: that device never saw it, so it may still use
/// it if it gets it another way, and so may anyone else.
#[tokio::test]
async fn failed_offer_keeps_the_send_and_frees_the_code() {
    let dir = tempdir::dir();
    let bob_dir = tempdir::dir();
    let sender = local_core(dir.path()).await;
    let bob = local_core(bob_dir.path()).await;

    let src = dir.path().join("notes.txt");
    std::fs::write(&src, make_payload(20 * 1024)).unwrap();
    let (id, mut send_stream) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut send_stream).await;

    // No address to dial (not on the LAN, no hint): it cannot be delivered.
    let (_oid, mut updates) = sender
        .offer_nearby(bob.endpoint_id(), id)
        .await
        .expect("offer");
    let update = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match updates.next().await {
                Some(OfferUpdate::Waiting) => continue,
                other => return other.expect("offer stream ended"),
            }
        }
    })
    .await
    .expect("no verdict in time");
    assert!(
        matches!(update, OfferUpdate::Failed { .. }),
        "got {update:?}"
    );

    // Bob gets the code some other way (a chat), and it works.
    let dest = bob_dir.path().join("out");
    let (_rid, mut rx) = bob.receive(ticket, dest.clone()).await.unwrap();
    wait_done(&mut rx).await;
    assert_eq!(
        std::fs::read(dest.join("notes.txt")).unwrap(),
        make_payload(20 * 1024)
    );
    let seen = drain_for(&mut send_stream, Duration::from_millis(300)).await;
    assert!(
        !seen.iter().any(|e| matches!(e, Progress::Cancelled { .. })),
        "a failed offer must not cancel the send: {seen:?}"
    );
}

/// The pairing fingerprint must depend on the WHOLE identity, not a short
/// prefix of its hex form. The old algorithm ignored everything past ~6 hex
/// chars, leaving ~21 grindable bits; the hashed one spreads every bit across
/// a 12-char (60-bit) code.
#[test]
fn fingerprint_depends_on_whole_identity() {
    use irohcore::NearbyDevice;
    // Differ only in the LAST hex char — collided under the old scheme.
    let a = "0000000000000000000000000000000000000000000000000000000000000001";
    let b = "0000000000000000000000000000000000000000000000000000000000000002";
    assert_ne!(
        NearbyDevice::fingerprint_for(a),
        NearbyDevice::fingerprint_for(b),
        "late identity bits must change the fingerprint"
    );
    // Shape: 4 space-separated groups of 3 base32 chars.
    let fp = NearbyDevice::fingerprint_for(a);
    let groups: Vec<&str> = fp.split(' ').collect();
    assert_eq!(groups.len(), 4, "expected 4 groups, got {fp:?}");
    assert!(
        groups.iter().all(|g| g.chars().count() == 3),
        "3 chars/group: {fp:?}"
    );
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

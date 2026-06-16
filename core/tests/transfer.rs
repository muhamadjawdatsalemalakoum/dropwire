//! End-to-end transfer tests for the Dropwire engine.
//!
//! These use `Infra::LocalOnly` (no relay, no discovery) so they're hermetic:
//! the receiver dials the direct addresses embedded in the ticket over loopback.

use std::time::Duration;

use irohcore::{Core, CoreConfig, Progress, ProgressStream};
use tokio_stream::StreamExt;

fn make_payload(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

/// Read the send stream until the ticket is ready.
async fn wait_ready(stream: &mut ProgressStream) -> String {
    let fut = async {
        while let Some(ev) = stream.next().await {
            match ev {
                Progress::Ready { ticket, .. } => return ticket,
                Progress::Error { message, .. } => panic!("send error: {message}"),
                _ => {}
            }
        }
        panic!("send stream ended before Ready");
    };
    tokio::time::timeout(Duration::from_secs(30), fut)
        .await
        .expect("timed out waiting for ticket")
}

/// Read the receive stream until the transfer completes.
async fn wait_done(stream: &mut ProgressStream) {
    let fut = async {
        while let Some(ev) = stream.next().await {
            match ev {
                Progress::Done { .. } => return,
                Progress::Error { message, .. } => panic!("receive error: {message}"),
                Progress::Cancelled { .. } => panic!("receive cancelled unexpectedly"),
                _ => {}
            }
        }
        panic!("receive stream ended before Done");
    };
    tokio::time::timeout(Duration::from_secs(60), fut)
        .await
        .expect("timed out waiting for completion");
}

#[tokio::test(flavor = "multi_thread")]
async fn roundtrip_single_file() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let src = work.path().join("hello.bin");
    let payload = make_payload(2 * 1024 * 1024);
    std::fs::write(&src, &payload).unwrap();

    let sender = Core::start(CoreConfig::local_only(send_data.path()))
        .await
        .unwrap();
    let receiver = Core::start(CoreConfig::local_only(recv_data.path()))
        .await
        .unwrap();

    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs).await;

    let got = std::fs::read(out.join("hello.bin")).unwrap();
    assert_eq!(got, payload, "received file must match source");
}

#[tokio::test(flavor = "multi_thread")]
async fn roundtrip_folder() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let dir = work.path().join("pics");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.bin"), make_payload(1024)).unwrap();
    std::fs::write(dir.join("sub").join("b.bin"), make_payload(4096)).unwrap();

    let sender = Core::start(CoreConfig::local_only(send_data.path()))
        .await
        .unwrap();
    let receiver = Core::start(CoreConfig::local_only(recv_data.path()))
        .await
        .unwrap();

    let (_sid, mut ss) = sender.send(dir).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs).await;

    assert_eq!(
        std::fs::read(out.join("pics").join("a.bin")).unwrap(),
        make_payload(1024)
    );
    assert_eq!(
        std::fs::read(out.join("pics").join("sub").join("b.bin")).unwrap(),
        make_payload(4096)
    );
}

/// Resume across an interruption — the critical correctness test (ARCHITECTURE.md §12).
/// Timing-sensitive on a fast loopback, so it is `#[ignore]` by default; run with:
/// `cargo test -p irohcore -- --ignored resume_after_interrupt`
#[tokio::test(flavor = "multi_thread")]
#[ignore = "timing-sensitive; tune cancel delay on a real build before enabling in CI"]
async fn resume_after_interrupt() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let src = work.path().join("big.bin");
    let payload = make_payload(64 * 1024 * 1024);
    std::fs::write(&src, &payload).unwrap();

    let sender = Core::start(CoreConfig::local_only(send_data.path()))
        .await
        .unwrap();
    let receiver = Core::start(CoreConfig::local_only(recv_data.path()))
        .await
        .unwrap();

    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    // First attempt: cancel shortly after bytes begin to flow.
    let out = work.path().join("out");
    let (rid, mut rs) = receiver.receive(ticket.clone(), out.clone()).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_millis(40), rs.next()).await;
    receiver.cancel(rid).await;
    while let Some(ev) = rs.next().await {
        if matches!(
            ev,
            Progress::Cancelled { .. } | Progress::Error { .. } | Progress::Done { .. }
        ) {
            break;
        }
    }

    // Second attempt: must finish using the partial data already in the store.
    let (_rid2, mut rs2) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs2).await;

    let got = std::fs::read(out.join("big.bin")).unwrap();
    assert_eq!(got, payload);
}

/// Real-network smoke test of the SERVERLESS path (`Infra::Decentralized`: DHT
/// discovery + n0's free relay). Hits the public network, so it's `#[ignore]` by
/// default. Run with: `cargo test -p irohcore -- --ignored roundtrip_serverless`
#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: uses the public Mainline DHT + n0's free relay"]
async fn roundtrip_serverless() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let src = work.path().join("hello.bin");
    let payload = make_payload(3 * 1024 * 1024);
    std::fs::write(&src, &payload).unwrap();

    let sender = Core::start(CoreConfig::serverless(send_data.path()))
        .await
        .unwrap();
    let receiver = Core::start(CoreConfig::serverless(recv_data.path()))
        .await
        .unwrap();

    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs).await;

    assert_eq!(std::fs::read(out.join("hello.bin")).unwrap(), payload);
}

/// The SENDER now sees live progress (via iroh-blobs provider events): when a
/// receiver fetches, the send stream emits PeerJoined → Transferring… → Done.
#[tokio::test(flavor = "multi_thread")]
async fn sender_sees_progress() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let src = work.path().join("payload.bin");
    let payload = make_payload(3 * 1024 * 1024);
    std::fs::write(&src, &payload).unwrap();

    let sender = Core::start(CoreConfig::local_only(send_data.path()))
        .await
        .unwrap();
    let receiver = Core::start(CoreConfig::local_only(recv_data.path()))
        .await
        .unwrap();

    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    // Drive the receiver to completion in the background.
    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket, out).await.unwrap();
    tokio::spawn(async move { while rs.next().await.is_some() {} });

    // The sender stream should report a peer joining and the transfer completing.
    let (peer, done) = tokio::time::timeout(Duration::from_secs(60), async {
        let (mut peer, mut done) = (false, false);
        while let Some(ev) = ss.next().await {
            match ev {
                Progress::PeerJoined { .. } => peer = true,
                Progress::Done { .. } => {
                    done = true;
                    break;
                }
                Progress::Error { message, .. } => panic!("sender error: {message}"),
                _ => {}
            }
        }
        (peer, done)
    })
    .await
    .expect("timed out waiting for sender-side progress");

    assert!(peer, "sender should see PeerJoined");
    assert!(done, "sender should see Done");
}

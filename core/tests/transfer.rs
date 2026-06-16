//! End-to-end transfer tests for the Dropwire engine.
//!
//! These use `Infra::LocalOnly` (no relay, no discovery) so they're hermetic:
//! the receiver dials the direct addresses embedded in the ticket over loopback.

use irohcore::{Core, CoreConfig, Progress};
use tokio_stream::StreamExt;

mod common;
use common::{drain_until_terminal, local_core, make_payload, wait_done, wait_ready};

#[tokio::test(flavor = "multi_thread")]
async fn roundtrip_single_file() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let src = work.path().join("hello.bin");
    let payload = make_payload(2 * 1024 * 1024);
    std::fs::write(&src, &payload).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;

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

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;

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
///
/// Determinism: instead of guessing a wall-clock cancel delay, we drive the first
/// receive until a `Transferring` event reports a real byte `offset > 0`, then
/// cancel. That leaves a partial in the receiver's `FsStore`; the second receive
/// must finish and the final bytes must be perfect regardless of how far the first
/// attempt got. This guards the #1 regression risk across iroh-blobs version bumps.
#[tokio::test(flavor = "multi_thread")]
async fn resume_after_interrupt() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let src = work.path().join("big.bin");
    let payload = make_payload(64 * 1024 * 1024);
    std::fs::write(&src, &payload).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;

    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    // First attempt: cancel deterministically once real bytes have landed.
    let out = work.path().join("out");
    let (rid, mut rs) = receiver.receive(ticket.clone(), out.clone()).await.unwrap();
    let mut interrupted_at = 0u64;
    while let Some(ev) = rs.next().await {
        match ev {
            Progress::Transferring { offset, .. } if offset > 0 => {
                interrupted_at = offset;
                receiver.cancel(rid).await;
                break;
            }
            Progress::Done { .. } => break, // raced to completion; still correct
            Progress::Error { message, .. } => panic!("first attempt error: {message}"),
            _ => {}
        }
    }
    drain_until_terminal(&mut rs).await;

    // Second attempt: must finish, reusing whatever partial is already in the store.
    let (_rid2, mut rs2) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs2).await;

    let got = std::fs::read(out.join("big.bin")).unwrap();
    assert_eq!(
        got.len(),
        payload.len(),
        "resumed file size must match source"
    );
    // Avoid dumping 64 MiB on failure: compare without assert_eq! formatting.
    assert!(
        got == payload,
        "resumed file must be byte-perfect (first attempt reached {interrupted_at} bytes)"
    );
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

/// The SENDER sees live progress (via iroh-blobs provider events): when a
/// receiver fetches, the send stream emits PeerJoined → Transferring… → Done.
#[tokio::test(flavor = "multi_thread")]
async fn sender_sees_progress() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let src = work.path().join("payload.bin");
    let payload = make_payload(3 * 1024 * 1024);
    std::fs::write(&src, &payload).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;

    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    // Drive the receiver to completion in the background.
    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket, out).await.unwrap();
    tokio::spawn(async move { while rs.next().await.is_some() {} });

    // The sender stream should report a peer joining and the transfer completing.
    let (peer, done) = tokio::time::timeout(std::time::Duration::from_secs(60), async {
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

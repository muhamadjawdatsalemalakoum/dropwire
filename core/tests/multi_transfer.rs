//! Multiple transfers run at the same time: one sender serving two contents, and
//! one receiver pulling both concurrently, all completing byte-perfect. Also:
//! what happens when the same content is sent twice.

mod common;
use std::time::Duration;

use common::{local_core, make_payload, wait_done, wait_ready};
use irohcore::{Direction, Progress, ProgressStream};
use tokio_stream::StreamExt;

/// Read a stream until `want` matches.
async fn wait_for(
    stream: &mut ProgressStream,
    what: &str,
    want: impl Fn(&Progress) -> bool,
) -> Progress {
    let fut = async {
        while let Some(ev) = stream.next().await {
            if want(&ev) {
                return ev;
            }
        }
        panic!("stream ended before {what}");
    };
    tokio::time::timeout(Duration::from_secs(30), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// Everything a send stream says within a short quiet window.
async fn drain_for(stream: &mut ProgressStream, window: Duration) -> Vec<Progress> {
    let mut seen = Vec::new();
    while let Ok(Some(ev)) = tokio::time::timeout(window, stream.next()).await {
        seen.push(ev);
    }
    seen
}

#[tokio::test(flavor = "multi_thread")]
async fn two_transfers_run_concurrently() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let a = work.path().join("a.bin");
    let b = work.path().join("b.bin");
    let pa = make_payload(3 * 1024 * 1024);
    let pb = make_payload(5 * 1024 * 1024);
    std::fs::write(&a, &pa).unwrap();
    std::fs::write(&b, &pb).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;

    // Two independent sends from one sender → two tickets, both served at once.
    let (_s1, mut ss1) = sender.send(a).await.unwrap();
    let t1 = wait_ready(&mut ss1).await;
    let (_s2, mut ss2) = sender.send(b).await.unwrap();
    let t2 = wait_ready(&mut ss2).await;

    // Two receives running at the same time on one receiver.
    let out1 = work.path().join("out1");
    let out2 = work.path().join("out2");
    let (_r1, mut rs1) = receiver.receive(t1, out1.clone()).await.unwrap();
    let (_r2, mut rs2) = receiver.receive(t2, out2.clone()).await.unwrap();

    // Drive both concurrently; both must finish.
    tokio::join!(wait_done(&mut rs1), wait_done(&mut rs2));

    assert_eq!(std::fs::read(out1.join("a.bin")).unwrap(), pa);
    assert_eq!(std::fs::read(out2.join("b.bin")).unwrap(), pb);

    // Both show up in history as concurrent receives.
    let recvs = receiver.transfers().await;
    assert!(
        recvs.len() >= 2,
        "both concurrent transfers are recorded in history"
    );
}

/// Sending the same files again while the first send is still waiting for its
/// receiver is refused with a clear reason. The first send is untouched: its
/// code still works and its progress still reaches its own card.
#[tokio::test(flavor = "multi_thread")]
async fn same_content_again_is_refused_while_undelivered() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let r1_data = tempfile::tempdir().unwrap();
    let r2_data = tempfile::tempdir().unwrap();

    let src = work.path().join("same.bin");
    let payload = make_payload(512 * 1024);
    std::fs::write(&src, &payload).unwrap();

    let sender = local_core(send_data.path()).await;
    let r1 = local_core(r1_data.path()).await;
    let r2 = local_core(r2_data.path()).await;

    let (_s1, mut ss1) = sender.send(src.clone()).await.unwrap();
    let t1 = wait_ready(&mut ss1).await;

    let (_s2, mut ss2) = sender.send(src).await.unwrap();
    let refused = wait_for(&mut ss2, "the second send's outcome", |ev| {
        matches!(ev, Progress::Error { .. } | Progress::Ready { .. })
    })
    .await;
    match refused {
        Progress::Error { message, .. } => assert_eq!(
            message,
            "This is already being shared. Use its code, or stop sharing it first."
        ),
        other => panic!("the second send must be refused, got {other:?}"),
    }

    let out = work.path().join("out");
    let (_rid, mut rs) = r1.receive(t1.clone(), out.clone()).await.unwrap();
    wait_done(&mut rs).await;
    assert_eq!(std::fs::read(out.join("same.bin")).unwrap(), payload);

    let seen = drain_for(&mut ss1, Duration::from_millis(500)).await;
    assert!(
        seen.iter()
            .any(|e| matches!(e, Progress::PeerJoined { .. })),
        "the first send must see its receiver: {seen:?}"
    );
    assert!(
        seen.iter().any(|e| matches!(e, Progress::Done { .. })),
        "the first send must see the delivery: {seen:?}"
    );
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e, Progress::Cancelled { .. } | Progress::Error { .. })),
        "the first send must keep going: {seen:?}"
    );

    // Still one-to-one, and only one send is on record.
    assert!(r2.inspect(t1).await.is_err(), "a second device is refused");
    let sends = sender
        .transfers()
        .await
        .into_iter()
        .filter(|r| r.direction == Direction::Send)
        .count();
    assert_eq!(sends, 1, "the refused send leaves no record");
}

/// Once the first send has delivered, sending the same files again (Resend)
/// takes over: the old send stops, and the new code starts fresh and unbound,
/// so it can go to someone new. The old send's teardown must not remove the
/// new send's registration.
#[tokio::test(flavor = "multi_thread")]
async fn same_content_again_takes_over_once_delivered() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let r1_data = tempfile::tempdir().unwrap();
    let r2_data = tempfile::tempdir().unwrap();
    let r3_data = tempfile::tempdir().unwrap();

    let src = work.path().join("again.bin");
    let payload = make_payload(512 * 1024);
    std::fs::write(&src, &payload).unwrap();

    let sender = local_core(send_data.path()).await;
    let r1 = local_core(r1_data.path()).await;
    let r2 = local_core(r2_data.path()).await;
    let r3 = local_core(r3_data.path()).await;

    let (_s1, mut ss1) = sender.send(src.clone()).await.unwrap();
    let t1 = wait_ready(&mut ss1).await;
    let (_rid, mut rs) = r1.receive(t1, work.path().join("out1")).await.unwrap();
    wait_done(&mut rs).await;
    wait_for(&mut ss1, "the first delivery", |ev| {
        matches!(ev, Progress::Done { .. })
    })
    .await;

    let (_s2, mut ss2) = sender.send(src).await.unwrap();
    let t2 = wait_ready(&mut ss2).await;
    wait_for(&mut ss1, "the old send to stop", |ev| {
        matches!(ev, Progress::Cancelled { .. })
    })
    .await;

    // The new code is live and unbound: a new device can take it.
    let out2 = work.path().join("out2");
    let (_rid2, mut rs2) = r2.receive(t2.clone(), out2.clone()).await.unwrap();
    wait_done(&mut rs2).await;
    assert_eq!(std::fs::read(out2.join("again.bin")).unwrap(), payload);

    let seen = drain_for(&mut ss2, Duration::from_millis(500)).await;
    assert!(
        seen.iter()
            .any(|e| matches!(e, Progress::PeerJoined { .. })),
        "the new send must see its receiver: {seen:?}"
    );
    assert!(
        seen.iter().any(|e| matches!(e, Progress::Done { .. })),
        "the new send must see the delivery: {seen:?}"
    );

    // And it is one-to-one again.
    assert!(r3.inspect(t2).await.is_err(), "a third device is refused");
}

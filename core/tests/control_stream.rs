//! The control channel — a free two-way side channel between peers (presence,
//! instant decline, chat) on its own ALPN, alongside the file transfer.

mod common;
use std::time::Duration;

use common::{local_core, make_payload, wait_done, wait_ready};
use irohcore::{CtrlMsg, Progress};
use tokio_stream::StreamExt;

async fn ticket_for(work: &std::path::Path, sender: &irohcore::Core) -> String {
    let src = work.join("f.bin");
    std::fs::write(&src, make_payload(1024)).unwrap();
    let (_sid, mut ss) = sender.send(src).await.unwrap();
    wait_ready(&mut ss).await
}

#[tokio::test(flavor = "multi_thread")]
async fn control_decline_reaches_the_sender() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;
    let ticket = ticket_for(work.path(), &sender).await;

    // Sender listens; receiver declines over the control channel.
    let mut ctrl = sender.subscribe_control();
    receiver
        .send_control(ticket, CtrlMsg::Decline)
        .await
        .unwrap();

    let got = tokio::time::timeout(std::time::Duration::from_secs(10), ctrl.recv())
        .await
        .expect("timed out waiting for control message")
        .expect("control channel closed");
    assert_eq!(got, CtrlMsg::Decline);
}

#[tokio::test(flavor = "multi_thread")]
async fn control_chat_roundtrips() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;
    let ticket = ticket_for(work.path(), &sender).await;

    let mut ctrl = sender.subscribe_control();
    let msg = CtrlMsg::Chat {
        text: "on my way!".to_string(),
    };
    receiver.send_control(ticket, msg.clone()).await.unwrap();

    let got = tokio::time::timeout(std::time::Duration::from_secs(10), ctrl.recv())
        .await
        .expect("timed out waiting for chat")
        .expect("control channel closed");
    assert_eq!(got, msg);
}

/// Whether the send stream reports an event matching `want` within `wait`.
async fn saw(
    stream: &mut irohcore::ProgressStream,
    wait: std::time::Duration,
    want: impl Fn(&Progress) -> bool,
) -> bool {
    let fut = async {
        while let Some(ev) = stream.next().await {
            if want(&ev) {
                return true;
            }
        }
        false
    };
    tokio::time::timeout(wait, fut).await.unwrap_or(false)
}

/// The receiver previews, then declines. The sender hears it on the send's
/// own stream, and the code is released: it now works for someone else.
#[tokio::test(flavor = "multi_thread")]
async fn decline_frees_the_code_for_someone_else() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let a_data = tempfile::tempdir().unwrap();
    let b_data = tempfile::tempdir().unwrap();

    let sender = local_core(send_data.path()).await;
    let a = local_core(a_data.path()).await;
    let b = local_core(b_data.path()).await;

    let src = work.path().join("f.bin");
    std::fs::write(&src, make_payload(64 * 1024)).unwrap();
    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    a.inspect(ticket.clone()).await.unwrap();
    assert!(
        b.inspect(ticket.clone()).await.is_err(),
        "bound to the first device"
    );

    a.decline(ticket.clone()).await.unwrap();
    assert!(
        saw(&mut ss, Duration::from_secs(10), |ev| {
            matches!(ev, Progress::Declined { .. })
        })
        .await,
        "the sender must hear the decline"
    );

    let preview = b.inspect(ticket).await.unwrap();
    assert_eq!(preview.file_count, 1, "the code now works for someone else");
}

/// Anyone can hold a copy of the code, but only the device it is bound to can
/// decline it. A decline from anyone else changes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn only_the_bound_device_can_decline() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let a_data = tempfile::tempdir().unwrap();
    let b_data = tempfile::tempdir().unwrap();

    let sender = local_core(send_data.path()).await;
    let a = local_core(a_data.path()).await;
    let b = local_core(b_data.path()).await;

    let src = work.path().join("f.bin");
    let payload = make_payload(64 * 1024);
    std::fs::write(&src, &payload).unwrap();
    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    a.inspect(ticket.clone()).await.unwrap();
    b.decline(ticket.clone()).await.unwrap();
    assert!(
        !saw(&mut ss, Duration::from_secs(1), |ev| {
            matches!(ev, Progress::Declined { .. })
        })
        .await,
        "a decline from a device the code is not bound to is ignored"
    );
    assert!(
        b.inspect(ticket.clone()).await.is_err(),
        "the code stays bound to the first device"
    );

    let out = work.path().join("out");
    let (_rid, mut rs) = a.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs).await;
    assert_eq!(std::fs::read(out.join("f.bin")).unwrap(), payload);
}

/// An older receiver's decline names no code. It is still honored when that
/// device holds exactly one of this sender's live codes, and ignored when it
/// holds two (there would be no telling which one it meant).
#[cfg(feature = "test-utils")]
#[tokio::test(flavor = "multi_thread")]
async fn decline_without_a_code_is_honored_only_when_unambiguous() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let a_data = tempfile::tempdir().unwrap();

    let sender = local_core(send_data.path()).await;
    let a = local_core(a_data.path()).await;

    let one = work.path().join("one.bin");
    let two = work.path().join("two.bin");
    std::fs::write(&one, make_payload(4 * 1024)).unwrap();
    std::fs::write(&two, make_payload(8 * 1024)).unwrap();
    let (_s1, mut ss1) = sender.send(one).await.unwrap();
    let t1 = wait_ready(&mut ss1).await;
    let (s2, mut ss2) = sender.send(two).await.unwrap();
    let t2 = wait_ready(&mut ss2).await;

    // Bound to both: a bare decline is ambiguous, so nothing happens.
    a.inspect(t1.clone()).await.unwrap();
    a.inspect(t2).await.unwrap();
    a.send_control_to(sender.test_dial_addr(), CtrlMsg::Decline)
        .await
        .unwrap();
    let declined = |ev: &Progress| matches!(ev, Progress::Declined { .. });
    assert!(!saw(&mut ss1, Duration::from_secs(1), declined).await);
    assert!(!saw(&mut ss2, Duration::from_millis(200), declined).await);

    // Bound to one live code only: the bare decline is clearly about it.
    sender.cancel(s2).await;
    common::drain_until_terminal(&mut ss2).await;
    a.send_control_to(sender.test_dial_addr(), CtrlMsg::Decline)
        .await
        .unwrap();
    assert!(saw(&mut ss1, Duration::from_secs(10), declined).await);
}

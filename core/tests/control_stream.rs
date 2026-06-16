//! The control channel — a free two-way side channel between peers (presence,
//! instant decline, chat) on its own ALPN, alongside the file transfer.

mod common;
use common::{local_core, make_payload, wait_ready};
use irohcore::CtrlMsg;

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

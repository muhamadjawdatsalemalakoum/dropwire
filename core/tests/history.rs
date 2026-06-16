//! The local transfer catalog records enough to re-share a send (source path)
//! and to resume a receive (ticket + dest).

mod common;
use common::{local_core, make_payload, wait_done, wait_ready};
use irohcore::Direction;

#[tokio::test(flavor = "multi_thread")]
async fn send_record_keeps_the_source_path() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();

    let src = work.path().join("doc.bin");
    std::fs::write(&src, make_payload(2048)).unwrap();

    let sender = local_core(send_data.path()).await;
    let (_sid, mut ss) = sender.send(src.clone()).await.unwrap();
    let _ticket = wait_ready(&mut ss).await;

    let recs = sender.transfers().await;
    let rec = recs
        .iter()
        .find(|r| matches!(r.direction, Direction::Send))
        .expect("a send record");
    assert_eq!(
        rec.source.as_deref(),
        Some(src.to_string_lossy().as_ref()),
        "send must remember its source path so it can be re-shared"
    );
    assert!(rec.dest.is_none(), "a send has no destination");
    assert!(!rec.ticket.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_record_keeps_ticket_and_dest() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let src = work.path().join("hi.bin");
    std::fs::write(&src, make_payload(4096)).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;
    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket.clone(), out.clone()).await.unwrap();
    wait_done(&mut rs).await;

    let recs = receiver.transfers().await;
    let rec = recs
        .iter()
        .find(|r| matches!(r.direction, Direction::Receive))
        .expect("a receive record");
    assert_eq!(rec.dest.as_deref(), Some(out.to_string_lossy().as_ref()));
    assert!(rec.source.is_none(), "a receive has no source path");
    assert_eq!(
        rec.ticket, ticket,
        "receive remembers its ticket (for resume)"
    );
}

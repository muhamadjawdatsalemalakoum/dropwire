//! "Preview before you accept" — `Core::inspect` must report the transfer's
//! file list, sizes, count, and total WITHOUT downloading any file content.

use irohcore::CoreError;

mod common;
use common::{dir_size, local_core, make_payload, wait_done, wait_ready};

#[tokio::test(flavor = "multi_thread")]
async fn inspect_lists_files_without_downloading() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    // A folder: a tiny file and a deliberately large one (16 MiB).
    let dir = work.path().join("docs");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.txt"), make_payload(1000)).unwrap();
    std::fs::write(
        dir.join("sub").join("big.bin"),
        make_payload(16 * 1024 * 1024),
    )
    .unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;

    let (_sid, mut ss) = sender.send(dir).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let preview = receiver.inspect(ticket).await.unwrap();

    // The file list, count, and total are reported correctly.
    assert_eq!(preview.file_count, 2);
    assert_eq!(preview.files.len(), 2);
    assert_eq!(preview.total_bytes, 1000 + 16 * 1024 * 1024);

    let mut names: Vec<_> = preview.files.iter().map(|f| f.name.clone()).collect();
    names.sort();
    assert_eq!(
        names,
        vec!["docs/a.txt".to_string(), "docs/sub/big.bin".to_string()]
    );

    let big = preview
        .files
        .iter()
        .find(|f| f.name.ends_with("big.bin"))
        .expect("big.bin in preview");
    assert_eq!(big.size, 16 * 1024 * 1024, "per-file size must be exact");

    // CRUCIAL: inspect must be metadata-only. The receiver's blob store must NOT
    // contain the 16 MiB payload — only tiny metadata was fetched over the wire.
    let store_bytes = dir_size(&recv_data.path().join("blobs"));
    assert!(
        store_bytes < 4 * 1024 * 1024,
        "inspect must not download content (receiver store grew to {store_bytes} bytes)"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn inspect_single_file() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let src = work.path().join("report.pdf");
    std::fs::write(&src, make_payload(123_456)).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;

    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let preview = receiver.inspect(ticket).await.unwrap();
    assert_eq!(preview.file_count, 1);
    assert_eq!(preview.files[0].name, "report.pdf");
    assert_eq!(preview.files[0].size, 123_456);
    assert_eq!(preview.total_bytes, 123_456);
}

#[tokio::test(flavor = "multi_thread")]
async fn inspect_then_receive_completes() {
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

    // Preview first…
    let preview = receiver.inspect(ticket.clone()).await.unwrap();
    assert_eq!(preview.file_count, 1);

    // …then accept: the normal receive must still complete byte-perfect.
    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs).await;
    assert_eq!(std::fs::read(out.join("hello.bin")).unwrap(), payload);
}

#[tokio::test(flavor = "multi_thread")]
async fn inspect_offline_sender_is_unreachable() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let src = work.path().join("f.bin");
    std::fs::write(&src, make_payload(1024)).unwrap();

    let sender = local_core(send_data.path()).await;
    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    // The sender goes away — the ticket now points at nothing.
    sender.shutdown().await.unwrap();

    let receiver = local_core(recv_data.path()).await;
    let err = receiver.inspect(ticket).await.unwrap_err();
    assert!(
        matches!(err, CoreError::Unreachable(_)),
        "an offline sender must surface as Unreachable, got: {err:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn inspect_invalid_ticket_errors() {
    let recv_data = tempfile::tempdir().unwrap();
    let receiver = local_core(recv_data.path()).await;

    let err = receiver
        .inspect("definitely-not-a-ticket".to_string())
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::InvalidTicket(_)));
}

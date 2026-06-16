//! Selective download — `Core::receive_selected` fetches and writes ONLY the
//! chosen files, leaving the rest neither downloaded nor on disk.

mod common;
use common::{local_core, make_payload, wait_done, wait_ready};

#[tokio::test(flavor = "multi_thread")]
async fn receive_selected_downloads_only_chosen_files() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    // Three files; names sort a.bin, b.bin, c.bin → file indices 0, 1, 2.
    let dir = work.path().join("set");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.bin"), make_payload(1000)).unwrap();
    std::fs::write(dir.join("b.bin"), make_payload(2000)).unwrap();
    std::fs::write(dir.join("c.bin"), make_payload(3000)).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;
    let (_sid, mut ss) = sender.send(dir).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    // Choose files 0 and 2 (a.bin, c.bin); skip 1 (b.bin).
    let out = work.path().join("out");
    let (_rid, mut rs) = receiver
        .receive_selected(ticket, out.clone(), vec![0, 2])
        .await
        .unwrap();
    wait_done(&mut rs).await;

    assert_eq!(
        std::fs::read(out.join("set").join("a.bin")).unwrap(),
        make_payload(1000)
    );
    assert_eq!(
        std::fs::read(out.join("set").join("c.bin")).unwrap(),
        make_payload(3000)
    );
    assert!(
        !out.join("set").join("b.bin").exists(),
        "unselected file must not be written"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_selected_single_file_from_folder() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let dir = work.path().join("pics");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("keep.bin"), make_payload(4096)).unwrap();
    std::fs::write(dir.join("skip.bin"), make_payload(8 * 1024 * 1024)).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;
    let (_sid, mut ss) = sender.send(dir).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    // keep.bin sorts before skip.bin → index 0.
    let out = work.path().join("out");
    let (_rid, mut rs) = receiver
        .receive_selected(ticket, out.clone(), vec![0])
        .await
        .unwrap();
    wait_done(&mut rs).await;

    assert_eq!(
        std::fs::read(out.join("pics").join("keep.bin")).unwrap(),
        make_payload(4096)
    );
    assert!(!out.join("pics").join("skip.bin").exists());

    // The 8 MiB unselected file must not have been downloaded into the store.
    let store_bytes = common::dir_size(&recv_data.path().join("blobs"));
    assert!(
        store_bytes < 4 * 1024 * 1024,
        "unselected content must not be fetched (store grew to {store_bytes} bytes)"
    );
}

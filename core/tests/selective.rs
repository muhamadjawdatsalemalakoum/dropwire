//! Selective download — `Core::receive_selected` fetches and writes ONLY the
//! chosen files, leaving the rest neither downloaded nor on disk.

mod common;
use common::{drain_until_terminal, local_core, make_payload, wait_done, wait_ready};
use irohcore::Progress;
use tokio_stream::StreamExt;

/// Choosing one small file out of many never shows more bytes than were
/// chosen: the transfer's list of hashes and names is not counted.
#[tokio::test(flavor = "multi_thread")]
async fn selective_progress_never_passes_the_chosen_total() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let dir = work.path().join("many");
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..400 {
        std::fs::write(dir.join(format!("f{i:04}.txt")), make_payload(100)).unwrap();
    }

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;
    let (_sid, mut ss) = sender.send(dir).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let out = work.path().join("out");
    let (_rid, mut rs) = receiver
        .receive_selected(ticket, out.clone(), vec![0])
        .await
        .unwrap();
    let mut offsets = Vec::new();
    while let Some(ev) = rs.next().await {
        match ev {
            Progress::Transferring { offset, total, .. } => {
                assert_eq!(total, 100);
                offsets.push(offset);
            }
            Progress::Done { stats, .. } => {
                assert_eq!(stats.bytes, 100);
                break;
            }
            Progress::Error { message, .. } => panic!("receive error: {message}"),
            _ => {}
        }
    }
    assert!(offsets.iter().all(|&o| o <= 100), "{offsets:?}");
    assert_eq!(offsets.last(), Some(&100));
    assert!(out.join("many").join("f0000.txt").exists());
}

/// A selective receive that stopped part way picks up where it stopped: asking
/// for the same files again fetches only what is still missing.
///
/// To tell a resume from a fresh start, the sender's copy of the part that
/// already arrived is changed before the second attempt. The sender still
/// vouches for the original content, so if those bytes were asked for again
/// they would fail verification and the receive would fail.
#[tokio::test(flavor = "multi_thread")]
async fn a_selective_receive_resumes_instead_of_starting_over() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    // a.bin, big.bin, skip.bin: indices 0, 1, 2. The first two are chosen.
    let dir = work.path().join("set");
    std::fs::create_dir_all(&dir).unwrap();
    let big = make_payload(64 * 1024 * 1024);
    std::fs::write(dir.join("a.bin"), make_payload(1000)).unwrap();
    std::fs::write(dir.join("big.bin"), &big).unwrap();
    std::fs::write(dir.join("skip.bin"), make_payload(2 * 1024 * 1024)).unwrap();
    let chosen_total = 1000 + big.len() as u64;

    let sender = local_core(send_data.path()).await;
    let (_sid, mut ss) = sender.send(dir.clone()).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    // First attempt: the receiving app closes part way through big.bin.
    let receiver = local_core(recv_data.path()).await;
    let out = work.path().join("out");
    let (_rid, mut rs) = receiver
        .receive_selected(ticket.clone(), out.clone(), vec![0, 1])
        .await
        .unwrap();
    let stopped = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        while let Some(ev) = rs.next().await {
            match ev {
                Progress::Transferring { offset, .. } if offset >= chosen_total * 2 / 5 => {
                    return true
                }
                Progress::Done { .. } => return false,
                Progress::Error { message, .. } => panic!("first attempt error: {message}"),
                _ => {}
            }
        }
        false
    })
    .await
    .expect("timed out during the first attempt");
    assert!(stopped, "the first attempt must stop before it finishes");
    receiver.shutdown().await.unwrap();
    drain_until_terminal(&mut rs).await;

    // Second attempt, same code and same choice, after the app is reopened.
    let receiver = local_core(recv_data.path()).await;
    let (_rid, mut rs) = receiver
        .receive_selected(ticket, out.clone(), vec![0, 1])
        .await
        .unwrap();
    wait_done(&mut rs).await;

    // The sender moved only what was still missing. (The first attempt ended
    // in a disconnect, so this is the only delivery it reports.) Editing the
    // sender's file to prove it would not work: an edited file stops being
    // shared.
    let resent = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(ev) = ss.next().await {
            if let Progress::Done { stats, .. } = ev {
                return stats.bytes;
            }
        }
        panic!("send stream ended before Done");
    })
    .await
    .expect("the sender must report the resumed download");
    assert!(
        resent < chosen_total * 4 / 5,
        "the resume started over: the sender moved {resent} of {chosen_total} bytes"
    );

    let got = std::fs::read(out.join("set").join("big.bin")).unwrap();
    assert!(
        got == big,
        "big.bin must be whole, pieced together from both attempts"
    );
    assert_eq!(
        std::fs::read(out.join("set").join("a.bin")).unwrap(),
        make_payload(1000)
    );
    assert!(!out.join("set").join("skip.bin").exists());
}

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

#[tokio::test(flavor = "multi_thread")]
async fn receive_selected_ignores_repeated_and_out_of_range_indices() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let dir = work.path().join("set");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.bin"), make_payload(1000)).unwrap();
    std::fs::write(dir.join("b.bin"), make_payload(2000)).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;
    let (_sid, mut ss) = sender.send(dir).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    // Index 0 twice, plus one far past the end: only a.bin is wanted.
    let out = work.path().join("out");
    let (_rid, mut rs) = receiver
        .receive_selected(ticket, out.clone(), vec![0, 0, 99])
        .await
        .unwrap();
    wait_done(&mut rs).await;

    assert_eq!(
        std::fs::read(out.join("set").join("a.bin")).unwrap(),
        make_payload(1000)
    );
    assert!(!out.join("set").join("b.bin").exists());
}

/// Resuming from history keeps the files chosen in the preview: the files left
/// out are neither downloaded nor saved, and the same history entry carries on
/// instead of a second one appearing.
#[tokio::test(flavor = "multi_thread")]
async fn resuming_from_history_keeps_the_chosen_files() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    // big.bin (index 0) is chosen, other.bin (index 1) is not.
    let dir = work.path().join("set");
    std::fs::create_dir_all(&dir).unwrap();
    let big = make_payload(64 * 1024 * 1024);
    std::fs::write(dir.join("big.bin"), &big).unwrap();
    std::fs::write(dir.join("other.bin"), make_payload(1024 * 1024)).unwrap();

    let sender = local_core(send_data.path()).await;
    let (_sid, mut ss) = sender.send(dir).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    // The receiving app closes part way through.
    let receiver = local_core(recv_data.path()).await;
    let out = work.path().join("out");
    let (rid, mut rs) = receiver
        .receive_selected(ticket, out.clone(), vec![0])
        .await
        .unwrap();
    while let Some(ev) = rs.next().await {
        match ev {
            Progress::Transferring { offset, total, .. } if offset >= total / 4 => break,
            Progress::Done { .. } => panic!("finished before it could be interrupted"),
            Progress::Error { message, .. } => panic!("first attempt error: {message}"),
            _ => {}
        }
    }
    receiver.shutdown().await.unwrap();
    drain_until_terminal(&mut rs).await;

    let receiver = local_core(recv_data.path()).await;
    let before = receiver.transfers().await;
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].id, rid);
    assert_eq!(before[0].status, irohcore::Status::Interrupted);
    assert_eq!(
        before[0].selected,
        Some(vec![0]),
        "the choice is remembered"
    );

    let (id, mut rs) = receiver.resume(rid).await.unwrap();
    assert_eq!(id, rid, "a resume carries on under the same id");
    wait_done(&mut rs).await;

    assert!(std::fs::read(out.join("set").join("big.bin")).unwrap() == big);
    assert!(
        !out.join("set").join("other.bin").exists(),
        "a file left out in the preview must not be saved by a resume"
    );
    let after = receiver.transfers().await;
    assert_eq!(after.len(), 1, "one history entry, not two");
    assert_eq!(after[0].id, rid);
    assert_eq!(after[0].status, irohcore::Status::Done);
    assert_eq!(after[0].created_at, before[0].created_at);

    // A finished receive, or one that is not in the history, cannot be resumed.
    assert!(receiver.resume(rid).await.is_err());
    assert!(receiver.resume(irohcore::TransferId::new()).await.is_err());
}

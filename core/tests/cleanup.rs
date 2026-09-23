//! What the blob store keeps. Received content is copied out to the chosen
//! folder, so the store's own copy must go once it is no longer needed, while
//! a send's original files must never be touched by that cleanup.
//!
//! Every engine in this file collects garbage every 100 ms (instead of every
//! minute), so the tests can watch the store shrink, and so every transfer here
//! also runs with the collector busy around it.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{dir_size, local_core, make_payload, wait_done, wait_ready};
use irohcore::{Core, Progress, ProgressStream};
use tokio_stream::StreamExt;

fn fast_gc() {
    irohcore::set_gc_interval_for_tests(Duration::from_millis(100));
}

/// Bytes held in the store's data folder (the database file is not counted).
fn store_data(data_dir: &Path) -> u64 {
    dir_size(&data_dir.join("blobs").join("data"))
}

/// Files in the store's data folder with the given extension.
fn store_files(data_dir: &Path, ext: &str) -> Vec<PathBuf> {
    std::fs::read_dir(data_dir.join("blobs").join("data"))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == ext))
                .collect()
        })
        .unwrap_or_default()
}

/// Wait (up to 20 s) until `check` holds.
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Drive a receive until it has moved at least `bytes`, returning early if it
/// finishes first.
async fn wait_for_bytes(stream: &mut ProgressStream, bytes: u64) {
    let fut = async {
        while let Some(ev) = stream.next().await {
            match ev {
                Progress::Transferring { offset, .. } if offset >= bytes => return,
                Progress::Done { .. } => panic!("finished before it could be interrupted"),
                Progress::Error { message, .. } => panic!("receive error: {message}"),
                _ => {}
            }
        }
        panic!("receive stream ended early");
    };
    tokio::time::timeout(Duration::from_secs(60), fut)
        .await
        .expect("timed out waiting for bytes to move");
}

/// Drain a stream to its end event and return it.
async fn wait_terminal(stream: &mut ProgressStream) -> Progress {
    let fut = async {
        while let Some(ev) = stream.next().await {
            if matches!(
                ev,
                Progress::Done { .. } | Progress::Error { .. } | Progress::Cancelled { .. }
            ) {
                return ev;
            }
        }
        panic!("stream ended without a final event");
    };
    tokio::time::timeout(Duration::from_secs(60), fut)
        .await
        .expect("timed out waiting for the transfer to end")
}

/// Sent files are imported by reference, so the store points at the user's
/// own file. Ending the send and letting the collector drop the entry must
/// leave that file exactly as it was.
#[tokio::test(flavor = "multi_thread")]
async fn collecting_an_ended_send_leaves_the_original_intact() {
    fast_gc();
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    // Big enough to be referenced in place (not copied into the database) and
    // to have its verification tree stored as a file we can watch.
    let src = work.path().join("original.bin");
    let payload = make_payload(8 * 1024 * 1024);
    std::fs::write(&src, &payload).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;
    let (sid, mut ss) = sender.send(src.clone()).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    // A live send survives many collections and still serves.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs).await;
    assert_eq!(std::fs::read(out.join("original.bin")).unwrap(), payload);
    assert!(
        !store_files(send_data.path(), "obao4").is_empty(),
        "the send should have an entry in the store while it is live"
    );

    sender.cancel(sid).await;
    while let Some(ev) = ss.next().await {
        if matches!(ev, Progress::Cancelled { .. }) {
            break;
        }
    }

    // The collector drops the ended send's entry...
    eventually("the ended send's entry is collected", || {
        store_files(send_data.path(), "obao4").is_empty()
    })
    .await;
    // ...and the user's file is untouched.
    let after = std::fs::read(&src).expect("the original must still exist");
    assert_eq!(
        after.len(),
        payload.len(),
        "the original must not be truncated"
    );
    assert!(
        after == payload,
        "the original must be byte-for-byte unchanged"
    );
}

/// Once every file is saved, the store's copy is reclaimed.
#[tokio::test(flavor = "multi_thread")]
async fn a_saved_receive_leaves_no_copy_in_the_store() {
    fast_gc();
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let dir = work.path().join("pics");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.bin"), make_payload(16 * 1024 * 1024)).unwrap();
    std::fs::write(dir.join("b.bin"), make_payload(3 * 1024 * 1024)).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;
    let (_sid, mut ss) = sender.send(dir).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs).await;

    assert_eq!(
        std::fs::read(out.join("pics").join("a.bin")).unwrap(),
        make_payload(16 * 1024 * 1024)
    );
    assert_eq!(
        std::fs::read(out.join("pics").join("b.bin")).unwrap(),
        make_payload(3 * 1024 * 1024)
    );
    eventually("the received copy is collected", || {
        store_data(recv_data.path()) < 64 * 1024
    })
    .await;
}

/// A cancel frees what the receive had downloaded so far.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_receive_frees_its_partial_data() {
    fast_gc();
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let src = work.path().join("big.bin");
    std::fs::write(&src, make_payload(64 * 1024 * 1024)).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;
    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let out = work.path().join("out");
    let (rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_for_bytes(&mut rs, 4 * 1024 * 1024).await;
    receiver.cancel(rid).await;
    assert!(matches!(
        wait_terminal(&mut rs).await,
        Progress::Cancelled { .. }
    ));

    eventually("the cancelled receive's data is collected", || {
        store_data(recv_data.path()) < 64 * 1024
    })
    .await;
    assert!(!out.join("big.bin").exists());
}

/// A receive that fails part way keeps what arrived, across collections and
/// across a restart, so it can be resumed; clearing it from history lets the
/// data go.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_receive_keeps_its_data_until_history_is_cleared() {
    fast_gc();
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let src = work.path().join("big.bin");
    std::fs::write(&src, make_payload(64 * 1024 * 1024)).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;
    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_for_bytes(&mut rs, 4 * 1024 * 1024).await;
    // The sender goes away mid-transfer.
    sender.shutdown().await.unwrap();
    assert!(matches!(
        wait_terminal(&mut rs).await,
        Progress::Error { .. }
    ));

    // Many collections later, the partial data is still there.
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let kept = store_data(recv_data.path());
    assert!(
        kept >= 1024 * 1024,
        "a failed receive must keep its data for a resume (store holds {kept} bytes)"
    );

    // And after a restart.
    receiver.shutdown().await.unwrap();
    let receiver: Core = local_core(recv_data.path()).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let kept = store_data(recv_data.path());
    assert!(
        kept >= 1024 * 1024,
        "the data must survive a restart (store holds {kept} bytes)"
    );

    receiver.clear_transfers().await;
    eventually("cleared history lets the data go", || {
        store_data(recv_data.path()) < 64 * 1024
    })
    .await;
}

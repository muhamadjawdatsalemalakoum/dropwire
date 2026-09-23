//! Receiving something this device already has. Files a device sends are kept
//! by reference (read from where they sit on disk), so the store's copy of a
//! file sent from here is only as good as that file.

mod common;
use std::time::Duration;

use common::{drain_until_terminal, local_core, make_payload, wait_done, wait_ready};
use irohcore::Progress;

/// Alice sends a file, then keeps editing it in place. Later Bob sends her the
/// original. Her store already "has" that content, by reference to the file
/// she edited, so no bytes would come over the network; the edited bytes must
/// not be saved as the verified original.
#[tokio::test(flavor = "multi_thread")]
async fn an_edited_local_copy_is_not_passed_off_as_received() {
    let work = tempfile::tempdir().unwrap();
    let alice_data = tempfile::tempdir().unwrap();
    let bob_data = tempfile::tempdir().unwrap();

    let original = make_payload(256 * 1024);
    let mine = work.path().join("report.bin");
    std::fs::write(&mine, &original).unwrap();

    let alice = local_core(alice_data.path()).await;
    let bob = local_core(bob_data.path()).await;

    // Alice shares it, then stops sharing.
    let (sid, mut ss) = alice.send(mine.clone()).await.unwrap();
    wait_ready(&mut ss).await;
    alice.cancel(sid).await;
    drain_until_terminal(&mut ss).await;

    // She edits it in place.
    std::fs::write(&mine, vec![7u8; original.len()]).unwrap();

    // Bob has the original and sends it to her.
    std::fs::create_dir_all(work.path().join("bob")).unwrap();
    let theirs = work.path().join("bob").join("report-original.bin");
    std::fs::write(&theirs, &original).unwrap();
    let (_bid, mut bs) = bob.send(theirs).await.unwrap();
    let ticket = wait_ready(&mut bs).await;

    let out = work.path().join("inbox");
    let (_rid, mut rs) = alice.receive(ticket, out.clone()).await.unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(30), drain_until_terminal(&mut rs))
        .await
        .expect("the receive must end");
    let saved = out.join("report-original.bin");
    match ended {
        Some(Progress::Error { message, .. }) => {
            assert!(
                message.starts_with("report-original.bin could not be verified"),
                "a plain reason: {message}"
            );
            assert!(!saved.exists(), "nothing unverified is left behind");
        }
        // Fetching it again from Bob would also be right, as long as the
        // bytes are the original ones.
        Some(Progress::Done { .. }) => assert!(
            std::fs::read(&saved).unwrap() == original,
            "the edited local bytes were saved as the received file"
        ),
        other => panic!("the receive must finish or fail, got {other:?}"),
    }
}

/// Receiving content the store holds from an earlier receive (not by
/// reference) still works, and the check passes.
#[tokio::test(flavor = "multi_thread")]
async fn receiving_the_same_thing_twice_still_works() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let payload = make_payload(256 * 1024);
    let src = work.path().join("twice.bin");
    std::fs::write(&src, &payload).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;
    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    for n in 1..=2 {
        let out = work.path().join(format!("out{n}"));
        let (_rid, mut rs) = receiver.receive(ticket.clone(), out.clone()).await.unwrap();
        wait_done(&mut rs).await;
        assert!(std::fs::read(out.join("twice.bin")).unwrap() == payload);
    }
}

//! Received file names are cleaned for Windows on every OS, so a name that is
//! legal where it was sent from never corrupts or aborts the save here.
//!
//! The sender side has to create those names on disk first, which Windows
//! cannot do, so these end-to-end cases run on macOS and Linux. The cleaning
//! rules themselves are unit-tested on every OS in `src/export.rs`.

mod common;

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn names_illegal_on_windows_are_saved_cleaned() {
    use common::{local_core, make_payload, wait_done, wait_ready};

    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let dir = work.path().join("notes");
    std::fs::create_dir_all(dir.join("Q&A: part 1")).unwrap();
    std::fs::write(dir.join("Invoice 3:15.pdf"), make_payload(1000)).unwrap();
    std::fs::write(dir.join("Why?.pdf"), make_payload(2000)).unwrap();
    std::fs::write(dir.join("Q&A: part 1").join("con.txt"), make_payload(300)).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;
    let (_sid, mut ss) = sender.send(dir).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs).await;

    let notes = out.join("notes");
    assert_eq!(
        std::fs::read(notes.join("Invoice 3_15.pdf")).unwrap(),
        make_payload(1000)
    );
    assert_eq!(
        std::fs::read(notes.join("Why_.pdf")).unwrap(),
        make_payload(2000)
    );
    assert_eq!(
        std::fs::read(notes.join("Q&A_ part 1").join("_con.txt")).unwrap(),
        make_payload(300)
    );
}

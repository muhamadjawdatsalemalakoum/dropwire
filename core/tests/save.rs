//! Saving a received transfer to disk: one file that cannot be written does not
//! stop the others, and the receiver is told exactly which ones failed.

mod common;

use std::time::Duration;

use irohcore::{Progress, ProgressStream};
use tokio_stream::StreamExt;

/// Drive a receive to its end and return the error message it finished with.
#[allow(dead_code)] // only the unix-only tests use it
async fn wait_error(stream: &mut ProgressStream) -> String {
    let fut = async {
        while let Some(ev) = stream.next().await {
            match ev {
                Progress::Error { message, .. } => return message,
                Progress::Done { .. } => panic!("receive finished without the expected error"),
                Progress::Cancelled { .. } => panic!("receive cancelled unexpectedly"),
                _ => {}
            }
        }
        panic!("receive stream ended before an error");
    };
    tokio::time::timeout(Duration::from_secs(60), fut)
        .await
        .expect("timed out waiting for the receive to fail")
}

/// A folder the receiver may not write into sits in the middle of the
/// transfer: the files before and after it are still saved, and the error
/// names the one that was not.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn one_unwritable_file_does_not_stop_the_rest() {
    use std::os::unix::fs::PermissionsExt;

    use common::{local_core, make_payload, wait_ready};

    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let dir = work.path().join("set");
    std::fs::create_dir_all(dir.join("locked")).unwrap();
    std::fs::write(dir.join("a.bin"), make_payload(1000)).unwrap();
    std::fs::write(dir.join("locked").join("c.bin"), make_payload(3000)).unwrap();
    std::fs::write(dir.join("z.bin"), make_payload(5000)).unwrap();

    // The receiver already holds this folder from an earlier copy of the same
    // transfer, but one subfolder has become read-only.
    let out = work.path().join("out");
    let existing = out.join("set");
    std::fs::create_dir_all(existing.join("locked")).unwrap();
    std::fs::write(existing.join("a.bin"), make_payload(1000)).unwrap();
    let locked = existing.join("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;
    let (_sid, mut ss) = sender.send(dir).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let (_rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    let message = wait_error(&mut rs).await;

    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(
        message.starts_with("1 of 3 files could not be saved: set/locked/c.bin"),
        "unexpected error: {message}"
    );
    assert_eq!(
        std::fs::read(existing.join("z.bin")).unwrap(),
        make_payload(5000),
        "a file after the failed one must still be saved"
    );
    assert_eq!(
        std::fs::read(existing.join("a.bin")).unwrap(),
        make_payload(1000)
    );
    assert!(!locked.join("c.bin").exists());
}

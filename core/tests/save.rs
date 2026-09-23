//! Saving a received transfer to disk: nothing already there is replaced, a
//! repeat of the same content is not duplicated, and one file that cannot be
//! written does not stop the others.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{local_core, make_payload, wait_ready};
use irohcore::{Core, Progress, ProgressStream, RenamedFile, TransferStats};
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

/// Drive a receive to completion and return its final stats.
async fn wait_stats(stream: &mut ProgressStream) -> TransferStats {
    let fut = async {
        while let Some(ev) = stream.next().await {
            match ev {
                Progress::Done { stats, .. } => return stats,
                Progress::Error { message, .. } => panic!("receive error: {message}"),
                Progress::Cancelled { .. } => panic!("receive cancelled unexpectedly"),
                _ => {}
            }
        }
        panic!("receive stream ended before Done");
    };
    tokio::time::timeout(Duration::from_secs(60), fut)
        .await
        .expect("timed out waiting for completion")
}

/// Send `path` from `sender`, receive it into `out` on `receiver`, then end
/// the send so the same content can be sent again later.
async fn transfer(sender: &Core, receiver: &Core, path: PathBuf, out: &Path) -> TransferStats {
    let (sid, mut ss) = sender.send(path).await.unwrap();
    let ticket = wait_ready(&mut ss).await;
    let (_rid, mut rs) = receiver.receive(ticket, out.to_path_buf()).await.unwrap();
    let stats = wait_stats(&mut rs).await;
    sender.cancel(sid).await;
    while let Some(ev) = ss.next().await {
        if matches!(ev, Progress::Cancelled { .. }) {
            break;
        }
    }
    stats
}

/// Every path under `dir`, depth first.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        }
        out.push(path);
    }
    out
}

/// No hidden temporary file may be left behind after a receive.
fn assert_no_leftovers(dir: &Path) {
    for path in walk(dir) {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            !name.starts_with(".dropwire-"),
            "temporary file left behind: {}",
            path.display()
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_with_a_taken_name_is_saved_beside_it() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();
    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;

    // Two different files that happen to share a name, sent one after another.
    for (dir, len) in [("first", 1000), ("second", 2000)] {
        std::fs::create_dir_all(work.path().join(dir)).unwrap();
        std::fs::write(work.path().join(dir).join("hello.bin"), make_payload(len)).unwrap();
    }
    let out = work.path().join("out");

    let first = transfer(
        &sender,
        &receiver,
        work.path().join("first").join("hello.bin"),
        &out,
    )
    .await;
    assert!(first.renamed.is_empty());
    let second = transfer(
        &sender,
        &receiver,
        work.path().join("second").join("hello.bin"),
        &out,
    )
    .await;

    assert_eq!(
        std::fs::read(out.join("hello.bin")).unwrap(),
        make_payload(1000)
    );
    assert_eq!(
        std::fs::read(out.join("hello (1).bin")).unwrap(),
        make_payload(2000)
    );
    assert_eq!(
        second.renamed,
        vec![RenamedFile {
            name: "hello.bin".into(),
            saved_as: "hello (1).bin".into()
        }]
    );
    assert_no_leftovers(&out);

    // The UI reads the rename list from the completion event as camelCase JSON.
    let json = serde_json::to_value(&second).unwrap();
    assert_eq!(json["renamed"][0]["name"], "hello.bin");
    assert_eq!(json["renamed"][0]["savedAs"], "hello (1).bin");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_existing_file_keeps_its_bytes() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();
    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;

    let out = work.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let mine = b"the receiver's own report, not to be touched".to_vec();
    std::fs::write(out.join("report.pdf"), &mine).unwrap();

    let src = work.path().join("report.pdf");
    std::fs::write(&src, make_payload(64 * 1024)).unwrap();
    transfer(&sender, &receiver, src, &out).await;

    assert_eq!(std::fs::read(out.join("report.pdf")).unwrap(), mine);
    assert_eq!(
        std::fs::read(out.join("report (1).pdf")).unwrap(),
        make_payload(64 * 1024)
    );
    assert_no_leftovers(&out);
}

#[tokio::test(flavor = "multi_thread")]
async fn receiving_the_same_content_again_does_not_duplicate_it() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();
    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;

    let dir = work.path().join("pics");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.bin"), make_payload(3000)).unwrap();
    std::fs::write(work.path().join("note.txt"), make_payload(100)).unwrap();
    let out = work.path().join("out");

    for _ in 0..2 {
        let stats = transfer(&sender, &receiver, dir.clone(), &out).await;
        assert!(stats.renamed.is_empty(), "{:?}", stats.renamed);
        let stats = transfer(&sender, &receiver, work.path().join("note.txt"), &out).await;
        assert!(stats.renamed.is_empty(), "{:?}", stats.renamed);
    }

    let mut names: Vec<String> = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    assert_eq!(names, ["note.txt", "pics"]);
    assert_eq!(
        std::fs::read(out.join("pics").join("a.bin")).unwrap(),
        make_payload(3000)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_sent_again_with_new_content_lands_in_a_numbered_folder() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();
    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;

    for (dir, len) in [("one", 1000), ("two", 2000)] {
        let pics = work.path().join(dir).join("pics");
        std::fs::create_dir_all(pics.join("sub")).unwrap();
        std::fs::write(pics.join("sub").join("a.bin"), make_payload(len)).unwrap();
    }
    let out = work.path().join("out");

    transfer(
        &sender,
        &receiver,
        work.path().join("one").join("pics"),
        &out,
    )
    .await;
    let stats = transfer(
        &sender,
        &receiver,
        work.path().join("two").join("pics"),
        &out,
    )
    .await;

    assert_eq!(
        std::fs::read(out.join("pics").join("sub").join("a.bin")).unwrap(),
        make_payload(1000)
    );
    assert_eq!(
        std::fs::read(out.join("pics (1)").join("sub").join("a.bin")).unwrap(),
        make_payload(2000)
    );
    assert_eq!(
        stats.renamed,
        vec![RenamedFile {
            name: "pics".into(),
            saved_as: "pics (1)".into()
        }]
    );
    assert_no_leftovers(&out);
}

/// A folder the receiver may not write into sits in the middle of the
/// transfer: the files before and after it are still saved, and the error
/// names the one that was not.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn one_unwritable_file_does_not_stop_the_rest() {
    use std::os::unix::fs::PermissionsExt;

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
    assert_no_leftovers(&out);
}

/// Received files carry the same "downloaded from the internet" mark a
/// browser download does, so the OS applies its usual caution to them.
#[cfg(any(windows, target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn received_files_are_marked_as_downloaded() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();
    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;

    let src = work.path().join("invoice.docm");
    std::fs::write(&src, make_payload(40 * 1024)).unwrap();
    let out = work.path().join("out");
    transfer(&sender, &receiver, src, &out).await;
    let saved = out.join("invoice.docm");
    assert_eq!(std::fs::read(&saved).unwrap(), make_payload(40 * 1024));

    #[cfg(windows)]
    {
        let mut stream = saved.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        let mark = std::fs::read_to_string(&stream).expect("Zone.Identifier stream");
        assert!(mark.contains("ZoneId=3"), "unexpected mark: {mark:?}");
        assert!(!mark.contains("Url"), "the mark must not record a source");
    }
    #[cfg(target_os = "macos")]
    {
        let mut buf = [0u8; 256];
        let n = rustix::fs::getxattr(saved.as_path(), "com.apple.quarantine", &mut buf[..])
            .expect("com.apple.quarantine attribute");
        let mark = String::from_utf8_lossy(&buf[..n]).to_string();
        assert!(mark.starts_with("0081;"), "unexpected mark: {mark:?}");
        assert!(mark.contains(";Dropwire;"), "unexpected mark: {mark:?}");
    }
}

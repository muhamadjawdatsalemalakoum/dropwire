//! The send side before a code exists: preparing (importing) what was chosen,
//! and what can and cannot be sent.

mod common;
use std::path::Path;
use std::time::Duration;

use common::{local_core, make_payload, wait_done};
use irohcore::{Progress, ProgressStream};
use tokio_stream::StreamExt;

/// Read a stream until `want` matches, returning everything seen on the way
/// (the match included).
async fn read_until(
    stream: &mut ProgressStream,
    what: &str,
    want: impl Fn(&Progress) -> bool,
) -> Vec<Progress> {
    let fut = async {
        let mut seen = Vec::new();
        while let Some(ev) = stream.next().await {
            let hit = want(&ev);
            seen.push(ev);
            if hit {
                return seen;
            }
        }
        panic!("stream ended before {what}: {seen:?}");
    };
    tokio::time::timeout(Duration::from_secs(60), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// Preparing a big file shows how far it has got, and Cancel stops it right
/// there: the rest is not hashed first, no code is minted, and nothing is
/// recorded.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_while_preparing_mints_no_code() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();

    // Big enough that hashing it takes a noticeable while. Extended rather
    // than written, so the test does not spend its time writing it out.
    let src = work.path().join("huge.bin");
    std::fs::File::create(&src)
        .unwrap()
        .set_len(512 * 1024 * 1024)
        .unwrap();

    let sender = local_core(send_data.path()).await;
    let (sid, mut ss) = sender.send(src).await.unwrap();

    let before = read_until(
        &mut ss,
        "progress part way through the file",
        |ev| matches!(ev, Progress::Importing { done, total, .. } if *done > 0 && *done < *total),
    )
    .await;
    assert!(
        matches!(before.first(), Some(Progress::Importing { done: 0, .. })),
        "the card gets numbers straight away: {before:?}"
    );

    sender.cancel(sid).await;
    let after = tokio::time::timeout(Duration::from_secs(10), async {
        let mut seen = Vec::new();
        while let Some(ev) = ss.next().await {
            seen.push(ev);
        }
        seen
    })
    .await
    .expect("a cancel while preparing must not wait for the import to finish");
    assert!(
        matches!(after.last(), Some(Progress::Cancelled { .. })),
        "the send ends cancelled: {after:?}"
    );
    assert!(
        !after
            .iter()
            .any(|ev| matches!(ev, Progress::Ready { .. } | Progress::Error { .. })),
        "no code after Cancel: {after:?}"
    );
    assert!(
        !sender.transfers().await.iter().any(|r| r.id == sid),
        "a send cancelled while preparing leaves no record"
    );
}

/// How a send ended up before any code: its Ready (with how many links were
/// left out) or its error message.
async fn outcome(stream: &mut ProgressStream) -> Result<(String, usize), String> {
    let seen = read_until(stream, "Ready or Error", |ev| {
        matches!(ev, Progress::Ready { .. } | Progress::Error { .. })
    })
    .await;
    match seen.last() {
        Some(Progress::Ready {
            ticket, skipped, ..
        }) => Ok((ticket.clone(), *skipped)),
        Some(Progress::Error { message, .. }) => Err(message.clone()),
        _ => unreachable!(),
    }
}

/// An empty folder (even one with empty folders in it) is refused with a
/// plain reason, and no code or record is made for nothing.
#[tokio::test(flavor = "multi_thread")]
async fn empty_folder_is_refused() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let dir = work.path().join("empty");
    std::fs::create_dir_all(dir.join("nested").join("deeper")).unwrap();

    let sender = local_core(send_data.path()).await;
    let (sid, mut ss) = sender.send(dir).await.unwrap();
    assert_eq!(
        outcome(&mut ss).await,
        Err("This folder has no files to send.".to_string())
    );
    assert!(!sender.transfers().await.iter().any(|r| r.id == sid));
}

/// Make a symlink to a file or folder. Creating one on Windows can need
/// Developer Mode or admin rights; `false` means this machine cannot.
fn link(target: &Path, link: &Path, dir: bool) -> bool {
    #[cfg(unix)]
    let made = {
        let _ = dir;
        std::os::unix::fs::symlink(target, link)
    };
    #[cfg(windows)]
    let made = if dir {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    };
    match made {
        Ok(()) => true,
        Err(e) => {
            eprintln!("cannot make symlinks here ({e}); skipping");
            false
        }
    }
}

/// A link inside a folder is sent (as the file, under the link's name) when
/// it points to a file in that folder. Links that point outside it, to a
/// folder, or to nothing are left out, and the sender is told how many.
/// Nothing from outside the chosen folder is ever sent.
#[tokio::test(flavor = "multi_thread")]
async fn folder_links_stay_inside_the_folder() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let dir = work.path().join("proj");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.txt"), make_payload(3000)).unwrap();
    std::fs::write(dir.join("sub").join("b.txt"), make_payload(500)).unwrap();
    let secret = work.path().join("secret.txt");
    std::fs::write(&secret, b"not for sending").unwrap();

    if !link(&dir.join("a.txt"), &dir.join("inside.txt"), false) {
        return;
    }
    assert!(link(&secret, &dir.join("outside.txt"), false));
    assert!(link(&dir.join("sub"), &dir.join("sub-link"), true));
    assert!(link(
        &work.path().join("missing.txt"),
        &dir.join("gone.txt"),
        false
    ));

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;
    let (_sid, mut ss) = sender.send(dir).await.unwrap();
    let (ticket, skipped) = outcome(&mut ss).await.expect("the folder is sent");
    assert_eq!(
        skipped, 3,
        "outside, folder and dangling links are left out"
    );

    let preview = receiver.inspect(ticket.clone()).await.unwrap();
    let mut names: Vec<_> = preview.files.iter().map(|f| f.name.clone()).collect();
    names.sort();
    assert_eq!(
        names,
        ["proj/a.txt", "proj/inside.txt", "proj/sub/b.txt"],
        "only the folder's own files, the inside link included"
    );

    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs).await;
    assert_eq!(
        std::fs::read(out.join("proj").join("inside.txt")).unwrap(),
        make_payload(3000),
        "the inside link arrives as the file it points to"
    );
    assert!(!out.join("proj").join("outside.txt").exists());
}

/// A folder of nothing but links to elsewhere has nothing to send, and says
/// why rather than making a code for an empty transfer.
#[tokio::test(flavor = "multi_thread")]
async fn folder_of_outside_links_is_refused() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();

    let dir = work.path().join("links");
    std::fs::create_dir_all(&dir).unwrap();
    let elsewhere = work.path().join("elsewhere.txt");
    std::fs::write(&elsewhere, b"stays here").unwrap();
    if !link(&elsewhere, &dir.join("elsewhere.txt"), false) {
        return;
    }

    let sender = local_core(send_data.path()).await;
    let (_sid, mut ss) = sender.send(dir).await.unwrap();
    assert_eq!(
        outcome(&mut ss).await,
        Err(
            "This folder only has links to things outside it, so there is nothing to send."
                .to_string()
        )
    );
}

/// Several files and folders chosen together go out under one code. Each
/// keeps its own name; clashing names are numbered (ignoring case, as most
/// desktop file systems do) so nothing is overwritten on arrival, and the
/// same path chosen twice is sent once.
#[tokio::test(flavor = "multi_thread")]
async fn several_paths_share_one_code() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    for d in ["one", "two", "three"] {
        std::fs::create_dir_all(work.path().join(d)).unwrap();
    }
    let first = work.path().join("one").join("photo.jpg");
    let second = work.path().join("two").join("photo.jpg");
    let third = work.path().join("three").join("Photo.JPG");
    std::fs::write(&first, make_payload(1000)).unwrap();
    std::fs::write(&second, make_payload(2000)).unwrap();
    std::fs::write(&third, make_payload(3000)).unwrap();
    let pics = work.path().join("pics");
    std::fs::create_dir_all(pics.join("sub")).unwrap();
    std::fs::write(pics.join("sub").join("x.bin"), make_payload(4000)).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;
    let chosen = vec![
        first.clone(),
        second.clone(),
        third.clone(),
        pics.clone(),
        first.clone(),
    ];
    let (sid, mut ss) = sender.send_many(chosen).await.unwrap();
    let (ticket, _) = outcome(&mut ss).await.expect("several paths are sent");

    let preview = receiver.inspect(ticket.clone()).await.unwrap();
    let names: Vec<_> = preview.files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "photo.jpg",
            "photo (2).jpg",
            "Photo (3).JPG",
            "pics/sub/x.bin"
        ],
        "one entry per file, in the order chosen, with clashes numbered"
    );
    assert_eq!(preview.total_bytes, 1000 + 2000 + 3000 + 4000);

    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs).await;
    assert_eq!(
        std::fs::read(out.join("photo.jpg")).unwrap(),
        make_payload(1000)
    );
    assert_eq!(
        std::fs::read(out.join("photo (2).jpg")).unwrap(),
        make_payload(2000)
    );
    assert_eq!(
        std::fs::read(out.join("Photo (3).JPG")).unwrap(),
        make_payload(3000)
    );
    assert_eq!(
        std::fs::read(out.join("pics").join("sub").join("x.bin")).unwrap(),
        make_payload(4000)
    );

    // History: named after the first thing chosen, and it remembers all of
    // them (so it can be sent again as a whole), but no single source.
    let rec = sender
        .transfers()
        .await
        .into_iter()
        .find(|r| r.id == sid)
        .expect("the send is on record");
    assert_eq!(rec.name, "photo.jpg and 3 more");
    assert_eq!(rec.file_count, 4);
    assert_eq!(rec.source, None, "no one path stands for the whole send");
    let path = |p: &Path| p.to_string_lossy().to_string();
    assert_eq!(
        rec.sources,
        vec![path(&first), path(&second), path(&third), path(&pics)]
    );
}

/// Several empty folders are refused the same way one is.
#[tokio::test(flavor = "multi_thread")]
async fn several_empty_folders_are_refused() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let a = work.path().join("a");
    let b = work.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();

    let sender = local_core(send_data.path()).await;
    let (_sid, mut ss) = sender.send_many(vec![a, b]).await.unwrap();
    assert_eq!(
        outcome(&mut ss).await,
        Err("These folders have no files to send.".to_string())
    );
    assert!(
        sender.send_many(Vec::new()).await.is_err(),
        "nothing chosen is an error straight away"
    );
}

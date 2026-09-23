//! The send side before a code exists: preparing (importing) what was chosen,
//! and what can and cannot be sent.

mod common;
use std::time::Duration;

use common::local_core;
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

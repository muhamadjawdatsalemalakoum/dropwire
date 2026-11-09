//! End-to-end transfer tests for the Dropwire engine.
//!
//! These use `Infra::LocalOnly` (no relay, no discovery) so they're hermetic:
//! the receiver dials the direct addresses embedded in the ticket over loopback.

use std::time::Duration;

use irohcore::{Core, CoreConfig, Progress, Status};
use tokio_stream::StreamExt;

mod common;
use common::{
    dir_size, done_bytes, drain_for, drain_until_terminal, local_core, make_payload,
    send_events_through_done, wait_done, wait_ready,
};

#[tokio::test(flavor = "multi_thread")]
async fn roundtrip_single_file() {
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

    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs).await;

    let got = std::fs::read(out.join("hello.bin")).unwrap();
    assert_eq!(got, payload, "received file must match source");
}

#[tokio::test(flavor = "multi_thread")]
async fn roundtrip_folder() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let dir = work.path().join("pics");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.bin"), make_payload(1024)).unwrap();
    std::fs::write(dir.join("sub").join("b.bin"), make_payload(4096)).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;

    let (_sid, mut ss) = sender.send(dir).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs).await;

    assert_eq!(
        std::fs::read(out.join("pics").join("a.bin")).unwrap(),
        make_payload(1024)
    );
    assert_eq!(
        std::fs::read(out.join("pics").join("sub").join("b.bin")).unwrap(),
        make_payload(4096)
    );
}

/// Resume across an interruption, the critical correctness test (ARCHITECTURE.md §12).
///
/// Determinism: instead of guessing a wall-clock cancel delay, we drive the first
/// receive until a `Transferring` event reports real bytes (but not all of them),
/// then cancel. That leaves a partial in the receiver's `FsStore`. The second
/// receive must fetch only what is missing (the sender's Done for it counts the
/// bytes that went over the wire), and the final bytes must be perfect. This
/// guards the #1 regression risk across iroh-blobs version bumps.
#[tokio::test(flavor = "multi_thread")]
async fn resume_after_interrupt() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let src = work.path().join("big.bin");
    let payload = make_payload(64 * 1024 * 1024);
    std::fs::write(&src, &payload).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;

    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    // First attempt: cancel deterministically once real file bytes have landed.
    const MIB: u64 = 1024 * 1024;
    let out = work.path().join("out");
    let (rid, mut rs) = receiver.receive(ticket.clone(), out.clone()).await.unwrap();
    let mut interrupted_at = 0u64;
    while let Some(ev) = rs.next().await {
        match ev {
            // Past the collection's own list (the first few dozen bytes), so
            // part of the file itself is in the store.
            Progress::Transferring { offset, total, .. } if offset > MIB && offset < total => {
                interrupted_at = offset;
                receiver.cancel(rid).await;
                break;
            }
            Progress::Done { .. } => {
                panic!("the first attempt finished before the interrupt; resume not exercised")
            }
            Progress::Error { message, .. } => panic!("first attempt error: {message}"),
            _ => {}
        }
    }
    let ended = drain_until_terminal(&mut rs).await;
    assert!(
        matches!(ended, Some(Progress::Cancelled { .. })),
        "the first attempt must end cancelled, got {ended:?}"
    );

    // The store must hold partial, but not complete, data to resume from.
    let partial = dir_size(&recv_data.path().join("blobs"));
    assert!(
        partial > 0 && partial < payload.len() as u64,
        "store should hold partial data after interruption (got {partial} of {})",
        payload.len()
    );

    // Second attempt: must finish, reusing the partial already in the store.
    let (_rid2, mut rs2) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs2).await;

    // The first attempt was cut short, so the only delivery the sender saw is
    // the second one, and it sent only what was missing. Starting over would
    // send the whole payload (and a little more for the collection's list).
    let seen = send_events_through_done(&mut ss).await;
    let dones = done_bytes(&seen);
    assert_eq!(dones.len(), 1, "one delivery: {seen:?}");
    assert!(
        dones[0] > 0 && dones[0] < payload.len() as u64,
        "the resumed download must fetch only the missing part (sent {} of {}, first \
         attempt reached {interrupted_at})",
        dones[0],
        payload.len()
    );

    let got = std::fs::read(out.join("big.bin")).unwrap();
    assert_eq!(
        got.len(),
        payload.len(),
        "resumed file size must match source"
    );
    // Avoid dumping 64 MiB on failure: compare without assert_eq! formatting.
    assert!(
        got == payload,
        "resumed file must be byte-perfect (first attempt reached {interrupted_at} bytes)"
    );
}

/// Resumed progress starts from what is already stored, instead of counting
/// up from zero and snapping to "received" at the end, and it never passes
/// the total.
#[tokio::test(flavor = "multi_thread")]
async fn resumed_progress_starts_from_what_is_already_there() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let src = work.path().join("big.bin");
    let payload = make_payload(64 * 1024 * 1024);
    std::fs::write(&src, &payload).unwrap();
    let total = payload.len() as u64;

    let sender = local_core(send_data.path()).await;
    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    // First attempt: the receiving app closes part way.
    let receiver = local_core(recv_data.path()).await;
    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket.clone(), out.clone()).await.unwrap();
    let mut reached = 0;
    while let Some(ev) = rs.next().await {
        match ev {
            Progress::Transferring { offset, .. } if offset >= total / 2 => {
                reached = offset;
                break;
            }
            Progress::Done { .. } => panic!("finished before it could be interrupted"),
            Progress::Error { message, .. } => panic!("first attempt error: {message}"),
            _ => {}
        }
    }
    receiver.shutdown().await.unwrap();
    drain_until_terminal(&mut rs).await;

    let receiver = local_core(recv_data.path()).await;
    let (_rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    let mut offsets = Vec::new();
    while let Some(ev) = rs.next().await {
        match ev {
            Progress::Transferring {
                offset, total: t, ..
            } => {
                assert_eq!(t, total);
                offsets.push(offset);
            }
            Progress::Done { .. } => break,
            Progress::Error { message, .. } => panic!("resume error: {message}"),
            _ => {}
        }
    }
    assert!(
        offsets[0] >= reached / 2,
        "resumed progress must start near {reached}, started at {}",
        offsets[0]
    );
    assert!(offsets.iter().all(|&o| o <= total), "{offsets:?}");
    assert_eq!(offsets.last(), Some(&total));
    assert!(std::fs::read(out.join("big.bin")).unwrap() == payload);
}

/// The finished receive reports what was previewed (the files' bytes, not the
/// list of names that travels with them) and how long the whole receive took,
/// not just the final save to disk.
#[tokio::test(flavor = "multi_thread")]
async fn receive_summary_covers_the_whole_transfer() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let dir = work.path().join("set");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("note.txt"), b"twelve bytes").unwrap();
    std::fs::write(dir.join("big.bin"), make_payload(48 * 1024 * 1024)).unwrap();
    let payload_total = 12 + 48 * 1024 * 1024;

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;
    let (_sid, mut ss) = sender.send(dir).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let preview = receiver.inspect(ticket.clone()).await.unwrap();
    assert_eq!(preview.total_bytes, payload_total);

    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket, out).await.unwrap();
    let (mut first, mut last) = (None, None);
    let stats = loop {
        match rs.next().await.expect("stream ended before Done") {
            Progress::Transferring { .. } => {
                let now = std::time::Instant::now();
                first.get_or_insert(now);
                last = Some(now);
            }
            Progress::Done { stats, .. } => break stats,
            Progress::Error { message, .. } => panic!("receive error: {message}"),
            _ => {}
        }
    };
    assert_eq!(stats.bytes, payload_total, "payload bytes, as previewed");
    if let (Some(first), Some(last)) = (first, last) {
        let downloading = last.duration_since(first).as_secs_f64();
        assert!(
            stats.seconds >= downloading,
            "the duration ({}s) must cover the download ({downloading}s)",
            stats.seconds
        );
    }
    let rec = receiver.transfers().await.remove(0);
    assert_eq!(rec.total_bytes, payload_total);
    assert_eq!(rec.transferred, payload_total);
}

/// Real-network smoke test of the SERVERLESS path (`Infra::Decentralized`: DHT
/// discovery + n0's free relay). Hits the public network, so it's `#[ignore]` by
/// default. Run with: `cargo test -p irohcore -- --ignored roundtrip_serverless`
#[tokio::test(flavor = "multi_thread")]
#[ignore = "networked: uses the public Mainline DHT + n0's free relay"]
async fn roundtrip_serverless() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let src = work.path().join("hello.bin");
    let payload = make_payload(3 * 1024 * 1024);
    std::fs::write(&src, &payload).unwrap();

    let sender = Core::start(CoreConfig::serverless(send_data.path()))
        .await
        .unwrap();
    let receiver = Core::start(CoreConfig::serverless(recv_data.path()))
        .await
        .unwrap();

    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs).await;

    assert_eq!(std::fs::read(out.join("hello.bin")).unwrap(), payload);
}

/// The SENDER sees live progress (via iroh-blobs provider events): when a
/// receiver downloads, the send stream says "previewing" for the size check,
/// then PeerJoined, Transferring, and exactly one Done that covers the whole
/// payload.
#[tokio::test(flavor = "multi_thread")]
async fn sender_sees_progress() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let src = work.path().join("payload.bin");
    let payload = make_payload(3 * 1024 * 1024);
    std::fs::write(&src, &payload).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;

    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket, out).await.unwrap();
    wait_done(&mut rs).await;
    let seen = send_events_through_done(&mut ss).await;

    let position = |want: fn(&Progress) -> bool| seen.iter().position(want);
    let previewing = position(|e| matches!(e, Progress::Previewing { .. }));
    let joined = position(|e| matches!(e, Progress::PeerJoined { .. }));
    let moving = position(|e| matches!(e, Progress::Transferring { .. }));
    assert!(joined.is_some(), "sender should see PeerJoined: {seen:?}");
    assert!(
        previewing.is_some() && previewing < joined,
        "the size check comes first, and is only a preview: {seen:?}"
    );
    assert!(
        moving > joined,
        "sender should see bytes moving after the receiver joins: {seen:?}"
    );
    assert_eq!(
        seen.iter()
            .filter(|e| matches!(e, Progress::PeerJoined { .. }))
            .count(),
        1,
        "one download, one join: {seen:?}"
    );
    let dones = done_bytes(&seen);
    assert_eq!(dones.len(), 1, "one download, one Done: {seen:?}");
    assert!(
        dones[0] >= payload.len() as u64,
        "the Done must be the real delivery, not a probe ({} bytes of {})",
        dones[0],
        payload.len()
    );
}

/// For a folder, the sender's progress is one bar for the whole transfer: every
/// Transferring event is measured against the folder's total size, and the
/// offset only ever grows. (It used to restart from zero for every file, and
/// for the collection's own root and names blob too.)
#[tokio::test(flavor = "multi_thread")]
async fn sender_progress_spans_the_whole_folder() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let dir = work.path().join("album");
    std::fs::create_dir_all(&dir).unwrap();
    let sizes = [4 * 1024 * 1024, 6 * 1024 * 1024, 8 * 1024 * 1024];
    for (name, size) in ["a.bin", "b.bin", "c.bin"].iter().zip(sizes) {
        std::fs::write(dir.join(name), make_payload(size)).unwrap();
    }
    let folder_total: u64 = sizes.iter().map(|&s| s as u64).sum();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;
    let (_sid, mut ss) = sender.send(dir).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let (_rid, mut rs) = receiver
        .receive(ticket, work.path().join("out"))
        .await
        .unwrap();
    wait_done(&mut rs).await;
    let seen = send_events_through_done(&mut ss).await;

    let moves: Vec<(u64, u64)> = seen
        .iter()
        .filter_map(|e| match e {
            Progress::Transferring { offset, total, .. } => Some((*offset, *total)),
            _ => None,
        })
        .collect();
    assert!(!moves.is_empty(), "the sender reports progress: {seen:?}");
    assert!(
        moves.iter().all(|&(_, total)| total == folder_total),
        "every update is against the whole folder ({folder_total} bytes): {moves:?}"
    );
    assert!(
        moves.windows(2).all(|w| w[0].0 <= w[1].0),
        "the offset never goes back: {moves:?}"
    );
    assert!(
        moves.iter().all(|&(offset, total)| offset <= total),
        "the offset never passes the total: {moves:?}"
    );
    assert_eq!(done_bytes(&seen).len(), 1, "one delivery: {seen:?}");
}

/// A preview is not a delivery. The receiver looking at the file list must
/// not make the sender say "Sent" or record the send as done; the sender is
/// told the receiver is looking. The download that follows is the delivery,
/// and it is reported once.
#[tokio::test(flavor = "multi_thread")]
async fn inspect_does_not_complete_send() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let src = work.path().join("report.bin");
    let payload = make_payload(2 * 1024 * 1024);
    std::fs::write(&src, &payload).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;

    let (sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;
    let record = || async {
        sender
            .transfers()
            .await
            .into_iter()
            .find(|r| r.id == sid)
            .expect("the send is on record")
    };

    let preview = receiver.inspect(ticket.clone()).await.unwrap();
    assert_eq!(preview.file_count, 1);
    let seen = drain_for(&mut ss, Duration::from_secs(2)).await;
    assert!(
        !seen.iter().any(|e| matches!(
            e,
            Progress::Done { .. } | Progress::PeerJoined { .. } | Progress::Transferring { .. }
        )),
        "a preview is not a delivery: {seen:?}"
    );
    assert_eq!(
        seen.iter()
            .filter(|e| matches!(e, Progress::Previewing { .. }))
            .count(),
        1,
        "the sender is told once that the receiver is looking: {seen:?}"
    );
    assert_eq!(
        record().await.status,
        Status::Active,
        "a preview must not record the send as done"
    );

    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs).await;
    assert_eq!(std::fs::read(out.join("report.bin")).unwrap(), payload);

    let seen = send_events_through_done(&mut ss).await;
    assert_eq!(
        done_bytes(&seen).len(),
        1,
        "preview then download is one delivery: {seen:?}"
    );
    let rec = record().await;
    assert_eq!(rec.status, Status::Done);
    assert_eq!(rec.transferred, payload.len() as u64);
}

/// When the receiver goes away mid-file (it cancels, or its connection drops),
/// the sender is told with `PeerLeft` instead of sitting on "Sending..." for
/// good. The send stays live, so the same device can come back and finish.
///
/// Driven by hand so it is deterministic: the client reads a little and then
/// stops, which QUIC flow control turns into a sender stuck mid-file, and then
/// it drops the request.
#[cfg(feature = "test-utils")]
#[tokio::test(flavor = "multi_thread")]
async fn sender_is_told_when_the_receiver_leaves() {
    use iroh_blobs::api::remote::GetProgressItem;
    use iroh_blobs::protocol::GetRequest;
    use iroh_blobs::store::mem::MemStore;
    use iroh_blobs::ticket::BlobTicket;

    /// Read the send stream until `want` matches, skipping everything else.
    async fn wait_for(
        stream: &mut irohcore::ProgressStream,
        what: &str,
        want: impl Fn(&Progress) -> bool,
    ) {
        let fut = async {
            while let Some(ev) = stream.next().await {
                if want(&ev) {
                    return;
                }
            }
            panic!("send stream ended before {what}");
        };
        tokio::time::timeout(std::time::Duration::from_secs(30), fut)
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();

    let src = work.path().join("movie.bin");
    let payload = make_payload(16 * 1024 * 1024);
    std::fs::write(&src, &payload).unwrap();

    let sender = local_core(send_data.path()).await;
    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;
    let root = ticket.parse::<BlobTicket>().unwrap().hash();

    let raw = common::raw_endpoint().await;
    let store = MemStore::new();
    {
        let conn = raw
            .connect(sender.test_dial_addr(), iroh_blobs::ALPN)
            .await
            .unwrap();
        let mut get = store
            .remote()
            .execute_get(conn, GetRequest::all(root))
            .stream();
        loop {
            match get.next().await {
                Some(GetProgressItem::Progress(n)) if n > 0 => break,
                Some(GetProgressItem::Progress(_)) => {}
                other => panic!("the download must be under way first, got {other:?}"),
            }
        }
        // The receiver goes away here, part way through the file.
    }
    wait_for(&mut ss, "PeerLeft", |ev| {
        matches!(ev, Progress::PeerLeft { .. })
    })
    .await;

    // The same device comes back and finishes; the sender sees it return.
    let conn = raw
        .connect(sender.test_dial_addr(), iroh_blobs::ALPN)
        .await
        .unwrap();
    store
        .remote()
        .execute_get(conn, GetRequest::all(root))
        .await
        .expect("the same device can come back after leaving");
    assert!(store.has(iroh_blobs::Hash::new(&payload)).await.unwrap());
    wait_for(&mut ss, "the receiver to rejoin", |ev| {
        matches!(ev, Progress::PeerJoined { .. })
    })
    .await;
    wait_for(&mut ss, "Done", |ev| matches!(ev, Progress::Done { .. })).await;
}

/// A failed send says which file and why, in plain words, with a code the app
/// can act on.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_file_fails_with_a_plain_reason() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let sender = local_core(send_data.path()).await;

    let gone = work.path().join("gone.pdf");
    let (_sid, mut ss) = sender.send(gone.clone()).await.unwrap();
    let (code, message) = loop {
        match ss.next().await.expect("stream ended before an error") {
            Progress::Error { code, message, .. } => break (code, message),
            Progress::Ready { .. } => panic!("a missing file must not be shared"),
            _ => {}
        }
    };
    assert_eq!(code, irohcore::ErrorCode::NotFound);
    assert_eq!(
        message,
        format!("Could not open {}: it is no longer there.", gone.display())
    );
}

/// When the sender goes away mid-download, the receive ends as interrupted,
/// not failed: history says Interrupted with what had arrived, and receiving
/// again once the sender is back finishes the file byte for byte.
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_connection_is_interrupted_and_resumable() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let src = work.path().join("big.bin");
    let payload = make_payload(64 * 1024 * 1024);
    std::fs::write(&src, &payload).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;
    let (_sid, mut ss) = sender.send(src.clone()).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let out = work.path().join("out");
    let (rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    while let Some(ev) = rs.next().await {
        match ev {
            Progress::Transferring { offset, total, .. } if offset >= total / 4 => break,
            Progress::Done { .. } => panic!("finished before the sender went away"),
            Progress::Error { message, .. } => panic!("early error: {message}"),
            _ => {}
        }
    }
    // The sender's app closes mid-transfer.
    sender.shutdown().await.unwrap();
    let (code, message) = loop {
        match rs.next().await.expect("stream ended without an outcome") {
            Progress::Error { code, message, .. } => break (code, message),
            Progress::Done { .. } => panic!("finished after the sender went away"),
            _ => {}
        }
    };
    assert_eq!(code, irohcore::ErrorCode::Interrupted, "{message}");
    assert!(message.contains("pick up where it left off"), "{message}");

    let rec = receiver
        .transfers()
        .await
        .into_iter()
        .find(|r| r.id == rid)
        .expect("the receive's record");
    assert_eq!(rec.status, irohcore::Status::Interrupted);
    assert!(rec.transferred > 0, "what arrived is recorded");

    // The sender comes back and shares the same file again: same content,
    // same hash, so the receive picks up the partial data.
    let sender = local_core(send_data.path()).await;
    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;
    let (_rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs).await;
    assert!(std::fs::read(out.join("big.bin")).unwrap() == payload);
}

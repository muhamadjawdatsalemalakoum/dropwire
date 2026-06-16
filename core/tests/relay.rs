//! Single-machine **relay-path** integration tests.
//!
//! Every other engine test uses `Infra::LocalOnly` — two endpoints talking over
//! direct loopback. That proves the application + protocol logic, but it never
//! exercises the *production transport*: discovery via the ticket's relay address
//! and bytes actually relayed through a relay server. That gap is where the
//! classic P2P production bug lives ("works on my loopback, dead behind a NAT").
//!
//! These tests close that gap on ONE machine, with no internet: we spin up an
//! in-process iroh relay (`iroh::test_utils::run_relay_server`) and build two
//! Cores that are *relay-only* (`Infra::LocalRelay` removes every direct IP
//! transport). The only path between them is through that relay — so if a
//! transfer completes here, the real relay fallback path works.
//!
//! Gated behind the `test-utils` feature (see Cargo.toml `[[test]]`), so it never
//! touches the shipped build.

mod common;
use common::{dir_size, make_payload, wait_done, wait_ready};

use irohcore::{Core, CoreConfig, Infra, Progress, Route};
use tokio_stream::StreamExt;

/// Build a relay-only [`Core`] backed by `dir`, reachable only via `relay_map`.
async fn relay_core(dir: &std::path::Path, relay_map: iroh::RelayMap) -> Core {
    Core::start(CoreConfig {
        data_dir: dir.to_path_buf(),
        infra: Infra::LocalRelay { relay_map },
    })
    .await
    .unwrap()
}

/// (a) A complete transfer — preview, then download — over the relay path.
///
/// Proves the production transport works end to end: the receiver discovers the
/// sender from the ticket's relay address and pulls every byte through the relay,
/// with no direct connection possible. The `route == Relayed` assertion is the
/// proof: neither side has an IP transport, so the path can only be the relay.
#[tokio::test(flavor = "multi_thread")]
async fn transfer_completes_over_relay() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let src = work.path().join("payload.bin");
    let payload = make_payload(4 * 1024 * 1024);
    std::fs::write(&src, &payload).unwrap();

    // One in-process relay; both peers can ONLY reach each other through it.
    // `_relay` is the server's drop guard — it must stay alive for the test.
    let (relay_map, _url, _relay) = iroh::test_utils::run_relay_server().await.unwrap();

    let sender = relay_core(send_data.path(), relay_map.clone()).await;
    let receiver = relay_core(recv_data.path(), relay_map.clone()).await;

    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    // Preview over the relay, and prove the route really is relayed.
    let preview = receiver.inspect(ticket.clone()).await.unwrap();
    assert_eq!(preview.file_count, 1);
    assert_eq!(preview.total_bytes, payload.len() as u64);
    assert_eq!(
        preview.route,
        Route::Relayed,
        "with no direct IP transport, the connection must run over the relay"
    );

    // Accept — the full download must complete byte-perfect over the relay.
    let out = work.path().join("out");
    let (_rid, mut rs) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs).await;
    assert_eq!(std::fs::read(out.join("payload.bin")).unwrap(), payload);

    sender.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}

/// (b) Resume after a mid-flight interruption, over the relay.
///
/// Start a large download, cancel it as soon as bytes are moving, confirm the
/// store holds partial-but-incomplete data, then receive again with the same
/// ticket. The engine must resume from that partial data and finish
/// byte-perfect — proving an interrupted relay transfer recovers rather than
/// restarting from zero or corrupting the result.
#[tokio::test(flavor = "multi_thread")]
async fn resume_after_interruption_over_relay() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    // Large enough that it cannot finish in the instant before we cancel.
    let src = work.path().join("big.bin");
    let payload = make_payload(32 * 1024 * 1024);
    std::fs::write(&src, &payload).unwrap();
    let total = payload.len() as u64;

    let (relay_map, _url, _relay) = iroh::test_utils::run_relay_server().await.unwrap();
    let sender = relay_core(send_data.path(), relay_map.clone()).await;
    let receiver = relay_core(recv_data.path(), relay_map.clone()).await;

    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let out = work.path().join("out");

    // First attempt: cancel the moment bytes are moving (mid-flight).
    let (rid, mut rs) = receiver.receive(ticket.clone(), out.clone()).await.unwrap();
    let interrupted = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        while let Some(ev) = rs.next().await {
            match ev {
                Progress::Transferring {
                    offset, total: t, ..
                } if offset < t => {
                    receiver.cancel(rid).await;
                    return true; // interrupted before completion
                }
                Progress::Done { .. } => return false, // raced to completion (payload too small)
                _ => {}
            }
        }
        false
    })
    .await
    .expect("timed out during the interrupted receive");
    assert!(
        interrupted,
        "payload must be large enough to interrupt before it completes"
    );
    // Drain the cancellation tail so the first receive fully unwinds.
    while let Some(ev) = rs.next().await {
        if matches!(
            ev,
            Progress::Cancelled { .. } | Progress::Error { .. } | Progress::Done { .. }
        ) {
            break;
        }
    }

    // The store must hold partial — but not complete — data to resume from.
    let partial = dir_size(&recv_data.path().join("blobs"));
    assert!(
        partial > 0 && partial < total,
        "store should hold partial data after interruption (got {partial} of {total})"
    );

    // Second attempt: resume with the same ticket and finish over the relay.
    let (_rid2, mut rs2) = receiver.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs2).await;
    assert_eq!(
        std::fs::read(out.join("big.bin")).unwrap(),
        payload,
        "the resumed transfer must reconstruct the file byte-perfectly"
    );

    sender.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}

//! One-to-one enforcement. The gate is deny by default: only the root of a
//! LIVE send is served, and only to the first device that uses its code. A
//! different device is denied (even from previewing); the same device may
//! preview and then download. Content from a send that has ended, content this
//! device received, and sends from before a restart are never served.

mod common;
use std::time::Duration;

use common::{local_core, make_payload, wait_done, wait_ready};
use irohcore::{CoreError, Progress, ProgressStream};
use tokio_stream::StreamExt;

/// Drive a SEND stream until the send has fully torn down.
async fn wait_cancelled(stream: &mut ProgressStream) {
    let fut = async {
        while let Some(ev) = stream.next().await {
            if let Progress::Cancelled { .. } = ev {
                return;
            }
        }
        panic!("send stream ended before Cancelled");
    };
    tokio::time::timeout(Duration::from_secs(30), fut)
        .await
        .expect("timed out waiting for the send to stop");
}

/// Whether an error chain is the gate's refusal: iroh-blobs resets a refused
/// request's stream with ERR_PERMISSION (1). Any other failure (an internal
/// error, an unreachable sender) means the gate was not what said no.
fn is_permission_refusal(chain: &str) -> bool {
    const NEEDLE: &str = "reset by peer: error 1";
    chain
        .match_indices(NEEDLE)
        .any(|(i, m)| !chain[i + m.len()..].starts_with(|c: char| c.is_ascii_digit()))
}

/// The request reached the sender and the gate refused it.
fn assert_refused<T: std::fmt::Debug>(res: Result<T, CoreError>, what: &str) {
    match res {
        Ok(v) => panic!("{what}: expected a refusal, got {v:?}"),
        Err(e) => {
            let chain = format!("{e:#}");
            assert!(
                is_permission_refusal(&chain),
                "{what}: expected the one-to-one gate to refuse, got: {chain}"
            );
        }
    }
}

/// Drive a RECEIVE stream to its end; it must fail at the gate, not complete
/// and not fail to connect.
async fn assert_receive_refused(stream: &mut ProgressStream, what: &str) {
    let fut = async {
        while let Some(ev) = stream.next().await {
            match ev {
                Progress::Error { message, .. } => return message,
                Progress::Done { .. } => panic!("{what}: the download completed"),
                _ => {}
            }
        }
        panic!("{what}: receive stream ended without an outcome");
    };
    let message = tokio::time::timeout(Duration::from_secs(30), fut)
        .await
        .expect("timed out waiting for the refusal");
    assert!(
        !message.contains("can't reach the sender"),
        "{what}: the sender was unreachable ({message}), so nothing was refused"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ticket_is_bound_to_first_device() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let r1_data = tempfile::tempdir().unwrap();
    let r2_data = tempfile::tempdir().unwrap();

    let src = work.path().join("secret.bin");
    let payload = make_payload(2 * 1024 * 1024);
    std::fs::write(&src, &payload).unwrap();

    let sender = local_core(send_data.path()).await;
    let r1 = local_core(r1_data.path()).await; // device 1
    let r2 = local_core(r2_data.path()).await; // device 2 (a different EndpointId)

    let (_sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    // Device 1 claims the ticket just by previewing it.
    let preview = r1.inspect(ticket.clone()).await.unwrap();
    assert_eq!(preview.file_count, 1);

    // Device 2 (a different device) is denied: it cannot even preview.
    assert_refused(
        r2.inspect(ticket.clone()).await,
        "a second device must be denied the same ticket (one-to-one)",
    );

    // Device 1 can still complete its download (same device as the binding).
    let out = work.path().join("out");
    let (_rid, mut rs) = r1.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs).await;
    assert_eq!(std::fs::read(out.join("secret.bin")).unwrap(), payload);
}

/// Cancel before anyone used the code: the code is dead on arrival.
#[tokio::test(flavor = "multi_thread")]
async fn code_is_refused_once_the_send_is_cancelled() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let r1_data = tempfile::tempdir().unwrap();

    let src = work.path().join("draft.bin");
    std::fs::write(&src, make_payload(64 * 1024)).unwrap();

    let sender = local_core(send_data.path()).await;
    let r1 = local_core(r1_data.path()).await;

    let (sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;
    sender.cancel(sid).await;
    wait_cancelled(&mut ss).await;

    assert_refused(
        r1.inspect(ticket).await,
        "preview after the send was cancelled",
    );
}

/// The Dismiss path: the bound receiver got everything, then the sender stops
/// sharing. The code is still out there (a group chat, say), but nobody can use
/// it any more: not a new device, and not the device that already received it.
#[tokio::test(flavor = "multi_thread")]
async fn code_is_refused_after_a_delivered_send_ends() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let r1_data = tempfile::tempdir().unwrap();
    let r2_data = tempfile::tempdir().unwrap();

    let src = work.path().join("payroll.bin");
    let payload = make_payload(256 * 1024);
    std::fs::write(&src, &payload).unwrap();

    let sender = local_core(send_data.path()).await;
    let r1 = local_core(r1_data.path()).await;
    let r2 = local_core(r2_data.path()).await;

    let (sid, mut ss) = sender.send(src).await.unwrap();
    let ticket = wait_ready(&mut ss).await;

    let (_rid, mut rs) = r1
        .receive(ticket.clone(), work.path().join("out1"))
        .await
        .unwrap();
    wait_done(&mut rs).await;

    sender.cancel(sid).await;
    wait_cancelled(&mut ss).await;

    assert_refused(
        r2.inspect(ticket.clone()).await,
        "a new device's preview after the send ended",
    );
    let (_rid2, mut rs2) = r2
        .receive(ticket.clone(), work.path().join("out2"))
        .await
        .unwrap();
    assert_receive_refused(&mut rs2, "a new device's download after the send ended").await;
    assert!(
        !work.path().join("out2").join("payroll.bin").exists(),
        "nothing may be written for a refused download"
    );

    assert_refused(
        r1.inspect(ticket).await,
        "the original receiver after the send ended",
    );
}

/// Tests that need to point a code at a specific device's current address
/// (a restarted sender, or a receiver). Loopback ports change on restart, so
/// the tests rebuild the ticket; in the app the endpoint id alone finds it.
#[cfg(feature = "test-utils")]
mod addressed {
    use super::*;
    use iroh_blobs::protocol::{ChunkRanges, GetManyRequest, GetRequest};
    use iroh_blobs::store::mem::MemStore;
    use iroh_blobs::ticket::BlobTicket;
    use iroh_blobs::{BlobFormat, Hash};

    fn ticket_at(addr: iroh::EndpointAddr, hash: Hash) -> String {
        BlobTicket::new(addr, hash, BlobFormat::HashSeq).to_string()
    }

    fn hash_of(ticket: &str) -> Hash {
        ticket.parse::<BlobTicket>().unwrap().hash()
    }

    /// A bare loopback endpoint that speaks the blobs protocol directly.
    async fn raw_endpoint() -> iroh::Endpoint {
        iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .relay_mode(iroh::endpoint::RelayMode::Disabled)
            .clear_ip_transports()
            .bind_addr("127.0.0.1:0")
            .unwrap()
            .bind()
            .await
            .unwrap()
    }

    /// Nothing is re-served after a restart. The sender presses Resend, which
    /// re-imports the same source into the same collection hash, so a
    /// receiver's old code works again (and an interrupted download resumes).
    #[tokio::test(flavor = "multi_thread")]
    async fn restart_refuses_old_codes_until_resend() {
        let work = tempfile::tempdir().unwrap();
        let send_data = tempfile::tempdir().unwrap();
        let r_data = tempfile::tempdir().unwrap();

        let src = work.path().join("report.bin");
        std::fs::write(&src, make_payload(128 * 1024)).unwrap();

        let sender = local_core(send_data.path()).await;
        let (_sid, mut ss) = sender.send(src.clone()).await.unwrap();
        let ticket = wait_ready(&mut ss).await;
        let hash = hash_of(&ticket);
        sender.shutdown().await.unwrap();
        drop(ss);

        // Same data dir: same identity, same store, same history.
        let sender = local_core(send_data.path()).await;
        let receiver = local_core(r_data.path()).await;
        let old_code = ticket_at(sender.test_dial_addr(), hash);
        assert_refused(
            receiver.inspect(old_code.clone()).await,
            "an old code after the sender restarted",
        );

        let (_sid, mut ss) = sender.send(src).await.unwrap();
        let resent = wait_ready(&mut ss).await;
        assert_eq!(
            hash_of(&resent),
            hash,
            "Resend must share the same content hash"
        );
        let preview = receiver.inspect(old_code).await.unwrap();
        assert_eq!(preview.file_count, 1);
    }

    /// A device never re-serves what it received: asking r1 for the content it
    /// downloaded is refused.
    #[tokio::test(flavor = "multi_thread")]
    async fn received_content_is_not_served() {
        let work = tempfile::tempdir().unwrap();
        let send_data = tempfile::tempdir().unwrap();
        let r1_data = tempfile::tempdir().unwrap();
        let r2_data = tempfile::tempdir().unwrap();

        let src = work.path().join("photo.bin");
        std::fs::write(&src, make_payload(256 * 1024)).unwrap();

        let sender = local_core(send_data.path()).await;
        let r1 = local_core(r1_data.path()).await;
        let r2 = local_core(r2_data.path()).await;

        let (_sid, mut ss) = sender.send(src).await.unwrap();
        let ticket = wait_ready(&mut ss).await;
        let (_rid, mut rs) = r1
            .receive(ticket.clone(), work.path().join("out"))
            .await
            .unwrap();
        wait_done(&mut rs).await;

        let from_r1 = ticket_at(r1.test_dial_addr(), hash_of(&ticket));
        assert_refused(
            r2.inspect(from_r1).await,
            "content r1 received, asked of r1",
        );
    }

    /// While a send is live, only its root is served, and only with a plain
    /// GET. A child blob fetched on its own and a GetMany are refused even for
    /// a device that has not been bound yet.
    #[tokio::test(flavor = "multi_thread")]
    async fn only_the_live_root_is_served() {
        let work = tempfile::tempdir().unwrap();
        let send_data = tempfile::tempdir().unwrap();
        let r1_data = tempfile::tempdir().unwrap();

        let src = work.path().join("notes.bin");
        let payload = make_payload(64 * 1024);
        std::fs::write(&src, &payload).unwrap();

        let sender = local_core(send_data.path()).await;
        let r1 = local_core(r1_data.path()).await;
        let (_sid, mut ss) = sender.send(src).await.unwrap();
        let ticket = wait_ready(&mut ss).await;
        let root = hash_of(&ticket);
        let child = Hash::new(&payload);

        let raw = raw_endpoint().await;
        let conn = raw
            .connect(sender.test_dial_addr(), iroh_blobs::ALPN)
            .await
            .unwrap();
        let store = MemStore::new();

        let child_get = store
            .remote()
            .execute_get(conn.clone(), GetRequest::blob(child))
            .await
            .expect_err("a child blob on its own must be refused");
        let chain = format!("{child_get:#}");
        assert!(is_permission_refusal(&chain), "child GET: {chain}");

        let many = GetManyRequest::builder()
            .hash(child, ChunkRanges::all())
            .hash(root, ChunkRanges::all())
            .build();
        let many_get = store
            .remote()
            .execute_get_many(conn.clone(), many)
            .await
            .expect_err("GetMany must be refused");
        let chain = format!("{many_get:#}");
        assert!(is_permission_refusal(&chain), "GetMany: {chain}");
        assert!(!store.has(child).await.unwrap(), "no content may arrive");

        // The root itself is served to its first requester, which shows the
        // refusals above came from the gate and not from a broken harness.
        store
            .remote()
            .execute_get(conn, GetRequest::all(root))
            .await
            .expect("the live root is served to its first requester");
        assert!(store.has(child).await.unwrap());

        // That requester now holds the code; everyone else is turned away.
        assert_refused(r1.inspect(ticket).await, "a second device after the first");
    }
    /// Cancel stops a transfer that is already moving, not just new requests.
    /// The client reads a little and then pauses, so QUIC flow control holds
    /// the sender mid-file while it cancels; once reading resumes, the rest of
    /// the file must never arrive.
    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_stops_a_transfer_in_flight() {
        use iroh_blobs::api::remote::GetProgressItem;

        let work = tempfile::tempdir().unwrap();
        let send_data = tempfile::tempdir().unwrap();

        let src = work.path().join("big.bin");
        let payload = make_payload(16 * 1024 * 1024);
        std::fs::write(&src, &payload).unwrap();

        let sender = local_core(send_data.path()).await;
        let (sid, mut ss) = sender.send(src).await.unwrap();
        let ticket = wait_ready(&mut ss).await;

        let raw = raw_endpoint().await;
        let conn = raw
            .connect(sender.test_dial_addr(), iroh_blobs::ALPN)
            .await
            .unwrap();
        let store = MemStore::new();
        let mut get = store
            .remote()
            .execute_get(conn, GetRequest::all(hash_of(&ticket)))
            .stream();

        let started = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                match get.next().await {
                    Some(GetProgressItem::Progress(n)) if n > 0 => return,
                    Some(GetProgressItem::Progress(_)) => {}
                    Some(GetProgressItem::Done(_)) => panic!("finished before the cancel"),
                    Some(GetProgressItem::Error(e)) => panic!("failed before the cancel: {e:#}"),
                    None => panic!("request ended before the cancel"),
                }
            }
        });
        started.await.expect("no bytes arrived");

        sender.cancel(sid).await;
        wait_cancelled(&mut ss).await;

        let outcome = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                match get.next().await {
                    Some(GetProgressItem::Progress(_)) => {}
                    Some(GetProgressItem::Done(_)) => return Ok(()),
                    Some(GetProgressItem::Error(e)) => return Err(format!("{e:#}")),
                    None => return Err("request ended".to_string()),
                }
            }
        })
        .await
        .expect("the stopped transfer never ended");
        assert!(
            outcome.is_err(),
            "a cancelled send must not finish a transfer that was in flight"
        );
        assert!(
            !store.has(Hash::new(&payload)).await.unwrap(),
            "the file must not arrive complete after the cancel"
        );
    }
}

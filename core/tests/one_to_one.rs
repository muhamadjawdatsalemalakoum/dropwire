//! One-to-one enforcement — a ticket is served to the FIRST device that uses it;
//! a different device is denied (even from previewing). The same device may
//! preview and then download.

mod common;
use common::{local_core, make_payload, wait_done, wait_ready};

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

    // Device 2 (a different device) is denied — it cannot even preview.
    let denied = r2.inspect(ticket.clone()).await;
    assert!(
        denied.is_err(),
        "a second device must be denied the same ticket (one-to-one)"
    );

    // Device 1 can still complete its download (same device as the binding).
    let out = work.path().join("out");
    let (_rid, mut rs) = r1.receive(ticket, out.clone()).await.unwrap();
    wait_done(&mut rs).await;
    assert_eq!(std::fs::read(out.join("secret.bin")).unwrap(), payload);
}

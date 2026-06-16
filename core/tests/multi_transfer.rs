//! Multiple transfers run at the same time: one sender serving two contents, and
//! one receiver pulling both concurrently, all completing byte-perfect.

mod common;
use common::{local_core, make_payload, wait_done, wait_ready};

#[tokio::test(flavor = "multi_thread")]
async fn two_transfers_run_concurrently() {
    let work = tempfile::tempdir().unwrap();
    let send_data = tempfile::tempdir().unwrap();
    let recv_data = tempfile::tempdir().unwrap();

    let a = work.path().join("a.bin");
    let b = work.path().join("b.bin");
    let pa = make_payload(3 * 1024 * 1024);
    let pb = make_payload(5 * 1024 * 1024);
    std::fs::write(&a, &pa).unwrap();
    std::fs::write(&b, &pb).unwrap();

    let sender = local_core(send_data.path()).await;
    let receiver = local_core(recv_data.path()).await;

    // Two independent sends from one sender → two tickets, both served at once.
    let (_s1, mut ss1) = sender.send(a).await.unwrap();
    let t1 = wait_ready(&mut ss1).await;
    let (_s2, mut ss2) = sender.send(b).await.unwrap();
    let t2 = wait_ready(&mut ss2).await;

    // Two receives running at the same time on one receiver.
    let out1 = work.path().join("out1");
    let out2 = work.path().join("out2");
    let (_r1, mut rs1) = receiver.receive(t1, out1.clone()).await.unwrap();
    let (_r2, mut rs2) = receiver.receive(t2, out2.clone()).await.unwrap();

    // Drive both concurrently; both must finish.
    tokio::join!(wait_done(&mut rs1), wait_done(&mut rs2));

    assert_eq!(std::fs::read(out1.join("a.bin")).unwrap(), pa);
    assert_eq!(std::fs::read(out2.join("b.bin")).unwrap(), pb);

    // Both show up in history as concurrent receives.
    let recvs = receiver.transfers().await;
    assert!(
        recvs.len() >= 2,
        "both concurrent transfers are recorded in history"
    );
}

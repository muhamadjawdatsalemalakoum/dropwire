//! LIVE-NETWORK test for the mDNS advertisement/browse loop.
//!
//! Ignored by default (`cargo test` skips it): real multicast is environment-
//! dependent (corporate Wi-Fi often blocks it). Run explicitly on a network
//! with multicast enabled:
//!
//! ```text
//! cargo test -p irohcore --features test-utils --test nearby_mdns -- --ignored --nocapture
//! ```
//!
//! What it proves: two engines on the same host see each other's
//! `_dropwire._udp.local.` announcements, with matching endpoint ids,
//! fingerprints, and a dialable LAN socket.

#![cfg(feature = "test-utils")]

use std::time::Duration;

use irohcore::{CoreConfig, Infra};

/// Two cores on one machine: A starts its nearby session, B browses, B must
/// discover A by endpoint id within the timeout. Then roles flip to prove
/// bidirectional visibility (A's browse loop picks up B's announcement).
#[tokio::test]
#[ignore = "requires a live multicast-capable network; run with --ignored"]
async fn mdns_two_instances_discover_each_other() {
    let dir_a = std::env::temp_dir().join(format!("dw-mdns-a-{}", std::process::id()));
    let dir_b = std::env::temp_dir().join(format!("dw-mdns-b-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();

    // LocalOnly keeps the test hermetic apart from multicast itself (no DHT).
    let a = irohcore::Core::start(CoreConfig {
        data_dir: dir_a.clone(),
        infra: Infra::LocalOnly,
    })
    .await
    .unwrap();
    let b = irohcore::Core::start(CoreConfig {
        data_dir: dir_b.clone(),
        infra: Infra::LocalOnly,
    })
    .await
    .unwrap();

    a.start_nearby().await.unwrap();
    b.start_nearby().await.unwrap();
    let eid_a = a.endpoint_id();

    // Poll B's view until A appears (mDNS announce + browse typically < 2 s).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut found = None;
    while tokio::time::Instant::now() < deadline {
        let devs = b.nearby_devices().await;
        if let Some(d) = devs.iter().find(|d| d.endpoint_id == eid_a) {
            found = Some(d.clone());
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let dev = found.expect("B never discovered A over mDNS");
    assert_eq!(
        dev.fingerprint,
        irohcore::NearbyDevice::fingerprint_for(&eid_a)
    );
    assert!(dev.addr.is_some(), "a LAN socket must be learned");
    println!("discovered: {} ({})", dev.name, dev.endpoint_id);

    // Bidirectional: A must also list B now that B advertises too.
    let eid_b = b.endpoint_id();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut reverse = false;
    while tokio::time::Instant::now() < deadline {
        if a.nearby_devices()
            .await
            .iter()
            .any(|d| d.endpoint_id == eid_b)
        {
            reverse = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(reverse, "A never discovered B over mDNS");

    a.stop_nearby().await;
    b.stop_nearby().await;

    // After stop, peers age out of the snapshot immediately (list cleared).
    assert!(b.nearby_devices().await.is_empty());

    let _ = a.shutdown().await;
    let _ = b.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}

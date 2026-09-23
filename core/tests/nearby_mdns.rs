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

/// These tests share the process's one mDNS browse, which runs only while
/// some engine has sharing on, so they take turns.
async fn serial() -> tokio::sync::MutexGuard<'static, ()> {
    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    SERIAL.lock().await
}

/// Two cores on one machine: A starts its nearby session, B browses, B must
/// discover A by endpoint id within the timeout. Then roles flip to prove
/// bidirectional visibility (A's browse loop picks up B's announcement).
#[tokio::test]
#[ignore = "requires a live multicast-capable network; run with --ignored"]
async fn mdns_two_instances_discover_each_other() {
    let _serial = serial().await;
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

    // After stop, the snapshot is empty straight away.
    assert!(b.nearby_devices().await.is_empty());

    let _ = a.shutdown().await;
    let _ = b.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}

/// Two engines on this machine, each in its own temp dir, both advertising.
async fn pair(tag: &str) -> (irohcore::Core, irohcore::Core, Vec<std::path::PathBuf>) {
    let dirs: Vec<_> = ["a", "b"]
        .iter()
        .map(|x| std::env::temp_dir().join(format!("dw-mdns-{tag}-{x}-{}", std::process::id())))
        .collect();
    let mut cores = Vec::new();
    for dir in &dirs {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).unwrap();
        let core = irohcore::Core::start(CoreConfig {
            data_dir: dir.clone(),
            infra: Infra::LocalOnly,
        })
        .await
        .unwrap();
        core.start_nearby().await.unwrap();
        cores.push(core);
    }
    let b = cores.pop().unwrap();
    let a = cores.pop().unwrap();
    (a, b, dirs)
}

/// Whether `core` lists `eid` within `secs`.
async fn sees(core: &irohcore::Core, eid: &str, secs: u64) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while tokio::time::Instant::now() < deadline {
        if core
            .nearby_devices()
            .await
            .iter()
            .any(|d| d.endpoint_id == eid)
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    false
}

/// Whether `core` stops listing `eid` within `secs`.
async fn loses(core: &irohcore::Core, eid: &str, secs: u64) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while tokio::time::Instant::now() < deadline {
        if !core
            .nearby_devices()
            .await
            .iter()
            .any(|d| d.endpoint_id == eid)
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    false
}

/// Whether `core` lists `eid` under `name` within `secs`.
async fn sees_named(core: &irohcore::Core, eid: &str, name: &str, secs: u64) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while tokio::time::Instant::now() < deadline {
        if core
            .nearby_devices()
            .await
            .iter()
            .any(|d| d.endpoint_id == eid && d.name == name)
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    false
}

/// A device whose name has a dot in it (a Mac's default hostname, say)
/// really leaves when it turns sharing off: its goodbye goes out, so the
/// others drop it within seconds instead of listing it for as long as the
/// app runs.
#[tokio::test]
#[ignore = "requires a live multicast-capable network; run with --ignored"]
async fn mdns_a_dotted_name_leaves_when_sharing_stops() {
    let _serial = serial().await;
    let (a, b, dirs) = pair("dotted").await;
    a.stop_nearby().await;
    a.set_device_name("Test-Mac.local".into()).await.unwrap();
    a.start_nearby().await.unwrap();
    let eid_a = a.endpoint_id();
    assert!(
        sees_named(&b, &eid_a, "Test-Mac.local", 20).await,
        "B never saw A under its dotted name"
    );

    a.stop_nearby().await;
    assert!(
        loses(&b, &eid_a, 10).await,
        "A is still listed after it left"
    );

    let _ = a.shutdown().await;
    let _ = b.shutdown().await;
    for dir in dirs {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Turning sharing off and on, or renaming the device (which re-registers),
/// still shows the devices already around, straight away. mdns-sd reports a
/// peer only when it first appears, so a list emptied on stop stayed empty
/// for minutes, and offers from those devices were turned away meanwhile.
#[tokio::test]
#[ignore = "requires a live multicast-capable network; run with --ignored"]
async fn mdns_peers_survive_a_toggle_and_a_rename() {
    let _serial = serial().await;
    let (a, b, dirs) = pair("toggle").await;
    let eid_a = a.endpoint_id();
    assert!(sees(&b, &eid_a, 20).await, "B never discovered A");

    b.stop_nearby().await;
    assert!(b.nearby_devices().await.is_empty(), "hidden while off");
    b.start_nearby().await.unwrap();
    assert!(sees(&b, &eid_a, 3).await, "A must show again at once");

    b.set_device_name("renamed-b".into()).await.unwrap();
    assert!(
        sees(&b, &eid_a, 3).await,
        "A must still show after a rename"
    );
    let eid_b = b.endpoint_id();
    assert!(
        sees_named(&a, &eid_b, "renamed-b", 10).await,
        "A must see B's new name"
    );
    // Two names with the same advertised label: the advertisement is
    // updated in place rather than withdrawn and made again.
    b.set_device_name("千代".into()).await.unwrap();
    assert!(sees_named(&a, &eid_b, "千代", 10).await, "first label");
    b.set_device_name("花子".into()).await.unwrap();
    assert!(sees_named(&a, &eid_b, "花子", 10).await, "same label");

    let _ = a.shutdown().await;
    let _ = b.shutdown().await;
    for dir in dirs {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// With sharing off everywhere in the app, the browse stops too, so a hidden
/// device sends no Dropwire queries. Turning sharing on again browses afresh,
/// and the devices around show again.
#[tokio::test]
#[ignore = "requires a live multicast-capable network; run with --ignored"]
async fn mdns_browse_runs_only_while_sharing() {
    let _serial = serial().await;
    let (a, b, dirs) = pair("browse").await;
    let (eid_a, eid_b) = (a.endpoint_id(), b.endpoint_id());
    assert!(a.test_nearby_browsing());
    assert!(sees(&a, &eid_b, 20).await, "A never discovered B");

    a.stop_nearby().await;
    assert!(
        a.test_nearby_browsing(),
        "B still shares, so it still browses"
    );
    b.stop_nearby().await;
    assert!(
        !a.test_nearby_browsing(),
        "nobody shares, so nothing browses"
    );

    a.start_nearby().await.unwrap();
    assert!(a.test_nearby_browsing());
    b.start_nearby().await.unwrap();
    assert!(sees(&a, &eid_b, 10).await, "A must see B again");
    assert!(sees(&b, &eid_a, 10).await, "B must see A again");

    let probe = a.clone();
    let _ = a.shutdown().await;
    let _ = b.shutdown().await;
    assert!(
        !probe.test_nearby_browsing(),
        "shut down, so nothing browses"
    );
    for dir in dirs {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Over real mDNS, a device can offer to one that sees it: the receiver's
/// visibility gate, fed by the live peer table, lets the offer through. (These
/// engines listen on loopback only, so the dial uses the loopback address as a
/// hint next to the LAN socket mDNS gave.)
#[tokio::test]
#[ignore = "requires a live multicast-capable network; run with --ignored"]
async fn mdns_offer_reaches_a_device_that_sees_the_sender() {
    let _serial = serial().await;
    use tokio_stream::StreamExt;

    let (a, b, dirs) = pair("offer").await;
    assert!(sees(&b, &a.endpoint_id(), 20).await, "B never discovered A");
    assert!(sees(&a, &b.endpoint_id(), 20).await, "A never discovered B");
    let mut offers = b.subscribe_offers();

    let src = dirs[0].join("hello.txt");
    std::fs::write(&src, b"hello over the local network").unwrap();
    let (id, mut stream) = a.send(src).await.unwrap();
    while let Some(ev) = stream.next().await {
        if let irohcore::Progress::Ready { .. } = ev {
            break;
        }
    }

    let (_oid, mut updates) = a
        .offer_nearby_dial(b.endpoint_id(), id, Some(b.test_dial_addr()))
        .await
        .unwrap();
    let offer = tokio::time::timeout(Duration::from_secs(15), offers.recv())
        .await
        .expect("the offer never reached B")
        .unwrap();
    assert_eq!(offer.from_endpoint_id, a.endpoint_id());
    b.respond_offer(offer.offer_id, true).await.unwrap();
    let verdict = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match updates.next().await {
                Some(irohcore::OfferUpdate::Waiting) => continue,
                other => return other,
            }
        }
    })
    .await
    .expect("no verdict");
    assert_eq!(verdict, Some(irohcore::OfferUpdate::Accepted));

    let _ = a.shutdown().await;
    let _ = b.shutdown().await;
    for dir in dirs {
        let _ = std::fs::remove_dir_all(dir);
    }
}

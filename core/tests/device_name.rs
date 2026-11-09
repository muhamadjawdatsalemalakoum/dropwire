//! The name this device shows to others is checked before anything changes:
//! a name the local network could not carry used to leave Nearby switched off
//! behind the user's back, with the engine holding the bad name.

mod common;
use common::local_core;

#[tokio::test]
async fn a_name_that_is_too_long_is_refused_and_nothing_changes() {
    let data = tempfile::tempdir().unwrap();
    let core = local_core(data.path()).await;
    let before = core.device_name().await;

    // About 90 wide characters: far past what the old code could advertise.
    let err = core
        .set_device_name("千".repeat(90))
        .await
        .expect_err("too long");
    assert!(err.to_string().contains("40 characters"), "{err}");
    assert_eq!(core.device_name().await, before);

    let err = core
        .set_device_name("x".repeat(41))
        .await
        .expect_err("one over");
    assert!(err.to_string().contains("40 characters"), "{err}");
    assert_eq!(core.device_name().await, before);

    // Forty characters is fine, however wide.
    core.set_device_name("千".repeat(40)).await.unwrap();
    assert_eq!(core.device_name().await, "千".repeat(40));
}

#[tokio::test]
async fn an_empty_name_is_refused() {
    let data = tempfile::tempdir().unwrap();
    let core = local_core(data.path()).await;
    let before = core.device_name().await;
    for blank in ["", "   ", "\n\t", "\u{202E}\u{200B}"] {
        let err = core.set_device_name(blank.into()).await.expect_err(blank);
        assert_eq!(err.to_string(), "A device name cannot be empty.");
    }
    assert_eq!(core.device_name().await, before);
}

#[tokio::test]
async fn a_name_is_tidied_before_it_is_used() {
    let data = tempfile::tempdir().unwrap();
    let core = local_core(data.path()).await;
    core.set_device_name("  Keon\u{202E}'s \r\n laptop\u{7} ".into())
        .await
        .unwrap();
    assert_eq!(core.device_name().await, "Keon's laptop");
}

#[tokio::test]
async fn the_default_name_is_one_the_device_can_take() {
    let data = tempfile::tempdir().unwrap();
    let core = local_core(data.path()).await;
    let name = core.device_name().await;
    assert!(!name.is_empty());
    assert!(name.chars().count() <= 40, "{name}");
    assert!(!name.to_ascii_lowercase().ends_with(".local"), "{name}");
    // Setting it again, as the setup screen does, is accepted as it is.
    core.set_device_name(name.clone()).await.unwrap();
    assert_eq!(core.device_name().await, name);
}

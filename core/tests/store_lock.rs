//! Starting a second engine on a data folder that is already in use must fail
//! promptly with an error, not hang. The desktop app turns that error into a
//! message (or waits for a copy that is still shutting down), which only works
//! if the error actually comes back.

mod common;

use std::time::Duration;

use common::local_core;
use irohcore::{Core, CoreConfig};

#[tokio::test(flavor = "multi_thread")]
async fn a_second_engine_on_a_folder_in_use_fails_instead_of_hanging() {
    let data = tempfile::tempdir().unwrap();
    let first = local_core(data.path()).await;

    let second = tokio::time::timeout(
        Duration::from_secs(20),
        Core::start(CoreConfig::local_only(data.path())),
    )
    .await
    .expect("starting a second engine on a folder in use must not hang");
    let err = second.err().expect("the second engine must not start");
    assert!(
        err.to_string().contains("already open"),
        "unexpected error: {err:#}"
    );

    // Once the first engine is gone, the folder is usable again.
    first.shutdown().await.unwrap();
    let again = tokio::time::timeout(
        Duration::from_secs(20),
        Core::start(CoreConfig::local_only(data.path())),
    )
    .await
    .expect("starting after the first engine shut down must not hang")
    .expect("the folder is free again after shutdown");
    again.shutdown().await.unwrap();
}

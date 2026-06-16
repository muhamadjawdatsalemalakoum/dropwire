//! Shared helpers for the engine integration tests.
//!
//! Lives under `tests/common/` (a subdirectory) so Cargo does NOT compile it as
//! its own test binary. Included from each test file with `mod common;`.
#![allow(dead_code)] // not every test file uses every helper

use std::path::Path;
use std::time::Duration;

use irohcore::{Core, CoreConfig, Progress, ProgressStream};
use tokio_stream::StreamExt;

/// Deterministic, position-dependent test payload of `n` bytes.
pub fn make_payload(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

/// Start a hermetic, loopback-only [`Core`] backed by `dir` (no relay/discovery).
pub async fn local_core(dir: &Path) -> Core {
    Core::start(CoreConfig::local_only(dir)).await.unwrap()
}

/// Drive a SEND stream until the ticket is minted, then return it.
pub async fn wait_ready(stream: &mut ProgressStream) -> String {
    let fut = async {
        while let Some(ev) = stream.next().await {
            match ev {
                Progress::Ready { ticket, .. } => return ticket,
                Progress::Error { message, .. } => panic!("send error: {message}"),
                _ => {}
            }
        }
        panic!("send stream ended before Ready");
    };
    tokio::time::timeout(Duration::from_secs(30), fut)
        .await
        .expect("timed out waiting for ticket")
}

/// Drive a RECEIVE stream until the transfer completes successfully.
pub async fn wait_done(stream: &mut ProgressStream) {
    let fut = async {
        while let Some(ev) = stream.next().await {
            match ev {
                Progress::Done { .. } => return,
                Progress::Error { message, .. } => panic!("receive error: {message}"),
                Progress::Cancelled { .. } => panic!("receive cancelled unexpectedly"),
                _ => {}
            }
        }
        panic!("receive stream ended before Done");
    };
    tokio::time::timeout(Duration::from_secs(60), fut)
        .await
        .expect("timed out waiting for completion");
}

/// Recursively sum the byte size of every file under `dir` (0 if it's absent).
/// Used to prove `inspect` does not download content into the receiver's store.
pub fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                total += dir_size(&path);
            } else if let Ok(meta) = std::fs::metadata(&path) {
                total += meta.len();
            }
        }
    }
    total
}

/// Drain a stream until any terminal event (Done / Error / Cancelled).
pub async fn drain_until_terminal(stream: &mut ProgressStream) {
    while let Some(ev) = stream.next().await {
        if matches!(
            ev,
            Progress::Done { .. } | Progress::Error { .. } | Progress::Cancelled { .. }
        ) {
            break;
        }
    }
}

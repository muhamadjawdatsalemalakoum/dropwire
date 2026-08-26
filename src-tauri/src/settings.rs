//! App-level settings, persisted next to the engine's data.
//!
//! These are preferences and local records, not engine state: what this device
//! calls itself, where receives land, which devices we have transferred with
//! before, and how the tray behaves. The engine owns identity and transfers;
//! this owns everything the user can change about the app.
//!
//! Stored as `settings.json` in the app data dir. Writes are best-effort: a
//! settings file we cannot write must never stop the app from transferring
//! files, so every failure degrades to "this preference will not survive a
//! restart" rather than an error the user has to clear.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// A device we have completed a transfer with. Remembering it grants nothing on
/// its own: the other side still confirms, and the receiver still sees the
/// verified file list before anything is written.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trusted {
    pub endpoint_id: String,
    pub name: String,
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub fingerprint: String,
    /// How many transfers we have completed with this device.
    #[serde(default)]
    pub transfers: u32,
    /// Unix seconds of the last completed transfer.
    #[serde(default)]
    pub last_seen: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// False until the two-screen setup has been completed once.
    pub onboarded: bool,
    /// What nearby devices call this one. `None` = derive from the hostname.
    pub device_name: Option<String>,
    /// Where received files land. `None` = the engine default (Downloads/Dropwire).
    pub dest_dir: Option<String>,
    /// Whether nearby sharing starts on. Off means invisible.
    pub nearby_on: bool,
    /// Theme: "auto" | "light" | "dark".
    pub theme: String,
    pub trusted: Vec<Trusted>,
    /// Let trusted devices skip the consent dialog. They still confirm on their
    /// side, and the verified file preview still gates the download.
    pub skip_code_for_trusted: bool,
    /// Keep running in the tray when the window is closed.
    pub tray_on_close: bool,
    pub start_at_login: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            onboarded: false,
            device_name: None,
            dest_dir: None,
            nearby_on: true,
            theme: "auto".into(),
            trusted: Vec::new(),
            skip_code_for_trusted: false,
            tray_on_close: true,
            start_at_login: false,
        }
    }
}

/// Settings plus the file they came from. Cheap to lock: every mutation is a
/// user action, never a hot path.
pub struct Store {
    path: PathBuf,
    inner: Mutex<Settings>,
}

impl Store {
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("settings.json");
        let inner = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .unwrap_or_default();
        Self {
            path,
            inner: Mutex::new(inner),
        }
    }

    pub fn get(&self) -> Settings {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Apply `f` and persist. Best-effort: a failed write is logged, not raised.
    pub fn update<F: FnOnce(&mut Settings)>(&self, f: F) -> Settings {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard);
        let snapshot = guard.clone();
        drop(guard);
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&snapshot) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&self.path, json) {
                    tracing_warn(&format!("settings write failed: {e}"));
                }
            }
            Err(e) => tracing_warn(&format!("settings encode failed: {e}")),
        }
        snapshot
    }

    /// Record a completed transfer with a device, or bump its counters.
    pub fn remember(&self, mut dev: Trusted) -> Settings {
        self.update(|s| {
            let now = now_secs();
            match s
                .trusted
                .iter_mut()
                .find(|t| t.endpoint_id == dev.endpoint_id)
            {
                Some(existing) => {
                    existing.transfers = existing.transfers.saturating_add(1);
                    existing.last_seen = now;
                    if !dev.name.is_empty() {
                        existing.name = std::mem::take(&mut dev.name);
                    }
                    if dev.os.is_some() {
                        existing.os = dev.os.take();
                    }
                    if !dev.fingerprint.is_empty() {
                        existing.fingerprint = std::mem::take(&mut dev.fingerprint);
                    }
                }
                None => {
                    dev.transfers = 1;
                    dev.last_seen = now;
                    s.trusted.push(dev);
                }
            }
        })
    }

    pub fn forget(&self, endpoint_id: &str) -> Settings {
        self.update(|s| s.trusted.retain(|t| t.endpoint_id != endpoint_id))
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn tracing_warn(msg: &str) {
    eprintln!("[dropwire] {msg}");
}

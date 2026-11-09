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
//!
//! Writes go to a temp file that is then renamed over `settings.json`, so a
//! crash or power loss mid-write leaves the old file or the new one, never a
//! truncated one. A file that cannot be read is set aside as
//! `settings.corrupt-<unix secs>.json` (and noted in the log) before defaults
//! are used, so the next save cannot destroy the only copy.

use std::io::Write;
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
    /// Mirrors the system's startup list: set only after the entry was added
    /// or removed, and corrected at launch if it was changed outside the app.
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
    /// False when `settings.json` could not be read and could not be set
    /// aside either. Saving would then overwrite the only copy, so this run
    /// keeps its changes in memory instead.
    persist: bool,
}

impl Store {
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("settings.json");
        let tmp = tmp_path(&path);
        let failure = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(settings) => {
                    // A temp file beside a good settings.json is a save that
                    // never finished; the file it was replacing still stands.
                    let _ = std::fs::remove_file(&tmp);
                    return Self::new(path, settings, true);
                }
                Err(e) => format!("could not be parsed ({e})"),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let settings = recover_tmp(&path, &tmp).unwrap_or_default();
                return Self::new(path, settings, true);
            }
            Err(e) => format!("could not be read ({e})"),
        };
        let persist = match set_aside(&path) {
            Some(backup) => {
                log_warn(&format!(
                    "{} {failure}. It was kept as {} and the defaults are in use.",
                    path.display(),
                    backup.display()
                ));
                true
            }
            None => {
                log_warn(&format!(
                    "{} {failure} and could not be set aside. The defaults are in use \
                     and changes will not be saved until it can be read again.",
                    path.display()
                ));
                false
            }
        };
        let settings = if persist {
            recover_tmp(&path, &tmp).unwrap_or_default()
        } else {
            Settings::default()
        };
        Self::new(path, settings, persist)
    }

    fn new(path: PathBuf, settings: Settings, persist: bool) -> Self {
        Self {
            path,
            inner: Mutex::new(settings),
            persist,
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
        // Written while the lock is held, so two quick changes reach the disk
        // in the order they were made.
        if self.persist {
            self.write(&snapshot);
        }
        drop(guard);
        snapshot
    }

    /// Write `settings` to a temp file and rename it over `settings.json`.
    fn write(&self, settings: &Settings) {
        let json = match serde_json::to_vec_pretty(settings) {
            Ok(json) => json,
            Err(e) => {
                log_warn(&format!("settings encode failed: {e}"));
                return;
            }
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = tmp_path(&self.path);
        let written = std::fs::File::create(&tmp).and_then(|mut f| {
            f.write_all(&json)?;
            // Best-effort: the rename below is what keeps the old file intact.
            let _ = f.sync_all();
            Ok(())
        });
        // std's rename replaces an existing target on every platform.
        if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, &self.path)) {
            log_warn(&format!("settings write failed: {e}"));
            let _ = std::fs::remove_file(&tmp);
        }
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

/// `settings.json.tmp`, the file a save writes before renaming it into place.
fn tmp_path(path: &Path) -> PathBuf {
    path.with_extension("json.tmp")
}

/// A save that crashed after writing its temp file but before the rename
/// left a complete copy there (a cut-off one does not parse). Use it and
/// move it into place.
fn recover_tmp(path: &Path, tmp: &Path) -> Option<Settings> {
    let bytes = std::fs::read(tmp).ok()?;
    let Ok(settings) = serde_json::from_slice::<Settings>(&bytes) else {
        let _ = std::fs::remove_file(tmp);
        return None;
    };
    if std::fs::rename(tmp, path).is_ok() {
        log_warn(&format!(
            "recovered settings from an unfinished save at {}",
            tmp.display()
        ));
    }
    Some(settings)
}

/// Move an unreadable settings file out of the way, keeping its contents, so
/// the next save cannot overwrite them. Returns where it went.
fn set_aside(path: &Path) -> Option<PathBuf> {
    let dir = path.parent()?;
    let stamp = now_secs();
    let backup = (0..100)
        .map(|n| {
            dir.join(if n == 0 {
                format!("settings.corrupt-{stamp}.json")
            } else {
                format!("settings.corrupt-{stamp}-{n}.json")
            })
        })
        .find(|p| !p.exists())?;
    if std::fs::rename(path, &backup).is_ok() || std::fs::copy(path, &backup).is_ok() {
        Some(backup)
    } else {
        None
    }
}

/// Note a settings problem where it can be found later. Release builds have
/// no console, so it also goes to the breadcrumb log beside the app data.
fn log_warn(msg: &str) {
    eprintln!("[dropwire] {msg}");
    // Unit tests must not write into the real app data folder.
    #[cfg(not(test))]
    crate::append_breadcrumb(&format!("[settings] {msg}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dropwire-settings-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn corrupt_copies(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .expect("read dir")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("settings.corrupt-"))
            })
            .collect()
    }

    #[test]
    fn a_save_replaces_the_file_and_leaves_no_temp_file() {
        let dir = scratch("save");
        let store = Store::load(&dir);
        store.update(|s| {
            s.onboarded = true;
            s.device_name = Some("Desk".into());
        });
        let again = Store::load(&dir).get();
        assert!(again.onboarded);
        assert_eq!(again.device_name.as_deref(), Some("Desk"));
        assert!(!tmp_path(&dir.join("settings.json")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cut-off file (power loss during an old in-place write) is kept aside,
    /// the app starts on defaults, and later saves never touch the copy.
    #[test]
    fn a_damaged_file_is_kept_aside_not_overwritten() {
        let dir = scratch("corrupt");
        let path = dir.join("settings.json");
        let damaged = br#"{"onboarded": true, "deviceName": "Desk", "trus"#;
        std::fs::write(&path, damaged).expect("write");

        let store = Store::load(&dir);
        assert!(!store.get().onboarded, "defaults expected");
        let copies = corrupt_copies(&dir);
        assert_eq!(copies.len(), 1, "{copies:?}");
        assert_eq!(std::fs::read(&copies[0]).expect("copy"), damaged);

        store.update(|s| s.onboarded = true);
        store.update(|s| s.theme = "dark".into());
        assert_eq!(std::fs::read(&copies[0]).expect("copy"), damaged);
        let saved = Store::load(&dir).get();
        assert!(saved.onboarded);
        assert_eq!(saved.theme, "dark");
        assert_eq!(corrupt_copies(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A save that stopped between writing the temp file and renaming it is
    /// picked up instead of starting over on defaults.
    #[test]
    fn an_unfinished_save_is_recovered() {
        let dir = scratch("tmp");
        let path = dir.join("settings.json");
        let saved = Settings {
            onboarded: true,
            device_name: Some("Laptop".into()),
            ..Settings::default()
        };
        std::fs::write(tmp_path(&path), serde_json::to_vec(&saved).expect("encode"))
            .expect("write tmp");

        let got = Store::load(&dir).get();
        assert!(got.onboarded);
        assert_eq!(got.device_name.as_deref(), Some("Laptop"));
        assert!(
            path.exists(),
            "the temp file should have been moved into place"
        );
        assert!(!tmp_path(&path).exists());

        // A cut-off temp file beside a good settings.json is dropped.
        std::fs::write(tmp_path(&path), b"{\"onboarded\": fa").expect("write tmp");
        let got = Store::load(&dir).get();
        assert!(got.onboarded);
        assert!(!tmp_path(&path).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

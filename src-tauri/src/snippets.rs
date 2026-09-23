//! Text sent with "Send text".
//!
//! Text is not a second protocol: each snippet is written to a small file and
//! takes the ordinary send path. Those files are often clipboard contents
//! (passwords, keys), so they live in the app's data folder, next to the
//! transfer history, and only as long as something can still use them: a
//! history record (Resend reads the file again) or a send that is still
//! serving it (a large snippet is read from the file while it is served).
//! Everything else is deleted when history is cleared, when such a send ends,
//! and at every launch.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct Snippets {
    dir: PathBuf,
    holds: Mutex<Holds>,
}

#[derive(Default)]
struct Holds {
    /// Written, not yet handed to a send.
    fresh: HashSet<PathBuf>,
    /// Sends still running, per snippet.
    sending: HashMap<PathBuf, usize>,
}

impl Snippets {
    /// Snippets kept in `dir`, which must be inside the app's data folder.
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            holds: Mutex::new(Holds::default()),
        }
    }

    fn holds(&self) -> std::sync::MutexGuard<'_, Holds> {
        self.holds.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Write `text` to a new file and hold it until a send takes it over.
    /// Two snippets in the same second get different names, so one can never
    /// overwrite the other or send the wrong text on Resend.
    pub fn write(&self, text: &str) -> Result<PathBuf, String> {
        let fail = |e: std::io::Error| format!("could not save the text to send: {e}");
        std::fs::create_dir_all(&self.dir).map_err(fail)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // Held while the file is created, so a sweep never sees it unheld.
        let mut holds = self.holds();
        for n in 0..1000u32 {
            let name = if n == 0 {
                format!("shared-text-{stamp}.txt")
            } else {
                format!("shared-text-{stamp}-{n}.txt")
            };
            let path = self.dir.join(name);
            let mut file = match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(fail(e)),
            };
            if let Err(e) = file.write_all(text.as_bytes()) {
                drop(file);
                let _ = std::fs::remove_file(&path);
                return Err(fail(e));
            }
            holds.fresh.insert(path.clone());
            return Ok(path);
        }
        Err("could not save the text to send: too many snippets at once".into())
    }

    /// A send is starting from `path`. If it is one of ours, hold it until
    /// [`Self::release`]; the returned path is the hold to release.
    pub fn hold_for_send(&self, path: &Path) -> Option<PathBuf> {
        if path.parent() != Some(self.dir.as_path()) {
            return None;
        }
        let mut holds = self.holds();
        holds.fresh.remove(path);
        *holds.sending.entry(path.to_path_buf()).or_default() += 1;
        Some(path.to_path_buf())
    }

    /// The send from `path` (a hold from [`Self::hold_for_send`]) has ended.
    /// Delete the snippet unless another send or a history record (`sources`)
    /// still uses it.
    pub fn release<'a>(&self, path: &Path, sources: impl IntoIterator<Item = &'a str>) {
        let mut holds = self.holds();
        let Some(n) = holds.sending.get_mut(path) else {
            return;
        };
        *n = n.saturating_sub(1);
        if *n > 0 {
            return;
        }
        holds.sending.remove(path);
        if holds.fresh.contains(path) {
            return;
        }
        let referenced = path
            .file_name()
            .is_some_and(|name| referenced_names(sources).contains(name));
        if !referenced {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Delete every snippet that no history record (`sources`) and no running
    /// send uses. Returns how many were deleted.
    pub fn sweep<'a>(&self, sources: impl IntoIterator<Item = &'a str>) -> usize {
        let keep = referenced_names(sources);
        let holds = self.holds();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return 0;
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if !entry.file_type().is_ok_and(|t| t.is_file())
                || keep.contains(&entry.file_name())
                || holds.fresh.contains(&path)
                || holds.sending.contains_key(&path)
            {
                continue;
            }
            if std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

/// File names that history records point at. Matching by name keeps a
/// snippet even if its folder was spelled differently when it was recorded;
/// the names are unique, so this only ever errs toward keeping one.
fn referenced_names<'a>(sources: impl IntoIterator<Item = &'a str>) -> HashSet<OsString> {
    sources
        .into_iter()
        .filter_map(|s| Path::new(s).file_name().map(|n| n.to_os_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dropwire-snippets-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn snippets_written_together_never_share_a_file() {
        let root = scratch("unique");
        let snippets = Snippets::new(root.join("sent-text"));
        let a = snippets.write("first").expect("write");
        let b = snippets.write("second").expect("write");
        assert_ne!(a, b);
        assert_eq!(std::fs::read_to_string(&a).expect("a"), "first");
        assert_eq!(std::fs::read_to_string(&b).expect("b"), "second");
        assert_eq!(a.parent(), Some(root.join("sent-text").as_path()));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A sweep deletes only what nothing uses: not a snippet a history record
    /// points at, not one waiting for its send, not one a send is serving.
    #[test]
    fn a_sweep_keeps_what_is_still_in_use() {
        let root = scratch("sweep");
        let snippets = Snippets::new(root.join("sent-text"));
        let recorded = snippets.write("in history").expect("write");
        let waiting = snippets.write("not sent yet").expect("write");
        let serving = snippets.write("being served").expect("write");
        let orphan = snippets.write("history cleared").expect("write");
        for p in [&recorded, &serving, &orphan] {
            assert!(snippets.hold_for_send(p).is_some());
            snippets.release(p, [s(&recorded).as_str(), s(p).as_str()]);
        }
        let serving_hold = snippets.hold_for_send(&serving).expect("ours");
        std::fs::write(root.join("sent-text").join("left-over.txt"), "x").expect("write");

        let removed = snippets.sweep([s(&recorded).as_str()]);
        assert_eq!(removed, 2, "the cleared snippet and the stray file");
        assert!(recorded.exists());
        assert!(waiting.exists());
        assert!(serving.exists());
        assert!(!orphan.exists());

        // The send ends after its history was cleared: the snippet goes too.
        snippets.release(&serving_hold, [s(&recorded).as_str()]);
        assert!(!serving.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Resending a snippet while its first send still serves it: the file
    /// stays until the last of them ends.
    #[test]
    fn a_snippet_outlives_every_send_that_uses_it() {
        let root = scratch("twice");
        let snippets = Snippets::new(root.join("sent-text"));
        let p = snippets.write("twice").expect("write");
        let first = snippets.hold_for_send(&p).expect("ours");
        let second = snippets.hold_for_send(&p).expect("ours");
        snippets.release(&first, []);
        assert!(p.exists());
        assert_eq!(snippets.sweep([]), 0);
        snippets.release(&second, []);
        assert!(!p.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn files_outside_the_snippet_folder_are_never_held_or_deleted() {
        let root = scratch("outside");
        std::fs::create_dir_all(&root).expect("dir");
        let snippets = Snippets::new(root.join("sent-text"));
        let mine = root.join("report.txt");
        std::fs::write(&mine, "keep").expect("write");
        assert!(snippets.hold_for_send(&mine).is_none());
        snippets.release(&mine, []);
        assert!(mine.exists());
        assert_eq!(snippets.sweep([]), 0);
        let _ = std::fs::remove_dir_all(&root);
    }
}

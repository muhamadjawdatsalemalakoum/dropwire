//! Turning the names in a received transfer into safe paths under the folder
//! the receiver chose.
//!
//! Names come from the sender, so they are untrusted: they may try to climb out
//! of the destination (`../x`, `/abs`, `C:evil`), or simply be legal on the
//! sender's OS and not on ours (`Why?.pdf` from a Mac, `a:b.txt` from Linux).
//! Every segment is cleaned with Windows' rules on every OS, so a transfer saves
//! under the same names everywhere, including exFAT or SMB targets on macOS and
//! Linux.
//!
//! Saving never replaces anything already on disk: see [`plan`] and [`save`].

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Component, Path, PathBuf};

use iroh_blobs::api::blobs::{ExportMode, ExportOptions};
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::Hash;
use tokio_util::sync::CancellationToken;

use crate::progress::RenamedFile;

/// Characters Windows refuses in a file name. `:` would otherwise open an NTFS
/// alternate data stream, or read as a drive prefix.
const RESERVED_CHARS: [char; 7] = ['<', '>', ':', '"', '|', '?', '*'];

/// Longest name segment we write, in bytes. 255 UTF-8 bytes are never more than
/// 255 UTF-16 units, so this fits NTFS as well as APFS and ext4.
const MAX_SEGMENT_BYTES: usize = 255;

/// Names Windows maps to devices, whatever the extension (`NUL.txt` is `NUL`).
fn is_reserved_device(stem: &str) -> bool {
    const NAMES: [&str; 6] = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];
    let upper = stem.trim_end_matches(' ').to_uppercase();
    if NAMES.contains(&upper.as_str()) {
        return true;
    }
    // COM0-9 and LPT0-9, plus the superscript digits Windows also honours.
    for prefix in ["COM", "LPT"] {
        if let Some(rest) = upper.strip_prefix(prefix) {
            let mut chars = rest.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                if c.is_ascii_digit() || matches!(c, '\u{b9}' | '\u{b2}' | '\u{b3}') {
                    return true;
                }
            }
        }
    }
    false
}

/// Cut `s` to at most `max` bytes on a char boundary.
fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Split a file name into stem and extension (with its dot), treating a leading
/// dot as part of the stem and keeping `.tar.gz`-style pairs together.
pub(crate) fn split_ext(name: &str) -> (&str, &str) {
    let Some(dot) = name.rfind('.').filter(|&i| i > 0) else {
        return (name, "");
    };
    let (stem, ext) = name.split_at(dot);
    if let Some(inner) = stem.rfind('.').filter(|&i| i > 0) {
        if stem[inner..].eq_ignore_ascii_case(".tar") {
            return name.split_at(inner);
        }
    }
    (stem, ext)
}

/// Fit `name` in [`MAX_SEGMENT_BYTES`], trimming the stem and keeping a
/// reasonably short extension.
pub(crate) fn cap_segment(name: &str) -> String {
    if name.len() <= MAX_SEGMENT_BYTES {
        return name.to_string();
    }
    let (stem, ext) = split_ext(name);
    let out = if !ext.is_empty() && ext.len() <= 32 {
        format!(
            "{}{ext}",
            truncate_bytes(stem, MAX_SEGMENT_BYTES - ext.len())
        )
    } else {
        truncate_bytes(name, MAX_SEGMENT_BYTES).to_string()
    };
    // The cut may have left a trailing dot or space, which Windows drops.
    let trimmed = out.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        "_".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Clean one path segment. `None` means the segment is dropped entirely (empty,
/// `.` or `..`), which is what keeps a name from climbing out of the folder.
pub(crate) fn sanitize_segment(seg: &str) -> Option<String> {
    if seg.is_empty() || seg == "." || seg == ".." {
        return None;
    }
    let replaced: String = seg
        .chars()
        .map(|c| {
            if c.is_control() || RESERVED_CHARS.contains(&c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    // Windows silently drops trailing dots and spaces, so "a." and "a" would be
    // the same file there.
    let trimmed = replaced.trim_end_matches(['.', ' ']);
    let mut out = if trimmed.is_empty() {
        "_".to_string()
    } else {
        trimmed.to_string()
    };
    let stem = out.split('.').next().unwrap_or("");
    if is_reserved_device(stem) {
        out.insert(0, '_');
    }
    Some(cap_segment(&out))
}

/// Split a sender-supplied relative name into cleaned segments. Never empty:
/// a name with nothing usable left becomes `file`.
pub(crate) fn sanitize_segments(name: &str) -> Vec<String> {
    // Split by hand rather than with `Path::components`, which on Windows reads
    // a segment like `c:foo` as a drive prefix and discards everything before it.
    let segs: Vec<String> = name
        .replace('\\', "/")
        .split('/')
        .filter_map(sanitize_segment)
        .collect();
    if segs.is_empty() {
        vec!["file".to_string()]
    } else {
        segs
    }
}

/// The cleaned relative path for a sender-supplied name.
#[cfg(test)]
pub(crate) fn sanitize_rel(name: &str) -> PathBuf {
    sanitize_segments(name).iter().collect()
}

/// Join cleaned segments onto `dest`, refusing anything that would not stay a
/// plain child of it. Sanitizing already guarantees this; the check keeps a
/// future change to the cleaning rules from quietly writing outside `dest`.
pub(crate) fn join_under(dest: &Path, segs: &[String]) -> Option<PathBuf> {
    let mut out = dest.to_path_buf();
    for seg in segs {
        let mut comps = Path::new(seg).components();
        match (comps.next(), comps.next()) {
            (Some(Component::Normal(_)), None) => out.push(seg),
            _ => return None,
        }
    }
    out.starts_with(dest).then_some(out)
}

/// How many failed files a receive error names before summarising the rest.
const FAILURES_NAMED: usize = 3;

/// The message a receive ends with when some files could not be written:
/// which ones (the first few, by the name they were sent with) and why.
pub(crate) fn describe_failures(failed: &[(String, String)], attempted: usize) -> String {
    let mut list = failed
        .iter()
        .take(FAILURES_NAMED)
        .map(|(name, why)| format!("{name} ({why})"))
        .collect::<Vec<_>>()
        .join(", ");
    let more = failed.len().saturating_sub(FAILURES_NAMED);
    if more > 0 {
        list.push_str(&format!(", and {more} more"));
    }
    if attempted <= 1 {
        format!("could not save {list}")
    } else {
        format!(
            "{} of {attempted} files could not be saved: {list}",
            failed.len()
        )
    }
}

/// How far "name (n)" counting goes before a file is reported as unsaveable.
const MAX_SUFFIX: u32 = 9999;

/// Most renames listed in a receive's final stats. A crafted transfer could
/// otherwise make the completion event arbitrarily large.
pub(crate) const MAX_RENAMED_REPORTED: usize = 1000;

/// Case folding for collision checks: Windows and macOS file systems treat
/// "README.md" and "Readme.md" as one file by default, Linux does not.
fn fold(s: &str) -> String {
    if cfg!(any(windows, target_os = "macos")) {
        s.to_lowercase()
    } else {
        s.to_string()
    }
}

/// "photo.jpg" becomes "photo (2).jpg"; a folder "Photos" becomes "Photos (2)".
/// Kept within [`MAX_SEGMENT_BYTES`] by trimming the stem.
pub(crate) fn with_suffix(name: &str, n: u32, is_dir: bool) -> String {
    let (stem, ext) = if is_dir {
        (name, "")
    } else {
        match split_ext(name) {
            (stem, ext) if ext.len() <= 32 => (stem, ext),
            _ => (name, ""),
        }
    };
    let tag = format!(" ({n})");
    let stem = truncate_bytes(stem, MAX_SEGMENT_BYTES - tag.len() - ext.len());
    format!("{stem}{tag}{ext}")
}

/// A file the receive has to write, as the transfer lists it.
pub(crate) struct Wanted {
    /// Position in the transfer's file list.
    pub index: usize,
    /// The name as sent (untrusted).
    pub name: String,
    pub hash: Hash,
    /// Verified size in bytes.
    pub size: u64,
}

/// Where one wanted file will be saved.
pub(crate) struct Planned {
    pub index: usize,
    pub name: String,
    pub hash: Hash,
    /// Cleaned relative path segments under the destination.
    pub segs: Vec<String>,
    /// The same content is already saved at that path (an earlier receive of
    /// this transfer), so there is nothing to write.
    pub present: bool,
}

/// The outcome of [`plan`].
#[derive(Default)]
pub(crate) struct Plan {
    pub files: Vec<Planned>,
    pub renamed: Vec<RenamedFile>,
    /// Files that could not be given any free name: (name as sent, reason).
    pub failed: Vec<(String, String)>,
}

impl Plan {
    fn note_rename(&mut self, name: String, saved_as: String) {
        if name != saved_as && self.renamed.len() < MAX_RENAMED_REPORTED {
            self.renamed.push(RenamedFile { name, saved_as });
        }
    }
}

/// The sent name split on slashes, without the parts cleaning always drops, so
/// it can be compared with where the file actually lands.
fn raw_segments(name: &str) -> Vec<String> {
    name.replace('\\', "/")
        .split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .map(str::to_string)
        .collect()
}

struct Item {
    wanted: Wanted,
    /// Cleaned segments; `segs[0]` is the top-level name.
    segs: Vec<String>,
    raw: Vec<String>,
}

/// Everything under one top-level name: a single file, or a folder's files.
struct Group {
    root: String,
    is_dir: bool,
    items: Vec<Item>,
    /// Per item: the path below the root after in-transfer collisions are
    /// resolved (`None` when no free name was found).
    rest: Vec<Option<Vec<String>>>,
}

/// Decide where every wanted file goes, without writing anything.
///
/// Nothing on disk is ever replaced: a top-level file or folder whose name is
/// taken is saved as "name (1)", "name (2)" and so on, decided once per
/// top-level name so a whole folder lands together as "Photos (1)/...". Names
/// that collide inside the transfer itself (after cleaning, or by case on
/// Windows and macOS) are told apart the same way. A name already holding the
/// very same content (an earlier receive of this transfer) is reused, so
/// receiving again does not pile up copies.
pub(crate) fn plan(dest: &Path, wanted: Vec<Wanted>) -> Plan {
    let mut out = Plan::default();

    // Group by top-level name. Folders with the same name (as the file system
    // sees it) share one group; single files each stand alone.
    let mut groups: Vec<Group> = Vec::new();
    let mut dir_group: HashMap<String, usize> = HashMap::new();
    for w in wanted {
        let segs = sanitize_segments(&w.name);
        let raw = raw_segments(&w.name);
        let item = Item {
            wanted: w,
            segs,
            raw,
        };
        let is_dir = item.segs.len() > 1;
        let key = fold(&item.segs[0]);
        match dir_group.get(&key).copied().filter(|_| is_dir) {
            Some(g) => groups[g].items.push(item),
            None => {
                if is_dir {
                    dir_group.insert(key, groups.len());
                }
                groups.push(Group {
                    root: item.segs[0].clone(),
                    is_dir,
                    items: vec![item],
                    rest: Vec::new(),
                });
            }
        }
    }

    let mut claimed_roots: HashSet<String> = HashSet::new();
    // Where numbering resumes for a top-level name that already came up, so a
    // crafted transfer repeating one name many times costs linear work.
    let mut next_suffix: HashMap<(String, bool), u32> = HashMap::new();
    for mut group in groups {
        group.rest = resolve_inside(&group.items);
        let start = next_suffix
            .entry((fold(&group.root), group.is_dir))
            .or_insert(0);
        let chosen = choose_root(dest, &group, &claimed_roots, *start);
        *start = chosen.as_ref().map_or(MAX_SUFFIX + 1, |c| c.2 + 1);
        let Some((root, present, _)) = chosen else {
            for item in group.items {
                out.failed
                    .push((item.wanted.name, "no free name to save it under".into()));
            }
            continue;
        };
        claimed_roots.insert(fold(&root));

        if group.is_dir {
            if let Some(raw_root) = group.items[0].raw.first() {
                out.note_rename(raw_root.clone(), root.clone());
            }
        } else {
            out.note_rename(group.items[0].raw.join("/"), root.clone());
        }

        for (k, item) in group.items.into_iter().enumerate() {
            let Some(rest) = group.rest[k].take() else {
                out.failed
                    .push((item.wanted.name, "no free name to save it under".into()));
                continue;
            };
            if group.is_dir && item.raw.get(1..) != Some(&rest[..]) {
                out.note_rename(item.raw.join("/"), format!("{root}/{}", rest.join("/")));
            }
            let mut segs = Vec::with_capacity(rest.len() + 1);
            segs.push(root.clone());
            segs.extend(rest);
            out.files.push(Planned {
                index: item.wanted.index,
                name: item.wanted.name,
                hash: item.wanted.hash,
                segs,
                present: present.get(k).copied().unwrap_or(false),
            });
        }
    }
    out.files.sort_by_key(|f| f.index);
    out
}

/// Give every file in a group a distinct path below the group's root. Files
/// whose cleaned name matches what was sent keep it; the others (and any file
/// that would sit where a subfolder has to go) get a " (n)" suffix.
fn resolve_inside(items: &[Item]) -> Vec<Option<Vec<String>>> {
    let rests: Vec<&[String]> = items.iter().map(|i| &i.segs[1..]).collect();
    let mut dirs: HashSet<String> = HashSet::new();
    for rest in &rests {
        for k in 1..rest.len() {
            dirs.insert(fold(&rest[..k].join("/")));
        }
    }
    let mut claimed: HashSet<String> = HashSet::new();
    let mut out: Vec<Option<Vec<String>>> = vec![None; items.len()];
    // Unchanged names claim first, so a file really called "a_b.txt" keeps its
    // name and the one sent as "a:b.txt" is the one renamed.
    for unchanged_first in [true, false] {
        for (k, item) in items.iter().enumerate() {
            let unchanged = item.raw.get(1..) == Some(rests[k]);
            if out[k].is_some() || unchanged != unchanged_first {
                continue;
            }
            let key = fold(&rests[k].join("/"));
            if !claimed.contains(&key) && !dirs.contains(&key) {
                claimed.insert(key);
                out[k] = Some(rests[k].to_vec());
            }
        }
    }
    // Where numbering resumes for a name that already collided, so many
    // copies of one name cost linear work rather than quadratic.
    let mut next_suffix: HashMap<String, u32> = HashMap::new();
    for (k, rest) in rests.iter().enumerate() {
        if out[k].is_some() {
            continue;
        }
        let Some((leaf, parent)) = rest.split_last() else {
            // A single file has nothing below its root, so it cannot collide here.
            out[k] = Some(Vec::new());
            continue;
        };
        let next = next_suffix.entry(fold(&rest.join("/"))).or_insert(1);
        while *next <= MAX_SUFFIX {
            let mut cand = parent.to_vec();
            cand.push(with_suffix(leaf, *next, false));
            *next += 1;
            let key = fold(&cand.join("/"));
            if !claimed.contains(&key) && !dirs.contains(&key) {
                claimed.insert(key);
                out[k] = Some(cand);
                break;
            }
        }
    }
    out
}

/// Pick the top-level name for a group: the sent one if it is free, otherwise
/// the first free "name (n)", trying numbers from `start` on. An existing
/// entry is reused only when it already holds this content. Returns the name,
/// per item whether its content is already there, and the number used.
fn choose_root(
    dest: &Path,
    group: &Group,
    claimed: &HashSet<String>,
    start: u32,
) -> Option<(String, Vec<bool>, u32)> {
    for n in start..=MAX_SUFFIX {
        let cand = if n == 0 {
            group.root.clone()
        } else {
            with_suffix(&group.root, n, group.is_dir)
        };
        if claimed.contains(&fold(&cand)) {
            continue;
        }
        let path = dest.join(&cand);
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Some((cand, vec![false; group.items.len()], n));
            }
            Err(_) => continue,
            Ok(meta) if group.is_dir => {
                if meta.is_dir() {
                    if let Some(present) = reusable_folder(&path, group) {
                        return Some((cand, present, n));
                    }
                }
            }
            Ok(meta) => {
                let item = &group.items[0];
                if meta.is_file()
                    && meta.len() == item.wanted.size
                    && same_content(&path, &item.wanted.hash)
                {
                    return Some((cand, vec![true], n));
                }
            }
        }
    }
    None
}

/// An existing folder can take this group only if everything the transfer
/// would put in it is either missing or already identical, and at least one
/// file is identical (so it is an earlier copy of this transfer, not an
/// unrelated folder that happens to share the name).
fn reusable_folder(root: &Path, group: &Group) -> Option<Vec<bool>> {
    // Cheap pass first: types and sizes only.
    let mut candidates: Vec<Option<PathBuf>> = Vec::with_capacity(group.items.len());
    for (k, item) in group.items.iter().enumerate() {
        let rest = group.rest[k].as_ref()?;
        let mut path = root.to_path_buf();
        let mut existing = None;
        for (depth, seg) in rest.iter().enumerate() {
            path.push(seg);
            match std::fs::symlink_metadata(&path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => break,
                Err(_) => return None,
                Ok(meta) if depth + 1 < rest.len() => {
                    // Symlinks are not followed: a link where a folder should be
                    // could point anywhere.
                    if !meta.is_dir() {
                        return None;
                    }
                }
                Ok(meta) => {
                    if !meta.is_file() || meta.len() != item.wanted.size {
                        return None;
                    }
                    existing = Some(path.clone());
                }
            }
        }
        candidates.push(existing);
    }
    if candidates.iter().all(Option::is_none) {
        return None;
    }
    // Then hash the files whose size already matches.
    let mut present = Vec::with_capacity(candidates.len());
    for (k, cand) in candidates.iter().enumerate() {
        match cand {
            Some(path) if same_content(path, &group.items[k].wanted.hash) => present.push(true),
            Some(_) => return None,
            None => present.push(false),
        }
    }
    Some(present)
}

/// Whether the file at `path` hashes to `hash` (BLAKE3, the transfer's own
/// content address).
fn same_content(path: &Path, hash: &Hash) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut hasher = blake3::Hasher::new();
    if hasher.update_reader(file).is_err() {
        return false;
    }
    hasher.finalize().as_bytes() == hash.as_bytes()
}

/// What happened to one planned file.
pub(crate) enum Saved {
    /// Written under the planned name or, if that was taken at the last
    /// moment, under the name given here.
    Written { renamed: Option<RenamedFile> },
    /// The receive was cancelled while this file was being written.
    Cancelled,
}

/// Write one planned file without ever opening an existing file for writing.
///
/// The final name is reserved first with `create_new`, which fails rather
/// than replace anything (and also catches aliases the planner could not
/// predict, such as Unicode normalization on macOS). The content is then
/// copied into a new hidden file in the same folder and renamed over the
/// empty reservation, so a half-written file never appears under the real
/// name. On failure or cancellation both are removed.
pub(crate) async fn save(
    store: &FsStore,
    dest: &Path,
    file: &Planned,
    transfer: &str,
    token: &CancellationToken,
) -> Result<Saved, String> {
    let mut target = join_under(dest, &file.segs).ok_or("unsafe file name")?;
    let parent = target.parent().ok_or("unsafe file name")?.to_path_buf();
    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;

    let leaf = file.segs.last().ok_or("unsafe file name")?;
    let mut renamed = None;
    let mut n = 0;
    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(_) => break,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && n < MAX_SUFFIX => {
                n += 1;
                let name = with_suffix(leaf, n, false);
                target = parent.join(&name);
                let mut segs = file.segs.clone();
                if let Some(last) = segs.last_mut() {
                    *last = name;
                }
                renamed = Some(RenamedFile {
                    name: raw_segments(&file.name).join("/"),
                    saved_as: segs.join("/"),
                });
            }
            Err(e) => return Err(e.to_string()),
        }
    }

    let temp = parent.join(format!(".dropwire-{transfer}-{}.part", file.index));
    let cleanup = |temp: &Path, target: &Path| {
        let _ = std::fs::remove_file(temp);
        let _ = std::fs::remove_file(target);
    };
    let export = store
        .export_with_opts(ExportOptions {
            hash: file.hash,
            target: temp.clone(),
            mode: ExportMode::Copy,
        })
        .finish();
    let copied = tokio::select! {
        res = export => res.map_err(|e| e.to_string()),
        _ = token.cancelled() => {
            cleanup(&temp, &target);
            // The store stops copying at its next chunk once nobody listens;
            // sweep again shortly in case it created the file after our removal.
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let _ = std::fs::remove_file(temp);
            });
            return Ok(Saved::Cancelled);
        }
    };
    if let Err(why) = copied {
        cleanup(&temp, &target);
        return Err(why);
    }

    // Replace our own empty reservation. A virus scanner can hold a brand-new
    // file open for a moment on Windows, so retry briefly before giving up.
    let mut attempt = 0;
    loop {
        match std::fs::rename(&temp, &target) {
            Ok(()) => break,
            Err(_) if attempt < 5 => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(50 * attempt)).await;
            }
            Err(e) => {
                cleanup(&temp, &target);
                return Err(e.to_string());
            }
        }
    }
    Ok(Saved::Written { renamed })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(name: &str) -> String {
        sanitize_segments(name).join("/")
    }

    fn dest() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\Users\me\Downloads\Dropwire")
        } else {
            PathBuf::from("/home/me/Downloads/Dropwire")
        }
    }

    /// Every case must land strictly inside the destination.
    fn assert_contained(name: &str) {
        let d = dest();
        let joined = join_under(&d, &sanitize_segments(name))
            .unwrap_or_else(|| panic!("{name:?} was not joinable under dest"));
        assert!(joined.starts_with(&d), "{name:?} escaped to {joined:?}");
        assert!(joined != d, "{name:?} collapsed onto dest itself");
    }

    #[test]
    fn windows_reserved_characters_are_replaced() {
        assert_eq!(clean("Invoice 3:15.pdf"), "Invoice 3_15.pdf");
        assert_eq!(clean("Why?.pdf"), "Why_.pdf");
        assert_eq!(clean("a<b>c|d\"e*f"), "a_b_c_d_e_f");
        assert_eq!(clean("10:30.log"), "10_30.log");
        assert_eq!(clean("tab\there\u{1}.txt"), "tab_here_.txt");
    }

    #[test]
    fn a_colon_never_becomes_a_drive_or_a_stream() {
        assert_eq!(clean("a:b.txt"), "a_b.txt");
        assert_eq!(clean("sub/c:evil.txt"), "sub/c_evil.txt");
        assert_eq!(clean("pics/D:/Users/x/run.bat"), "pics/D_/Users/x/run.bat");
        assert_eq!(clean("pics/C:evil.dll"), "pics/C_evil.dll");
        assert_eq!(clean("notes/ab:c.txt"), "notes/ab_c.txt");
        assert_eq!(clean("C:/abs"), "C_/abs");
        assert_eq!(clean("a.docm:Zone.Identifier"), "a.docm_Zone.Identifier");
        for name in [
            "a:b.txt",
            "sub/c:evil.txt",
            "pics/D:/Users/x/run.bat",
            "pics/C:evil.dll",
            "C:/abs",
            "C:",
            "C:\\Windows\\System32\\x.dll",
            "\\\\server\\share\\x",
        ] {
            assert_contained(name);
        }
        assert_eq!(
            sanitize_rel("a.docm:Zone.Identifier").components().count(),
            1
        );
    }

    #[test]
    fn trailing_dots_and_spaces_are_dropped() {
        assert_eq!(clean("trail. "), "trail");
        assert_eq!(clean("dir. /x.txt"), "dir/x.txt");
        assert_eq!(clean("..."), "_");
        assert_eq!(clean(".. "), "_");
        assert_eq!(clean("   "), "_");
    }

    #[test]
    fn reserved_device_names_are_prefixed() {
        assert_eq!(clean("NUL"), "_NUL");
        assert_eq!(clean("con.txt"), "_con.txt");
        assert_eq!(clean("COM1.log"), "_COM1.log");
        assert_eq!(clean("com1.tar.gz"), "_com1.tar.gz");
        assert_eq!(clean("lpt9"), "_lpt9");
        assert_eq!(clean("COM\u{b9}"), "_COM\u{b9}");
        assert_eq!(clean("aux .txt"), "_aux .txt");
        assert_eq!(clean("conout$"), "_conout$");
        assert_eq!(clean("sub/prn/x"), "sub/_prn/x");
        // Not device names: longer stems and other digits stay as they are.
        assert_eq!(clean("console.txt"), "console.txt");
        assert_eq!(clean("COM10"), "COM10");
        assert_eq!(clean("nul-report.pdf"), "nul-report.pdf");
    }

    #[test]
    fn traversal_and_absolute_parts_are_dropped() {
        assert_eq!(clean("../x"), "x");
        assert_eq!(clean("../../x"), "x");
        assert_eq!(clean("/abs/x"), "abs/x");
        assert_eq!(clean("/etc/passwd"), "etc/passwd");
        assert_eq!(clean("a/./b/../c"), "a/b/c");
        assert_eq!(clean("..\\..\\x"), "x");
        for name in ["../x", "../../x", "/abs/x", "..\\..\\x", "a/../../b"] {
            assert_contained(name);
        }
    }

    #[test]
    fn nothing_usable_falls_back_to_file() {
        assert_eq!(clean(""), "file");
        assert_eq!(clean("/"), "file");
        assert_eq!(clean("../.."), "file");
    }

    #[test]
    fn long_names_are_capped_and_keep_their_extension() {
        let long = format!("{}.txt", "a".repeat(296));
        assert_eq!(long.len(), 300);
        let out = clean(&long);
        assert_eq!(out.len(), MAX_SEGMENT_BYTES);
        assert!(out.ends_with(".txt"));

        // Multi-byte names are cut on a char boundary.
        let wide = format!("{}.pdf", "\u{e9}".repeat(200));
        let out = clean(&wide);
        assert!(out.len() <= MAX_SEGMENT_BYTES);
        assert!(out.ends_with(".pdf"));
        assert!(out.trim_end_matches(".pdf").chars().all(|c| c == '\u{e9}'));

        // A huge "extension" is not worth keeping.
        let odd = format!("x.{}", "b".repeat(400));
        assert_eq!(clean(&odd).len(), MAX_SEGMENT_BYTES);
    }

    #[test]
    fn ordinary_names_are_untouched() {
        for name in [
            "photo.jpg",
            "Photos/2024/IMG_0001.JPG",
            ".bashrc",
            "r\u{e9}sum\u{e9} final (2).docx",
            "\u{1f600} party.png",
            "archive.tar.gz",
        ] {
            assert_eq!(clean(name), name);
        }
    }

    #[test]
    fn names_that_collide_after_cleaning_are_detectable() {
        // Both map to the same path; the export step renames the second.
        assert_eq!(clean("a:b.txt"), clean("a_b.txt"));
        assert_eq!(clean("a."), clean("a"));
    }

    #[test]
    fn extensions_split_sensibly() {
        assert_eq!(split_ext("a.txt"), ("a", ".txt"));
        assert_eq!(split_ext("archive.tar.gz"), ("archive", ".tar.gz"));
        assert_eq!(split_ext(".bashrc"), (".bashrc", ""));
        assert_eq!(split_ext("noext"), ("noext", ""));
        assert_eq!(split_ext("v1.2.final.pdf"), ("v1.2.final", ".pdf"));
    }

    #[test]
    fn failures_are_named_and_counted() {
        let one = vec![("a.txt".to_string(), "denied".to_string())];
        assert_eq!(describe_failures(&one, 1), "could not save a.txt (denied)");
        assert_eq!(
            describe_failures(&one, 4),
            "1 of 4 files could not be saved: a.txt (denied)"
        );
        let many: Vec<_> = (0..5)
            .map(|i| (format!("f{i}"), "disk full".to_string()))
            .collect();
        assert_eq!(
            describe_failures(&many, 9),
            "5 of 9 files could not be saved: f0 (disk full), f1 (disk full), \
             f2 (disk full), and 2 more"
        );
    }

    #[test]
    fn suffixes_go_before_the_extension() {
        assert_eq!(with_suffix("photo.jpg", 1, false), "photo (1).jpg");
        assert_eq!(
            with_suffix("archive.tar.gz", 2, false),
            "archive (2).tar.gz"
        );
        assert_eq!(with_suffix(".bashrc", 1, false), ".bashrc (1)");
        assert_eq!(with_suffix("noext", 3, false), "noext (3)");
        assert_eq!(with_suffix("My.Photos", 1, true), "My.Photos (1)");
        let long = format!("{}.txt", "a".repeat(251));
        let out = with_suffix(&long, 12, false);
        assert!(out.len() <= MAX_SEGMENT_BYTES);
        assert!(out.ends_with(" (12).txt"));
    }

    fn wanted(index: usize, name: &str, data: &[u8]) -> Wanted {
        Wanted {
            index,
            name: name.to_string(),
            hash: Hash::new(data),
            size: data.len() as u64,
        }
    }

    fn saved(plan: &Plan) -> Vec<String> {
        plan.files.iter().map(|f| f.segs.join("/")).collect()
    }

    fn renames(plan: &Plan) -> Vec<(String, String)> {
        plan.renamed
            .iter()
            .map(|r| (r.name.clone(), r.saved_as.clone()))
            .collect()
    }

    #[test]
    fn free_names_are_kept() {
        let dest = tempfile::tempdir().unwrap();
        let plan = plan(
            dest.path(),
            vec![
                wanted(0, "pics/a.jpg", b"a"),
                wanted(1, "pics/sub/b.jpg", b"b"),
            ],
        );
        assert_eq!(saved(&plan), ["pics/a.jpg", "pics/sub/b.jpg"]);
        assert!(plan.renamed.is_empty());
        assert!(plan.failed.is_empty());
        assert!(plan.files.iter().all(|f| !f.present));
    }

    #[test]
    fn a_taken_file_name_gets_a_number() {
        let dest = tempfile::tempdir().unwrap();
        std::fs::write(dest.path().join("IMG_0001.JPG"), b"last week's photo").unwrap();
        std::fs::write(dest.path().join("IMG_0001 (1).JPG"), b"another one").unwrap();
        let plan = plan(
            dest.path(),
            vec![wanted(0, "IMG_0001.JPG", b"today's photo")],
        );
        assert_eq!(saved(&plan), ["IMG_0001 (2).JPG"]);
        assert_eq!(
            renames(&plan),
            [("IMG_0001.JPG".to_string(), "IMG_0001 (2).JPG".to_string())]
        );
    }

    #[test]
    fn identical_content_already_saved_is_not_duplicated() {
        let dest = tempfile::tempdir().unwrap();
        std::fs::write(dest.path().join("x.txt"), b"other").unwrap();
        std::fs::write(dest.path().join("x (1).txt"), b"same bytes").unwrap();
        let plan = plan(dest.path(), vec![wanted(0, "x.txt", b"same bytes")]);
        // Skips the unrelated x.txt and recognises its own earlier copy.
        assert_eq!(saved(&plan), ["x (1).txt"]);
        assert!(plan.files[0].present);
    }

    #[test]
    fn a_taken_folder_name_moves_the_whole_folder() {
        let dest = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dest.path().join("Photos")).unwrap();
        std::fs::write(dest.path().join("Photos").join("mine.jpg"), b"mine").unwrap();
        let plan = plan(
            dest.path(),
            vec![
                wanted(0, "Photos/a.jpg", b"a"),
                wanted(1, "Photos/trip/b.jpg", b"b"),
            ],
        );
        assert_eq!(saved(&plan), ["Photos (1)/a.jpg", "Photos (1)/trip/b.jpg"]);
        assert_eq!(
            renames(&plan),
            [("Photos".to_string(), "Photos (1)".to_string())]
        );
    }

    #[test]
    fn an_earlier_copy_of_the_same_folder_is_completed_in_place() {
        let dest = tempfile::tempdir().unwrap();
        let pics = dest.path().join("pics");
        std::fs::create_dir_all(&pics).unwrap();
        std::fs::write(pics.join("a.jpg"), b"a").unwrap();
        let plan = plan(
            dest.path(),
            vec![wanted(0, "pics/a.jpg", b"a"), wanted(1, "pics/b.jpg", b"b")],
        );
        assert_eq!(saved(&plan), ["pics/a.jpg", "pics/b.jpg"]);
        assert!(plan.files[0].present);
        assert!(!plan.files[1].present);
        assert!(plan.renamed.is_empty());
    }

    #[test]
    fn a_folder_with_a_conflicting_file_is_not_merged_into() {
        let dest = tempfile::tempdir().unwrap();
        let pics = dest.path().join("pics");
        std::fs::create_dir_all(&pics).unwrap();
        std::fs::write(pics.join("a.jpg"), b"a").unwrap();
        std::fs::write(pics.join("b.jpg"), b"not b").unwrap();
        let plan = plan(
            dest.path(),
            vec![wanted(0, "pics/a.jpg", b"a"), wanted(1, "pics/b.jpg", b"b")],
        );
        assert_eq!(saved(&plan), ["pics (1)/a.jpg", "pics (1)/b.jpg"]);
    }

    #[test]
    fn names_that_collide_after_cleaning_are_both_kept() {
        let dest = tempfile::tempdir().unwrap();
        let plan = plan(
            dest.path(),
            vec![
                wanted(0, "set/a:b.txt", b"colon"),
                wanted(1, "set/a_b.txt", b"underscore"),
                wanted(2, "set/c.", b"dot"),
                wanted(3, "set/c", b"plain"),
            ],
        );
        // The file really named "a_b.txt" keeps its name.
        assert_eq!(
            saved(&plan),
            ["set/a_b (1).txt", "set/a_b.txt", "set/c (1)", "set/c"]
        );
        assert_eq!(
            renames(&plan),
            [
                ("set/a:b.txt".to_string(), "set/a_b (1).txt".to_string()),
                ("set/c.".to_string(), "set/c (1)".to_string()),
            ]
        );
    }

    #[test]
    fn a_file_where_a_folder_must_go_is_renamed() {
        let dest = tempfile::tempdir().unwrap();
        let plan = plan(
            dest.path(),
            vec![wanted(0, "set/a", b"file"), wanted(1, "set/a/b", b"inner")],
        );
        assert_eq!(saved(&plan), ["set/a (1)", "set/a/b"]);
    }

    #[test]
    fn top_level_names_in_one_transfer_do_not_share_a_path() {
        let dest = tempfile::tempdir().unwrap();
        let plan = plan(
            dest.path(),
            vec![wanted(0, "a:b.txt", b"one"), wanted(1, "a_b.txt", b"two")],
        );
        let names = saved(&plan);
        assert_eq!(names.len(), 2);
        assert_ne!(names[0], names[1]);
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn names_differing_only_in_case_are_both_kept() {
        let dest = tempfile::tempdir().unwrap();
        let plan = plan(
            dest.path(),
            vec![
                wanted(0, "docs/README.md", b"upper"),
                wanted(1, "docs/Readme.md", b"mixed"),
            ],
        );
        assert_eq!(saved(&plan), ["docs/README.md", "docs/Readme (1).md"]);
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn an_existing_name_in_other_case_counts_as_taken() {
        let dest = tempfile::tempdir().unwrap();
        std::fs::write(dest.path().join("REPORT.PDF"), b"old").unwrap();
        let plan = plan(dest.path(), vec![wanted(0, "report.pdf", b"new")]);
        assert_eq!(saved(&plan), ["report (1).pdf"]);
    }

    #[test]
    fn many_copies_of_one_name_are_numbered_in_order() {
        let dest = tempfile::tempdir().unwrap();
        let top: Vec<Wanted> = (0..300)
            .map(|i| wanted(i, "a.txt", format!("copy {i}").as_bytes()))
            .collect();
        let plan_top = plan(dest.path(), top);
        let names = saved(&plan_top);
        assert_eq!(names[0], "a.txt");
        assert_eq!(names[1], "a (1).txt");
        assert_eq!(names[299], "a (299).txt");

        // Inside a folder: "x", "x.", "x.." ... all clean to "x".
        let inner: Vec<Wanted> = (0..50)
            .map(|i| wanted(i, &format!("set/x{}", ".".repeat(i)), &[i as u8]))
            .collect();
        let plan_inner = plan(dest.path(), inner);
        let names = saved(&plan_inner);
        assert_eq!(names[0], "set/x");
        assert_eq!(names[1], "set/x (1)");
        assert_eq!(names[49], "set/x (49)");
        let unique: HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), 50);
    }
}

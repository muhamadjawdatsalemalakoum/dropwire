//! Turning the names in a received transfer into safe paths under the folder
//! the receiver chose.
//!
//! Names come from the sender, so they are untrusted: they may try to climb out
//! of the destination (`../x`, `/abs`, `C:evil`), or simply be legal on the
//! sender's OS and not on ours (`Why?.pdf` from a Mac, `a:b.txt` from Linux).
//! Every segment is cleaned with Windows' rules on every OS, so a transfer saves
//! under the same names everywhere, including exFAT or SMB targets on macOS and
//! Linux.

use std::path::{Component, Path, PathBuf};

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
}

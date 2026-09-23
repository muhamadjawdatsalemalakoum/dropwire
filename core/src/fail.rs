//! Turning failures into words a person can act on.
//!
//! What reaches the screen says which step failed and why ("Could not read
//! report.pdf: it is open in another app"), with a stable [`ErrorCode`] the app
//! can branch on. The full technical chain is logged locally for diagnosis and
//! never sent anywhere.

use std::error::Error as StdError;
use std::io;

use crate::error::CoreError;
use crate::progress::ErrorCode;

/// The code and message a failed transfer ends with.
pub(crate) fn describe(e: &anyhow::Error) -> (ErrorCode, String) {
    if let Some(core) = e.downcast_ref::<CoreError>() {
        return describe_core(core);
    }
    let chain: Vec<&(dyn StdError + 'static)> = e.chain().collect();
    if let Some((depth, io)) = chain
        .iter()
        .enumerate()
        .find_map(|(i, c)| c.downcast_ref::<io::Error>().map(|io| (i, io)))
    {
        let (code, reason) = io_reason(io);
        // The outermost message names the step ("could not read x.pdf"); the
        // I/O error says why. Anything in between is internal detail.
        return if depth == 0 {
            (code, sentence(&reason))
        } else {
            (code, sentence(&format!("{e}: {reason}")))
        };
    }
    (ErrorCode::Other, sentence(&format!("{e:#}")))
}

fn describe_core(e: &CoreError) -> (ErrorCode, String) {
    match e {
        CoreError::Other(inner) => describe(inner),
        CoreError::Io(io) => {
            let (code, reason) = io_reason(io);
            (code, sentence(&reason))
        }
        other => (other.code(), sentence(&other.to_string())),
    }
}

/// A short, plain reason for an I/O failure, and its code.
pub(crate) fn io_reason(e: &io::Error) -> (ErrorCode, String) {
    if let Some((code, reason)) = e.raw_os_error().and_then(os_reason) {
        return (code, reason.to_string());
    }
    match e.kind() {
        io::ErrorKind::NotFound => (ErrorCode::NotFound, "it is no longer there".into()),
        io::ErrorKind::PermissionDenied => {
            (ErrorCode::PermissionDenied, "access was denied".into())
        }
        _ => (ErrorCode::Other, plain_os_text(&e.to_string())),
    }
}

/// The reason behind any error: its I/O cause in plain words if it has one,
/// otherwise its own text.
pub(crate) fn reason_of(e: &(dyn StdError + 'static)) -> String {
    let mut cur = Some(e);
    while let Some(err) = cur {
        if let Some(io) = err.downcast_ref::<io::Error>() {
            return io_reason(io).1;
        }
        cur = err.source();
    }
    plain_os_text(&e.to_string())
}

#[cfg(windows)]
fn os_reason(code: i32) -> Option<(ErrorCode, &'static str)> {
    Some(match code {
        // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION
        32 | 33 => (ErrorCode::FileInUse, "it is open in another app"),
        // ERROR_HANDLE_DISK_FULL, ERROR_DISK_FULL
        39 | 112 => (ErrorCode::DiskFull, "the disk is full"),
        // ERROR_WRITE_PROTECT
        19 => (ErrorCode::PermissionDenied, "the drive is write-protected"),
        // ERROR_FILENAME_EXCED_RANGE
        206 => (ErrorCode::Other, "the name or folder path is too long"),
        // ERROR_INVALID_NAME
        123 => (ErrorCode::Other, "the name is not allowed on this drive"),
        // ERROR_FILE_TOO_LARGE
        223 => (ErrorCode::Other, "the file is too large for this drive"),
        _ => return None,
    })
}

#[cfg(unix)]
fn os_reason(code: i32) -> Option<(ErrorCode, &'static str)> {
    const EFBIG: i32 = 27;
    const ENOSPC: i32 = 28;
    const EROFS: i32 = 30;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const ENAMETOOLONG: i32 = 36;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const EDQUOT: i32 = 122;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    const ENAMETOOLONG: i32 = 63;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    const EDQUOT: i32 = 69;
    Some(match code {
        ENOSPC | EDQUOT => (ErrorCode::DiskFull, "the disk is full"),
        EROFS => (ErrorCode::PermissionDenied, "the drive is read-only"),
        EFBIG => (ErrorCode::Other, "the file is too large for this drive"),
        ENAMETOOLONG => (ErrorCode::Other, "the name or folder path is too long"),
        _ => return None,
    })
}

#[cfg(not(any(windows, unix)))]
fn os_reason(_code: i32) -> Option<(ErrorCode, &'static str)> {
    None
}

/// An operating system message without the "(os error 5)" tail and the final
/// period, starting lower case so it reads well after a colon.
fn plain_os_text(text: &str) -> String {
    let text = match text.rfind(" (os error ") {
        Some(i) if text.ends_with(')') => &text[..i],
        _ => text,
    };
    let text = text.trim().trim_end_matches('.');
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        // Leave "SMB share ..." alone; lower "Access is denied".
        (Some(first), Some(second)) if first.is_uppercase() && !second.is_uppercase() => first
            .to_lowercase()
            .chain(text[first.len_utf8()..].chars())
            .collect(),
        _ => text.to_string(),
    }
}

/// Start with a capital letter, end with a period: the message is shown as a
/// sentence of its own.
pub(crate) fn sentence(text: &str) -> String {
    let text = text.trim();
    let mut chars = text.chars();
    let mut out: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => return String::new(),
    };
    if !out.ends_with(['.', '!', '?', ')']) {
        out.push('.');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn common_io_failures_read_plainly() {
        let (code, why) = io_reason(&io::Error::from(io::ErrorKind::NotFound));
        assert_eq!(
            (code, why.as_str()),
            (ErrorCode::NotFound, "it is no longer there")
        );
        let (code, why) = io_reason(&io::Error::from(io::ErrorKind::PermissionDenied));
        assert_eq!(
            (code, why.as_str()),
            (ErrorCode::PermissionDenied, "access was denied")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_codes_are_recognised() {
        let full = io_reason(&io::Error::from_raw_os_error(112));
        assert_eq!(full.0, ErrorCode::DiskFull);
        let busy = io_reason(&io::Error::from_raw_os_error(32));
        assert_eq!(
            busy,
            (ErrorCode::FileInUse, "it is open in another app".into())
        );
        let denied = io_reason(&io::Error::from_raw_os_error(5));
        assert_eq!(denied.1, "access was denied");
    }

    #[cfg(unix)]
    #[test]
    fn unix_codes_are_recognised() {
        assert_eq!(
            io_reason(&io::Error::from_raw_os_error(28)).0,
            ErrorCode::DiskFull
        );
        assert_eq!(
            io_reason(&io::Error::from_raw_os_error(30)).1,
            "the drive is read-only"
        );
    }

    #[test]
    fn os_tails_are_dropped() {
        assert_eq!(
            plain_os_text("The device is not ready. (os error 21)"),
            "the device is not ready"
        );
        assert_eq!(plain_os_text("SMB share gone"), "SMB share gone");
    }

    #[test]
    fn the_failing_step_and_its_cause_are_both_kept() {
        let e = Err::<(), _>(io::Error::from(io::ErrorKind::NotFound))
            .context("could not read report.pdf")
            .unwrap_err();
        assert_eq!(
            describe(&e),
            (
                ErrorCode::NotFound,
                "Could not read report.pdf: it is no longer there.".to_string()
            )
        );

        let e = anyhow::anyhow!("inner detail").context("the download stopped");
        assert_eq!(describe(&e).1, "The download stopped: inner detail.");
    }
}

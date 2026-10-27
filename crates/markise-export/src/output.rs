//! Cancellation-safe publication of export results: everything is written to a temporary
//! file in the destination folder and made visible with one rename, after a last
//! cancellation check, so a cancelled, failed or timed-out export never leaves a partial file
//! and never destroys an existing one.
//!
//! Cancellation is cooperative: it is checked right before the final rename, so a cancel
//! requested after that check (a few microseconds) still publishes the finished file.

use std::path::Path;

use crate::job::JobControl;

/// Writes `bytes` to `path` (replacing it) through a temporary file; refuses after cancellation.
pub fn publish(path: &Path, bytes: &[u8], control: &JobControl) -> Result<(), String> {
    publish_with(path, bytes, control, true, &|| {})
}

/// [`publish`] with the overwrite policy (`false`: fail atomically if `path` exists) and a hook
/// run between writing the temporary file and making it visible (tests).
pub fn publish_with(
    path: &Path,
    bytes: &[u8],
    control: &JobControl,
    overwrite: bool,
    before_persist: &dyn Fn(),
) -> Result<(), String> {
    control.check().map_err(|e| e.to_string())?;
    let mut tmp =
        tempfile::NamedTempFile::new_in(folder(path)).map_err(|e| format!("{}: {e}", folder(path).display()))?;
    std::io::Write::write_all(&mut tmp, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    before_persist();
    persist(tmp.into_temp_path(), path, control, overwrite)
}

/// A temporary path next to `dest` (same folder, same extension) for a tool that writes the
/// file itself; deleted when dropped unless [`persist`]ed.
pub fn temp_path_for(dest: &Path) -> Result<tempfile::TempPath, String> {
    let suffix = dest
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    tempfile::Builder::new()
        .prefix(".markise-export-")
        .suffix(&suffix)
        .tempfile_in(folder(dest))
        .map(|f| f.into_temp_path())
        .map_err(|e| format!("{}: {e}", folder(dest).display()))
}

/// Error text of a publication refused because the export was cancelled.
pub const CANCELLED: &str = "export cancelled";

/// Makes `tmp` visible as `dest` after a final cancellation check. On cancellation or a
/// rename failure the temporary file is removed explicitly; a cleanup failure is appended to
/// the error.
pub fn persist(tmp: tempfile::TempPath, dest: &Path, control: &JobControl, overwrite: bool) -> Result<(), String> {
    if control.is_cancelled() {
        return Err(with_cleanup(CANCELLED.to_string(), tmp));
    }
    let r = if overwrite {
        tmp.persist(dest)
    } else {
        tmp.persist_noclobber(dest)
    };
    match r {
        Ok(()) => Ok(()),
        Err(e) => {
            let msg = if !overwrite && e.error.kind() == std::io::ErrorKind::AlreadyExists {
                format!("{}: the file exists (not overwritten)", dest.display())
            } else {
                format!("{}: {}", dest.display(), e.error)
            };
            Err(with_cleanup(msg, e.path))
        }
    }
}

fn with_cleanup(msg: String, tmp: tempfile::TempPath) -> String {
    match discard(tmp) {
        Ok(()) => msg,
        Err(c) => format!("{msg}; {c}"),
    }
}

/// Removes an unpublished temporary file, retrying briefly (on Windows a just-terminated
/// process may still hold it); returns an error message when it cannot be removed.
pub fn discard(tmp: tempfile::TempPath) -> Result<(), String> {
    let path = match tmp.keep() {
        Ok(p) => p,
        Err(e) => return Err(format!("temporary file: {}", e.error)),
    };
    let mut delay = std::time::Duration::from_millis(20);
    for _ in 0..5 {
        match std::fs::remove_file(&path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => {
                std::thread::sleep(delay);
                delay *= 2;
            }
        }
    }
    std::fs::remove_file(&path).map_err(|e| format!("temporary file {} could not be removed: {e}", path.display()))
}

fn folder(path: &Path) -> &Path {
    path.parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_during_write_and_noclobber() {
        let d = tempfile::tempdir().unwrap();
        let c = JobControl::new();
        let target = d.path().join("y");
        let c2 = c.clone();
        assert!(publish_with(&target, b"y", &c, true, &|| c2.cancel()).is_err());
        assert!(!target.exists());
        assert_eq!(
            std::fs::read_dir(d.path()).unwrap().count(),
            0,
            "temporary file removed"
        );
        std::fs::write(&target, b"keep").unwrap();
        let e = publish_with(&target, b"new", &JobControl::new(), false, &|| {}).unwrap_err();
        assert!(e.contains("exists"), "{e}");
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        let t = temp_path_for(&d.path().join("a.docx")).unwrap();
        assert!(t.to_string_lossy().ends_with(".docx"));
        let p = t.to_path_buf();
        std::fs::write(&p, b"partial").unwrap();
        discard(t).unwrap();
        assert!(!p.exists());
    }
}

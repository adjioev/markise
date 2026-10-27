//! Diagnostics that survive without a terminal: a small persistent log with the start line,
//! panics (with a backtrace) and fatal errors, and on Windows the parent console for terminal
//! launches (release builds are GUI-subsystem executables without a console of their own).
//!
//! Privacy: entries never contain document text — panics are logged with thread name, source
//! location and backtrace but without the panic message (it may quote user data).
//!
//! Log file (`markise.log`, rotated to `markise.log.1` before it would exceed 1 MiB, so both
//! together stay under 2 MiB; each entry is capped at 16 KiB):
//! - Windows: `%LOCALAPPDATA%\Markise\logs\`
//! - macOS: `~/Library/Logs/Markise/`
//! - Linux and others: `$XDG_STATE_HOME/markise/logs/` (default `~/.local/state/markise/logs/`)
//!
//! Logging is best effort: an unwritable log folder never affects the app.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum size of `markise.log`; an entry that would exceed it rotates the file first.
const MAX_LOG_BYTES: u64 = 1 << 20;
/// Maximum size of one entry (longer entries are truncated).
const MAX_ENTRY_BYTES: usize = 16 << 10;

/// Folder of the log file for this platform, if the environment names a home folder.
pub fn log_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        env_dir("LOCALAPPDATA").map(|d| d.join("Markise").join("logs"))
    }
    #[cfg(target_os = "macos")]
    {
        env_dir("HOME").map(|h| h.join("Library").join("Logs").join("Markise"))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        env_dir("XDG_STATE_HOME")
            .or_else(|| env_dir("HOME").map(|h| h.join(".local").join("state")))
            .map(|d| d.join("markise").join("logs"))
    }
}

/// An absolute folder from an environment variable (relative or empty values are ignored).
fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute())
}

/// Appends one entry to the log (best effort).
pub fn log(entry: &str) {
    if let Some(dir) = log_dir() {
        let _ = append(&dir.join("markise.log"), entry);
    }
}

fn append(path: &Path, entry: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut line = format!("[unix {secs}] {}", truncate(entry, MAX_ENTRY_BYTES));
    line.push('\n');
    let current = fs::metadata(path).map_or(0, |m| m.len());
    if current > 0 && current + line.len() as u64 > MAX_LOG_BYTES {
        let mut old = path.as_os_str().to_owned();
        old.push(".1");
        fs::rename(path, old)?; // replaces an older markise.log.1
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(line.as_bytes())
}

/// At most `max` bytes of `s`, cut at a character boundary, marked when cut.
fn truncate(s: &str, max: usize) -> std::borrow::Cow<'_, str> {
    if s.len() <= max {
        return s.into();
    }
    const MARK: &str = " [truncated]";
    let mut end = max - MARK.len();
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{MARK}", &s[..end]).into()
}

/// Records panics in the log, then runs the previous (default) hook, which prints to stderr.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let backtrace = std::backtrace::Backtrace::force_capture();
        // the payload (message) is not persisted: it may quote document text
        let location = info.location().map_or_else(
            || "unknown location".to_string(),
            |l| format!("{}:{}:{}", l.file(), l.line(), l.column()),
        );
        log(&format!(
            "panic in thread '{}' at {location}\n{backtrace}",
            thread.name().unwrap_or("<unnamed>")
        ));
        previous(info);
    }));
}

/// Windows: when started from a terminal, write stdout/stderr to that terminal. Standard
/// handles that are already valid (redirected to a file or pipe, or a console build) are saved
/// before `AttachConsole` (which may replace them) and restored afterwards; only missing
/// stdout/stderr handles get the console.
#[cfg(windows)]
pub fn attach_parent_console() {
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        SetStdHandle,
    };

    fn missing(h: HANDLE) -> bool {
        h.is_null() || h == INVALID_HANDLE_VALUE
    }
    // SAFETY: plain Win32 calls with valid arguments; the console handle stays open for the
    // lifetime of the process, as standard handles do.
    unsafe {
        let saved: Vec<_> = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
            .into_iter()
            .map(|id| (id, GetStdHandle(id)))
            .collect();
        let wanted: Vec<_> = saved
            .iter()
            .filter(|&&(id, h)| id != STD_INPUT_HANDLE && missing(h))
            .map(|&(id, _)| id)
            .collect();
        if wanted.is_empty() {
            return;
        }
        let attached = AttachConsole(ATTACH_PARENT_PROCESS) != 0;
        let restore = || {
            for &(id, h) in &saved {
                if !missing(h) {
                    SetStdHandle(id, h);
                }
            }
        };
        restore();
        if !attached {
            return;
        }
        let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
        let console = CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        if missing(console) {
            return;
        }
        for id in wanted {
            SetStdHandle(id, console);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_rotates_before_exceeding_the_limit() {
        let dir = std::env::temp_dir().join(format!("markise-diag-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("logs").join("markise.log");
        let old = dir.join("logs").join("markise.log.1");
        append(&path, "first").unwrap();
        assert!(fs::read_to_string(&path).unwrap().ends_with("] first\n"));
        // an entry that still fits does not rotate
        fs::write(&path, vec![b'w'; MAX_LOG_BYTES as usize - 100]).unwrap();
        append(&path, "fits").unwrap();
        assert!(!old.exists());
        // one that would exceed the limit rotates first
        fs::write(&path, vec![b'x'; MAX_LOG_BYTES as usize - 10]).unwrap();
        append(&path, "second").unwrap();
        assert!(fs::read_to_string(&path).unwrap().ends_with("] second\n"));
        assert_eq!(fs::metadata(&old).unwrap().len(), MAX_LOG_BYTES - 10);
        // the rotated file is replaced on the next rotation; both stay within the limit
        fs::write(&path, vec![b'z'; MAX_LOG_BYTES as usize]).unwrap();
        append(&path, "third").unwrap();
        assert!(fs::metadata(&path).unwrap().len() <= MAX_LOG_BYTES);
        assert!(fs::metadata(&old).unwrap().len() <= MAX_LOG_BYTES);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn entries_are_capped() {
        let long = "é".repeat(MAX_ENTRY_BYTES);
        let cut = truncate(&long, MAX_ENTRY_BYTES);
        assert!(cut.len() <= MAX_ENTRY_BYTES);
        assert!(cut.ends_with(" [truncated]"));
        assert_eq!(truncate("short", MAX_ENTRY_BYTES), "short");
    }

    #[test]
    fn log_dir_is_absolute_when_known() {
        if let Some(d) = log_dir() {
            assert!(d.is_absolute());
        }
    }
}

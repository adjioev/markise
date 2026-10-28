//! File and folder operations of the sidebar file tree and the File menu (spec 04 §2, §4.1):
//! new file / new folder with a free "Untitled" name, rename that refuses to replace an
//! existing entry, and move to the Trash. All of them are small, synchronous file system
//! calls made in response to a user action.

use std::io;
use std::path::{Path, PathBuf};

/// Extension of new documents (spec 05: default extension `md`).
pub const DEFAULT_EXTENSION: &str = "md";

/// Base name of new documents and folders.
pub const UNTITLED: &str = "Untitled";

/// The first free name in `dir` of the series `Untitled.md`, `Untitled 2.md`, … (`ext`
/// `None`: folders, `Untitled`, `Untitled 2`, …). Existence is checked case-insensitively on
/// case-insensitive file systems by the OS itself.
pub fn free_name(dir: &Path, stem: &str, ext: Option<&str>) -> PathBuf {
    (1..)
        .map(|i| {
            let base = if i == 1 {
                stem.to_string()
            } else {
                format!("{stem} {i}")
            };
            let name = match ext {
                Some(e) => format!("{base}.{e}"),
                None => base,
            };
            dir.join(name)
        })
        .find(|p| std::fs::symlink_metadata(p).is_err())
        .expect("an unbounded series has a free name")
}

/// Creates an empty `Untitled….md` in `dir` (never replacing an existing file) and returns
/// its path.
pub fn create_file(dir: &Path) -> io::Result<PathBuf> {
    for _ in 0..16 {
        let p = free_name(dir, UNTITLED, Some(DEFAULT_EXTENSION));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(_) => return Ok(p),
            // created by someone else in the meantime: take the next name
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("no free file name"))
}

/// Creates an empty `Untitled…` folder in `dir` and returns its path.
pub fn create_folder(dir: &Path) -> io::Result<PathBuf> {
    for _ in 0..16 {
        let p = free_name(dir, UNTITLED, None);
        match std::fs::create_dir(&p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("no free folder name"))
}

/// Why a new name is not acceptable (`None`: it is): empty, `.`/`..`, a path separator or
/// a character Windows forbids in file names.
pub fn invalid_name(name: &str) -> Option<&'static str> {
    let t = name.trim();
    if t.is_empty() {
        return Some("The name is empty.");
    }
    if t == "." || t == ".." {
        return Some("This name is reserved.");
    }
    if t != name {
        return Some("The name starts or ends with a space.");
    }
    if name
        .chars()
        .any(|c| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control())
    {
        return Some("The name contains a character that is not allowed in file names (/ \\ : * ? \" < > |).");
    }
    None
}

/// Renames `path` to `new_name` in the same folder and returns the new path. Refuses an
/// invalid name and never replaces an existing entry: the rename itself is atomic
/// no-replace where the OS offers it ([`rename_noreplace`]). A change of letter case only
/// of the same entry (case-insensitive file systems) is allowed.
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf, String> {
    if let Some(why) = invalid_name(new_name) {
        return Err(why.into());
    }
    let parent = path.parent().ok_or("This item cannot be renamed.")?;
    let target = parent.join(new_name);
    if target == path {
        return Ok(target);
    }
    let exists = || format!("“{new_name}” already exists in this folder.");
    if case_only_change(path, &target) && same_entry(path, &target) {
        // the target name finds this very entry: a no-replace rename would refuse it
        std::fs::rename(path, &target).map_err(|e| format!("Cannot rename: {e}"))?;
        return Ok(target);
    }
    match rename_noreplace(path, &target) {
        Ok(()) => Ok(target),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(exists()),
        Err(e) => Err(format!("Cannot rename: {e}")),
    }
}

/// The two paths differ only in the letter case of their last component.
fn case_only_change(a: &Path, b: &Path) -> bool {
    a.parent() == b.parent()
        && match (a.file_name(), b.file_name()) {
            (Some(x), Some(y)) => x != y && x.to_string_lossy().to_lowercase() == y.to_string_lossy().to_lowercase(),
            _ => false,
        }
}

/// `a` and `b` are the same directory entry, compared without following symbolic links
/// (a link and the file it points to are different entries).
#[cfg(unix)]
fn same_entry(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) {
        (Ok(x), Ok(y)) => (x.dev(), x.ino()) == (y.dev(), y.ino()),
        _ => false,
    }
}

/// Windows: the same volume serial number and file index, read from handles opened on the
/// entries themselves (reparse points not followed). When the identity cannot be read the
/// entries are treated as different, so the rename stays no-replace.
#[cfg(windows)]
fn same_entry(a: &Path, b: &Path) -> bool {
    matches!((file_identity(a), file_identity(b)), (Some(x), Some(y)) if x == y)
}

#[cfg(windows)]
fn file_identity(p: &Path) -> Option<(u32, u32, u32)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_EXISTING,
    };
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: a valid NUL-terminated UTF-16 path; no access rights requested (metadata only)
    let h = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return None;
    }
    // SAFETY: `h` is an open handle; the struct is plain data filled by the call
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(h, &mut info) } != 0;
    // SAFETY: closing the handle opened above exactly once
    unsafe { CloseHandle(h) };
    ok.then_some((info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow))
}

/// Other platforms: no reliable identity — never treated as the same entry (a case-only
/// rename then goes through the no-replace path).
#[cfg(not(any(unix, windows)))]
fn same_entry(_: &Path, _: &Path) -> bool {
    false
}

/// Renames `from` to `to`, failing with `AlreadyExists` instead of replacing an existing
/// entry — atomically (macOS `renamex_np(RENAME_EXCL)`, Linux `renameat2(RENAME_NOREPLACE)`,
/// Windows `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING`). Where the call is not
/// supported (old kernels, some file systems), files fall back to hard link + unlink (also
/// no-replace); folders to a checked rename (the only remaining race window).
pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    match native_noreplace(from, to) {
        Some(Err(e)) if unsupported(&e) => fallback_noreplace(from, to),
        Some(r) => r,
        None => fallback_noreplace(from, to),
    }
}

#[cfg(unix)]
fn cstr(p: &Path) -> io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
}

#[cfg(target_os = "macos")]
fn native_noreplace(from: &Path, to: &Path) -> Option<io::Result<()>> {
    let r = (|| {
        let (f, t) = (cstr(from)?, cstr(to)?);
        // SAFETY: two valid NUL-terminated paths; the call does not keep the pointers
        if unsafe { libc::renamex_np(f.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    })();
    Some(r)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn native_noreplace(from: &Path, to: &Path) -> Option<io::Result<()>> {
    let r = (|| {
        let (f, t) = (cstr(from)?, cstr(to)?);
        // SAFETY: two valid NUL-terminated paths relative to the working directory
        let rc = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                f.as_ptr(),
                libc::AT_FDCWD,
                t.as_ptr(),
                libc::RENAME_NOREPLACE as _,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    })();
    Some(r)
}

#[cfg(windows)]
fn native_noreplace(from: &Path, to: &Path) -> Option<io::Result<()>> {
    use std::os::windows::ffi::OsStrExt;
    let wide = |p: &Path| p.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let (f, t) = (wide(from), wide(to));
    // SAFETY: two valid NUL-terminated UTF-16 paths; no MOVEFILE_REPLACE_EXISTING flag, so an
    // existing target makes the call fail
    let ok = unsafe { windows_sys::Win32::Storage::FileSystem::MoveFileExW(f.as_ptr(), t.as_ptr(), 0) };
    Some(if ok != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android", windows)))]
fn native_noreplace(_: &Path, _: &Path) -> Option<io::Result<()>> {
    None
}

/// The no-replace call is not available here (the error is not about the paths).
fn unsupported(e: &io::Error) -> bool {
    #[cfg(unix)]
    {
        matches!(e.raw_os_error(), Some(libc::EINVAL | libc::ENOSYS | libc::ENOTSUP))
    }
    #[cfg(not(unix))]
    {
        e.kind() == io::ErrorKind::Unsupported
    }
}

fn fallback_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.is_file() {
        // a hard link is never created over an existing entry
        std::fs::hard_link(from, to)?;
        return std::fs::remove_file(from).inspect_err(|_| {
            let _ = std::fs::remove_file(to);
        });
    }
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(io::Error::from(io::ErrorKind::AlreadyExists));
    }
    std::fs::rename(from, to)
}

/// Moves `path` (file or folder) to the Trash / Recycle Bin.
pub fn move_to_trash(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let ctx = {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        let mut c = trash::TrashContext::default();
        // NSFileManager: no Finder automation permission prompt (the Finder method would add
        // "Put Back" support at the cost of an Apple Events permission request)
        c.set_delete_method(DeleteMethod::NsFileManager);
        c
    };
    #[cfg(not(target_os = "macos"))]
    let ctx = trash::TrashContext::default();
    ctx.delete(path).map_err(|e| format!("Cannot move to the Trash: {e}"))
}

/// `path` with its prefix `from` replaced by `to` (a document inside a renamed folder).
pub fn rebase(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    path.strip_prefix(from).ok().map(|rest| {
        if rest.as_os_str().is_empty() {
            to.to_path_buf()
        } else {
            to.join(rest)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("markise-fsops-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::canonicalize(&d).unwrap()
    }

    #[test]
    fn new_files_and_folders_take_the_next_free_untitled_name() {
        let d = dir("names");
        assert_eq!(create_file(&d).unwrap(), d.join("Untitled.md"));
        assert_eq!(create_file(&d).unwrap(), d.join("Untitled 2.md"));
        std::fs::write(d.join("Untitled 3.md"), "keep").unwrap();
        assert_eq!(create_file(&d).unwrap(), d.join("Untitled 4.md"));
        assert_eq!(std::fs::read_to_string(d.join("Untitled 3.md")).unwrap(), "keep");
        assert_eq!(create_folder(&d).unwrap(), d.join("Untitled"));
        assert_eq!(create_folder(&d).unwrap(), d.join("Untitled 2"));
        assert!(d.join("Untitled 2").is_dir() && d.join("Untitled 2.md").is_file());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rename_refuses_collisions_and_bad_names() {
        let d = dir("rename");
        let a = d.join("a.md");
        std::fs::write(&a, "A").unwrap();
        std::fs::write(d.join("b.md"), "B").unwrap();
        assert!(rename(&a, "b.md").unwrap_err().contains("already exists"));
        assert_eq!(std::fs::read_to_string(d.join("b.md")).unwrap(), "B");
        for bad in ["", " ", "..", "x/y.md", "a:b.md", " lead.md"] {
            assert!(rename(&a, bad).is_err(), "{bad:?}");
        }
        let c = rename(&a, "c.md").unwrap();
        assert_eq!(c, d.join("c.md"));
        assert!(!a.exists() && std::fs::read_to_string(&c).unwrap() == "A");
        // letter case only: allowed on every file system
        let upper = rename(&c, "C.md").unwrap();
        assert_eq!(std::fs::read_to_string(&upper).unwrap(), "A");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Whether names in `d` are compared ignoring case.
    fn case_insensitive(d: &Path) -> bool {
        let probe = d.join("case-probe");
        std::fs::write(&probe, "").unwrap();
        let r = d.join("CASE-PROBE").exists();
        std::fs::remove_file(&probe).unwrap();
        r
    }

    #[cfg(unix)]
    #[test]
    fn case_only_rename_never_replaces_a_distinct_entry() {
        let d = dir("case-distinct");
        if case_insensitive(&d) {
            // a.md and A.md cannot both exist here; the same-entry case is covered elsewhere
            let _ = std::fs::remove_dir_all(&d);
            return;
        }
        std::fs::write(d.join("a.md"), "lower").unwrap();
        std::fs::write(d.join("A.md"), "upper").unwrap();
        assert!(rename(&d.join("a.md"), "A.md").unwrap_err().contains("already exists"));
        assert_eq!(std::fs::read_to_string(d.join("A.md")).unwrap(), "upper");
        assert_eq!(std::fs::read_to_string(d.join("a.md")).unwrap(), "lower");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn renaming_a_symlink_onto_its_target_is_a_collision() {
        let d = dir("symlink");
        let target = d.join("real.md");
        std::fs::write(&target, "precious").unwrap();
        let link = d.join("link.md");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(rename(&link, "real.md").unwrap_err().contains("already exists"));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "precious");
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        // also onto another spelling of the target's name — a collision only where the file
        // system ignores case (macOS default); on a case-sensitive one (Linux CI) "REAL.md"
        // is a free, different name and the rename may proceed
        if case_insensitive(&d) {
            assert!(rename(&link, "REAL.md").unwrap_err().contains("already exists"));
            assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        }
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "precious");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn noreplace_rename_and_its_fallback_never_replace() {
        let d = dir("noreplace");
        let (a, b) = (d.join("a.md"), d.join("b.md"));
        std::fs::write(&a, "A").unwrap();
        std::fs::write(&b, "B").unwrap();
        assert_eq!(
            rename_noreplace(&a, &b).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            fallback_noreplace(&a, &b).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "B");
        // folders onto folders too
        std::fs::create_dir(d.join("x")).unwrap();
        std::fs::create_dir(d.join("y")).unwrap();
        std::fs::write(d.join("y").join("keep.md"), "k").unwrap();
        assert!(rename_noreplace(&d.join("x"), &d.join("y")).is_err());
        assert!(fallback_noreplace(&d.join("x"), &d.join("y")).is_err());
        assert!(d.join("y").join("keep.md").is_file());
        // and they do rename to a free name
        fallback_noreplace(&a, &d.join("c.md")).unwrap();
        assert!(!a.exists() && std::fs::read_to_string(d.join("c.md")).unwrap() == "A");
        rename_noreplace(&d.join("c.md"), &a).unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "A");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rebase_moves_paths_inside_a_renamed_folder() {
        let (from, to) = (Path::new("/r/old"), Path::new("/r/new"));
        assert_eq!(
            rebase(Path::new("/r/old/x/y.md"), from, to),
            Some(PathBuf::from("/r/new/x/y.md"))
        );
        assert_eq!(rebase(from, from, to), Some(to.to_path_buf()));
        assert_eq!(rebase(Path::new("/r/older/y.md"), from, to), None);
    }
}

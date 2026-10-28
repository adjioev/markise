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
/// invalid name and never replaces an existing entry (a change of letter case only is
/// allowed: on a case-insensitive file system that is the same entry).
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf, String> {
    if let Some(why) = invalid_name(new_name) {
        return Err(why.into());
    }
    let parent = path.parent().ok_or("This item cannot be renamed.")?;
    let target = parent.join(new_name);
    if target == path {
        return Ok(target);
    }
    if std::fs::symlink_metadata(&target).is_ok() && !same_entry(path, &target) {
        return Err(format!("“{new_name}” already exists in this folder."));
    }
    std::fs::rename(path, &target).map_err(|e| format!("Cannot rename: {e}"))?;
    Ok(target)
}

/// `a` and `b` name the same file system entry (e.g. differ in letter case on a
/// case-insensitive file system).
fn same_entry(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => {
            // canonicalize keeps the spelling it was given for the last component on some
            // systems: compare the parents and the names case-insensitively
            x.parent() == y.parent()
                && x.file_name().map(|n| n.to_string_lossy().to_lowercase())
                    == y.file_name().map(|n| n.to_string_lossy().to_lowercase())
                && file_id(a) == file_id(b)
        }
        _ => false,
    }
}

#[cfg(unix)]
fn file_id(p: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p).ok().map(|m| (m.dev(), m.ino()))
}

#[cfg(not(unix))]
fn file_id(p: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(p).ok()
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

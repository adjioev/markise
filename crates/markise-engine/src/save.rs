//! Saving with conflict detection and a recoverable replacement.
//!
//! Protocol of [`save_checked`] (no file system offers a portable compare-and-swap, so this
//! is a best-effort protocol with an honest guarantee, not an atomic CAS):
//! 1. check that the file still holds the expected version (early conflict, nothing written);
//! 2. write the new content to a uniquely named temporary file created exclusively in the
//!    target directory (never following an existing entry) and fsync it;
//! 3. check the expected version again (catches writers active during step 2);
//! 4. keep the current file recoverable: hard-link it (or copy it, where links are not
//!    supported) to a unique `.<name>.markise-backup-…` entry;
//! 5. rename the temporary file over the target and fsync the directory;
//! 6. verify the backup still holds the expected version: if it does, the backup is deleted;
//!    if not, another program wrote the file between steps 3 and 5 — the save succeeded, but
//!    the backup with the other program's content is kept and reported
//!    ([`SaveOutcome::kept_backup`]).
//!
//! Guarantee: the replaced content is never silently lost, except for a writer that replaces
//! the file by rename in the instant between steps 4 and 5 (that version is overwritten).
use std::hash::Hasher;
use std::io::Write;
use std::path::Path;
use std::time::SystemTime;

/// Identity of a file's content on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskVersion {
    pub len: u64,
    pub mtime: Option<SystemTime>,
    /// Hash of the bytes (stable within a process).
    pub hash: u64,
}

impl DiskVersion {
    pub fn of_bytes(bytes: &[u8], mtime: Option<SystemTime>) -> Self {
        DiskVersion {
            len: bytes.len() as u64,
            mtime,
            hash: content_hash(bytes),
        }
    }

    /// Same content (mtime ignored: touching a file is not a change).
    pub fn same_content(&self, other: &DiskVersion) -> bool {
        self.len == other.len && self.hash == other.hash
    }
}

pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write(bytes);
    h.finish()
}

/// Current version of `path`, `Ok(None)` if it does not exist.
pub fn disk_version(path: &Path) -> std::io::Result<Option<DiskVersion>> {
    match std::fs::read(path) {
        Ok(b) => {
            let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            Ok(Some(DiskVersion::of_bytes(&b, mtime)))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveError {
    /// The file on disk is not the expected version (`found`: what is there now).
    Conflict {
        found: Option<DiskVersion>,
    },
    Io(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Conflict { .. } => write!(f, "the file was changed on disk by another program"),
            SaveError::Io(e) => write!(f, "{e}"),
        }
    }
}

/// What the file on disk must be for the save to proceed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    /// This exact content (the version the document was loaded from / last saved as, or the
    /// version the user saw in a conflict and chose to overwrite).
    Version(DiskVersion),
    /// The file must not exist (it was deleted; spec C18 writes it again).
    Missing,
    /// Either the given version or no file (deleted after loading).
    VersionOrMissing(DiskVersion),
}

impl Expect {
    fn accepts(&self, found: &Option<DiskVersion>) -> bool {
        match (self, found) {
            (Expect::Version(v) | Expect::VersionOrMissing(v), Some(f)) => v.same_content(f),
            (Expect::Missing | Expect::VersionOrMissing(_), None) => true,
            _ => false,
        }
    }
}

/// Result of a successful save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveOutcome {
    pub version: DiskVersion,
    /// Another program changed the file during the save; its content was kept here.
    pub kept_backup: Option<std::path::PathBuf>,
}

/// Test hooks run between the protocol steps (simulate concurrent writers).
#[derive(Default)]
pub struct SaveHooks {
    /// After the temporary file is written and fsynced (before the re-check).
    pub after_temp: Option<Box<dyn Fn() + Send + Sync>>,
    /// After the backup link is made (before the rename).
    pub after_backup: Option<Box<dyn Fn() + Send + Sync>>,
}

fn io(path: &Path, e: std::io::Error) -> SaveError {
    SaveError::Io(format!("{}: {e}", path.display()))
}

/// Writes `text` to `path` if the file on disk matches `expect` (see the module docs).
pub fn save_checked(path: &Path, text: &str, expect: Expect) -> Result<SaveOutcome, SaveError> {
    save_checked_with(path, text, expect, &SaveHooks::default())
}

pub fn save_checked_with(path: &Path, text: &str, expect: Expect, hooks: &SaveHooks) -> Result<SaveOutcome, SaveError> {
    // 1. early check
    let found = disk_version(path).map_err(|e| io(path, e))?;
    if !expect.accepts(&found) {
        return Err(SaveError::Conflict { found });
    }
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("document");
    // 2. temporary file (exclusive, random name), fsynced
    let mut tmp = tempfile::Builder::new()
        .prefix(&format!(".{name}."))
        .suffix(".markise-tmp")
        .tempfile_in(dir)
        .map_err(|e| io(path, e))?;
    tmp.write_all(text.as_bytes()).map_err(|e| io(path, e))?;
    tmp.as_file().sync_all().map_err(|e| io(path, e))?;
    if let Some(h) = &hooks.after_temp {
        h();
    }
    // 3. re-check after the (possibly slow) write
    let found = disk_version(path).map_err(|e| io(path, e))?;
    if !expect.accepts(&found) {
        return Err(SaveError::Conflict { found });
    }
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(tmp.path(), meta.permissions());
    }
    // 4. keep the current file recoverable until the replacement is verified
    let backup = if found.is_some() {
        let b = tempfile::Builder::new()
            .prefix(&format!(".{name}.markise-backup-"))
            .make_in(dir, |p| {
                std::fs::hard_link(path, p).or_else(|_| std::fs::copy(path, p).map(|_| ()))
            })
            .map_err(|e| io(path, e))?;
        Some(b.into_temp_path())
    } else {
        None
    };
    if let Some(h) = &hooks.after_backup {
        h();
    }
    // 5. replace
    tmp.persist(path).map_err(|e| io(path, e.error))?;
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    // 6. verify what was replaced
    let mut kept_backup = None;
    if let Some(b) = backup {
        let replaced = disk_version(&b).ok().flatten();
        if !expect.accepts(&replaced) {
            kept_backup = Some(b.keep().map_err(|e| io(path, e.error))?);
        } // else: dropped → deleted
    }
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    Ok(SaveOutcome {
        version: DiskVersion::of_bytes(text.as_bytes(), mtime),
        kept_backup,
    })
}

/// Unconditional atomic write (no version check).
pub fn save_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("document");
    let mut tmp = tempfile::Builder::new()
        .prefix(&format!(".{name}."))
        .suffix(".markise-tmp")
        .tempfile_in(dir)?;
    tmp.write_all(text.as_bytes())?;
    tmp.as_file().sync_all()?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(tmp.path(), meta.permissions());
    }
    tmp.persist(path).map_err(|e| e.error)?;
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("markise-engine-save-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn replaces_content_and_leaves_no_temp_file() {
        let d = dir("basic");
        let p = d.join("doc.md");
        std::fs::write(&p, "old").unwrap();
        save_atomic(&p, "new\r\ntext").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new\r\ntext");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
        assert!(save_atomic(&d.join("no/such/dir/x.md"), "x").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn planted_symlinks_at_old_temp_names_are_never_followed() {
        let d = dir("symlink");
        let p = d.join("doc.md");
        let victim = d.join("victim.txt");
        std::fs::write(&p, "original").unwrap();
        std::fs::write(&victim, "precious").unwrap();
        // the M1 checkpoint used a predictable `.doc.md.markise-<pid>.tmp`
        let old_name = d.join(format!(".doc.md.markise-{}.tmp", std::process::id()));
        std::os::unix::fs::symlink(&victim, &old_name).unwrap();
        std::os::unix::fs::symlink(&p, d.join(".doc.md.markise-tmp")).unwrap();
        save_atomic(&p, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "precious");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn conflicting_disk_content_blocks_the_save() {
        let d = dir("conflict");
        let p = d.join("doc.md");
        std::fs::write(&p, "v1").unwrap();
        let v1 = disk_version(&p).unwrap().unwrap();
        std::fs::write(&p, "external").unwrap();
        let err = save_checked(&p, "mine", Expect::Version(v1)).unwrap_err();
        let SaveError::Conflict { found: Some(found) } = err else {
            panic!("{err:?}")
        };
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "external", "nothing written");
        // explicit overwrite of exactly the observed version
        let v2 = save_checked(&p, "mine", Expect::Version(found)).unwrap().version;
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "mine");
        assert!(v2.same_content(&disk_version(&p).unwrap().unwrap()));
        // deleted file: Missing / VersionOrMissing write it again, Version refuses
        std::fs::remove_file(&p).unwrap();
        assert!(matches!(
            save_checked(&p, "x", Expect::Version(v2)),
            Err(SaveError::Conflict { found: None })
        ));
        save_checked(&p, "again", Expect::VersionOrMissing(v2)).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "again");
        let _ = std::fs::remove_dir_all(&d);
    }

    fn entries(d: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn writer_during_the_temp_write_is_a_conflict() {
        // second review, item 1: a concurrent write after the first check
        let d = dir("race1");
        let p = d.join("doc.md");
        std::fs::write(&p, "v1").unwrap();
        let v1 = disk_version(&p).unwrap().unwrap();
        let p2 = p.clone();
        let hooks = SaveHooks {
            after_temp: Some(Box::new(move || std::fs::write(&p2, "external").unwrap())),
            ..Default::default()
        };
        let r = save_checked_with(&p, "mine", Expect::Version(v1), &hooks);
        assert!(matches!(r, Err(SaveError::Conflict { found: Some(_) })), "{r:?}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "external");
        assert_eq!(entries(&d), vec!["doc.md"], "temporary file removed");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn writer_in_the_last_instant_is_kept_in_a_backup() {
        let d = dir("race2");
        let p = d.join("doc.md");
        std::fs::write(&p, "v1").unwrap();
        let v1 = disk_version(&p).unwrap().unwrap();
        let p2 = p.clone();
        // an in-place write after the backup link: lands in the linked inode
        let hooks = SaveHooks {
            after_backup: Some(Box::new(move || std::fs::write(&p2, "late external").unwrap())),
            ..Default::default()
        };
        let out = save_checked_with(&p, "mine", Expect::Version(v1), &hooks).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "mine");
        let b = out.kept_backup.expect("backup kept");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "late external");
        // the normal path leaves no backup behind
        let out = save_checked(&p, "mine 2", Expect::Version(out.version)).unwrap();
        assert!(out.kept_backup.is_none());
        assert_eq!(entries(&d).len(), 2, "doc + the one kept backup: {:?}", entries(&d));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn io_failure_reports_and_keeps_the_original() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("ro");
        let p = d.join("doc.md");
        std::fs::write(&p, "original").unwrap();
        let v = disk_version(&p).unwrap().unwrap();
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o555)).unwrap();
        let r = save_checked(&p, "new", Expect::Version(v));
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(r, Err(SaveError::Io(_))), "{r:?}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "original");
        let _ = std::fs::remove_dir_all(&d);
    }
}

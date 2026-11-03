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
//! 5. rename the temporary file over the target and fsync the directory (on Windows, a
//!    rename refused because another process has the target open — antivirus, indexer — is
//!    retried with a short backoff for up to about a second, re-checking the expected
//!    version before each retry; issue #65);
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
use std::time::{Duration, SystemTime};

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

    /// The content identity of these bytes as a text: equal to [`text_identity`] of the text
    /// they decode to (when they are valid UTF-8). Lets the UI take a saved or loaded text's
    /// identity from the worker that wrote or read it instead of hashing the text again.
    pub fn text_identity(&self) -> TextIdentity {
        (self.len as usize, self.hash)
    }
}

/// Content identity of a text: its length in bytes and [`content_hash`] of its bytes.
pub type TextIdentity = (usize, u64);

/// Content identity of `text`, the same as [`DiskVersion::text_identity`] of its bytes.
pub fn text_identity(text: &str) -> TextIdentity {
    (text.len(), content_hash(text.as_bytes()))
}

/// Hash of `bytes`, stable within a process (not across processes or builds: never persist
/// it). aHash with fixed keys: about 10× faster than SipHash on large inputs (a 100 MiB file
/// is hashed in ~8 ms), which matters because loads, saves and the file watcher hash whole
/// files (issue #29). File contents are not adversarial input to this comparison.
///
/// The bytes go through [`PieceHasher`]: aHash's result depends on how input is split
/// across `write` calls, so every path (in-memory text, streamed file) feeds the same
/// fixed [`HASH_CHUNK`] pieces and agrees for the same bytes however they were read
/// (issue #29, part 4: [`disk_version`] streams the file instead of copying it).
pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut h = PieceHasher::default();
    h.feed(bytes);
    h.finish()
}

/// Piece size of [`content_hash`].
const HASH_CHUNK: usize = 1 << 20;

/// Feeds aHash whole [`HASH_CHUNK`] pieces plus one final partial piece (none for empty
/// input), independent of the boundaries of the [`PieceHasher::feed`] calls.
#[derive(Default)]
struct PieceHasher {
    h: ahash::AHasher,
    /// Start of the current piece (< `HASH_CHUNK` bytes).
    part: Vec<u8>,
}

impl PieceHasher {
    fn feed(&mut self, mut data: &[u8]) {
        if !self.part.is_empty() {
            let take = (HASH_CHUNK - self.part.len()).min(data.len());
            self.part.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.part.len() < HASH_CHUNK {
                return;
            }
            self.h.write(&self.part);
            self.part.clear();
        }
        while data.len() >= HASH_CHUNK {
            self.h.write(&data[..HASH_CHUNK]);
            data = &data[HASH_CHUNK..];
        }
        self.part.extend_from_slice(data);
    }

    fn finish(mut self) -> u64 {
        if !self.part.is_empty() {
            self.h.write(&self.part);
        }
        self.h.finish()
    }
}

/// Current version of `path`, `Ok(None)` if it does not exist. Streams the file through a
/// [`HASH_CHUNK`] buffer, never holding all of it.
pub fn disk_version(path: &Path) -> std::io::Result<Option<DiskVersion>> {
    use std::io::Read;
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut h = PieceHasher::default();
    let mut buf = vec![0u8; HASH_CHUNK];
    let mut len = 0u64;
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                h.feed(&buf[..n]);
                len += n as u64;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    Ok(Some(DiskVersion {
        len,
        mtime,
        hash: h.finish(),
    }))
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
    // 5. replace (retrying a transiently refused rename; nothing is replaced while another
    // program changed the file in the meantime — the backup is still the expected version)
    persist_retrying(
        tmp,
        path,
        |pause| {
            std::thread::sleep(pause);
            let found = disk_version(path).map_err(|e| io(path, e))?;
            if expect.accepts(&found) {
                Ok(())
            } else {
                Err(SaveError::Conflict { found })
            }
        },
        |e| io(path, e),
    )?;
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
    persist_replacing(tmp, path)?;
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Renames `tmp` over `path`, retrying the transient Windows sharing conflicts of
/// [`rename_error_is_transient`] for about a second (issue #65; issue #147: the settings file is
/// replaced the same way, and a reader or virus scanner holding it open made a write fail).
pub(crate) fn persist_replacing(tmp: tempfile::NamedTempFile, path: &Path) -> std::io::Result<()> {
    persist_retrying(
        tmp,
        path,
        |pause| {
            std::thread::sleep(pause);
            Ok(())
        },
        |e| e,
    )
}

/// Pauses before each retry of the final rename (issue #65): about one second in total.
const RENAME_RETRY_PAUSES: [Duration; 7] = [
    Duration::from_millis(10),
    Duration::from_millis(20),
    Duration::from_millis(40),
    Duration::from_millis(80),
    Duration::from_millis(160),
    Duration::from_millis(250),
    Duration::from_millis(400),
];

/// Windows error codes of a rename refused because another process has the target open
/// without sharing deletion (antivirus, search indexer, backup tools): `ERROR_ACCESS_DENIED`
/// and `ERROR_SHARING_VIOLATION`. They usually clear within milliseconds.
fn is_sharing_conflict_code(code: Option<i32>) -> bool {
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    matches!(code, Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION))
}

/// The final rename failed in a way worth retrying: only on Windows, where another process
/// holding the target open blocks a replacing rename (elsewhere the codes mean other things
/// and a rename over an open file succeeds).
fn rename_error_is_transient(e: &std::io::Error) -> bool {
    cfg!(windows) && is_sharing_conflict_code(e.raw_os_error())
}

/// Renames `tmp` over `path`, retrying transient failures ([`rename_error_is_transient`])
/// after the pauses of [`RENAME_RETRY_PAUSES`]. `between` waits out a pause and may stop the
/// retries (a version re-check); `fail` maps the final I/O error.
fn persist_retrying<E>(
    tmp: tempfile::NamedTempFile,
    path: &Path,
    between: impl FnMut(Duration) -> Result<(), E>,
    fail: impl Fn(std::io::Error) -> E,
) -> Result<(), E> {
    retry_transient(
        tmp,
        |t| t.persist(path).map(|_| ()).map_err(|e| (e.error, e.file)),
        rename_error_is_transient,
        &RENAME_RETRY_PAUSES,
        between,
        fail,
    )
}

/// Runs `attempt` until it succeeds, fails with an error `transient` rejects, or the pauses
/// run out. A failed attempt hands its state back for the next one; `between` is called with
/// each pause before a retry and may stop the loop with its own error.
fn retry_transient<S, T, E>(
    mut state: S,
    mut attempt: impl FnMut(S) -> Result<T, (std::io::Error, S)>,
    transient: impl Fn(&std::io::Error) -> bool,
    pauses: &[Duration],
    mut between: impl FnMut(Duration) -> Result<(), E>,
    fail: impl Fn(std::io::Error) -> E,
) -> Result<T, E> {
    let mut pauses = pauses.iter();
    loop {
        match attempt(state) {
            Ok(t) => return Ok(t),
            Err((e, s)) => {
                let Some(pause) = pauses.next().filter(|_| transient(&e)) else {
                    return Err(fail(e));
                };
                between(*pause)?;
                state = s;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamed_disk_version_matches_the_in_memory_hash() {
        // issue #29, part 4: the file is hashed through a buffer; the result must equal
        // `content_hash` / `text_identity` of the same bytes in memory (the UI compares them)
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f.md");
        let sizes = [
            0,
            1,
            1000,
            HASH_CHUNK - 1,
            HASH_CHUNK,
            HASH_CHUNK + 1,
            3 * HASH_CHUNK + 12_345,
        ];
        for n in sizes {
            let bytes: Vec<u8> = (0..n).map(|i| b'a' + (i * 7 % 26) as u8).collect();
            std::fs::write(&p, &bytes).unwrap();
            let v = disk_version(&p).unwrap().unwrap();
            assert_eq!(v.hash, content_hash(&bytes), "{n} bytes");
            assert_eq!(v.len, n as u64);
            let text = std::str::from_utf8(&bytes).unwrap();
            assert_eq!(v.text_identity(), text_identity(text), "{n} bytes");
            // any feed boundaries give the same hash
            for step in [1usize << 12, 7_777, HASH_CHUNK + 3] {
                let mut h = PieceHasher::default();
                bytes.chunks(step).for_each(|c| h.feed(c));
                assert_eq!(h.finish(), v.hash, "{n} bytes fed in {step}");
            }
        }
        assert_eq!(disk_version(&d.path().join("missing")).unwrap(), None);
    }

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

    #[test]
    fn rename_retry_classification_65() {
        assert!(is_sharing_conflict_code(Some(5)), "ERROR_ACCESS_DENIED");
        assert!(is_sharing_conflict_code(Some(32)), "ERROR_SHARING_VIOLATION");
        for other in [None, Some(0), Some(2), Some(3), Some(33), Some(112)] {
            assert!(!is_sharing_conflict_code(other), "{other:?}");
        }
        let denied = std::io::Error::from_raw_os_error(5);
        assert_eq!(rename_error_is_transient(&denied), cfg!(windows), "Windows only");
        let total: Duration = RENAME_RETRY_PAUSES.iter().sum();
        assert!(
            total <= Duration::from_millis(1000),
            "bounded by about a second: {total:?}"
        );
        assert!(
            total >= Duration::from_millis(500),
            "long enough to outlast a scan: {total:?}"
        );
        assert!(RENAME_RETRY_PAUSES.windows(2).all(|w| w[0] <= w[1]), "backoff grows");
    }

    #[test]
    fn retry_transient_retries_only_transient_errors_within_the_pauses_65() {
        let sharing = |n: &u32| (std::io::Error::from_raw_os_error(32), *n);
        let transient = |e: &std::io::Error| is_sharing_conflict_code(e.raw_os_error());
        let run = |fails: u32, err: &dyn Fn(&u32) -> (std::io::Error, u32)| {
            let mut slept = Vec::new();
            let r = retry_transient(
                0u32,
                |n| if n < fails { Err(err(&(n + 1))) } else { Ok(n) },
                transient,
                &RENAME_RETRY_PAUSES,
                |d| {
                    slept.push(d);
                    Ok::<(), std::io::Error>(())
                },
                |e| e,
            );
            (r, slept)
        };
        // succeeds on the third attempt after two pauses
        let (r, slept) = run(2, &sharing);
        assert_eq!(r.unwrap(), 2);
        assert_eq!(slept, RENAME_RETRY_PAUSES[..2]);
        // a lasting refusal gives up after all pauses (one attempt more than pauses)
        let (r, slept) = run(u32::MAX, &sharing);
        assert_eq!(r.unwrap_err().raw_os_error(), Some(32));
        assert_eq!(slept, RENAME_RETRY_PAUSES);
        // other errors are reported at once
        let (r, slept) = run(u32::MAX, &|n| (std::io::Error::from_raw_os_error(2), *n));
        assert_eq!(r.unwrap_err().raw_os_error(), Some(2));
        assert!(slept.is_empty());
        // `between` can stop the retries (the version re-check of `save_checked`)
        let r: Result<u32, &str> = retry_transient(
            0u32,
            |n| Err((std::io::Error::from_raw_os_error(5), n)),
            transient,
            &RENAME_RETRY_PAUSES,
            |_| Err("changed on disk"),
            |_| "io",
        );
        assert_eq!(r.unwrap_err(), "changed on disk");
    }

    /// Issue #65: another process holding the target open without delete sharing (as
    /// antivirus scanners and indexers do) makes the replacing rename fail with access
    /// denied; the save retries and succeeds once the handle is closed.
    #[cfg(windows)]
    #[test]
    fn save_waits_out_a_briefly_held_target_65() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
        let hold = |p: &Path| {
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(p)
                .unwrap()
        };
        let d = dir("held");
        let p = d.join("doc.md");
        std::fs::write(&p, "v1").unwrap();
        let v1 = disk_version(&p).unwrap().unwrap();
        // the hold really blocks a plain rename (otherwise the test proves nothing)
        let probe = d.join("probe.tmp");
        std::fs::write(&probe, "x").unwrap();
        let held = hold(&p);
        let e = std::fs::rename(&probe, &p).unwrap_err();
        assert!(rename_error_is_transient(&e), "{e:?}");
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            drop(held);
        });
        let out = save_checked(&p, "mine", Expect::Version(v1)).unwrap();
        release.join().unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "mine");
        assert!(out.kept_backup.is_none());
        // the unconditional writer retries as well
        let held = hold(&p);
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            drop(held);
        });
        save_atomic(&p, "atomic").unwrap();
        release.join().unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "atomic");
        // a hold outlasting the retries fails the save and keeps the file
        let held = hold(&p);
        let v = disk_version(&p).unwrap().unwrap();
        let r = save_checked(&p, "never", Expect::Version(v));
        drop(held);
        assert!(matches!(r, Err(SaveError::Io(_))), "{r:?}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "atomic");
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

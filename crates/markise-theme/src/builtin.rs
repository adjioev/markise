//! Built-in themes (ADR 0004): the original Markise Light/Dark themes ([`crate::Theme::light`],
//! [`crate::Theme::dark`]) plus permissively licensed community themes vendored under
//! `assets/themes/<name>/` (each with its upstream `LICENSE` and a `SOURCE.md` pinning the
//! commit; see the repository `NOTICE`).
//!
//! The vendored files are embedded into the binary at build time and imported at run time by
//! the regular CSS importer, exactly like a user theme: [`extract`] writes them once into a
//! content-addressed folder (`<root>/<hash>/<name>/…`) and the app imports the variant it needs
//! from there. Only the selected theme is imported, off the UI thread.

use std::io;
use std::path::{Path, PathBuf};

/// One embedded file of a built-in theme.
#[derive(Debug)]
pub struct BuiltinFile {
    /// Path relative to the theme folder (`/`-separated).
    pub path: &'static str,
    pub bytes: &'static [u8],
}

/// A vendored community theme with a light and a dark variant.
#[derive(Debug)]
pub struct BuiltinTheme {
    /// Stable id (setting value `builtin:<id>`); also the folder name.
    pub id: &'static str,
    /// Menu label.
    pub name: &'static str,
    /// Light variant CSS (path inside the folder).
    pub light: &'static str,
    /// Dark variant CSS.
    pub dark: &'static str,
    pub files: &'static [BuiltinFile],
}

macro_rules! file {
    ($dir:literal, $path:literal) => {
        BuiltinFile {
            path: $path,
            bytes: include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/themes/",
                $dir,
                "/",
                $path
            )),
        }
    };
}

/// The vendored themes, in menu order.
pub static BUILTIN_THEMES: &[BuiltinTheme] = &[
    BuiltinTheme {
        id: "drake",
        name: "Drake",
        light: "drake-light.css",
        dark: "drake-dark.css",
        files: &[
            file!("drake", "drake-light.css"),
            file!("drake", "drake-dark.css"),
            file!("drake", "drake/font.css"),
            file!("drake", "LICENSE"),
            file!("drake", "SOURCE.md"),
        ],
    },
    BuiltinTheme {
        id: "lapis",
        name: "Lapis",
        light: "lapis.css",
        dark: "lapis-dark.css",
        files: &[
            file!("lapis", "lapis.css"),
            file!("lapis", "lapis-dark.css"),
            file!("lapis", "LICENSE"),
            file!("lapis", "SOURCE.md"),
        ],
    },
    BuiltinTheme {
        id: "phycat",
        name: "Phycat",
        light: "phycat-sky.css",
        dark: "phycat-abyss.css",
        files: &[
            file!("phycat", "phycat-sky.css"),
            file!("phycat", "phycat-abyss.css"),
            file!("phycat", "phycat/phycat.light.css"),
            file!("phycat", "phycat/phycat.dark.css"),
            file!("phycat", "LICENSE"),
            file!("phycat", "SOURCE.md"),
        ],
    },
];

/// The built-in theme with this id.
pub fn builtin(id: &str) -> Option<&'static BuiltinTheme> {
    BUILTIN_THEMES.iter().find(|t| t.id == id)
}

/// FNV-1a over every embedded path and byte: names the extraction folder, so a new build with
/// changed theme files never reuses stale copies.
pub fn content_hash() -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: &[u8]| {
        for &x in b {
            h ^= x as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0100_0000_01b3);
    };
    for t in BUILTIN_THEMES {
        eat(t.id.as_bytes());
        for f in t.files {
            eat(f.path.as_bytes());
            eat(f.bytes);
        }
    }
    h
}

/// Name of the ownership marker in each extracted generation: lists the files Markise wrote.
pub const MANIFEST: &str = ".markise-builtin-manifest";

/// Lease file of a generation: every process using the generation holds a shared lock on it
/// for its lifetime; pruning needs the exclusive lock, so generations in use are never pruned.
pub const LEASE: &str = ".markise-lease";

/// Root-level lock serializing extraction and pruning across processes.
pub const ROOT_LOCK: &str = ".markise-builtin.lock";

/// Generations kept besides the current one, even when unleased (builds that predate leases).
const KEEP_OLD: usize = 1;

/// Leases held by this process (kept open until exit).
static LEASES: std::sync::Mutex<Vec<std::fs::File>> = std::sync::Mutex::new(Vec::new());

/// Creates the folders of `rel` below `root` one component at a time without following
/// symbolic links: an existing component that is a link or not a folder is an error.
fn ensure_dir(root: &Path, rel: &Path) -> io::Result<PathBuf> {
    let mut p = root.to_path_buf();
    for c in rel.components() {
        let std::path::Component::Normal(c) = c else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "unsafe path"));
        };
        p.push(c);
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{} is a link or a file", p.display()),
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => match std::fs::create_dir(&p) {
                Ok(()) => {}
                // another instance created it meanwhile: accept a real folder, not a link
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if !std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir()) {
                        return Err(e);
                    }
                }
                Err(e) => return Err(e),
            },
            Err(e) => return Err(e),
        }
    }
    Ok(p)
}

/// True when no component of `rel` below `root` (including the last) is a symbolic link.
fn no_links(root: &Path, rel: &Path) -> bool {
    let mut p = root.to_path_buf();
    for c in rel.components() {
        let std::path::Component::Normal(c) = c else {
            return false;
        };
        p.push(c);
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => return false,
            Ok(_) => {}
            Err(_) => return true, // missing: nothing to follow
        }
    }
    true
}

/// Writes the embedded files below `root/<hash>/` (skipping files that already hold the same
/// bytes) and returns that folder; theme `t` lives in `<returned>/<t.id>/`.
///
/// Folders are created without following symbolic links; each file is written to an
/// exclusively created temporary file and renamed into place. The generation records the files
/// it owns in [`MANIFEST`], and this process holds a shared [`LEASE`] on it. Older
/// generations are pruned only when they have a manifest, are not among the newest
/// [`KEEP_OLD`] other generations, and no process holds their lease; only manifest-listed
/// files are deleted (never through a symbolic link), so unexpected files and their folders
/// stay. Threat model (ADR 0004): accidents — several Markise instances, external editors,
/// cloud sync, crashes — not a hostile local process.
pub fn extract(root: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    let root = std::fs::canonicalize(root)?;
    // Extraction (taking a lease) and pruning (removing unleased generations and their lease
    // files) are serialized across processes on one stable root-level lock, so a starting
    // instance can never be waiting on a lease file the pruner is about to unlink.
    let root_lock = std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(ROOT_LOCK))?;
    root_lock.lock()?;
    let hash = format!("{:016x}", content_hash());
    let base = ensure_dir(&root, Path::new(&hash))?;
    take_lease(&base)?;
    let mut owned = Vec::new();
    for t in BUILTIN_THEMES {
        for f in t.files {
            let rel = Path::new(t.id).join(f.path);
            owned.push(format!("{}/{}", t.id, f.path));
            let parent = ensure_dir(&base, rel.parent().expect("file has a folder"))?;
            let dest = base.join(&rel);
            if std::fs::symlink_metadata(&dest).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{} is a link", dest.display()),
                ));
            }
            if std::fs::read(&dest).is_ok_and(|old| old == f.bytes) {
                continue;
            }
            let mut tmp = tempfile::Builder::new().prefix(".extract-").tempfile_in(&parent)?;
            io::Write::write_all(&mut tmp, f.bytes)?;
            tmp.persist(&dest).map_err(|e| e.error)?;
        }
    }
    let manifest = owned.join("\n") + "\n";
    if std::fs::read_to_string(base.join(MANIFEST)).ok().as_deref() != Some(manifest.as_str()) {
        let mut tmp = tempfile::Builder::new().prefix(".extract-").tempfile_in(&base)?;
        io::Write::write_all(&mut tmp, manifest.as_bytes())?;
        tmp.persist(base.join(MANIFEST)).map_err(|e| e.error)?;
    } else {
        // touch: orders generations by last use for KEEP_OLD
        let _ = std::fs::File::options()
            .append(true)
            .open(base.join(MANIFEST))
            .and_then(|f| f.set_modified(std::time::SystemTime::now()));
    }
    prune(&root, &hash);
    drop(root_lock);
    Ok(base)
}

fn take_lease(gen_dir: &Path) -> io::Result<()> {
    let f = std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(gen_dir.join(LEASE))?;
    f.lock_shared()?;
    if let Ok(mut l) = LEASES.lock() {
        l.push(f);
    }
    Ok(())
}

/// Removes old generations owned by Markise and not in use (see [`extract`]); best effort.
fn prune(root: &Path, current: &str) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    let mut old: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for e in rd.flatten().take(256) {
        let name = e.file_name().to_string_lossy().into_owned();
        // DirEntry::file_type does not follow links: symlinked generations are skipped
        if name == current || !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let m = e.path().join(MANIFEST);
        if let Ok(meta) = std::fs::symlink_metadata(&m)
            && meta.is_file()
        {
            old.push((meta.modified().unwrap_or(std::time::UNIX_EPOCH), e.path()));
        }
    }
    old.sort_by_key(|a| std::cmp::Reverse(a.0)); // newest first
    for (_, dir) in old.into_iter().skip(KEEP_OLD) {
        if !no_links(&dir, Path::new(LEASE)) {
            continue;
        }
        let Ok(lease) = std::fs::File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(LEASE))
        else {
            continue;
        };
        if lease.try_lock().is_err() {
            continue; // another process is using this generation
        }
        let Ok(list) = std::fs::read_to_string(dir.join(MANIFEST)) else {
            continue;
        };
        for rel in list.lines().take(4096) {
            let rel = Path::new(rel);
            if rel.as_os_str().is_empty() || !no_links(&dir, rel) {
                continue; // never delete through a symbolic link
            }
            let p = dir.join(rel);
            if std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_file()) {
                let _ = std::fs::remove_file(&p);
            }
            // remove now-empty parents up to the generation folder
            let mut parent = p.parent();
            while let Some(d) = parent {
                if d == dir || std::fs::remove_dir(d).is_err() {
                    break;
                }
                parent = d.parent();
            }
        }
        let _ = std::fs::remove_file(dir.join(MANIFEST));
        let _ = std::fs::remove_file(dir.join(LEASE));
        drop(lease);
        let _ = std::fs::remove_dir(&dir); // only if empty: unexpected files keep it
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn old_generation(root: &Path, name: &str, age_secs: u64) -> PathBuf {
        let g = root.join(name);
        std::fs::create_dir_all(g.join("drake")).unwrap();
        std::fs::write(g.join("drake").join("drake-light.css"), "old").unwrap();
        std::fs::write(g.join(MANIFEST), "drake/drake-light.css\n").unwrap();
        let t = std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs);
        std::fs::File::options()
            .append(true)
            .open(g.join(MANIFEST))
            .unwrap()
            .set_modified(t)
            .unwrap();
        g
    }

    #[test]
    fn concurrent_first_extractions_all_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("fresh");
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let root = root.clone();
                std::thread::spawn(move || extract(&root))
            })
            .collect();
        let bases: Vec<PathBuf> = handles.into_iter().map(|h| h.join().unwrap().unwrap()).collect();
        assert!(bases.windows(2).all(|w| w[0] == w[1]));
        assert!(bases[0].join("lapis").join("lapis.css").is_file());
        assert!(bases[0].join(LEASE).is_file());
    }

    #[test]
    fn folder_creation_accepts_a_folder_created_meanwhile_but_not_a_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("a")).unwrap();
        assert!(ensure_dir(dir.path(), Path::new("a/b")).is_ok());
        std::fs::write(dir.path().join("f"), "x").unwrap();
        assert!(ensure_dir(dir.path(), Path::new("f/g")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn pruning_and_extraction_never_follow_symbolic_links() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("builtin");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("drake-light.css"), "precious").unwrap();
        old_generation(&root, "cccccccccccccccc", 50); // newest old one: kept anyway
        let g = old_generation(&root, "dddddddddddddddd", 500);
        // the old generation's folder is swapped for a link to an outside folder
        std::fs::remove_dir_all(g.join("drake")).unwrap();
        std::os::unix::fs::symlink(&outside, g.join("drake")).unwrap();
        extract(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(outside.join("drake-light.css")).unwrap(),
            "precious"
        );

        // extraction refuses to write through a linked folder of the current generation
        let dir2 = tempfile::tempdir().unwrap();
        let root2 = dir2.path().join("builtin");
        let cur = root2.join(format!("{:016x}", content_hash()));
        std::fs::create_dir_all(&cur).unwrap();
        std::os::unix::fs::symlink(&outside, cur.join("drake")).unwrap();
        assert!(extract(&root2).is_err());
        assert_eq!(
            std::fs::read_dir(&outside).unwrap().count(),
            1,
            "nothing written outside"
        );
    }

    #[test]
    fn a_generation_leased_by_another_process_is_not_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        old_generation(root, "eeeeeeeeeeeeeeee", 50); // newest old one: kept by KEEP_OLD
        let busy = old_generation(root, "ffffffffffffffff", 500);
        // another process (here: a second file handle) holds a shared lease
        let lease = std::fs::File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(busy.join(LEASE))
            .unwrap();
        lease.lock_shared().unwrap();
        extract(root).unwrap();
        assert!(
            busy.join("drake").join("drake-light.css").exists(),
            "leased generation kept"
        );
        drop(lease);
        extract(root).unwrap();
        assert!(!busy.join("drake").exists(), "pruned once unleased");
    }

    #[test]
    fn every_builtin_has_license_source_and_both_variants() {
        for t in BUILTIN_THEMES {
            let has = |p: &str| t.files.iter().any(|f| f.path == p);
            assert!(has("LICENSE") && has("SOURCE.md"), "{}", t.id);
            assert!(has(t.light) && has(t.dark), "{}", t.id);
            let lic = t.files.iter().find(|f| f.path == "LICENSE").unwrap();
            assert!(
                std::str::from_utf8(lic.bytes).unwrap().contains("MIT License"),
                "{}",
                t.id
            );
        }
    }

    #[test]
    fn extract_is_idempotent_and_prunes_only_owned_old_generations() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // not ours: a hex-named folder without a manifest is never touched
        std::fs::create_dir_all(root.join("0123456789abcdef").join("x")).unwrap();
        std::fs::create_dir_all(root.join("keep-me")).unwrap();
        // two old generations of ours; the newer one is kept, the older pruned except for an
        // unexpected user file
        for (name, age) in [("aaaaaaaaaaaaaaaa", 200), ("bbbbbbbbbbbbbbbb", 100)] {
            let g = root.join(name);
            std::fs::create_dir_all(g.join("drake")).unwrap();
            std::fs::write(g.join("drake").join("drake-light.css"), "old").unwrap();
            std::fs::write(g.join(MANIFEST), "drake/drake-light.css\n").unwrap();
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs(age);
            std::fs::File::options()
                .append(true)
                .open(g.join(MANIFEST))
                .unwrap()
                .set_modified(t)
                .unwrap();
        }
        std::fs::write(root.join("aaaaaaaaaaaaaaaa").join("notes.txt"), "mine").unwrap();
        let a = extract(root).unwrap();
        let b = extract(root).unwrap();
        assert_eq!(a, b);
        assert!(a.join("phycat").join("phycat").join("phycat.light.css").is_file());
        assert!(a.join("drake").join("LICENSE").is_file());
        assert!(a.join(MANIFEST).is_file());
        assert!(root.join("0123456789abcdef").join("x").exists(), "unowned folder kept");
        assert!(root.join("keep-me").exists());
        assert!(
            root.join("bbbbbbbbbbbbbbbb")
                .join("drake")
                .join("drake-light.css")
                .exists(),
            "newest old generation kept"
        );
        let older = root.join("aaaaaaaaaaaaaaaa");
        assert!(
            !older.join("drake").exists(),
            "owned files of the older generation removed"
        );
        assert!(older.join("notes.txt").exists(), "unexpected files preserved");
        // no temp files left behind
        let leftovers = std::fs::read_dir(a.join("drake"))
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with(".extract-"));
        assert!(!leftovers);
    }
}

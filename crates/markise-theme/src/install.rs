//! "Import theme…" (ADR 0004): copy a CSS theme and its assets into the user theme folder as
//! an isolated package.
//!
//! Layout in the user folder for a theme `{name}.css`:
//! - `{name}.markise-theme/` — the package: the chosen `{name}.css`, its asset folder
//!   `{name}/` (spec 03 §1.2) and the files the importer actually reads for it (`@import`
//!   targets, existing local `@font-face` files), at their paths relative to the source folder;
//! - `{name}.css` — a one-line stub that `@import`s the package's stylesheet, so discovery,
//!   the menu label and `{name}.user.css` work as for any theme. Relative paths inside the
//!   package keep working; nothing outside the package is written, so an imported theme can
//!   never overwrite another theme's files (e.g. a shared `common.css`).
//!
//! Safety:
//! - **Preflight:** if the stub or the package already exists, nothing is written and
//!   [`InstallError::Collision`] lists them; the caller asks the user and retries with
//!   `replace = true`.
//! - **Staging:** files are copied into a freshly created temporary folder inside the user
//!   folder; the package is moved into place by rename (the old one kept aside until the new one
//!   and the stub are in place, restored on failure), so a failure leaves the user folder as it
//!   was.
//! - **Confinement** (TI-9): every source is resolved with `canonicalize` and must lie inside
//!   the chosen theme's folder; symbolic links inside the asset folder are skipped; only regular
//!   files are copied; destination paths are built from normal components only, inside the
//!   fresh staging folder (no pre-existing directories or links are followed). User layers
//!   (`base.user.css`, `{name}.user.css`) are not copied.
//! - **Bounds** ([`InstallLimits`]): files, bytes, depth, and entries / folders visited while
//!   enumerating (checked during enumeration, not after).

use std::io;
use std::path::{Component, Path, PathBuf};

use crate::import::{ImportOptions, import_theme_file};
use crate::{FontSource, ImportResult};

/// Suffix of the package folder next to the stub.
pub const PACKAGE_SUFFIX: &str = ".markise-theme";

/// First line of a generated stub.
const STUB_MARKER: &str = "/* Markise: imported theme package";

/// Bounds for one install.
#[derive(Clone, Debug)]
pub struct InstallLimits {
    pub max_files: usize,
    pub max_total_bytes: u64,
    pub max_file_bytes: u64,
    pub max_depth: usize,
    /// Directory entries looked at while enumerating the asset folder.
    pub max_entries: usize,
    /// Folders opened while enumerating the asset folder.
    pub max_dirs: usize,
}

impl Default for InstallLimits {
    fn default() -> Self {
        InstallLimits {
            max_files: 512,
            max_total_bytes: 64 << 20,
            max_file_bytes: 32 << 20,
            max_depth: 8,
            max_entries: 4096,
            max_dirs: 256,
        }
    }
}

/// Result of [`install_theme`].
#[derive(Debug)]
pub struct Installed {
    /// The theme stylesheet in the user theme folder (the stub).
    pub css: PathBuf,
    /// The package folder.
    pub package: PathBuf,
    /// Files written into the package, relative to it.
    pub copied: Vec<PathBuf>,
    /// Sources not copied and why (outside the theme folder, symlink, limit, …).
    pub skipped: Vec<(PathBuf, String)>,
    /// A theme of the same name was replaced.
    pub replaced: bool,
    /// Import of the source (its report is what the UI shows).
    pub import: ImportResult,
}

#[derive(Debug)]
pub enum InstallError {
    /// Not a theme, unreadable, …
    Invalid(String),
    /// These paths already exist in the user folder; nothing was changed. Retry with
    /// `replace = true` after asking the user.
    Collision(Vec<PathBuf>),
    /// The install failed after the old package was moved aside and it could not be put back
    /// (e.g. cloud sync recreated the destination): the old package is kept at `backup`.
    Recovery {
        error: String,
        backup: PathBuf,
    },
    Io(io::Error),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::Invalid(m) => f.write_str(m),
            InstallError::Collision(p) => write!(
                f,
                "a theme with this name already exists ({})",
                p.iter()
                    .filter_map(|p| p.file_name())
                    .map(|n| n.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            InstallError::Recovery { error, backup } => write!(
                f,
                "{error}; the previous version of the theme was kept at {}",
                backup.display()
            ),
            InstallError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<io::Error> for InstallError {
    fn from(e: io::Error) -> Self {
        InstallError::Io(e)
    }
}

fn invalid(msg: impl Into<String>) -> InstallError {
    InstallError::Invalid(msg.into())
}

fn safe_rel(rel: &Path) -> bool {
    !rel.as_os_str().is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_)))
}

fn exists_no_follow(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// Copies the theme `src_css` (and its assets) into `dest_dir` (created if missing) as a
/// package; see the module docs.
pub fn install_theme(
    src_css: &Path,
    dest_dir: &Path,
    limits: &InstallLimits,
    replace: bool,
) -> Result<Installed, InstallError> {
    install_theme_with(src_css, dest_dir, limits, replace, &Hooks::default())
}

/// Test hooks: simulate other writers at fixed points of the commit.
#[derive(Default)]
struct Hooks<'a> {
    before_commit: Option<&'a dyn Fn()>,
    after_backup: Option<&'a dyn Fn()>,
}

/// Name of the advisory lock file in the user theme folder: installs of all Markise processes
/// are serialized on it.
pub const LOCK_FILE: &str = ".markise-install.lock";

/// [`install_theme`] with a hook run just before the commit (tests simulate a concurrent
/// writer with it).
fn install_theme_with(
    src_css: &Path,
    dest_dir: &Path,
    limits: &InstallLimits,
    replace: bool,
    hooks: &Hooks,
) -> Result<Installed, InstallError> {
    let meta = std::fs::metadata(src_css)?;
    if !meta.is_file() {
        return Err(invalid(format!("{} is not a file", src_css.display())));
    }
    let src = std::fs::canonicalize(src_css)?;
    let file_name = src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| invalid("the theme file name is not valid UTF-8"))?
        .to_string();
    let lower = file_name.to_ascii_lowercase();
    if !lower.ends_with(".css") || lower.ends_with(".user.css") {
        return Err(invalid("choose a theme .css file (not a .user.css override)"));
    }
    let stem = file_name[..file_name.len() - 4].to_string();
    if stem.is_empty() || stem.starts_with('.') || stem.contains(['"', '\\', '\n', '\r', '/']) {
        return Err(invalid("unsupported theme file name"));
    }
    let src_dir = src
        .parent()
        .ok_or_else(|| invalid("the theme has no folder"))?
        .to_path_buf();
    let import =
        import_theme_file(&src, &ImportOptions::default()).map_err(|e| invalid(format!("cannot import: {e}")))?;

    std::fs::create_dir_all(dest_dir)?;
    let dest_root = std::fs::canonicalize(dest_dir)?;
    let stub = dest_root.join(&file_name);
    let package = dest_root.join(format!("{stem}{PACKAGE_SUFFIX}"));
    let mut out = Installed {
        css: stub.clone(),
        package: package.clone(),
        copied: Vec::new(),
        skipped: Vec::new(),
        replaced: false,
        import,
    };
    if src_dir.starts_with(&dest_root) {
        // already in the user theme folder (or one of its packages): nothing to copy
        out.css = src.clone();
        return Ok(out);
    }

    // ---- one install at a time across Markise processes (advisory lock, released on drop)
    let lock = std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dest_root.join(LOCK_FILE))?;
    lock.lock()?;

    // ---- preflight: report collisions before touching anything
    let collisions: Vec<PathBuf> = [&stub, &package]
        .into_iter()
        .filter(|p| exists_no_follow(p))
        .cloned()
        .collect();
    if !collisions.is_empty() && !replace {
        return Err(InstallError::Collision(collisions));
    }
    out.replaced = !collisions.is_empty();

    // ---- collect candidate sources (bounded)
    let mut sources: Vec<PathBuf> = vec![src.clone()];
    let asset_dir = src_dir.join(&stem);
    if std::fs::symlink_metadata(&asset_dir).is_ok_and(|m| m.is_dir()) {
        let mut budget = Budget {
            entries: limits.max_entries,
            dirs: limits.max_dirs,
        };
        walk(&asset_dir, 0, limits, &mut budget, &mut sources, &mut out.skipped);
    }
    let user_layers = ["base.user.css".to_string(), format!("{stem}.user.css")];
    for f in out.import.report.files.iter().skip(1) {
        if f.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| user_layers.iter().any(|u| u == n))
            && f.parent()
                .is_some_and(|p| std::fs::canonicalize(p).ok().as_ref() == Some(&src_dir))
        {
            continue;
        }
        sources.push(f.clone());
    }
    for face in &out.import.theme.fonts.faces {
        for s in &face.sources {
            if let FontSource::File { path, exists: true, .. } = s {
                sources.push(path.clone());
            }
        }
    }

    // ---- stage into a fresh folder inside the user folder (same file system for rename)
    let staging = tempfile::Builder::new()
        .prefix(".markise-install-")
        .tempdir_in(&dest_root)?;
    let mut total = 0u64;
    let mut done: Vec<PathBuf> = Vec::new();
    for s in sources {
        let Ok(canon) = std::fs::canonicalize(&s) else {
            out.skipped.push((s, "missing".into()));
            continue;
        };
        if done.contains(&canon) {
            continue;
        }
        let Ok(rel) = canon.strip_prefix(&src_dir).map(Path::to_path_buf) else {
            out.skipped.push((s, "outside the theme folder".into()));
            continue;
        };
        if !safe_rel(&rel) {
            out.skipped.push((s, "unsafe path".into()));
            continue;
        }
        let Ok(m) = std::fs::metadata(&canon) else { continue };
        if !m.is_file() {
            out.skipped.push((s, "not a regular file".into()));
            continue;
        }
        if m.len() > limits.max_file_bytes
            || total + m.len() > limits.max_total_bytes
            || out.copied.len() >= limits.max_files
        {
            out.skipped.push((s, "size or file-count limit".into()));
            continue;
        }
        let dest = staging.path().join(&rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?; // inside the fresh staging folder only
        }
        // an error drops `staging` (removed) and leaves the user folder untouched
        std::fs::copy(&canon, &dest)?;
        total += m.len();
        done.push(canon);
        out.copied.push(rel);
    }
    let pkg_css = Path::new(&file_name);
    if !staging.path().join(pkg_css).is_file() {
        return Err(invalid("the theme file could not be copied"));
    }

    // ---- commit. Other Markise processes wait on the lock; an external program (editor,
    // cloud sync) may still have created a file meanwhile, so collisions are checked again
    // and the stub is published without replacing when replacement was not approved.
    if let Some(h) = hooks.before_commit {
        h();
    }
    if !replace && (exists_no_follow(&stub) || exists_no_follow(&package)) {
        let existing = [&stub, &package]
            .into_iter()
            .filter(|p| exists_no_follow(p))
            .cloned()
            .collect();
        return Err(InstallError::Collision(existing)); // staging is removed on drop
    }
    // old package aside into a fresh, uniquely named folder (nothing is deleted before use)
    let backup_dir = if exists_no_follow(&package) {
        let d = tempfile::Builder::new()
            .prefix(".markise-old-")
            .tempdir_in(&dest_root)?;
        std::fs::rename(&package, d.path().join("package"))?;
        Some(d)
    } else {
        None
    };
    if let Some(h) = hooks.after_backup {
        h();
    }
    // Rollback touches only what this transaction moved or created. The backup is deleted only
    // after a successful restore (or publication); otherwise it is kept and reported.
    let rollback = |published: bool, backup_dir: Option<tempfile::TempDir>, error: String| -> InstallError {
        if published {
            let _ = std::fs::remove_dir_all(&package);
        }
        let Some(d) = backup_dir else {
            return InstallError::Invalid(error);
        };
        let restored = !exists_no_follow(&package) && std::fs::rename(d.path().join("package"), &package).is_ok();
        if restored {
            drop(d); // now empty
            InstallError::Invalid(error)
        } else {
            let kept = d.keep();
            InstallError::Recovery {
                error,
                backup: kept.join("package"),
            }
        }
    };
    if exists_no_follow(&package) {
        // appeared after it was moved aside: not ours to replace
        return Err(
            match rollback(false, backup_dir, "the theme folder was recreated meanwhile".into()) {
                InstallError::Invalid(_) => InstallError::Collision(vec![package.clone()]),
                e => e,
            },
        );
    }
    let staged = staging.keep();
    if let Err(e) = std::fs::rename(&staged, &package) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(match rollback(false, backup_dir, e.to_string()) {
            InstallError::Invalid(_) => e.into(),
            r => r,
        });
    }
    let stub_text = format!(
        "{STUB_MARKER}; edit the files in {stem}{PACKAGE_SUFFIX}/ */\n@import \"{stem}{PACKAGE_SUFFIX}/{file_name}\";\n"
    );
    let write_stub = || -> io::Result<()> {
        let mut tmp = tempfile::Builder::new()
            .prefix(".markise-stub-")
            .tempfile_in(&dest_root)?;
        io::Write::write_all(&mut tmp, stub_text.as_bytes())?;
        if replace {
            // rename over the old stub (a symbolic link there is replaced, not followed)
            tmp.persist(&stub).map(|_| ()).map_err(|e| e.error)
        } else {
            // publish without replacing: a hard link fails if the stub appeared meanwhile
            std::fs::hard_link(tmp.path(), &stub)
        }
    };
    if let Err(e) = write_stub() {
        let collision = e.kind() == io::ErrorKind::AlreadyExists;
        return Err(match rollback(true, backup_dir, e.to_string()) {
            InstallError::Invalid(_) if collision => InstallError::Collision(vec![stub.clone()]),
            InstallError::Invalid(_) => e.into(),
            r => r,
        });
    }
    drop(backup_dir); // the replaced package is removed with its temporary folder
    drop(lock);
    Ok(out)
}

/// Whether `css` is a stub written by [`install_theme`].
pub fn is_package_stub(css: &Path) -> bool {
    std::fs::read_to_string(css).is_ok_and(|t| t.starts_with(STUB_MARKER))
}

struct Budget {
    entries: usize,
    dirs: usize,
}

fn walk(
    dir: &Path,
    depth: usize,
    limits: &InstallLimits,
    budget: &mut Budget,
    out: &mut Vec<PathBuf>,
    skipped: &mut Vec<(PathBuf, String)>,
) {
    if depth > limits.max_depth {
        skipped.push((dir.to_path_buf(), "folder nesting limit".into()));
        return;
    }
    if budget.dirs == 0 {
        skipped.push((dir.to_path_buf(), "folder limit".into()));
        return;
    }
    budget.dirs -= 1;
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    // read at most the remaining entry budget from this folder, then sort that part
    let mut entries = Vec::new();
    for e in rd {
        if budget.entries == 0 {
            skipped.push((dir.to_path_buf(), "entry limit".into()));
            break;
        }
        budget.entries -= 1;
        if let Ok(e) = e {
            entries.push(e);
        }
    }
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if out.len() > limits.max_files {
            return;
        }
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            skipped.push((p, "symbolic link".into()));
        } else if ft.is_dir() {
            walk(&p, depth + 1, limits, budget, out, skipped);
        } else if ft.is_file() {
            out.push(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rgba;

    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dest = tmp.path().join("user-themes");
        let outside = tmp.path().join("outside");
        let assets = src.join("ocean-dark");
        std::fs::create_dir_all(assets.join("img")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.css"), "p { color: red }").unwrap();
        std::fs::write(outside.join("secret.txt"), "x").unwrap();
        std::fs::write(src.join("common.css"), "#write a { color: #123456 }").unwrap();
        std::fs::write(src.join("unrelated.css"), "p { color: blue }").unwrap();
        std::fs::write(src.join("base.user.css"), "p { color: green }").unwrap();
        std::fs::write(assets.join("font.woff2"), [0u8; 16]).unwrap();
        std::fs::write(assets.join("img").join("bg.png"), [1u8; 8]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.join("secret.txt"), assets.join("link.txt")).unwrap();
        std::fs::write(
            src.join("ocean-dark.css"),
            "@import 'common.css'; @import '../outside/secret.css';\n\
             @font-face { font-family: X; src: url(ocean-dark/font.woff2) }\n\
             body { background: #101418 }",
        )
        .unwrap();
        (tmp, src, dest, outside)
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut v = Vec::new();
        fn rec(base: &Path, d: &Path, v: &mut Vec<String>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if std::fs::symlink_metadata(&p).unwrap().is_dir() {
                    rec(base, &p, v);
                } else {
                    v.push(p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"));
                }
            }
        }
        rec(dir, dir, &mut v);
        v.sort();
        v
    }

    #[test]
    fn installs_an_isolated_package_and_never_touches_other_user_files() {
        let (_tmp, src, dest, _outside) = setup();
        std::fs::create_dir_all(&dest).unwrap();
        // an unrelated user theme file with the same name as a dependency
        std::fs::write(dest.join("common.css"), "/* mine */ p { color: #ff00ff }").unwrap();
        let r = install_theme(&src.join("ocean-dark.css"), &dest, &InstallLimits::default(), false).unwrap();
        assert_eq!(
            std::fs::read_to_string(dest.join("common.css")).unwrap(),
            "/* mine */ p { color: #ff00ff }",
            "existing user files are untouched"
        );
        assert_eq!(
            files(&dest)
                .into_iter()
                .filter(|f| !f.ends_with(LOCK_FILE))
                .collect::<Vec<_>>(),
            vec![
                "common.css",
                "ocean-dark.css",
                "ocean-dark.markise-theme/common.css",
                "ocean-dark.markise-theme/ocean-dark.css",
                "ocean-dark.markise-theme/ocean-dark/font.woff2",
                "ocean-dark.markise-theme/ocean-dark/img/bg.png",
            ],
            "no unreferenced siblings, user layers, symlinks or outside files"
        );
        assert!(is_package_stub(&r.css));
        assert!(!r.replaced);
        // the installed theme imports the same way from its new home
        let again = import_theme_file(&r.css, &ImportOptions::default()).unwrap();
        assert_eq!(again.theme.page_bg, Rgba::hex(0x101418));
        assert_eq!(again.theme.link, Rgba::hex(0x123456));
        // installing from the user folder itself copies nothing
        let r3 = install_theme(&r.css, &dest, &InstallLimits::default(), false).unwrap();
        assert!(r3.copied.is_empty());
    }

    #[test]
    fn collisions_are_reported_before_anything_changes_and_replace_swaps_the_package() {
        let (_tmp, src, dest, _outside) = setup();
        install_theme(&src.join("ocean-dark.css"), &dest, &InstallLimits::default(), false).unwrap();
        let before = files(&dest);
        std::fs::remove_file(src.join("ocean-dark").join("img").join("bg.png")).unwrap();
        match install_theme(&src.join("ocean-dark.css"), &dest, &InstallLimits::default(), false) {
            Err(InstallError::Collision(p)) => assert_eq!(p.len(), 2, "{p:?}"),
            other => panic!("expected a collision, got {other:?}"),
        }
        assert_eq!(files(&dest), before, "nothing changed");
        let r = install_theme(&src.join("ocean-dark.css"), &dest, &InstallLimits::default(), true).unwrap();
        assert!(r.replaced);
        let after = files(&dest);
        assert!(
            !after.iter().any(|f| f.ends_with("bg.png")),
            "old package replaced: {after:?}"
        );
        assert!(
            !after
                .iter()
                .any(|f| f.contains(".markise-old-") || f.contains(".markise-install-"))
        );
        // a plain user theme of the same name also counts as a collision
        std::fs::write(dest.join("plain.css"), "p {}").unwrap();
        std::fs::write(src.join("plain.css"), "p { color: red }").unwrap();
        assert!(matches!(
            install_theme(&src.join("plain.css"), &dest, &InstallLimits::default(), false),
            Err(InstallError::Collision(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_copy_leaves_the_user_folder_unchanged() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, src, dest, _outside) = setup();
        install_theme(&src.join("ocean-dark.css"), &dest, &InstallLimits::default(), false).unwrap();
        let before = files(&dest);
        let locked = src.join("ocean-dark").join("img").join("bg.png");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let r = install_theme(&src.join("ocean-dark.css"), &dest, &InstallLimits::default(), true);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
        if std::fs::read(&locked).is_ok() && r.is_ok() {
            return; // running as root: permissions do not apply
        }
        assert!(r.is_err());
        assert_eq!(files(&dest), before, "old package intact, no staging leftovers");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_package_path_is_replaced_without_writing_through_it() {
        let (tmp, src, dest, _outside) = setup();
        std::fs::create_dir_all(&dest).unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, dest.join("ocean-dark.markise-theme")).unwrap();
        assert!(matches!(
            install_theme(&src.join("ocean-dark.css"), &dest, &InstallLimits::default(), false),
            Err(InstallError::Collision(_))
        ));
        install_theme(&src.join("ocean-dark.css"), &dest, &InstallLimits::default(), true).unwrap();
        assert!(
            std::fs::read_dir(&elsewhere).unwrap().next().is_none(),
            "nothing written outside"
        );
        assert!(
            std::fs::symlink_metadata(dest.join("ocean-dark.markise-theme"))
                .unwrap()
                .is_dir()
        );
    }

    #[test]
    fn enumeration_is_bounded_while_walking() {
        let (_tmp, src, dest, _outside) = setup();
        let wide = src.join("ocean-dark").join("wide");
        for i in 0..3000 {
            std::fs::create_dir_all(wide.join(format!("d{i}"))).unwrap();
        }
        let limits = InstallLimits {
            max_entries: 500,
            max_dirs: 50,
            ..Default::default()
        };
        let t = std::time::Instant::now();
        let r = install_theme(&src.join("ocean-dark.css"), &dest, &limits, false).unwrap();
        assert!(t.elapsed() < std::time::Duration::from_secs(5));
        assert!(
            r.skipped
                .iter()
                .any(|(_, why)| why == "entry limit" || why == "folder limit"),
            "{:?}",
            r.skipped
        );
    }

    #[test]
    fn a_stub_that_appears_during_staging_is_a_collision_and_nothing_is_left_behind() {
        let (_tmp, src, dest, _outside) = setup();
        let stub = dest.join("ocean-dark.css");
        let write = || std::fs::write(&stub, "/* written by another program */").unwrap();
        let hooks = Hooks {
            before_commit: Some(&write),
            ..Default::default()
        };
        let r = install_theme_with(
            &src.join("ocean-dark.css"),
            &dest,
            &InstallLimits::default(),
            false,
            &hooks,
        );
        assert!(matches!(r, Err(InstallError::Collision(_))), "{r:?}");
        assert_eq!(
            std::fs::read_to_string(&stub).unwrap(),
            "/* written by another program */"
        );
        let left: Vec<String> = files(&dest).into_iter().filter(|f| !f.ends_with(LOCK_FILE)).collect();
        assert_eq!(left, vec!["ocean-dark.css"], "no package, staging or backup left");
    }

    #[test]
    fn the_old_package_is_kept_when_it_cannot_be_restored() {
        let (_tmp, src, dest, _outside) = setup();
        install_theme(&src.join("ocean-dark.css"), &dest, &InstallLimits::default(), false).unwrap();
        let pkg = dest.join("ocean-dark.markise-theme");
        // cloud sync recreates the package folder right after the old one was moved aside
        let recreate = || {
            std::fs::create_dir_all(&pkg).unwrap();
            std::fs::write(pkg.join("synced.txt"), "from the cloud").unwrap();
        };
        let hooks = Hooks {
            after_backup: Some(&recreate),
            ..Default::default()
        };
        let r = install_theme_with(
            &src.join("ocean-dark.css"),
            &dest,
            &InstallLimits::default(),
            true,
            &hooks,
        );
        let Err(InstallError::Recovery { backup, .. }) = r else {
            panic!("expected a recovery error, got {r:?}");
        };
        assert!(
            backup.join("ocean-dark.css").is_file(),
            "old package kept at {}",
            backup.display()
        );
        assert!(pkg.join("synced.txt").is_file(), "the recreated folder is not touched");
    }

    #[test]
    fn concurrent_installs_are_serialized() {
        let (_tmp, src, dest, _outside) = setup();
        install_theme(&src.join("ocean-dark.css"), &dest, &InstallLimits::default(), false).unwrap();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let (src, dest) = (src.clone(), dest.clone());
                std::thread::spawn(move || {
                    install_theme(&src.join("ocean-dark.css"), &dest, &InstallLimits::default(), true).map(|_| ())
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap().unwrap();
        }
        let left: Vec<String> = files(&dest).into_iter().filter(|f| !f.ends_with(LOCK_FILE)).collect();
        assert_eq!(
            left.iter()
                .filter(|f| f.ends_with("ocean-dark.markise-theme/ocean-dark.css"))
                .count(),
            1
        );
        assert!(
            !left.iter().any(|f| f.contains(".markise-old-")
                || f.contains(".markise-install-")
                || f.contains(".markise-stub-")),
            "{left:?}"
        );
        assert!(is_package_stub(&dest.join("ocean-dark.css")));
    }

    #[test]
    fn rejects_non_theme_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("x.user.css"), "").unwrap();
        std::fs::write(tmp.path().join("x.txt"), "").unwrap();
        let dest = tmp.path().join("d");
        let l = InstallLimits::default();
        assert!(install_theme(&tmp.path().join("x.user.css"), &dest, &l, false).is_err());
        assert!(install_theme(&tmp.path().join("x.txt"), &dest, &l, false).is_err());
        assert!(install_theme(tmp.path(), &dest, &l, false).is_err());
    }
}

//! Local image references in exported HTML (EX-12): resolving them like the viewer (document
//! folder, `typora-root-url`), embedding them as `data:` URIs within per-file and per-export
//! budgets, or rewriting them relative to the exported file.
//!
//! Hardening: only regular files are read, through the opened handle, with a bounded read
//! (no FIFO/device can block the export, a growing file cannot exceed the limit); each file
//! is encoded once per export; `file:` URLs are parsed with their authority; query and
//! fragment suffixes are kept apart from the path; links are computed between canonical
//! (symlink-resolved) paths, and a reference that cannot be resolved is left unchanged.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::Assets;
use crate::job::JobControl;

/// A local image reference: the file plus the URL suffix (`?query#fragment`) to keep.
pub type LocalRef = markise_doc::resource::LocalFile;

/// The local file an image `src` refers to, or `None` when it is not a local reference.
/// This is the shared resolver of the viewer and the PDF/PNG export
/// (`markise_doc::resource::local_file`).
pub fn local_ref(assets: &Assets, src: &str) -> Option<LocalRef> {
    markise_doc::resource::local_file(&assets.doc_dir, assets.root_url.as_deref(), src)
}

/// MIME type of embeddable raster images (by extension; the formats the export sanitizer
/// accepts as `data:` images, C40).
fn raster_mime(p: &Path) -> Option<&'static str> {
    let ext = p.extension()?.to_string_lossy().to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

/// Reads a regular file of at most `max` bytes (shared hardened reader,
/// `markise_doc::resource::read_regular`): FIFOs, devices and directories are rejected
/// without blocking; the limit holds even if the file grows.
pub fn read_bounded(path: &Path, max: u64) -> Result<Vec<u8>, String> {
    markise_doc::resource::read_regular(path, max).map_err(|e| match e.kind() {
        std::io::ErrorKind::InvalidData => format!("over the {} KB limit (kept as a link)", max / 1024),
        _ => e.to_string(),
    })
}

/// `data:` URI of the raster image at `path` (at most `max` bytes).
pub fn data_uri(path: &Path, max: u64) -> Result<String, String> {
    let mime = raster_mime(path).ok_or("not a raster image (kept as a link)")?;
    let bytes = read_bounded(path, max)?;
    Ok(format!("data:{mime};base64,{}", base64(&bytes)))
}

/// Test hook type (see `Embedder::before_read`).
#[cfg(test)]
type ReadHook = Box<dyn Fn(&Path)>;

/// Embedding state of one export: each file is encoded once; the embedded output and the
/// cache are bounded by the same budget. The budget is checked before a file is read, the
/// read goes through the handle whose size was checked and is capped by what the budget still
/// allows (a file growing or swapped in between is rejected before encoding). Only emitted
/// encodings are cached (no negative entries), so the cache stays within the budget.
pub struct Embedder {
    /// Per-file limit (bytes of the image file).
    pub max_file: u64,
    /// Budget for the embedded `data:` output of the whole export (bytes, every occurrence
    /// counts).
    pub budget: usize,
    used: usize,
    cache: HashMap<PathBuf, Arc<str>>,
    cache_bytes: usize,
    budget_warned: bool,
    /// Test hook run between the size check and the read.
    #[cfg(test)]
    before_read: Option<ReadHook>,
}

impl std::fmt::Debug for Embedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Embedder")
            .field("used", &self.used)
            .field("cache_bytes", &self.cache_bytes)
            .finish()
    }
}

/// Outcome of [`Embedder::embed`].
#[derive(Debug, PartialEq)]
pub enum Embedded {
    Uri(Arc<str>),
    /// Not embedded (the reason is a user-facing warning; `None` = already reported).
    Link(Option<String>),
}

/// Raw bytes whose base64 encoding fits in `room` bytes after the `data:` prefix.
fn raw_capacity(room: usize) -> u64 {
    (room / 4 * 3) as u64
}

impl Embedder {
    pub fn new(max_file: u64, budget: usize) -> Self {
        Embedder {
            max_file,
            budget,
            used: 0,
            cache: HashMap::new(),
            cache_bytes: 0,
            budget_warned: false,
            #[cfg(test)]
            before_read: None,
        }
    }

    /// Bytes of embedded output so far.
    pub fn used(&self) -> usize {
        self.used
    }

    /// Bytes of encodings held in the cache.
    pub fn cache_bytes(&self) -> usize {
        self.cache_bytes
    }

    fn over_budget(&mut self) -> Embedded {
        let first = !self.budget_warned;
        self.budget_warned = true;
        Embedded::Link(first.then(|| {
            format!(
                "embedded images reached the {} MB limit for one export; further images are links",
                self.budget >> 20
            )
        }))
    }

    /// The `data:` URI of `path`, encoded once per export, if the budgets allow it.
    pub fn embed(&mut self, path: &Path, control: Option<&JobControl>) -> Embedded {
        use std::io::Read;
        if control.is_some_and(JobControl::is_cancelled) {
            return Embedded::Link(None); // the export is discarded anyway
        }
        let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(uri) = self.cache.get(&key).cloned() {
            if self.used + uri.len() > self.budget {
                return self.over_budget();
            }
            self.used += uri.len();
            return Embedded::Uri(uri);
        }
        let Some(mime) = raster_mime(path) else {
            return Embedded::Link(Some("not a raster image (kept as a link)".into()));
        };
        let (file, len) = match markise_doc::resource::open_regular(path) {
            Ok(x) => x,
            Err(e) => return Embedded::Link(Some(e.to_string())),
        };
        if len > self.max_file {
            return Embedded::Link(Some(format!(
                "over the {} KB limit (kept as a link)",
                self.max_file / 1024
            )));
        }
        let prefix = "data:;base64,".len() + mime.len();
        let room = self.budget.saturating_sub(self.used + prefix);
        let allowed = self.max_file.min(raw_capacity(room));
        if len > allowed {
            return self.over_budget(); // not read, not cached
        }
        #[cfg(test)]
        if let Some(h) = &self.before_read {
            h(path);
        }
        // the same handle, capped by what both limits still allow
        let mut bytes = Vec::with_capacity(len as usize);
        if let Err(e) = file.take(allowed + 1).read_to_end(&mut bytes) {
            return Embedded::Link(Some(format!("{}: {e}", path.display())));
        }
        if bytes.len() as u64 > allowed {
            return Embedded::Link(Some(format!(
                "{} changed while it was read (kept as a link)",
                path.display()
            )));
        }
        let uri: Arc<str> = format!("data:{mime};base64,{}", base64(&bytes)).into();
        drop(bytes);
        debug_assert!(self.used + uri.len() <= self.budget);
        self.used += uri.len();
        self.cache_bytes += uri.len();
        self.cache.insert(key, uri.clone());
        Embedded::Uri(uri)
    }
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n =
            (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// A reference to `r` from a page in `from_dir`, or `None` when either path cannot be
/// resolved on disk (the caller keeps the original reference). Both sides are canonical
/// (symlinks resolved, so `..` means what the filesystem means); the result is a relative
/// path when one exists, else a `file:` URL (e.g. another Windows drive), followed by the
/// original `?query#fragment`.
pub fn link_from(from_dir: &Path, r: &LocalRef) -> Option<String> {
    let from = std::fs::canonicalize(from_dir).ok()?;
    let to = std::fs::canonicalize(&r.path).ok()?;
    Some(format!("{}{}", relative_link(&from, &to), r.suffix))
}

/// Link between two canonical paths (no filesystem access).
fn relative_link(from: &Path, to: &Path) -> String {
    let fc: Vec<Component> = from.components().collect();
    let tc: Vec<Component> = to.components().collect();
    if fc.first() != tc.first() {
        return file_url(to);
    }
    let common = fc.iter().zip(&tc).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".into(); fc.len() - common];
    parts.extend(
        tc[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    encode_path(&parts.join("/"))
}

fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy();
    // Windows canonical paths are verbatim (`\\?\C:\…`, `\\?\UNC\server\…`)
    let s = s
        .strip_prefix(r"\\?\UNC\")
        .map(|r| format!(r"\\{r}"))
        .unwrap_or_else(|| s.strip_prefix(r"\\?\").unwrap_or(&s).to_string());
    let s = s.replace('\\', "/");
    if let Some(unc) = s.strip_prefix("//") {
        return format!("file://{}", encode_path(unc));
    }
    let s = if s.starts_with('/') { s } else { format!("/{s}") };
    format!("file://{}", encode_path(&s))
}

/// Percent-encodes what is not valid in a URL path: ASCII outside the unreserved/sub-delims
/// set (space, `%`, `#`, `?`, quotes, …) and control characters. Non-ASCII letters stay as
/// literal UTF-8, which HTML5 accepts in `src` (EX-12).
fn encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            'A'..='Z'
            | 'a'..='z'
            | '0'..='9'
            | '-'
            | '_'
            | '.'
            | '~'
            | '/'
            | ':'
            | '@'
            | '!'
            | '$'
            | '&'
            | '\''
            | '('
            | ')'
            | '*'
            | '+'
            | ','
            | ';'
            | '=' => out.push(ch),
            c if !c.is_ascii() && !c.is_whitespace() && !c.is_control() => out.push(c),
            c => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets(doc: &Path) -> Assets {
        Assets {
            doc_dir: doc.to_path_buf(),
            root_url: None,
            out_dir: doc.to_path_buf(),
        }
    }

    #[test]
    fn resolves_like_the_viewer_and_skips_remote() {
        let a = assets(Path::new("/docs"));
        let r = |s: &str| local_ref(&a, s);
        assert_eq!(r("images/a.png").unwrap().path, PathBuf::from("/docs/images/a.png"));
        assert_eq!(r("my%20pic.png").unwrap().path, PathBuf::from("/docs/my pic.png"));
        assert_eq!(r("мои/рис.png").unwrap().path, PathBuf::from("/docs/мои/рис.png"));
        assert_eq!(r("ф").unwrap().path, PathBuf::from("/docs/ф"));
        assert_eq!(r("https://e.org/x.png"), None);
        assert_eq!(r("data:image/png;base64,AA"), None);
        assert_eq!(r("//cdn.example/x.png"), None);
        let mut root = a.clone();
        root.root_url = Some("assets".into());
        assert_eq!(
            local_ref(&root, "/img/b.png").unwrap().path,
            PathBuf::from("/docs/assets/img/b.png")
        );
    }

    // Codex round 7: query and fragment are not part of the file name.
    #[test]
    fn query_and_fragment_are_kept_apart() {
        let a = assets(Path::new("/docs"));
        let r = local_ref(&a, "image.png?v=2#preview").unwrap();
        assert_eq!(r.path, PathBuf::from("/docs/image.png"));
        assert_eq!(r.suffix, "?v=2#preview");
        let r = local_ref(&a, "file:///tmp/a.png#x").unwrap();
        assert_eq!(r.suffix, "#x");
    }

    // Codex round 7: file: URL authorities.
    #[cfg(unix)]
    #[test]
    fn file_urls_with_authorities() {
        let a = assets(Path::new("/docs"));
        let p = |s: &str| local_ref(&a, s).map(|r| r.path);
        assert_eq!(p("file:///tmp/a.png"), Some(PathBuf::from("/tmp/a.png")));
        assert_eq!(p("file://localhost/tmp/a%20b.png"), Some(PathBuf::from("/tmp/a b.png")));
        assert_eq!(p("FILE:/tmp/a.png"), Some(PathBuf::from("/tmp/a.png")));
        assert_eq!(p("file://server/share/a.png"), None, "other hosts are not read");
    }

    #[cfg(windows)]
    #[test]
    fn file_urls_with_authorities() {
        let a = assets(Path::new(r"C:\docs"));
        let p = |s: &str| local_ref(&a, s).map(|r| r.path);
        assert_eq!(p("file:///C:/tmp/a.png"), Some(PathBuf::from("C:/tmp/a.png")));
        assert_eq!(p("file://localhost/C:/tmp/a.png"), Some(PathBuf::from("C:/tmp/a.png")));
        assert_eq!(
            p("file://server/share/a.png"),
            Some(PathBuf::from(r"\\server\share\a.png"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn relative_links_between_canonical_paths() {
        assert_eq!(
            relative_link(Path::new("/exports"), Path::new("/docs/images/a.png")),
            "../docs/images/a.png"
        );
        assert_eq!(
            relative_link(Path::new("/docs"), Path::new("/docs/a b/рис 1%#.png")),
            "a%20b/рис%201%25%23.png"
        );
        assert_eq!(relative_link(Path::new("/a/b/c"), Path::new("/a/x.png")), "../../x.png");
    }

    // Codex round 7: `..` after a symlink means what the filesystem means.
    #[cfg(unix)]
    #[test]
    fn symlinks_are_resolved_before_rebasing() {
        let d = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(d.path()).unwrap();
        std::fs::create_dir_all(root.join("media/sub")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(root.join("media/a.png"), b"x").unwrap();
        std::os::unix::fs::symlink(root.join("media/sub"), root.join("docs/link")).unwrap();
        let a = Assets {
            doc_dir: root.join("docs"),
            root_url: None,
            out_dir: root.join("out"),
        };
        let r = local_ref(&a, "link/../a.png#f").unwrap();
        let link = link_from(&a.out_dir, &r).unwrap();
        assert_eq!(link, "../media/a.png#f");
        // an unresolvable target keeps the original reference
        let missing = local_ref(&a, "nope.png").unwrap();
        assert_eq!(link_from(&a.out_dir, &missing), None);
    }

    // Codex round 7: bounded reads of regular files only.
    #[test]
    fn reads_are_bounded_and_regular_files_only() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.png");
        std::fs::write(&p, vec![1u8; 100]).unwrap();
        assert_eq!(read_bounded(&p, 100).unwrap().len(), 100);
        assert!(read_bounded(&p, 99).is_err());
        assert!(read_bounded(d.path(), 1000).is_err(), "directory");
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_named_png_does_not_block() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("pipe.png");
        let c = std::ffi::CString::new(p.to_string_lossy().as_bytes()).unwrap();
        // SAFETY: plain syscall with a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let t = std::time::Instant::now();
        assert!(data_uri(&p, 1 << 20).is_err());
        assert!(t.elapsed() < std::time::Duration::from_secs(2));
    }

    // Codex round 7: one encoding per file, an aggregate budget, cancellation.
    #[test]
    fn embedder_dedupes_bounds_and_stops_on_cancel() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.png");
        std::fs::write(&p, vec![7u8; 300]).unwrap();
        let mut e = Embedder::new(1 << 20, 1000);
        let Embedded::Uri(u1) = e.embed(&p, None) else { panic!() };
        let Embedded::Uri(u2) = e.embed(&p, None) else { panic!() };
        assert!(Arc::ptr_eq(&u1, &u2), "encoded once");
        assert_eq!(e.used(), 2 * u1.len());
        // third occurrence exceeds the 1000-byte budget: link, warned once
        assert!(matches!(e.embed(&p, None), Embedded::Link(Some(_))));
        assert_eq!(e.embed(&p, None), Embedded::Link(None));
        let c = JobControl::new();
        c.cancel();
        let mut e = Embedder::new(1 << 20, 1 << 20);
        assert_eq!(e.embed(&p, Some(&c)), Embedded::Link(None));
    }

    // Codex round 8: after the budget is exhausted, further distinct files are neither read,
    // encoded nor cached — memory stays bounded by the budget.
    #[test]
    fn distinct_files_after_budget_exhaustion_stay_bounded() {
        let d = tempfile::tempdir().unwrap();
        let budget = 2000;
        let mut e = Embedder::new(1 << 20, budget);
        let mut uris = 0;
        for i in 0..200 {
            let p = d.path().join(format!("img{i}.png"));
            std::fs::write(&p, vec![i as u8; 600]).unwrap();
            if matches!(e.embed(&p, None), Embedded::Uri(_)) {
                uris += 1;
            }
            assert!(e.cache_bytes() <= budget, "cache {} > budget", e.cache_bytes());
            assert!(e.used() <= budget);
        }
        assert_eq!(uris, 2, "600-byte files encode to ~822 bytes: two fit in 2000");
        assert!(e.cache.len() <= 2);
    }

    // Codex round 9: growth between the size check and the read never exceeds the budget.
    #[test]
    fn growth_between_check_and_read_is_rejected() {
        use std::io::Write;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("g.png");
        std::fs::write(&p, vec![1u8; 300]).unwrap();
        let mut e = Embedder::new(1 << 20, 1000);
        e.before_read = Some(Box::new(|p: &Path| {
            let mut f = std::fs::OpenOptions::new().append(true).open(p).unwrap();
            f.write_all(&[2u8; 5000]).unwrap();
        }));
        assert!(matches!(e.embed(&p, None), Embedded::Link(Some(w)) if w.contains("changed")));
        assert_eq!(e.used(), 0);
        assert_eq!(e.cache_bytes(), 0);
        assert!(e.cache.is_empty());
    }

    // Codex round 9: rejected references are not cached.
    #[test]
    fn rejected_references_are_not_cached() {
        let d = tempfile::tempdir().unwrap();
        let mut e = Embedder::new(10, 1 << 20);
        for i in 0..500 {
            let missing = d.path().join(format!("missing{i}.png"));
            let unsupported = d.path().join(format!("x{i}.svg"));
            let big = d.path().join(format!("big{i}.png"));
            std::fs::write(&big, [0u8; 64]).unwrap();
            for p in [missing, unsupported, big] {
                assert!(matches!(e.embed(&p, None), Embedded::Link(Some(_))));
            }
        }
        assert!(e.cache.is_empty());
        assert_eq!(e.cache_bytes(), 0);
    }

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}

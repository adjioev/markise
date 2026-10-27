//! Resolution of resource paths (images) against a document folder and an optional root for
//! absolute paths — format-neutral; a format supplies the root
//! ([`crate::FormatDoc::resource_root`], Markdown: front matter `typora-root-url`, spec 01
//! R8.1).

use std::path::{Path, PathBuf};

/// `true` when `p` starts with a URL scheme of two or more characters (`http:`, `data:` …;
/// a drive letter `C:` is not one).
pub fn has_scheme(p: &str) -> bool {
    p.split_once(':').is_some_and(|(s, _)| {
        s.len() >= 2
            && s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.'))
    })
}

/// An absolute path on any platform (`/x`, `\\server`, `C:\x`, `C:/x`).
pub fn is_absolute_like(p: &str) -> bool {
    p.starts_with('/')
        || p.starts_with('\\')
        || (p.len() >= 3 && p.as_bytes()[1] == b':' && matches!(p.as_bytes()[2], b'\\' | b'/'))
}

/// Resolves an image `src` for a document in `doc_dir` (R8.1): URLs as they are; a
/// root-relative `/x` against `root` when given; absolute paths as they are; anything else
/// relative to the document folder.
pub fn resolve(doc_dir: &Path, root: Option<&str>, src: &str) -> PathBuf {
    if has_scheme(src) && !(src.len() >= 2 && src.as_bytes()[1] == b':') {
        return PathBuf::from(src);
    }
    if let Some(root) = root
        && src.starts_with('/')
    {
        let root_path = if is_absolute_like(root) {
            PathBuf::from(root)
        } else {
            doc_dir.join(root.trim_start_matches("./"))
        };
        return root_path.join(src.trim_start_matches('/'));
    }
    if is_absolute_like(src) {
        return PathBuf::from(src);
    }
    doc_dir.join(src.trim_start_matches("./"))
}

/// A local file referenced by an image source: the path plus the URL suffix (`?query` /
/// `#fragment`) that is not part of the file name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalFile {
    pub path: PathBuf,
    pub suffix: String,
}

/// The one resolver of document resource references (viewer, HTML export, PDF/PNG export):
/// `None` for remote URLs, `data:`, protocol-relative `//host/…`, in-page anchors and `file:`
/// URLs of another host; otherwise the local file. `?query#fragment` is split off first, the
/// path is percent-decoded, `file:` URLs are parsed with their authority (empty or
/// `localhost` → local; on Windows another host → UNC `\\host\share\…`), and the rest
/// resolves like [`resolve`] (root-relative `/x` against the resource root, R8.1).
pub fn local_file(doc_dir: &Path, root: Option<&str>, src: &str) -> Option<LocalFile> {
    local_file_with(doc_dir, root, src, &|p| std::fs::metadata(p).is_ok())
}

/// [`local_file`] with an injectable existence check (tests).
#[doc(hidden)]
pub fn local_file_with(
    doc_dir: &Path,
    root: Option<&str>,
    src: &str,
    exists: &dyn Fn(&Path) -> bool,
) -> Option<LocalFile> {
    let s = src.trim();
    if s.is_empty() || s.starts_with('#') {
        return None;
    }
    // native Windows paths (`\\?\C:\…`, `\\?\UNC\…`, `\\.\…`, `\\server\share\…`, `C:\…`) are
    // file system paths, not URLs: no suffix splitting, no decoding
    if is_native_windows_path(s) {
        return Some(LocalFile {
            path: PathBuf::from(s),
            suffix: String::new(),
        });
    }
    if s.starts_with("//") {
        return None; // protocol-relative URL
    }
    if s.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("file:")) {
        let (url, suffix) = split_suffix(&s[5..]);
        return file_url_path(url).map(|path| LocalFile {
            path,
            suffix: suffix.to_string(),
        });
    }
    let drive = s.len() >= 2 && s.as_bytes()[1] == b':' && s.as_bytes()[0].is_ascii_alphabetic();
    if !drive
        && s.split_once(':').is_some_and(|(sc, _)| {
            !sc.is_empty()
                && sc
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.'))
        })
    {
        return None;
    }
    // Candidates, first existing wins (compatible with files whose names contain `%`, `?` or
    // `#` literally): the reference as written, then without `?query#fragment`, then that
    // percent-decoded (only when the decoding is valid UTF-8). When none exists, the URL
    // reading (split and decoded) is returned. Plain references need no file system access.
    let (path, suffix) = split_suffix(s);
    let decoded = percent_decode_strict(path);
    let ambiguous = !suffix.is_empty() || decoded.as_deref().is_some_and(|d| d != path);
    if ambiguous {
        let whole = resolve(doc_dir, root, s);
        if exists(&whole) {
            return Some(LocalFile {
                path: whole,
                suffix: String::new(),
            });
        }
        let literal = resolve(doc_dir, root, path);
        if exists(&literal) {
            return Some(LocalFile {
                path: literal,
                suffix: suffix.to_string(),
            });
        }
    }
    Some(LocalFile {
        path: resolve(doc_dir, root, decoded.as_deref().unwrap_or(path)),
        suffix: suffix.to_string(),
    })
}

/// `\\?\…`, `\\.\…`, `\\server\share…` or `C:\…` (backslash forms; `C:/…` is handled as a
/// drive path by the URL branch too, but this keeps `?` in extended-length paths intact).
fn is_native_windows_path(s: &str) -> bool {
    let b = s.as_bytes();
    s.starts_with("\\\\") || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\')
}

fn split_suffix(s: &str) -> (&str, &str) {
    match s.find(['?', '#']) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    }
}

fn file_url_path(url: &str) -> Option<PathBuf> {
    let (authority, path) = match url.strip_prefix("//") {
        Some(rest) => match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        },
        None => ("", url),
    };
    let path = percent_decode(path);
    if authority.is_empty() || authority.eq_ignore_ascii_case("localhost") {
        if path.is_empty() {
            return None;
        }
        if cfg!(windows) {
            let p = path.strip_prefix('/').unwrap_or(&path);
            let drive = p.len() >= 2 && p.as_bytes()[1] == b':';
            return Some(PathBuf::from(if drive { p.to_string() } else { path.clone() }));
        }
        return Some(PathBuf::from(path));
    }
    if cfg!(windows) {
        return Some(PathBuf::from(format!(
            r"\\{}{}",
            percent_decode(authority),
            path.replace('/', "\\")
        )));
    }
    None
}

/// Percent-decodes `%XX` (bytes; never slices inside UTF-8; malformed escapes stay literal);
/// `None` when the decoded bytes are not valid UTF-8 (never lossy).
pub fn percent_decode_strict(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// [`percent_decode_strict`], falling back to the input when the result is not UTF-8.
pub fn percent_decode(s: &str) -> String {
    percent_decode_strict(s).unwrap_or_else(|| s.to_string())
}

/// Shared test table for [`local_file`] (doc folder `/docs`): (source, resource root,
/// expected path relative to the platform root or `None`, expected suffix). Consumers (HTML
/// and PDF/PNG export) run it through their own entry points.
#[doc(hidden)]
pub const RESOLVER_CASES: &[(&str, Option<&str>, Option<&str>, &str)] = &[
    ("images/a.png", None, Some("/docs/images/a.png"), ""),
    ("./a.png", None, Some("/docs/a.png"), ""),
    ("my%20pic.png", None, Some("/docs/my pic.png"), ""),
    ("мои/рис 1.png", None, Some("/docs/мои/рис 1.png"), ""),
    ("image.png?v=2#preview", None, Some("/docs/image.png"), "?v=2#preview"),
    ("/photo.png", Some("./media"), Some("/docs/media/photo.png"), ""),
    ("/photo.png", Some("media"), Some("/docs/media/photo.png"), ""),
    ("https://e.org/x.png", None, None, ""),
    ("data:image/png;base64,AA", None, None, ""),
    ("//cdn.example/x.png", None, None, ""),
    ("#anchor", None, None, ""),
];

/// Opens `path` for reading only if it is a regular file, without blocking: a FIFO, device or
/// directory (also one swapped in after the check) is rejected instead of hanging the caller.
/// Returns the file and its size from the opened handle.
pub fn open_regular(path: &Path) -> std::io::Result<(std::fs::File, u64)> {
    use std::io::{Error, ErrorKind};
    let not_regular = || {
        Error::new(
            ErrorKind::InvalidInput,
            format!("{} is not a regular file", path.display()),
        )
    };
    if !std::fs::metadata(path)?.is_file() {
        return Err(not_regular());
    }
    #[cfg(windows)]
    {
        // reparse points other than symbolic links (junctions, app-exec links, cloud
        // placeholders …) are not followed
        use std::os::windows::fs::MetadataExt;
        let lm = std::fs::symlink_metadata(path)?;
        if lm.file_attributes() & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT != 0
            && !lm.file_type().is_symlink()
        {
            return Err(not_regular());
        }
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NONBLOCK);
    }
    let f = opts.open(path)?;
    let m = f.metadata()?;
    if !m.is_file() {
        return Err(not_regular());
    }
    #[cfg(windows)]
    {
        // `is_file` on Windows only means "not a directory or symlink": the handle's type tells
        // disk files from pipes and character devices
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_DISK, GetFileType};
        // SAFETY: `f` owns a valid handle for the duration of the call.
        if unsafe { GetFileType(f.as_raw_handle() as _) } != FILE_TYPE_DISK {
            return Err(not_regular());
        }
    }
    Ok((f, m.len()))
}

/// Reads the regular file `path` (see [`open_regular`]) of at most `max` bytes. The limit is
/// checked on the opened handle and enforced while reading, so a file that grows or is
/// replaced after the check cannot exceed it. An oversized file is an `InvalidData` error.
pub fn read_regular(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Error, ErrorKind, Read};
    let too_large = || {
        Error::new(
            ErrorKind::InvalidData,
            format!("{} is larger than {max} bytes", path.display()),
        )
    };
    let (f, len) = open_regular(path)?;
    if len > max {
        return Err(too_large());
    }
    let mut buf = Vec::with_capacity(len as usize);
    f.take(max.saturating_add(1)).read_to_end(&mut buf)?;
    if buf.len() as u64 > max {
        return Err(too_large());
    }
    Ok(buf)
}

#[cfg(test)]
mod read_tests {
    use super::*;

    #[test]
    fn resolver_table() {
        for &(src, root, want, suffix) in RESOLVER_CASES {
            let got = local_file(Path::new("/docs"), root, src);
            match want {
                None => assert_eq!(got, None, "{src}"),
                Some(w) => {
                    let got = got.unwrap_or_else(|| panic!("{src}"));
                    assert_eq!(
                        got.path,
                        Path::new("/docs").join(w.trim_start_matches("/docs/")),
                        "{src}"
                    );
                    assert_eq!(got.suffix, suffix, "{src}");
                }
            }
        }
        #[cfg(unix)]
        {
            let p = |s: &str| local_file(Path::new("/docs"), None, s).map(|r| r.path);
            assert_eq!(p("file:///tmp/a.png"), Some(PathBuf::from("/tmp/a.png")));
            assert_eq!(p("file://localhost/tmp/a%20b.png"), Some(PathBuf::from("/tmp/a b.png")));
            assert_eq!(p("file://server/share/a.png"), None);
        }
        assert_eq!(percent_decode("%€.png"), "%€.png");
        assert_eq!(percent_decode("%e2%82%ac"), "€");
        assert_eq!(percent_decode("%AB.png"), "%AB.png", "never lossy");
        assert_eq!(percent_decode_strict("%AB"), None);
    }

    // Codex round 10: literal names win; decoding only as a fallback.
    #[test]
    fn literal_percent_names_win() {
        let d = Path::new("/docs");
        let both = |p: &Path| p == d.join("photo%20final.png") || p == d.join("photo final.png");
        let r = local_file_with(d, None, "photo%20final.png", &both).unwrap();
        assert_eq!(r.path, d.join("photo%20final.png"), "literal wins when both exist");
        let decoded_only = |p: &Path| p == d.join("photo final.png");
        let r = local_file_with(d, None, "photo%20final.png", &decoded_only).unwrap();
        assert_eq!(r.path, d.join("photo final.png"));
        let hash = |p: &Path| p == d.join("C# notes.png");
        let r = local_file_with(d, None, "C# notes.png", &hash).unwrap();
        assert_eq!((r.path, r.suffix.as_str()), (d.join("C# notes.png"), ""));
        let none = |_: &Path| false;
        let r = local_file_with(d, None, "%AB.png", &none).unwrap();
        assert_eq!(r.path, d.join("%AB.png"), "invalid UTF-8 escapes are not decoded");
    }

    // Codex round 10: Windows namespace paths are not URLs.
    #[test]
    fn windows_namespace_paths_pass_through() {
        let d = Path::new("/docs");
        let none = |_: &Path| false;
        for p in [
            r"\\?\C:\images\a.png",
            r"\\?\UNC\server\share\a.png",
            r"\\.\C:\x.png",
            r"\\server\share\a#1.png",
            r"C:\images\a?.png",
        ] {
            let r = local_file_with(d, None, p, &none).unwrap();
            assert_eq!(r.path, PathBuf::from(p), "{p}");
            assert_eq!(r.suffix, "", "{p}");
        }
    }

    #[test]
    fn regular_files_only_and_bounded() {
        let d = std::env::temp_dir().join(format!("markise-doc-read-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("a.bin");
        std::fs::write(&p, vec![1u8; 100]).unwrap();
        assert_eq!(read_regular(&p, 100).unwrap().len(), 100);
        assert_eq!(
            read_regular(&p, 99).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        assert_eq!(
            read_regular(&d, 1000).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        #[cfg(unix)]
        {
            let fifo = d.join("pipe.png");
            let c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
            // SAFETY: plain syscall with a valid NUL-terminated path.
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
            let t = std::time::Instant::now();
            assert!(read_regular(&fifo, 1 << 20).is_err());
            assert!(t.elapsed() < std::time::Duration::from_secs(2), "a FIFO must not block");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    // Codex round 9: Windows pipes and devices are not regular files.
    #[cfg(windows)]
    #[test]
    fn windows_pipes_and_devices_are_rejected() {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
        use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_TYPE_BYTE, PIPE_WAIT};
        let t = std::time::Instant::now();
        for dev in ["NUL", "CON", r"\\.\NUL"] {
            assert!(read_regular(Path::new(dev), 1 << 20).is_err(), "{dev}");
        }
        let name = format!(r"\\.\pipe\markise-test-{}", std::process::id());
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: valid NUL-terminated name; the handle is closed below.
        let h = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_WAIT,
                1,
                4096,
                4096,
                0,
                std::ptr::null(),
            )
        };
        assert_ne!(h, INVALID_HANDLE_VALUE);
        assert!(read_regular(Path::new(&name), 1 << 20).is_err(), "named pipe");
        // SAFETY: closing our own handle once.
        unsafe { CloseHandle(h) };
        assert!(t.elapsed() < std::time::Duration::from_secs(5), "nothing blocked");
    }
}

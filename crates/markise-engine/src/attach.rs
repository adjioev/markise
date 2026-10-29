//! Pasted and dropped images and files (spec 04 §7.1–§7.3, spec 05 §4.4, spec 06 §14
//! `attachmentFolderPath` and V14.4 naming).
//!
//! The front end turns a paste (clipboard image data, a copied file) or a drop into a
//! [`Request`]; [`spawn`] runs it on a worker thread (encoding, copying and every other file
//! access happen there, never on the UI thread) and returns the Markdown to insert, one line
//! per item: `![](assets/Pasted%20image%2020261029101500.png)` for images, `[name](path)` for
//! other files.
//!
//! **Where files go** ([`image_folder`]): a folder next to the note (default `assets`), a fixed
//! folder below the root, or the note's own folder — or, in an Obsidian vault that sets
//! `attachmentFolderPath`, that folder. The root is the vault (the opened folder) when the
//! note is inside it, else the note's folder. A never-saved document has no folder: its pasted
//! images go to the app-data folder for user images (spec 04 §7.1, spec 05 §1) and are linked
//! with an absolute path; they stay there when the document is saved later.
//!
//! **Safety** (the threat model of [`crate::vault`] applies): files are only created below the
//! root directory *handle* — the vault's, opened at mount, when there is one — with `cap-std`:
//! each folder is created and entered without following symbolic links, the file is created
//! exclusively without following one, so nothing is ever overwritten and a component swapped
//! for a link makes the write fail instead of escaping. Folder names are checked first: no
//! `..`, no hidden components (so never `.obsidian/`), names valid on every platform. A name
//! that is taken gets a numeric suffix (`Pasted image 20261029101500 1.png`, V14.4).
//!
//! **Limits:** files above [`MAX_BYTES`] are refused with a message. Clipboard images above
//! [`MAX_PIXELS`] are refused before anything is allocated; the PNG is streamed row by row into
//! the new file through a writer that aborts past [`MAX_BYTES`] (the partial file is removed),
//! so no second copy of the pixels and no whole PNG is held in memory; at most two encodes run
//! at once.
//!
//! Undo of the inserted text does not delete the written file (a file is never deleted
//! behind the user's back); the file watcher reports the new file like any other.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use crate::Wake;
use crate::settings::{ImageLocation, ImageSettings};

/// Largest file written or copied (encoded size for pasted images).
pub const MAX_BYTES: u64 = 50 << 20;
/// Largest clipboard image encoded (pixels).
pub const MAX_PIXELS: u64 = 100_000_000;
/// Name suffixes tried before giving up (`name 1.png` … `name 999.png`).
const MAX_SUFFIX: u32 = 999;

/// File extensions inserted as images (`![](…)`); anything else becomes a link.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "avif", "tif", "tiff", "ico", "heic",
];

/// Whether `name` (a file name or path) has an image extension.
pub fn is_image(name: &str) -> bool {
    let n = name.rsplit(['/', '\\']).next().unwrap_or(name);
    n.rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Whether pasted text is a single web image URL (`https://host/x.png?size=2`).
pub fn is_image_url(text: &str) -> bool {
    let t = text.trim();
    let rest = t
        .strip_prefix("https://")
        .or_else(|| t.strip_prefix("http://"))
        .unwrap_or("");
    if rest.is_empty() || t.chars().any(|c| c.is_whitespace() || c.is_control()) || t.contains(['(', ')', '<', '>']) {
        return false;
    }
    let path = rest.split(['?', '#']).next().unwrap_or("");
    path.contains('/') && is_image(path)
}

/// Current local time as `YYYYMMDDHHMMSS` (pasted image names, V14.4).
pub fn stamp_now() -> String {
    chrono::Local::now().format("%Y%m%d%H%M%S").to_string()
}

/// The name of a pasted clipboard image (Obsidian-compatible, V14.4).
pub fn pasted_image_name(stamp: &str) -> String {
    format!("Pasted image {stamp}.png")
}

/// The `n`-th candidate for `name`: the name itself, then `stem n.ext` (V14.4).
pub fn candidate_name(name: &str, n: u32) -> String {
    if n == 0 {
        return name.to_string();
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem} {n}.{ext}"),
        _ => format!("{name} {n}"),
    }
}

/// Joins `/`-separated folder paths (either may be empty).
fn join(a: &str, b: &str) -> String {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b.to_string(),
        (_, true) => a.to_string(),
        _ => format!("{a}/{b}"),
    }
}

/// `abs` below `root` as a `/`-separated path spelled as on disk (`Some("")` for the root
/// itself, `None` outside it).
pub fn rel_below(root: &Path, abs: &Path) -> Option<String> {
    let r = abs.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for c in r.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

/// Why `folder` (`/`-separated, below the root) cannot hold attachments: empty or relative
/// parts, hidden folders (never `.obsidian/`), names not valid on every platform.
pub fn check_folder(folder: &str) -> Result<(), String> {
    if folder.is_empty() {
        return Ok(());
    }
    for seg in folder.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(format!("The image folder “{folder}” has an empty or relative part."));
        }
        if seg.starts_with('.') {
            return Err(format!(
                "The image folder “{folder}” is hidden; files are never written into hidden folders such as .obsidian."
            ));
        }
    }
    crate::vault::check_new_note(folder).map_err(|e| format!("The image folder “{folder}”: {e}"))
}

/// The image folder (below `root`, `/`-separated) for the note `note` (a file below `root`):
/// the Obsidian attachment folder when `s.obsidian` and the vault sets one
/// (`obsidian_folder`, V14.2), else `s.location`. Checked with [`check_folder`].
pub fn image_folder(
    s: &ImageSettings,
    root: &Path,
    note: &Path,
    obsidian_folder: Option<&str>,
) -> Result<String, String> {
    let note_dir = note.parent().unwrap_or(root);
    let note_rel = rel_below(root, note_dir).ok_or("The note is outside the folder.")?;
    let clean = |f: &str| f.trim().trim_matches('/').to_string();
    let folder = match obsidian_folder.filter(|_| s.obsidian) {
        Some(o) => {
            let o = o.trim();
            if o == "/" {
                String::new()
            } else if o == "." || o == "./" {
                note_rel
            } else if let Some(sub) = o.strip_prefix("./") {
                join(&note_rel, &clean(sub))
            } else {
                clean(o)
            }
        }
        None => match s.location {
            ImageLocation::NoteFolder => join(&note_rel, &clean(&s.folder)),
            ImageLocation::RootFolder => clean(&s.folder),
            ImageLocation::SameFolder => note_rel,
        },
    };
    check_folder(&folder)?;
    Ok(folder)
}

/// How a destination is written in Markdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DestForm {
    /// Percent-encoded: the V2.6.3 set (space, `%`, `#`, `(`, `)`, `<`, `>`, `^`, `|`, `[`, `]`,
    /// `\\`, controls) plus `?`, which the resource resolver would otherwise split off as a
    /// query (`markise_doc::resource::local_file`); readable by Obsidian and Typora.
    Percent,
    /// Wrapped in `<…>` (CommonMark), only `<`, `>` and line breaks escaped.
    Angle,
}

/// `path` (`/`-separated) as a Markdown link destination.
pub fn markdown_dest(path: &str, form: DestForm) -> String {
    match form {
        DestForm::Percent => crate::vault::percent_encode_path(path).replace('?', "%3F"),
        DestForm::Angle => {
            let mut out = String::with_capacity(path.len() + 2);
            out.push('<');
            for c in path.chars() {
                match c {
                    '<' => out.push_str("%3C"),
                    '>' => out.push_str("%3E"),
                    '\n' => out.push_str("%0A"),
                    '\r' => out.push_str("%0D"),
                    c => out.push(c),
                }
            }
            out.push('>');
            out
        }
    }
}

/// `s` without a Windows verbatim prefix: `\\?\C:\x` → `C:\x`, `\\?\UNC\srv\share` →
/// `\\srv\share` (what `canonicalize` returns on Windows; such paths are not valid in links).
pub fn simplify_verbatim(s: &str) -> String {
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.len() >= 2 && rest.as_bytes()[1] == b':' && rest.as_bytes()[0].is_ascii_alphabetic() => {
            rest.to_string()
        }
        _ => s.to_string(),
    }
}

/// [`simplify_verbatim`] for paths (identity outside Windows).
fn simplify(p: &Path) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(simplify_verbatim(&p.to_string_lossy()))
    } else {
        p.to_path_buf()
    }
}

/// `p` with `/` separators (Windows backslashes turned around).
fn slashed(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    if cfg!(windows) { s.replace('\\', "/") } else { s }
}

/// A `file:` URL for the absolute path `file` (path percent-encoded like destinations):
/// `file:///home/a/x.png`, `file:///C:/a/x.png`, `file://server/share/x.png`.
pub fn file_url(file: &Path) -> String {
    let s = slashed(&simplify(file));
    let enc = |p: &str| markdown_dest(p, DestForm::Percent);
    if let Some(unc) = s.strip_prefix("//") {
        return format!("file://{}", enc(unc));
    }
    if s.starts_with('/') {
        format!("file://{}", enc(&s))
    } else {
        format!("file:///{}", enc(&s))
    }
}

/// The resolved resource root (`typora-root-url`, spec 01 R8.1) of a note in `note_dir`:
/// absolute as written, otherwise relative to the note's folder.
pub fn resource_root_dir(note_dir: Option<&Path>, root: &str) -> Option<PathBuf> {
    let root = root.trim();
    if root.is_empty() {
        return None;
    }
    let p = if markise_doc::resource::is_absolute_like(root) {
        PathBuf::from(root)
    } else {
        note_dir?.join(root.trim_start_matches("./"))
    };
    // `..` and `.` resolved lexically, so files below the root are recognised
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                if !out.pop() {
                    return Some(p);
                }
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    Some(out)
}

/// The Markdown destination (encoded) written for `file` from a note in `note_dir`:
/// - relative to the note's folder when `relative` and a relative path exists;
/// - else, when the document has a resource root (`root`, resolved: [`resource_root_dir`]),
///   root-relative (`/assets/x.png`) for a file below it, a `file:` URL otherwise (a plain
///   absolute path would be read as root-relative);
/// - else the absolute path (`/home/a/x.png`; `C:/a/x.png` on Windows; a `file:` URL for UNC
///   paths).
///
/// Windows verbatim prefixes are removed first. Every form resolves back to `file` through
/// `markise_doc::resource::local_file`.
pub fn destination(note_dir: Option<&Path>, root: Option<&Path>, file: &Path, relative: bool) -> String {
    let enc = |p: &str| markdown_dest(p, DestForm::Percent);
    let file = simplify(file);
    if relative
        && let Some(dir) = note_dir.map(simplify)
        && let Some(r) = crate::vault::relative_path(&dir, &file)
    {
        return enc(&r);
    }
    if let Some(root) = root.map(simplify) {
        return match rel_below(&root, &file) {
            Some(r) => format!("/{}", enc(&r)),
            None => file_url(&file),
        };
    }
    let s = slashed(&file);
    if s.starts_with("//") { file_url(&file) } else { enc(&s) }
}

/// A Markdown label with its syntax characters escaped.
fn escape_label(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '[' | ']' | '*' | '_' | '`' | '<' | '>' | '!' | '&') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The Markdown inserted for `file`: an image for image files, else a link labelled with the
/// file name; the destination per [`destination`].
pub fn markdown_for(note_dir: Option<&Path>, root: Option<&Path>, file: &Path, relative: bool) -> String {
    let dest = destination(note_dir, root, file, relative);
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if is_image(&name) {
        format!("![]({dest})")
    } else {
        format!("[{}]({dest})", escape_label(&name))
    }
}

/// Creates `folder/name` below `root_dir` (the directory opened for `root_path`) and fills it
/// through `fill`. Folders are created and entered without following links; the file is
/// created exclusively without following one, taking the first free [`candidate_name`]. A file
/// whose filling fails is removed again (it was just created by this call). Returns the
/// path below `root_path`. File I/O: worker threads only.
pub fn create_unique(
    root_dir: &cap_std::fs::Dir,
    root_path: &Path,
    folder: &str,
    name: &str,
    fill: &mut dyn FnMut(&mut cap_std::fs::File) -> std::io::Result<()>,
) -> Result<PathBuf, String> {
    use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
    check_folder(folder)?;
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        return Err(format!("“{name}” is not a usable file name."));
    }
    let mut dir = root_dir.try_clone().map_err(|e| format!("the folder: {e}"))?;
    let mut path = root_path.to_path_buf();
    if !folder.is_empty() {
        for seg in folder.split('/') {
            match dir.create_dir(seg) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(format!("Cannot create the folder “{seg}”: {e}")),
            }
            dir = dir.open_dir_nofollow(seg).map_err(|e| {
                if is_link_error(&e) {
                    format!("“{seg}” is a symbolic link; files are not written through links.")
                } else {
                    format!("“{seg}”: {e}")
                }
            })?;
            path.push(seg);
        }
    }
    let mut opts = cap_std::fs::OpenOptions::new();
    opts.write(true).create_new(true).follow(FollowSymlinks::No);
    for n in 0..=MAX_SUFFIX {
        let cand = candidate_name(name, n);
        match dir.open_with(&cand, &opts) {
            Ok(mut f) => {
                if let Err(e) = fill(&mut f).and_then(|_| f.flush()) {
                    drop(f);
                    let _ = dir.remove_file(&cand);
                    return Err(format!("Cannot write “{cand}”: {e}"));
                }
                path.push(&cand);
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) if is_link_error(&e) => continue, // a link of that name: taken
            Err(e) => return Err(format!("Cannot create “{cand}”: {e}")),
        }
    }
    Err(format!("No free name for “{name}” in the image folder."))
}

fn is_link_error(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    if e.raw_os_error() == Some(libc::ELOOP) || e.raw_os_error() == Some(libc::ENOTDIR) {
        return true;
    }
    e.to_string().contains("symlink")
}

/// One pasted or dropped item.
#[derive(Clone, Debug)]
pub enum Item {
    /// Clipboard image data (premultiplied, as egui delivers it): written as a PNG named
    /// [`pasted_image_name`].
    Image(Arc<epaint::ColorImage>),
    /// A file (dropped, or copied in a file manager and pasted).
    File(PathBuf),
}

/// What [`spawn`] needs: the items and where the note is.
#[derive(Clone, Debug)]
pub struct Request {
    pub items: Vec<Item>,
    pub settings: ImageSettings,
    /// The note's file; `None` for a never-saved document.
    pub note: Option<PathBuf>,
    /// The vault (opened folder) root and the directory handle opened at mount, when known.
    pub vault: Option<(PathBuf, Option<Arc<cap_std::fs::Dir>>)>,
    /// The vault's `attachmentFolderPath` (an Obsidian vault that sets it).
    pub obsidian_folder: Option<String>,
    /// App-data folder for images pasted into a never-saved document (`None`: refused).
    pub user_images: Option<PathBuf>,
    /// `YYYYMMDDHHMMSS` for pasted image names ([`stamp_now`]; fixed in tests).
    pub stamp: String,
    /// The document's resource root as written (`typora-root-url`), if any.
    pub resource_root: Option<String>,
}

/// The result of a [`Request`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Markdown per item that succeeded, in item order (for the note's location at request
    /// time; see [`Outcome::render`]).
    pub inserts: Vec<String>,
    /// The file each insert points to (the written copy, or the original when linked in place).
    pub targets: Vec<PathBuf>,
    /// Files created.
    pub written: Vec<PathBuf>,
    /// One message per item that failed.
    pub errors: Vec<String>,
}

impl Outcome {
    /// The Markdown to insert: one line per item.
    pub fn text(&self) -> String {
        self.inserts.join("\n")
    }

    /// The Markdown to insert for a note that is now in `note_dir` (it may have been saved
    /// under another name meanwhile), with the resolved resource root `root` and the
    /// relative-paths setting `relative`: the links are generated again from [`Self::targets`]
    /// with [`markdown_for`], so they resolve to the files wherever the note is now.
    pub fn render(&self, note_dir: Option<&Path>, root: Option<&Path>, relative: bool) -> String {
        let relative = relative && note_dir.is_some();
        self.targets
            .iter()
            .map(|t| markdown_for(note_dir, root, t, relative))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Where new files of a request go.
struct Target {
    dir: cap_std::fs::Dir,
    root: PathBuf,
    folder: String,
}

/// Opens the target of `req` (file I/O: worker threads only).
fn target(req: &Request) -> Result<Target, String> {
    let Some(note) = req.note.as_deref() else {
        let root = req
            .user_images
            .clone()
            .ok_or("Save the document first: it has no folder for images yet.")?;
        std::fs::create_dir_all(&root).map_err(|e| format!("Cannot create {}: {e}", root.display()))?;
        let dir = crate::vault::open_root(&root).map_err(|e| format!("Cannot open {}: {e}", root.display()))?;
        return Ok(Target {
            dir,
            root,
            folder: String::new(),
        });
    };
    let note_dir = note.parent().ok_or("The note has no folder.")?;
    let in_vault = req.vault.as_ref().filter(|(root, _)| note.starts_with(root));
    let (root, handle) = match in_vault {
        Some((root, handle)) => (root.clone(), handle.clone()),
        None => (note_dir.to_path_buf(), None),
    };
    let obsidian = in_vault.and(req.obsidian_folder.as_deref());
    let folder = image_folder(&req.settings, &root, note, obsidian)?;
    let dir = match handle {
        Some(h) => h.try_clone().map_err(|e| format!("the folder: {e}"))?,
        None => crate::vault::open_root(&root).map_err(|e| format!("Cannot open {}: {e}", root.display()))?,
    };
    Ok(Target { dir, root, folder })
}

/// Checks clipboard pixels against the dimension budget before anything is allocated.
fn check_dimensions(img: &epaint::ColorImage) -> Result<(), String> {
    let [w, h] = img.size;
    if w > u32::MAX as usize || h > u32::MAX as usize || (w as u64).saturating_mul(h as u64) > MAX_PIXELS {
        return Err(format!("The clipboard image is too large ({w}×{h} pixels)."));
    }
    if w == 0 || h == 0 || img.pixels.len() != w * h {
        return Err("The clipboard image is empty.".into());
    }
    Ok(())
}

/// Concurrent PNG encodes (each holds a row buffer and the encoder's state; the pixels belong
/// to the paste event).
const ENCODE_SLOTS: usize = 2;
static ENCODES: (std::sync::Mutex<usize>, std::sync::Condvar) = (std::sync::Mutex::new(0), std::sync::Condvar::new());

/// One of the [`ENCODE_SLOTS`]; released on drop.
struct EncodeSlot;

impl EncodeSlot {
    fn acquire() -> EncodeSlot {
        let (m, cv) = &ENCODES;
        let mut n = m.lock().unwrap_or_else(|e| e.into_inner());
        while *n >= ENCODE_SLOTS {
            n = cv.wait(n).unwrap_or_else(|e| e.into_inner());
        }
        *n += 1;
        EncodeSlot
    }
}

impl Drop for EncodeSlot {
    fn drop(&mut self) {
        let (m, cv) = &ENCODES;
        let mut n = m.lock().unwrap_or_else(|e| e.into_inner());
        *n = n.saturating_sub(1);
        cv.notify_one();
    }
}

/// A writer that fails once more than `left` bytes were written (the encoded file stays below
/// [`MAX_BYTES`] without buffering it whole). `over` records that the limit was hit (the
/// encoder may wrap the error in its own kind).
struct Limited<'a> {
    inner: &'a mut dyn Write,
    left: u64,
    over: std::rc::Rc<std::cell::Cell<bool>>,
}

const TOO_LARGE: &str = "the image is larger than the size limit";

impl Write for Limited<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() as u64 > self.left {
            self.over.set(true);
            return Err(std::io::Error::new(std::io::ErrorKind::FileTooLarge, TOO_LARGE));
        }
        let n = self.inner.write(buf)?;
        self.left -= n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Streams clipboard pixels as PNG into `out`, one row at a time (premultiplied → straight
/// alpha per row; no second copy of the image, no buffered PNG). Fails past [`MAX_BYTES`].
fn encode_png_into(img: &epaint::ColorImage, out: &mut dyn Write, limit: u64) -> std::io::Result<()> {
    let [w, h] = img.size;
    let over = std::rc::Rc::new(std::cell::Cell::new(false));
    let r = (|| -> std::io::Result<()> {
        let limited = Limited {
            inner: out,
            left: limit,
            over: over.clone(),
        };
        let mut enc = png::Encoder::new(limited, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let io = |e: png::EncodingError| match e {
            png::EncodingError::IoError(e) => e,
            e => std::io::Error::other(e.to_string()),
        };
        let mut writer = enc.write_header().map_err(io)?;
        let mut stream = writer.stream_writer().map_err(io)?;
        let mut row = Vec::with_capacity(w * 4);
        for y in 0..h {
            row.clear();
            for c in &img.pixels[y * w..(y + 1) * w] {
                row.extend_from_slice(&c.to_srgba_unmultiplied());
            }
            stream.write_all(&row)?;
        }
        stream.finish().map_err(io)?;
        writer.finish().map_err(io)
    })();
    if over.get() {
        return Err(std::io::Error::new(std::io::ErrorKind::FileTooLarge, TOO_LARGE));
    }
    r
}

fn too_large(what: &str, bytes: u64) -> String {
    format!(
        "{what} is too large ({:.0} MB; the limit is {} MB).",
        bytes as f64 / (1 << 20) as f64,
        MAX_BYTES >> 20
    )
}

/// Runs `req` (file I/O: worker threads only — see [`spawn`]).
pub fn run(req: &Request) -> Outcome {
    let mut out = Outcome::default();
    let note_dir = req.note.as_deref().and_then(Path::parent);
    // a never-saved document has no folder to be relative to
    let relative = req.settings.relative_paths && note_dir.is_some();
    let res_root = req
        .resource_root
        .as_deref()
        .and_then(|r| resource_root_dir(note_dir, r));
    let res_root = res_root.as_deref();
    let mut target_cache: Option<Result<Target, String>> = None;
    let mut target = |req: &Request| -> Result<(cap_std::fs::Dir, PathBuf, String), String> {
        let t = target_cache.get_or_insert_with(|| target(req));
        match t {
            Ok(t) => Ok((
                t.dir.try_clone().map_err(|e| format!("the folder: {e}"))?,
                t.root.clone(),
                t.folder.clone(),
            )),
            Err(e) => Err(e.clone()),
        }
    };
    for item in &req.items {
        let r: Result<(String, PathBuf), String> = match item {
            Item::Image(img) => (|| {
                check_dimensions(img)?;
                let (dir, root, folder) = target(req)?;
                let name = pasted_image_name(&req.stamp);
                let _slot = EncodeSlot::acquire();
                let path = create_unique(&dir, &root, &folder, &name, &mut |f| encode_png_into(img, f, MAX_BYTES))
                    .map_err(|e| {
                        if e.contains(TOO_LARGE) {
                            format!("The pasted image is too large (the limit is {} MB).", MAX_BYTES >> 20)
                        } else {
                            e
                        }
                    })?;
                let md = markdown_for(note_dir, res_root, &path, relative);
                out.written.push(path.clone());
                Ok((md, path))
            })(),
            Item::File(src) => (|| {
                let name = src
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .ok_or_else(|| format!("{} has no file name.", src.display()))?;
                let meta = std::fs::metadata(src).map_err(|e| format!("{name}: {e}"))?;
                let image = is_image(&name);
                let copy = meta.is_file()
                    && req.note.is_some()
                    && if image {
                        req.settings.copy_images
                    } else {
                        req.settings.copy_files
                    };
                // a file already inside the folder (vault) is linked where it is
                let inside = req
                    .vault
                    .as_ref()
                    .map(|(r, _)| r.as_path())
                    .or(note_dir)
                    .is_some_and(|r| src.starts_with(r));
                if !copy || inside {
                    return Ok((markdown_for(note_dir, res_root, src, relative), src.clone()));
                }
                if meta.len() > MAX_BYTES {
                    return Err(too_large(&format!("“{name}”"), meta.len()));
                }
                let (dir, root, folder) = target(req)?;
                let mut from = std::fs::File::open(src).map_err(|e| format!("{name}: {e}"))?;
                let path = create_unique(&dir, &root, &folder, &name, &mut |f| {
                    // bounded: a file that grew past the limit meanwhile is not copied whole
                    let n = std::io::copy(&mut (&mut from).take(MAX_BYTES + 1), f)?;
                    if n > MAX_BYTES {
                        return Err(std::io::Error::other("the file grew past the size limit"));
                    }
                    Ok(())
                })?;
                let md = markdown_for(note_dir, res_root, &path, relative);
                out.written.push(path.clone());
                Ok((md, path))
            })(),
        };
        match r {
            Ok((md, target)) => {
                out.inserts.push(md);
                out.targets.push(target);
            }
            Err(e) => out.errors.push(e),
        }
    }
    out
}

/// Runs `req` on a short-lived worker thread; the result arrives on the receiver (the caller
/// is woken).
pub fn spawn(req: Request, wake: Wake) -> Receiver<Outcome> {
    let (tx, rx) = channel();
    let spawned = std::thread::Builder::new().name("markise-attach".into()).spawn({
        let tx = tx.clone();
        move || {
            let _ = tx.send(run(&req));
            wake();
        }
    });
    if let Err(e) = spawned {
        let _ = tx.send(Outcome {
            errors: vec![format!("cannot start a thread: {e}")],
            ..Default::default()
        });
    }
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("markise-attach-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::canonicalize(&d).unwrap()
    }

    fn settings(location: ImageLocation, folder: &str) -> ImageSettings {
        ImageSettings {
            location,
            folder: folder.into(),
            ..Default::default()
        }
    }

    #[test]
    fn image_and_url_detection() {
        assert!(is_image("a.PNG") && is_image("dir/x y.jpeg") && is_image("C:\\p\\a.webp"));
        assert!(!is_image(".png") && !is_image("notes.md") && !is_image("png"));
        assert!(is_image_url("https://example.com/a/b.png"));
        assert!(is_image_url(" http://example.com/x.JPG?w=2#f \n"));
        assert!(!is_image_url("https://example.com/page"));
        assert!(!is_image_url("https://example.com/a b.png"));
        assert!(!is_image_url("ftp://example.com/a.png"));
        assert!(!is_image_url("see https://example.com/a.png"));
        assert!(!is_image_url("https://a.png"));
    }

    /// V14.4: `Pasted image YYYYMMDDHHMMSS.png`, collisions get ` 1`, ` 2`, …
    #[test]
    fn names_and_collision_suffixes_v14_4() {
        assert_eq!(pasted_image_name("20261029101500"), "Pasted image 20261029101500.png");
        assert_eq!(candidate_name("a.png", 0), "a.png");
        assert_eq!(candidate_name("a.png", 2), "a 2.png");
        assert_eq!(candidate_name("archive.tar.gz", 1), "archive.tar 1.gz");
        assert_eq!(candidate_name("README", 1), "README 1");
        assert_eq!(stamp_now().len(), 14);
    }

    #[test]
    fn folder_per_setting() {
        let root = Path::new("/v");
        let note = Path::new("/v/Notes/Day/today.md");
        let f = |s: ImageSettings, o: Option<&str>| image_folder(&s, root, note, o);
        assert_eq!(
            f(settings(ImageLocation::NoteFolder, "assets"), None).unwrap(),
            "Notes/Day/assets"
        );
        assert_eq!(
            f(settings(ImageLocation::NoteFolder, "/img/"), None).unwrap(),
            "Notes/Day/img"
        );
        assert_eq!(f(settings(ImageLocation::NoteFolder, ""), None).unwrap(), "Notes/Day");
        assert_eq!(
            f(settings(ImageLocation::RootFolder, "media/pics"), None).unwrap(),
            "media/pics"
        );
        assert_eq!(
            f(settings(ImageLocation::SameFolder, "ignored"), None).unwrap(),
            "Notes/Day"
        );
        // a note in the root
        let top = image_folder(
            &settings(ImageLocation::SameFolder, ""),
            root,
            Path::new("/v/a.md"),
            None,
        );
        assert_eq!(top.unwrap(), "");
    }

    /// Spec 06 V14.2 `attachmentFolderPath`: `/`, `./`, `./name`, a vault folder.
    #[test]
    fn obsidian_attachment_folder_v14_2() {
        let root = Path::new("/v");
        let note = Path::new("/v/Notes/today.md");
        let s = settings(ImageLocation::NoteFolder, "assets");
        let f = |o: &str| image_folder(&s, root, note, Some(o));
        assert_eq!(f("/").unwrap(), "");
        assert_eq!(f("./").unwrap(), "Notes");
        assert_eq!(f("./attachments").unwrap(), "Notes/attachments");
        assert_eq!(f("Files/Attachments").unwrap(), "Files/Attachments");
        assert_eq!(f("Files/").unwrap(), "Files");
        assert!(f(".obsidian").is_err(), "never into .obsidian");
        assert!(f("../outside").is_err());
        // switched off: Markise's own location
        let off = ImageSettings {
            obsidian: false,
            ..s.clone()
        };
        assert_eq!(image_folder(&off, root, note, Some("/")).unwrap(), "Notes/assets");
    }

    #[test]
    fn obsidian_app_json_attachment_folder_is_read() {
        let s = crate::vault::VaultSettings::obsidian(Some(r#"{"attachmentFolderPath": "./att"}"#));
        assert_eq!(s.attachment_folder.as_deref(), Some("./att"));
        assert_eq!(
            crate::vault::VaultSettings::obsidian(Some("{}")).attachment_folder,
            None
        );
        assert_eq!(crate::vault::VaultSettings::plain().attachment_folder, None);
    }

    #[test]
    fn folders_are_checked() {
        assert!(check_folder("").is_ok());
        assert!(check_folder("a/b c").is_ok());
        for bad in ["..", "a/../b", ".obsidian", "a/.hidden", "a//b", "con", "a:b"] {
            assert!(check_folder(bad).is_err(), "{bad}");
        }
        let s = settings(ImageLocation::RootFolder, "../up");
        assert!(image_folder(&s, Path::new("/v"), Path::new("/v/n.md"), None).is_err());
    }

    /// Relative paths from the note to the file; spaces percent-encoded (V2.6.3) or the whole
    /// destination in `<…>`.
    #[test]
    fn link_paths_and_destinations() {
        let note_dir = Path::new("/v/Notes");
        let link_path = |dir: Option<&Path>, f: &Path, rel: bool| destination(dir, None, f, rel);
        assert_eq!(
            link_path(Some(note_dir), Path::new("/v/Notes/assets/a.png"), true),
            "assets/a.png"
        );
        assert_eq!(
            link_path(Some(note_dir), Path::new("/v/media/a.png"), true),
            "../media/a.png"
        );
        assert_eq!(link_path(Some(note_dir), Path::new("/v/Notes/a.png"), true), "a.png");
        assert_eq!(
            link_path(Some(note_dir), Path::new("/v/media/a.png"), false),
            "/v/media/a.png"
        );
        assert_eq!(link_path(None, Path::new("/tmp/a.png"), true), "/tmp/a.png");
        assert_eq!(
            markdown_dest("assets/Pasted image 1.png", DestForm::Percent),
            "assets/Pasted%20image%201.png"
        );
        assert_eq!(markdown_dest("a (1)#.png", DestForm::Percent), "a%20%281%29%23.png");
        assert_eq!(markdown_dest("Café/é.png", DestForm::Percent), "Café/é.png");
        assert_eq!(markdown_dest("my pics/a b.png", DestForm::Angle), "<my pics/a b.png>");
        assert_eq!(markdown_dest("a<b>.png", DestForm::Angle), "<a%3Cb%3E.png>");
        assert_eq!(
            markdown_for(Some(note_dir), None, Path::new("/v/Notes/doc [v2].pdf"), true),
            "[doc \\[v2\\].pdf](doc%20%5Bv2%5D.pdf)"
        );
        assert_eq!(
            markdown_for(Some(note_dir), None, Path::new("/v/x/a b.png"), true),
            "![](../x/a%20b.png)"
        );
    }

    fn red() -> Arc<epaint::ColorImage> {
        Arc::new(epaint::ColorImage::new([2, 1], vec![epaint::Color32::RED; 2]))
    }

    /// Clipboard images: the dimension budget is checked before any allocation, the PNG is
    /// streamed through a size-limited writer that aborts past the limit.
    #[test]
    fn png_streaming_and_limits() {
        // 20000×20000 claimed without pixels: refused by size, nothing allocated
        let huge = epaint::ColorImage {
            size: [20_000, 20_000],
            source_size: epaint::Vec2::ZERO,
            pixels: Vec::new(),
        };
        assert!(check_dimensions(&huge).unwrap_err().contains("too large"));
        let empty = epaint::ColorImage::new([0, 0], Vec::new());
        assert!(check_dimensions(&empty).is_err());
        // noise does not compress: 64×64 RGBA ≈ 16 KiB, so a 4 KiB limit aborts
        let mut seed = 1u32;
        let px: Vec<epaint::Color32> = (0..64 * 64)
            .map(|_| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let b = seed.to_le_bytes();
                epaint::Color32::from_rgba_unmultiplied(b[0], b[1], b[2], 255)
            })
            .collect();
        let noise = epaint::ColorImage::new([64, 64], px);
        let mut out = Vec::new();
        let e = encode_png_into(&noise, &mut out, 4096).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::FileTooLarge);
        assert!(out.len() <= 4096);
        // within the limit: a valid PNG with straight alpha
        let half = epaint::ColorImage::new([1, 1], vec![epaint::Color32::from_rgba_unmultiplied(200, 100, 0, 128)]);
        let mut out = Vec::new();
        encode_png_into(&half, &mut out, MAX_BYTES).unwrap();
        let mut dec = png::Decoder::new(std::io::Cursor::new(out)).read_info().unwrap();
        let mut buf = vec![0; dec.output_buffer_size().unwrap()];
        dec.next_frame(&mut buf).unwrap();
        assert_eq!(buf[3], 128);
        assert!((buf[0] as i32 - 200).abs() <= 2, "{buf:?}");
        // a refused write leaves no file behind
        let d = tmp("png-limit");
        let dir = crate::vault::open_root(&d).unwrap();
        let r = create_unique(&dir, &d, "", "n.png", &mut |f| encode_png_into(&noise, f, 4096));
        assert!(r.unwrap_err().contains(TOO_LARGE));
        assert!(!d.join("n.png").exists());
    }

    /// Parser → resolver round trip: for every name the inserted Markdown parses to a
    /// destination that `markise_doc::resource::local_file` resolves to exactly that file.
    #[test]
    fn inserted_destinations_resolve_back_to_the_file() {
        use markise_md::inline::{InlineContext, InlineKind, parse_inlines};
        let d = tmp("roundtrip");
        let note_dir = d.join("Notes");
        std::fs::create_dir_all(note_dir.join("assets")).unwrap();
        std::fs::create_dir_all(d.join("Other dir")).unwrap();
        let nfd = "Cafe\u{301}.png";
        let names = [
            "plain.png",
            "a b.png",
            "a b?.png",
            "what?#.png",
            "50% done.png",
            "100%25.png",
            "x#y.png",
            "(paren) [brack].png",
            "a)b(c.png",
            "]x[.png",
            "<angle>.png".trim_matches(|c| cfg!(windows) && (c == '<' || c == '>')),
            "pipe|caret^.png",
            "Café.png",
            nfd,
            "日本語 名前.png",
            "quote\"s'.png",
            "back`tick.png",
            "amp&semi;.png",
            "doc v2.pdf",
            "x?y.pdf",
        ];
        let opts = markise_md::ParseOptions::default();
        for (i, name) in names.iter().enumerate() {
            if cfg!(windows) && name.contains(['<', '>', '|', '?', '"']) {
                continue;
            }
            let file = if i % 2 == 0 {
                note_dir.join("assets").join(name)
            } else {
                d.join("Other dir").join(name)
            };
            std::fs::write(&file, b"x").unwrap();
            let elsewhere = tmp("roundtrip-root");
            // as a user writes them (no Windows verbatim prefix: those take no `/` separators)
            let plain = |p: &Path| simplify_verbatim(&p.to_string_lossy());
            let roots: [Option<String>; 4] = [None, Some(plain(&d)), Some(plain(&elsewhere)), Some("..".into())];
            let doc_dir = PathBuf::from(plain(&note_dir));
            for (relative, root) in [true, false]
                .into_iter()
                .flat_map(|r| roots.iter().map(move |o| (r, o)))
            {
                let root_dir = root.as_deref().and_then(|r| resource_root_dir(Some(&note_dir), r));
                let md = markdown_for(Some(&note_dir), root_dir.as_deref(), &file, relative);
                let nodes = parse_inlines(&md, &opts, &InlineContext::default());
                assert_eq!(nodes.len(), 1, "{name}: one node in {md}");
                let dest = match &nodes[0].kind {
                    InlineKind::Image { dest, .. } | InlineKind::Link { dest, .. } => dest.clone(),
                    k => panic!("{name}: {md} → {k:?}"),
                };
                if !relative && root.as_deref() == Some(plain(&d).as_str()) {
                    assert!(
                        dest.starts_with('/') && !dest.starts_with("//"),
                        "{name}: root-relative {dest}"
                    );
                }
                let got = markise_doc::resource::local_file(&doc_dir, root.as_deref(), &dest)
                    .unwrap_or_else(|| panic!("{name}: {md} not local"));
                assert!(got.suffix.is_empty(), "{name}: {md} split a suffix {:?}", got.suffix);
                let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|e| panic!("{name}: {md}: {e}"));
                assert_eq!(canon(&got.path), canon(&file), "{name}: {md}");
            }
        }
    }

    #[test]
    fn windows_verbatim_prefixes_and_file_urls() {
        assert_eq!(simplify_verbatim(r"\\?\C:\Users\a\x.png"), r"C:\Users\a\x.png");
        assert_eq!(simplify_verbatim(r"\\?\UNC\srv\share\x.png"), r"\\srv\share\x.png");
        assert_eq!(simplify_verbatim(r"\\?\Volume{x}\a"), r"\\?\Volume{x}\a");
        assert_eq!(simplify_verbatim("/home/a"), "/home/a");
        #[cfg(unix)]
        {
            assert_eq!(file_url(Path::new("/home/a b/x?.png")), "file:///home/a%20b/x%3F.png");
            // a resource root: root-relative below it, a file URL elsewhere
            let root = Path::new("/v/site");
            let note = Path::new("/v/site/docs");
            assert_eq!(
                destination(Some(note), Some(root), Path::new("/v/site/img/a.png"), false),
                "/img/a.png"
            );
            assert_eq!(
                destination(Some(note), Some(root), Path::new("/tmp/a b.png"), false),
                "file:///tmp/a%20b.png"
            );
            assert_eq!(
                destination(Some(note), Some(root), Path::new("/v/site/docs/a.png"), true),
                "a.png"
            );
            assert_eq!(
                destination(Some(note), None, Path::new("/tmp/a.png"), false),
                "/tmp/a.png"
            );
        }
    }

    /// The links of a finished request are generated again for the note's current location
    /// (Save As while the job ran).
    #[test]
    fn outcome_renders_for_the_current_note_location() {
        let a = tmp("render-a");
        let b = tmp("render-b");
        std::fs::write(a.join("n.md"), "x").unwrap();
        let out = run(&Request {
            items: vec![Item::Image(red())],
            settings: ImageSettings::default(),
            note: Some(a.join("n.md")),
            vault: None,
            obsidian_folder: None,
            user_images: None,
            stamp: "3".into(),
            resource_root: None,
        });
        assert_eq!(out.text(), "![](assets/Pasted%20image%203.png)");
        assert_eq!(out.render(Some(&a), None, true), out.text());
        let moved = out.render(Some(&b), None, true);
        let dest = &moved[4..moved.len() - 1];
        let got = markise_doc::resource::local_file(&b, None, dest).unwrap();
        assert_eq!(
            std::fs::canonicalize(got.path).unwrap(),
            std::fs::canonicalize(a.join("assets/Pasted image 3.png")).unwrap()
        );
    }

    #[test]
    fn create_unique_never_overwrites() {
        let d = tmp("unique");
        let dir = crate::vault::open_root(&d).unwrap();
        let w = |b: &'static [u8]| move |f: &mut cap_std::fs::File| f.write_all(b);
        let a = create_unique(&dir, &d, "x/y", "a.png", &mut w(b"1")).unwrap();
        let b = create_unique(&dir, &d, "x/y", "a.png", &mut w(b"2")).unwrap();
        let c = create_unique(&dir, &d, "x/y", "a.png", &mut w(b"3")).unwrap();
        assert_eq!(a, d.join("x/y/a.png"));
        assert_eq!(b, d.join("x/y/a 1.png"));
        assert_eq!(c, d.join("x/y/a 2.png"));
        assert_eq!(std::fs::read(&a).unwrap(), b"1");
        assert!(create_unique(&dir, &d, ".obsidian", "a.png", &mut w(b"x")).is_err());
        assert!(create_unique(&dir, &d, "", "../a.png", &mut w(b"x")).is_err());
        // a failed fill leaves nothing behind
        let e = create_unique(&dir, &d, "", "bad.png", &mut |_| Err(std::io::Error::other("boom")));
        assert!(e.is_err());
        assert!(!d.join("bad.png").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_folders_are_not_followed() {
        let d = tmp("link");
        let outside = tmp("link-outside");
        std::os::unix::fs::symlink(&outside, d.join("assets")).unwrap();
        let dir = crate::vault::open_root(&d).unwrap();
        let r = create_unique(&dir, &d, "assets", "a.png", &mut |f| f.write_all(b"x"));
        assert!(r.is_err(), "{r:?}");
        assert!(
            std::fs::read_dir(&outside).unwrap().next().is_none(),
            "nothing written outside"
        );
    }

    #[test]
    fn pasted_image_is_written_as_png_next_to_the_note() {
        let d = tmp("paste");
        std::fs::write(d.join("note.md"), "x").unwrap();
        let req = Request {
            items: vec![Item::Image(red()), Item::Image(red())],
            settings: ImageSettings::default(),
            note: Some(d.join("note.md")),
            vault: None,
            obsidian_folder: None,
            user_images: None,
            stamp: "20261029101500".into(),
            resource_root: None,
        };
        let out = run(&req);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(
            out.text(),
            "![](assets/Pasted%20image%2020261029101500.png)\n![](assets/Pasted%20image%2020261029101500%201.png)"
        );
        let bytes = std::fs::read(d.join("assets/Pasted image 20261029101500.png")).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        let dec = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
        assert_eq!((dec.info().width, dec.info().height), (2, 1));
    }

    #[test]
    fn untitled_documents_use_the_user_images_folder() {
        let d = tmp("untitled");
        let mut req = Request {
            items: vec![Item::Image(red())],
            settings: ImageSettings::default(),
            note: None,
            vault: None,
            obsidian_folder: None,
            user_images: Some(d.join("user-images")),
            stamp: "20260101000000".into(),
            resource_root: None,
        };
        let out = run(&req);
        let p = d.join("user-images/Pasted image 20260101000000.png");
        assert!(p.is_file(), "{out:?}");
        assert_eq!(out.inserts, [format!("![]({})", destination(None, None, &p, false))]);
        req.user_images = None;
        assert_eq!(run(&req).errors.len(), 1);
    }

    #[test]
    fn dropped_files_copy_or_link_per_setting() {
        let d = tmp("drop");
        let src = tmp("drop-src");
        std::fs::write(src.join("photo one.jpg"), b"jpg").unwrap();
        std::fs::write(src.join("report.pdf"), b"pdf").unwrap();
        std::fs::write(d.join("note.md"), "x").unwrap();
        std::fs::write(d.join("inside.png"), b"png").unwrap();
        let mut req = Request {
            items: vec![
                Item::File(src.join("photo one.jpg")),
                Item::File(src.join("report.pdf")),
                Item::File(d.join("inside.png")),
            ],
            settings: ImageSettings::default(),
            note: Some(d.join("note.md")),
            vault: None,
            obsidian_folder: None,
            user_images: None,
            stamp: "1".into(),
            resource_root: None,
        };
        let out = run(&req);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let pdf = destination(Some(&d), None, &src.join("report.pdf"), true);
        assert_eq!(
            out.inserts,
            [
                "![](assets/photo%20one.jpg)".to_string(),
                format!("[report.pdf]({pdf})"),
                "![](inside.png)".to_string(),
            ]
        );
        assert_eq!(std::fs::read(d.join("assets/photo one.jpg")).unwrap(), b"jpg");
        assert!(
            !d.join("assets/report.pdf").exists(),
            "other files are linked, not copied"
        );
        assert!(
            !d.join("assets/inside.png").exists(),
            "files inside the folder are linked"
        );
        // link in place; copy other files when asked
        req.settings.copy_images = false;
        req.settings.copy_files = true;
        let out = run(&req);
        assert!(out.inserts[0].contains("photo%20one.jpg") && out.inserts[0].contains(".."));
        assert_eq!(out.inserts[1], "[report.pdf](assets/report.pdf)");
        assert!(d.join("assets/report.pdf").is_file());
    }

    #[test]
    fn oversized_files_are_refused() {
        let d = tmp("big");
        let src = tmp("big-src");
        let f = std::fs::File::create(src.join("huge.png")).unwrap();
        f.set_len(MAX_BYTES + 1).unwrap();
        std::fs::write(d.join("note.md"), "x").unwrap();
        let req = Request {
            items: vec![Item::File(src.join("huge.png"))],
            settings: ImageSettings::default(),
            note: Some(d.join("note.md")),
            vault: None,
            obsidian_folder: None,
            user_images: None,
            stamp: "1".into(),
            resource_root: None,
        };
        let out = run(&req);
        assert!(out.inserts.is_empty());
        assert!(out.errors[0].contains("too large"), "{:?}", out.errors);
        assert!(!d.join("assets").exists());
    }

    #[test]
    fn vault_root_and_obsidian_folder_are_used() {
        let v = tmp("vault");
        std::fs::create_dir_all(v.join("Notes")).unwrap();
        std::fs::write(v.join("Notes/n.md"), "x").unwrap();
        let handle = Arc::new(crate::vault::open_root(&v).unwrap());
        let req = Request {
            items: vec![Item::Image(red())],
            settings: settings(ImageLocation::RootFolder, "media"),
            note: Some(v.join("Notes/n.md")),
            vault: Some((v.clone(), Some(handle.clone()))),
            obsidian_folder: Some("Attachments".into()),
            user_images: None,
            stamp: "2".into(),
            resource_root: None,
        };
        let out = run(&req);
        assert_eq!(out.inserts, ["![](../Attachments/Pasted%20image%202.png)"]);
        let out = run(&Request {
            obsidian_folder: None,
            ..req
        });
        assert_eq!(out.inserts, ["![](../media/Pasted%20image%202.png)"]);
    }
}

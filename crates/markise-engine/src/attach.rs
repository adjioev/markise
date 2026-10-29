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
//! **Limits:** files above [`MAX_BYTES`] (encoded size for pasted images) are refused with a
//! message; clipboard images above [`MAX_PIXELS`] are refused before encoding.
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
    /// Percent-encoded (V2.6.3: space → `%20`, …); readable by Obsidian and Typora.
    Percent,
    /// Wrapped in `<…>` (CommonMark), only `<`, `>` and line breaks escaped.
    Angle,
}

/// `path` (`/`-separated) as a Markdown link destination.
pub fn markdown_dest(path: &str, form: DestForm) -> String {
    match form {
        DestForm::Percent => crate::vault::percent_encode_path(path),
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

/// The path written for `file`: relative to `note_dir` when `relative` and a relative path
/// exists (same drive), else absolute; `/`-separated.
pub fn link_path(note_dir: Option<&Path>, file: &Path, relative: bool) -> String {
    if relative
        && let Some(dir) = note_dir
        && let Some(r) = crate::vault::relative_path(dir, file)
    {
        return r;
    }
    let s = file.to_string_lossy().into_owned();
    if cfg!(windows) { s.replace('\\', "/") } else { s }
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
/// file name.
pub fn markdown_for(note_dir: Option<&Path>, file: &Path, relative: bool) -> String {
    let dest = markdown_dest(&link_path(note_dir, file, relative), DestForm::Percent);
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
}

/// The result of a [`Request`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Markdown per item that succeeded, in item order.
    pub inserts: Vec<String>,
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

/// Encodes clipboard pixels as PNG.
fn encode_png(img: &epaint::ColorImage) -> Result<Vec<u8>, String> {
    let [w, h] = img.size;
    if w == 0 || h == 0 {
        return Err("The clipboard image is empty.".into());
    }
    if (w as u64) * (h as u64) > MAX_PIXELS {
        return Err(format!("The clipboard image is too large ({w}×{h} pixels)."));
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for c in &img.pixels {
        rgba.extend_from_slice(&c.to_srgba_unmultiplied());
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut wr = enc
            .write_header()
            .map_err(|e| format!("Cannot encode the image: {e}"))?;
        wr.write_image_data(&rgba)
            .map_err(|e| format!("Cannot encode the image: {e}"))?;
        wr.finish().map_err(|e| format!("Cannot encode the image: {e}"))?;
    }
    Ok(out)
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
        let r: Result<String, String> = match item {
            Item::Image(img) => (|| {
                let png = encode_png(img)?;
                if png.len() as u64 > MAX_BYTES {
                    return Err(too_large("The pasted image", png.len() as u64));
                }
                let (dir, root, folder) = target(req)?;
                let name = pasted_image_name(&req.stamp);
                let path = create_unique(&dir, &root, &folder, &name, &mut |f| f.write_all(&png))?;
                let md = format!(
                    "![]({})",
                    markdown_dest(&link_path(note_dir, &path, relative), DestForm::Percent)
                );
                out.written.push(path);
                Ok(md)
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
                    return Ok(markdown_for(note_dir, src, relative));
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
                let md = markdown_for(note_dir, &path, relative);
                out.written.push(path);
                Ok(md)
            })(),
        };
        match r {
            Ok(md) => out.inserts.push(md),
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
            markdown_for(Some(note_dir), Path::new("/v/Notes/doc [v2].pdf"), true),
            "[doc \\[v2\\].pdf](doc%20%5Bv2%5D.pdf)"
        );
        assert_eq!(
            markdown_for(Some(note_dir), Path::new("/v/x/a b.png"), true),
            "![](../x/a%20b.png)"
        );
    }

    fn red() -> Arc<epaint::ColorImage> {
        Arc::new(epaint::ColorImage::new([2, 1], vec![epaint::Color32::RED; 2]))
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
        };
        let out = run(&req);
        let p = d.join("user-images/Pasted image 20260101000000.png");
        assert!(p.is_file(), "{out:?}");
        assert_eq!(
            out.inserts,
            [format!(
                "![]({})",
                crate::vault::percent_encode_path(&link_path(None, &p, false))
            )]
        );
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
        };
        let out = run(&req);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let pdf = link_path(Some(&d), &src.join("report.pdf"), true);
        assert_eq!(
            out.inserts,
            [
                "![](assets/photo%20one.jpg)".to_string(),
                format!("[report.pdf]({})", crate::vault::percent_encode_path(&pdf)),
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

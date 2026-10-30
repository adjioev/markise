//! Folder listing for the sidebar file tree: one directory at a time (lazy expansion), Markdown
//! and plain-text files only (spec 05 §1.1), hidden entries skipped (spec 04 §4.3 default),
//! folders first, natural sort (spec 04 §4.2: "file2" < "file10").

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use crate::Wake;

/// Markdown extensions (spec 05 §1.1, interop list) plus plain text.
pub const MARKDOWN_EXTENSIONS: &[&str] = &[
    "md",
    "markdown",
    "mmd",
    "mdown",
    "mkd",
    "mdwn",
    "mdtxt",
    "mdtext",
    "rmarkdown",
    "rmd",
    "apib",
    "qmd",
    "mdx",
];
pub const TEXT_EXTENSIONS: &[&str] = &["txt", "text"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

pub fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| MARKDOWN_EXTENSIONS.contains(&e.as_str()) || TEXT_EXTENSIONS.contains(&e.as_str()))
}

/// Natural ordering: case-insensitive, digit runs compared numerically.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let mut n1 = String::new();
                while let Some(c) = x.peek().copied().filter(char::is_ascii_digit) {
                    n1.push(c);
                    x.next();
                }
                let mut n2 = String::new();
                while let Some(c) = y.peek().copied().filter(char::is_ascii_digit) {
                    n2.push(c);
                    y.next();
                }
                let t1 = n1.trim_start_matches('0');
                let t2 = n2.trim_start_matches('0');
                let o = t1.len().cmp(&t2.len()).then_with(|| t1.cmp(t2));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_lowercase().cmp(d.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// Whether a directory entry is a folder, without opening the entry itself when its type is
/// known from the listing (on Windows an open handle, even a momentary one for metadata,
/// makes a concurrent save's rename over that file fail); only symbolic links are followed
/// with a metadata call. `None`: the entry vanished.
pub fn entry_is_dir(e: &std::fs::DirEntry) -> Option<bool> {
    let t = e.file_type().ok()?;
    if t.is_symlink() {
        std::fs::metadata(e.path()).ok().map(|m| m.is_dir())
    } else {
        Some(t.is_dir())
    }
}

/// Lists one directory: sub-folders and Markdown/text files, hidden entries skipped.
pub fn list_dir(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let Ok(e) = e else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = e.path();
        let is_dir = entry_is_dir(&e).unwrap_or(false);
        if is_dir || is_markdown(&path) {
            out.push(Entry { name, path, is_dir });
        }
    }
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| natural_cmp(&a.name, &b.name)));
    Ok(out)
}

/// Result of one listing: (directory, request generation, entries or error).
pub type Listing = (PathBuf, u64, Result<Vec<Entry>, String>);

/// A single background worker that lists directories one at a time (bounded: no thread per
/// request). Callers coalesce requests per directory; results carry the request generation.
pub struct Lister {
    tx: std::sync::mpsc::Sender<(PathBuf, u64)>,
    rx: Receiver<Listing>,
}

impl Lister {
    pub fn new(wake: Wake) -> Result<Lister, String> {
        let (tx, job_rx) = channel::<(PathBuf, u64)>();
        let (res_tx, rx) = channel();
        std::thread::Builder::new()
            .name("markise-list".into())
            .spawn(move || {
                while let Ok((dir, generation)) = job_rx.recv() {
                    let r = list_dir(&dir).map_err(|e| e.to_string());
                    let published = crate::wake_track::publishing();
                    if res_tx.send((dir, generation, r)).is_err() {
                        drop(published);
                        break;
                    }
                    wake();
                    drop(published);
                }
            })
            .map_err(|e| format!("cannot start listing thread: {e}"))?;
        Ok(Lister { tx, rx })
    }

    pub fn request(&self, dir: PathBuf, generation: u64) -> bool {
        self.tx.send((dir, generation)).is_ok()
    }

    pub fn try_recv(&self) -> Option<Listing> {
        self.rx.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_sort_order() {
        let mut v = vec!["file10.md", "File2.md", "file1.md", "a.md", "file02b.md", "B.md"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            v,
            vec!["a.md", "B.md", "file1.md", "File2.md", "file02b.md", "file10.md"]
        );
    }

    #[test]
    fn lists_markdown_and_folders_only() {
        let dir = std::env::temp_dir().join(format!("markise-engine-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        for f in ["n10.md", "n9.markdown", "notes.txt", "img.png", ".hidden.md", "README"] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        let names: Vec<_> = list_dir(&dir).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["sub", "n9.markdown", "n10.md", "notes.txt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! The images of a folder, for a picker to show: the nearest first, within a bound on depth and count.
//! It runs on a worker and is told when to stop. Nothing outside the folder is reached through a link.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::attach::is_image;

/// How deep below the folder it looks.
pub const MAX_DEPTH: usize = 6;
/// How many images it lists at most.
pub const MAX_IMAGES: usize = 2000;
/// How many folder entries it looks at at most, images or not.
pub const MAX_ENTRIES: usize = 100_000;

/// One image found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedImage {
    pub path: PathBuf,
    /// Below the folder, with `/`.
    pub rel: String,
}

/// What was found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageListing {
    pub images: Vec<ListedImage>,
    /// There may be more than this: a bound was reached.
    pub truncated: bool,
    /// Folders that could not be read.
    pub unreadable: usize,
}

/// The images in `root` and below it, breadth first and by name within a folder. Hidden files and folders
/// are left out. A link counts only when it is to a file inside `root`; a link to a folder is not followed.
pub fn list_images(root: &Path, cancel: &AtomicBool) -> ImageListing {
    list_images_within(root, cancel, MAX_IMAGES, MAX_ENTRIES)
}

/// [`list_images`] with its bounds given.
pub fn list_images_within(root: &Path, cancel: &AtomicBool, max_images: usize, max_entries: usize) -> ImageListing {
    let mut out = ImageListing::default();
    let Ok(inside) = std::fs::canonicalize(root) else {
        out.unreadable = 1;
        return out;
    };
    let mut seen = 0usize;
    let mut level: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    let mut depth = 0usize;
    while !level.is_empty() {
        let mut next = Vec::new();
        for (dir, prefix) in level {
            let Ok(read) = std::fs::read_dir(&dir) else {
                out.unreadable += 1;
                continue;
            };
            let mut entries: Vec<_> = read.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                if cancel.load(Ordering::Relaxed) {
                    return out;
                }
                seen += 1;
                if seen > max_entries {
                    out.truncated = true;
                    return out;
                }
                let Some(name) = e.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                if name.starts_with('.') {
                    continue;
                }
                let Ok(kind) = e.file_type() else { continue };
                let path = e.path();
                let rel = format!("{prefix}{name}");
                let file = if kind.is_symlink() {
                    std::fs::canonicalize(&path).is_ok_and(|t| t.starts_with(&inside) && t.is_file())
                } else if kind.is_dir() {
                    if depth < MAX_DEPTH {
                        next.push((path.clone(), format!("{rel}/")));
                    } else {
                        out.truncated = true;
                    }
                    false
                } else {
                    kind.is_file()
                };
                if file && is_image(&name) {
                    if out.images.len() >= max_images {
                        out.truncated = true;
                        return out;
                    }
                    out.images.push(ListedImage { path, rel });
                }
            }
        }
        next.sort();
        level = next;
        depth += 1;
    }
    out
}

/// The destinations written in a Markdown text as `](…)`, as written, angle brackets off. A cheap look,
/// for a mark next to an image and no more: code counts too.
pub fn destinations_in(text: &str) -> HashSet<&str> {
    let mut out = HashSet::new();
    let mut rest = text;
    while let Some(i) = rest.find("](") {
        rest = &rest[i + 2..];
        let t = rest.trim_start_matches([' ', '\t']);
        let dest = if let Some(t) = t.strip_prefix('<') {
            t.find('>').map(|e| &t[..e])
        } else {
            t.find([' ', '\t', '\r', '\n', ')']).map(|e| &t[..e])
        };
        if let Some(d) = dest.filter(|d| !d.is_empty()) {
            out.insert(d);
        }
    }
    out
}

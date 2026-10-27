//! Adapter-owned face-name index for `local()` (ADR 0005 §4.1 "FaceNameIndex"): fontique 0.12
//! looks up families, not faces by full or PostScript name. Built off the UI thread by scanning
//! font folders; faces are memory-mapped on use, not during the scan's lifetime.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use skrifa::raw::FileRef;
use skrifa::string::StringId;
use skrifa::{FontRef, MetadataProvider};

use crate::face::open_file;

/// Location of one face.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaceLocation {
    pub path: PathBuf,
    pub index: u32,
}

/// Full name (name ID 4), PostScript name (ID 6) and typographic/legacy family + subfamily
/// ("Family Subfamily") → face location. Keys are case-folded (CSS matches `local()` names
/// case-insensitively in practice; the spec leaves it to the platform).
#[derive(Clone, Debug, Default)]
pub struct FaceNameIndex {
    map: HashMap<String, FaceLocation>,
    files: usize,
}

fn key(s: &str) -> String {
    s.trim().to_lowercase()
}

impl FaceNameIndex {
    /// The platform font folders (same set as ADR 0004's system index).
    pub fn platform_dirs() -> Vec<PathBuf> {
        #[allow(unused_mut)]
        let mut v = Vec::new();
        #[cfg(target_os = "macos")]
        {
            v.push(PathBuf::from("/System/Library/Fonts"));
            v.push(PathBuf::from("/Library/Fonts"));
            if let Some(h) = std::env::var_os("HOME") {
                v.push(PathBuf::from(h).join("Library/Fonts"));
            }
        }
        #[cfg(target_os = "windows")]
        {
            if let Some(w) = std::env::var_os("WINDIR") {
                v.push(PathBuf::from(w).join("Fonts"));
            }
            if let Some(l) = std::env::var_os("LOCALAPPDATA") {
                v.push(PathBuf::from(l).join("Microsoft\\Windows\\Fonts"));
            }
        }
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            v.push(PathBuf::from("/usr/share/fonts"));
            v.push(PathBuf::from("/usr/local/share/fonts"));
            if let Some(h) = std::env::var_os("HOME") {
                v.push(PathBuf::from(&h).join(".local/share/fonts"));
                v.push(PathBuf::from(h).join(".fonts"));
            }
        }
        v
    }

    /// Scans `dirs` recursively (depth ≤ 5). Unreadable files are skipped.
    pub fn scan(dirs: &[PathBuf]) -> FaceNameIndex {
        let mut idx = FaceNameIndex::default();
        let mut files = Vec::new();
        for d in dirs {
            collect(d, 0, &mut files);
        }
        for f in files {
            idx.add_file(&f);
        }
        idx
    }

    /// Indexes every face of one font file.
    pub fn add_file(&mut self, path: &Path) {
        let Ok(bytes) = open_file(path, 256 << 20) else {
            return;
        };
        let data = bytes.as_ref();
        // (original collection index, face): malformed entries are skipped without shifting
        // the indices of the faces after them
        let fonts: Vec<(u32, FontRef)> = match FileRef::new(data) {
            Ok(FileRef::Font(f)) => vec![(0, f)],
            Ok(FileRef::Collection(c)) => c
                .iter()
                .enumerate()
                .filter_map(|(i, f)| f.ok().map(|f| (i as u32, f)))
                .collect(),
            Err(_) => return,
        };
        self.files += 1;
        for (i, f) in &fonts {
            let loc = FaceLocation {
                path: path.to_path_buf(),
                index: *i,
            };
            let get = |id| {
                f.localized_strings(id)
                    .english_or_first()
                    .map(|s| s.chars().collect::<String>())
            };
            for id in [StringId::FULL_NAME, StringId::POSTSCRIPT_NAME] {
                if let Some(n) = get(id) {
                    self.map.entry(key(&n)).or_insert_with(|| loc.clone());
                }
            }
            let fam = get(StringId::TYPOGRAPHIC_FAMILY_NAME).or_else(|| get(StringId::FAMILY_NAME));
            let sub = get(StringId::TYPOGRAPHIC_SUBFAMILY_NAME).or_else(|| get(StringId::SUBFAMILY_NAME));
            if let (Some(fam), Some(sub)) = (fam, sub) {
                self.map
                    .entry(key(&format!("{fam} {sub}")))
                    .or_insert_with(|| loc.clone());
            }
        }
    }

    /// Looks up a `local()` name.
    pub fn find(&self, name: &str) -> Option<&FaceLocation> {
        self.map.get(&key(name))
    }

    /// Number of indexed names / files.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn files(&self) -> usize {
        self.files
    }
}

fn collect(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 5 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            collect(&p, depth + 1, out);
        } else if let Some(ext) = p.extension().and_then(|x| x.to_str())
            && matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc" | "otc")
        {
            out.push(p);
        }
    }
}

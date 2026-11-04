//! Reading a package archive (`.tar.gz`) into memory, with limits, without touching the disk.

use std::collections::BTreeMap;
use std::io::Read;

use flate2::read::GzDecoder;
use tar::{Archive, EntryType};
use unicode_normalization::UnicodeNormalization;

use crate::limits::{MAX_FILE_BYTES, MAX_FILES, MAX_UNPACKED_BYTES};
use crate::paths::clean_relative;

/// The files of an archive: path to content.
pub type Files = BTreeMap<String, Vec<u8>>;

/// Unpacks `bytes`. Only regular files and folders are allowed. A link of any kind, a device or
/// an unsafe path refuses the whole archive; so does anything over a limit, which is enforced
/// while decompressing and does not trust the sizes the archive states.
pub fn read_archive(bytes: &[u8]) -> Result<Files, String> {
    // headers and skipped entries count too; running into this cap is checked after the loop,
    // since to the reader it looks like the end of the archive
    let reader = GzDecoder::new(bytes).take(MAX_UNPACKED_BYTES * 2);
    let mut archive = Archive::new(reader);
    let mut files = Files::new();
    let mut names = Names::default();
    let mut total = 0u64;
    let mut entries = 0usize;
    let broken = |e: std::io::Error| format!("the archive cannot be read: {e}");
    for entry in archive.entries().map_err(broken)? {
        let mut entry = entry.map_err(broken)?;
        entries += 1;
        if entries > MAX_FILES {
            return Err(format!("the archive holds more than {MAX_FILES} entries"));
        }
        let raw = entry.path_bytes();
        let raw = std::str::from_utf8(&raw)
            .map_err(|_| "a path in the archive is not UTF-8".to_string())?
            .to_string();
        match entry.header().entry_type() {
            EntryType::XGlobalHeader => continue,
            EntryType::Directory => {
                names.add_folder(&clean_relative(&raw)?)?;
                continue;
            }
            EntryType::Regular => {}
            other => return Err(format!("\"{raw}\" is not a regular file or folder ({other:?})")),
        }
        let path = clean_relative(&raw)?;
        if path.is_empty() {
            return Err(format!("\"{raw}\" is a file without a name"));
        }
        names.add_file(&path)?;
        if entry.size() > MAX_FILE_BYTES {
            return Err(format!("\"{path}\" is larger than {MAX_FILE_BYTES} bytes"));
        }
        let mut content = Vec::with_capacity(entry.size() as usize);
        entry
            .by_ref()
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut content)
            .map_err(broken)?;
        total += content.len() as u64;
        if content.len() as u64 > MAX_FILE_BYTES {
            return Err(format!("\"{path}\" is larger than {MAX_FILE_BYTES} bytes"));
        }
        if total > MAX_UNPACKED_BYTES {
            return Err(format!("the archive unpacks to more than {MAX_UNPACKED_BYTES} bytes"));
        }
        files.insert(path, content);
    }
    if archive.into_inner().limit() == 0 {
        return Err(format!("the archive unpacks to more than {MAX_UNPACKED_BYTES} bytes"));
    }
    Ok(files)
}

/// The names seen so far, so that one archive means the same tree on every disk. A disk that
/// ignores case or Unicode normalization would make two such names one file or folder, and the
/// files read back would not be the files accepted.
#[derive(Default)]
struct Names {
    /// Folded path to the path as written and whether it is a folder.
    seen: BTreeMap<String, (String, bool)>,
}

impl Names {
    fn add_folder(&mut self, path: &str) -> Result<(), String> {
        let mut end = 0;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            end += part.len() + usize::from(end > 0);
            self.add(&path[..end], true)?;
        }
        Ok(())
    }

    fn add_file(&mut self, path: &str) -> Result<(), String> {
        if let Some((parent, _)) = path.rsplit_once('/') {
            self.add_folder(parent)?;
        }
        self.add(path, false)
    }

    fn add(&mut self, path: &str, folder: bool) -> Result<(), String> {
        match self.seen.get(&fold(path)) {
            None => {
                self.seen.insert(fold(path), (path.to_string(), folder));
                Ok(())
            }
            Some((seen, _)) if seen != path => {
                Err(format!("\"{seen}\" and \"{path}\" differ only by case or Unicode form"))
            }
            Some((_, was_folder)) if *was_folder != folder => Err(format!("\"{path}\" is both a file and a folder")),
            Some((_, true)) => Ok(()),
            Some((_, false)) => Err(format!("\"{path}\" appears twice")),
        }
    }
}

/// A form two names share when a disk may treat them as one: upper then lower case also joins
/// pairs that lowering alone keeps apart, such as a final sigma.
fn fold(path: &str) -> String {
    let composed: String = path.nfc().collect();
    composed.to_uppercase().to_lowercase().nfc().collect()
}

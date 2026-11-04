//! Reading a package archive (`.tar.gz`) into memory, with limits, without touching the disk.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

use flate2::read::GzDecoder;
use tar::{Archive, EntryType};

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
    let mut folded = BTreeSet::new();
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
                clean_relative(&raw)?;
                continue;
            }
            EntryType::Regular => {}
            other => return Err(format!("\"{raw}\" is not a regular file or folder ({other:?})")),
        }
        let path = clean_relative(&raw)?;
        if path.is_empty() {
            return Err(format!("\"{raw}\" is a file without a name"));
        }
        // a case-insensitive disk would make these one file
        if !folded.insert(path.to_lowercase()) {
            return Err(format!("\"{path}\" appears twice"));
        }
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

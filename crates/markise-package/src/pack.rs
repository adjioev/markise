//! A folder to a package archive, for tools and tests: what `cargo xtask block-package` builds.
//! The app never packs anything.

use std::fs;
use std::path::Path;

use flate2::Compression;
use flate2::write::GzEncoder;

use crate::archive::{Files, read_archive};
use crate::paths::clean_relative;
use crate::store::Sha256Hex;
use crate::{PackageId, PackageIndex};

/// A valid archive and what a binding needs to name it.
#[derive(Clone, Debug)]
pub struct Packed {
    pub bytes: Vec<u8>,
    pub sha256: Sha256Hex,
    pub id: PackageId,
}

/// Packs the regular files below `root`, and checks the result as a document's binding would. The
/// same files give the same bytes: paths in order, with no dates, owners or modes of their own. A
/// link anywhere is an error, as in an archive; `.DS_Store`, which a file manager leaves, is left out.
pub fn pack_folder(root: &Path) -> Result<Packed, String> {
    let mut files = Files::new();
    collect(root, "", &mut files)?;
    let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::best()));
    let broken = |e: std::io::Error| format!("the archive cannot be written: {e}");
    for (path, content) in &files {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        tar.append_data(&mut header, path, content.as_slice()).map_err(broken)?;
    }
    let bytes = tar.into_inner().map_err(broken)?.finish().map_err(broken)?;
    let index = PackageIndex::from_files(&read_archive(&bytes)?).map_err(|e| e.to_string())?;
    Ok(Packed {
        sha256: Sha256Hex::of(&bytes),
        id: index.id().clone(),
        bytes,
    })
}

fn collect(folder: &Path, prefix: &str, files: &mut Files) -> Result<(), String> {
    let cannot = |e: std::io::Error| format!("{}: {e}", folder.display());
    for entry in fs::read_dir(folder).map_err(cannot)? {
        let entry = entry.map_err(cannot)?;
        let name = entry.file_name();
        let name = name.to_str().ok_or("a file name is not UTF-8")?;
        if name == ".DS_Store" {
            continue;
        }
        let path = clean_relative(&format!("{prefix}{name}"))?;
        let kind = entry.file_type().map_err(cannot)?;
        if kind.is_dir() {
            collect(&entry.path(), &format!("{path}/"), files)?;
        } else if kind.is_file() {
            files.insert(path, fs::read(entry.path()).map_err(cannot)?);
        } else {
            return Err(format!("\"{path}\" is not a regular file or folder"));
        }
    }
    Ok(())
}

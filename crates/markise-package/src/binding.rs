//! `blocks.config.json`: the file in a content folder that binds its documents to a package.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::paths::clean_relative;
use crate::value::is_token;

/// The binding's file name.
pub const BINDING_FILE: &str = "blocks.config.json";

/// A binding file is small; anything larger is not one.
const MAX_BINDING_BYTES: u64 = 64 << 10;
/// Folders looked at above a document.
const MAX_LEVELS: usize = 64;

/// A parsed, checked binding. `source` has passed the path rules and `sha256` is lowercase hex.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub file: PathBuf,
    pub library: String,
    pub version: String,
    pub source: String,
    pub sha256: String,
}

impl Binding {
    /// The folder the binding applies to, which `source` is relative to.
    pub fn folder(&self) -> &Path {
        self.file.parent().unwrap_or(Path::new("."))
    }
}

/// The nearest binding file above `doc`: its own folder first, up to and including `home`, or
/// the file system root for a document outside `home`. A nearer binding replaces farther ones.
/// Looks at one name per folder and lists nothing. A link is reported as found: [`read_binding`]
/// refuses it, and the nearest binding must not silently be skipped.
pub fn find_binding(doc: &Path, home: Option<&Path>) -> Option<PathBuf> {
    for dir in doc.ancestors().skip(1).take(MAX_LEVELS) {
        if dir.as_os_str().is_empty() {
            return None;
        }
        let candidate = dir.join(BINDING_FILE);
        if std::fs::symlink_metadata(&candidate).is_ok() {
            return Some(candidate);
        }
        if home == Some(dir) {
            return None;
        }
    }
    None
}

/// Reads and checks the binding at `file`. Touches only that file.
pub fn read_binding(file: &Path) -> Result<Binding, String> {
    let meta = std::fs::symlink_metadata(file).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink() {
        return Err("the binding file is a link".to_string());
    }
    let bytes = markise_doc::resource::read_regular(file, MAX_BINDING_BYTES).map_err(|e| e.to_string())?;
    parse_binding(file, &bytes)
}

/// Checks the bytes of a binding file.
pub fn parse_binding(file: &Path, bytes: &[u8]) -> Result<Binding, String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|e| format!("not valid JSON: {e}"))?;
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("the binding has no \"{key}\""))
    };
    let library = text("library")?;
    if !is_token(library) {
        return Err(format!("library \"{library}\" is not valid"));
    }
    let version = text("version")?;
    if version.is_empty() || version.len() > crate::limits::MAX_NAME_BYTES {
        return Err("version is not valid".to_string());
    }
    let source = text("source")?;
    if source.contains("://") {
        return Err(format!(
            "source \"{source}\" is a network address; this version of Markise reads packages only from a file next to the binding"
        ));
    }
    let clean = clean_relative(source).map_err(|e| format!("source: {e}"))?;
    if clean.is_empty() {
        return Err("source does not name a file".to_string());
    }
    let sha256 = text("sha256")?.to_ascii_lowercase();
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("sha256 must be 64 hexadecimal digits".to_string());
    }
    Ok(Binding {
        file: file.to_path_buf(),
        library: library.to_string(),
        version: version.to_string(),
        source: clean,
        sha256,
    })
}

/// Reads the archive a binding names. The real path, links resolved, must stay inside the
/// binding's folder.
pub fn read_source(binding: &Binding) -> Result<Vec<u8>, String> {
    let folder = binding.folder();
    let real_folder = std::fs::canonicalize(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let path = folder.join(&binding.source);
    let real = std::fs::canonicalize(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !real.starts_with(&real_folder) {
        return Err(format!(
            "{} leads outside the binding's folder through a link",
            binding.source
        ));
    }
    markise_doc::resource::read_regular(&real, crate::limits::MAX_ARCHIVE_BYTES).map_err(|e| e.to_string())
}

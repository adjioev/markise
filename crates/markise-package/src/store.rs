//! The per-user store: unpacked packages by hash, and the record of what the user accepted.
//!
//! ```text
//! <root>/trust.json                      what the user accepted
//! <root>/packages/<sha256>/files/...     the package's files
//! <root>/packages/<sha256>/integrity.json   sha256 of every file
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::PackageId;
use crate::archive::Files;
use crate::limits::{MAX_FILES, MAX_UNPACKED_BYTES};
use crate::paths::clean_relative;

const TRUST_FILE: &str = "trust.json";
const TRUST_VERSION: u64 = 1;
const MAX_TRUST_BYTES: u64 = 4 << 20;
const MAX_INTEGRITY_BYTES: u64 = 4 << 20;

/// The sha256 of `bytes`, lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Something the user accepted: the package's identity and the hash of its archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accepted {
    pub id: PackageId,
    pub sha256: String,
}

/// The store below one folder. Blocking: use it off the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// A store at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Store {
        Store { root: root.into() }
    }

    /// The store of a config folder: `<config>/markise/block-packages`.
    pub fn in_config(config_dir: &Path) -> Store {
        Store::new(config_dir.join("markise").join("block-packages"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn package_dir(&self, sha256: &str) -> PathBuf {
        self.root.join("packages").join(sha256)
    }

    /// The folder holding the unpacked files of `sha256`.
    pub fn files_dir(&self, sha256: &str) -> PathBuf {
        self.package_dir(sha256).join("files")
    }

    /// What the user accepted. A missing or unreadable record counts as nothing accepted, so
    /// damage leads to a question, never to using a package unasked.
    pub fn accepted(&self) -> Vec<Accepted> {
        self.read_trust().map(|t| t.accepted).unwrap_or_default()
    }

    /// Whether the user accepted this library at this hash.
    pub fn is_accepted(&self, library: &str, sha256: &str) -> bool {
        self.accepted()
            .iter()
            .any(|a| a.id.library == library && a.sha256 == sha256)
    }

    /// Records a yes. Another Markise writing at the same time can lose this record, which
    /// only means the question comes again.
    pub fn accept(&self, id: &PackageId, sha256: &str) -> Result<(), String> {
        let mut trust = self.read_trust_for_update()?;
        if !trust
            .accepted
            .iter()
            .any(|a| a.id.library == id.library && a.sha256 == sha256)
        {
            trust.accepted.push(Accepted {
                id: id.clone(),
                sha256: sha256.to_string(),
            });
        }
        self.write_trust(&trust)
    }

    /// Withdraws a yes and removes the unpacked files.
    pub fn forget(&self, library: &str, sha256: &str) -> Result<(), String> {
        let mut trust = self.read_trust_for_update()?;
        trust
            .accepted
            .retain(|a| !(a.id.library == library && a.sha256 == sha256));
        self.write_trust(&trust)?;
        if !trust.accepted.iter().any(|a| a.sha256 == sha256) {
            remove_if_present(&self.package_dir(sha256))?;
        }
        Ok(())
    }

    fn read_trust(&self) -> Result<Trust, String> {
        let path = self.root.join(TRUST_FILE);
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Trust::default()),
            Err(e) => return Err(format!("{}: {e}", path.display())),
            Ok(m) if !m.is_file() => return Err(format!("{} is not a file", path.display())),
            Ok(_) => {}
        }
        let bytes = markise_doc::resource::read_regular(&path, MAX_TRUST_BYTES)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Trust::parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The record to change. A file from a newer Markise is not rewritten; a damaged one is
    /// (it grants nothing).
    fn read_trust_for_update(&self) -> Result<Trust, String> {
        match self.read_trust() {
            Err(e) if e.contains(NEWER_TRUST) => Err(e),
            other => Ok(other.unwrap_or_default()),
        }
    }

    fn write_trust(&self, trust: &Trust) -> Result<(), String> {
        std::fs::create_dir_all(&self.root).map_err(|e| format!("{}: {e}", self.root.display()))?;
        let mut tmp =
            tempfile::NamedTempFile::new_in(&self.root).map_err(|e| format!("{}: {e}", self.root.display()))?;
        std::io::Write::write_all(&mut tmp, trust.to_json().as_bytes()).map_err(|e| e.to_string())?;
        tmp.persist(self.root.join(TRUST_FILE)).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Unpacks `files` as the package `sha256`. The package appears whole or not at all.
    pub fn install(&self, sha256: &str, files: &Files) -> Result<(), String> {
        let packages = self.root.join("packages");
        std::fs::create_dir_all(&packages).map_err(|e| format!("{}: {e}", packages.display()))?;
        let staging = tempfile::Builder::new()
            .prefix(".staging-")
            .tempdir_in(&packages)
            .map_err(|e| format!("{}: {e}", packages.display()))?;
        let files_dir = staging.path().join("files");
        let mut integrity = serde_json::Map::new();
        for (path, content) in files {
            // the archive reader checked these too; the store never trusts its caller on paths
            let clean = clean_relative(path)?;
            if clean.is_empty() || clean != *path {
                return Err(format!("\"{path}\" is not a clean path"));
            }
            let target = files_dir.join(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{path}: {e}"))?;
            }
            write_new(&target, content).map_err(|e| format!("{path}: {e}"))?;
            integrity.insert(path.clone(), Value::String(sha256_hex(content)));
        }
        let record = serde_json::to_vec_pretty(&Value::Object(integrity)).map_err(|e| e.to_string())?;
        write_new(&staging.path().join("integrity.json"), &record).map_err(|e| e.to_string())?;

        let target = self.package_dir(sha256);
        remove_if_present(&target)?;
        let staged = staging.keep();
        if let Err(e) = std::fs::rename(&staged, &target) {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(format!("{}: {e}", target.display()));
        }
        Ok(())
    }

    /// Reads an installed package back, checking every file against its integrity record.
    /// `Ok(None)`: not installed. `Err`: damaged, with the reason.
    pub fn load(&self, sha256: &str) -> Result<Option<Files>, String> {
        let dir = self.package_dir(sha256);
        match std::fs::symlink_metadata(&dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", dir.display())),
            Ok(m) if !m.is_dir() => return Err(format!("{} is not a folder", dir.display())),
            Ok(_) => {}
        }
        let record = markise_doc::resource::read_regular(&dir.join("integrity.json"), MAX_INTEGRITY_BYTES)
            .map_err(|e| format!("the integrity record cannot be read: {e}"))?;
        let record: BTreeMap<String, String> = match serde_json::from_slice::<Value>(&record) {
            Ok(Value::Object(map)) => map
                .into_iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
                .collect(),
            _ => return Err("the integrity record is damaged".to_string()),
        };
        let mut files = Files::new();
        let mut budget = Budget::default();
        read_tree(&dir.join("files"), "", &mut files, &mut budget)?;
        if files.len() != record.len() {
            return Err("files were added to or removed from the package".to_string());
        }
        for (path, content) in &files {
            if record.get(path).map(String::as_str) != Some(sha256_hex(content).as_str()) {
                return Err(format!("{path} was changed"));
            }
        }
        Ok(Some(files))
    }

    /// Removes a damaged package so that it can be installed again.
    pub fn remove_package(&self, sha256: &str) -> Result<(), String> {
        remove_if_present(&self.package_dir(sha256))
    }
}

fn remove_if_present(dir: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", dir.display())),
        // a link or file here is not ours: remove the entry, never what it points to
        Ok(m) if !m.is_dir() => std::fs::remove_file(dir).map_err(|e| format!("{}: {e}", dir.display())),
        Ok(_) => std::fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display())),
    }
}

fn write_new(path: &Path, content: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(content)
}

#[derive(Default)]
struct Budget {
    files: usize,
    bytes: u64,
}

/// Reads a tree without following anything: an entry that is not a plain file or folder is
/// damage, not something to walk into.
fn read_tree(dir: &Path, prefix: &str, out: &mut Files, budget: &mut Budget) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "a file name is not UTF-8".to_string())?;
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let kind = entry.file_type().map_err(|e| format!("{path}: {e}"))?;
        if kind.is_dir() {
            read_tree(&entry.path(), &path, out, budget)?;
        } else if kind.is_file() {
            budget.files += 1;
            if budget.files > MAX_FILES {
                return Err("too many files in the package".to_string());
            }
            let remaining = MAX_UNPACKED_BYTES.saturating_sub(budget.bytes);
            let content =
                markise_doc::resource::read_regular(&entry.path(), remaining).map_err(|e| format!("{path}: {e}"))?;
            budget.bytes += content.len() as u64;
            out.insert(path, content);
        } else {
            return Err(format!("{path} is not a regular file"));
        }
    }
    Ok(())
}

const NEWER_TRUST: &str = "written by a newer version of Markise";

#[derive(Default)]
struct Trust {
    accepted: Vec<Accepted>,
}

impl Trust {
    fn parse(bytes: &[u8]) -> Result<Trust, String> {
        let value: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        match value.get("version").and_then(Value::as_u64) {
            Some(TRUST_VERSION) => {}
            Some(v) if v > TRUST_VERSION => return Err(NEWER_TRUST.to_string()),
            _ => return Err("not a trust record".to_string()),
        }
        let items = value
            .get("accepted")
            .and_then(Value::as_array)
            .ok_or("not a trust record")?;
        let text = |item: &Value, key: &str| item.get(key).and_then(Value::as_str).map(str::to_string);
        Ok(Trust {
            accepted: items
                .iter()
                .filter_map(|item| {
                    Some(Accepted {
                        id: PackageId {
                            library: text(item, "library")?,
                            version: text(item, "version")?,
                        },
                        sha256: text(item, "sha256")?,
                    })
                })
                .collect(),
        })
    }

    fn to_json(&self) -> String {
        let accepted: Vec<Value> = self
            .accepted
            .iter()
            .map(|a| json!({ "library": a.id.library, "version": a.id.version, "sha256": a.sha256 }))
            .collect();
        let value = json!({ "version": TRUST_VERSION, "accepted": accepted });
        serde_json::to_string_pretty(&value).unwrap_or_default()
    }
}

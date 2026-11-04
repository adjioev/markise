//! From a document path to its package: discovery, verification, consent, the cache.
//! Everything here blocks on the file system; the UI calls it on a worker thread.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::archive::{Files, read_archive};
use crate::binding::{Binding, find_binding, read_binding, read_source};
use crate::store::{Store, sha256_hex};
use crate::{PackageError, PackageId, PackageIndex};

/// A package ready for use: accepted, verified, unpacked and indexed.
#[derive(Clone, Debug)]
pub struct InstalledPackage {
    pub binding: Binding,
    pub sha256: String,
    /// The folder with the package's files.
    pub root: PathBuf,
    pub index: Arc<PackageIndex>,
}

impl InstalledPackage {
    /// The file `relative` of the package, for later phases that hand files to a renderer.
    /// The tree holds no links ([`Store::load`] refuses them), so a clean path stays inside.
    pub fn asset_path(&self, relative: &str) -> Result<PathBuf, String> {
        let clean = crate::paths::clean_relative(relative)?;
        if clean.is_empty() {
            return Err("the path does not name a file".to_string());
        }
        Ok(self.root.join(clean))
    }
}

/// A valid package the user has not accepted at this hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub binding: Binding,
    pub id: PackageId,
    pub sha256: String,
    pub block_count: usize,
    /// The version of this library the user accepted before, when this one differs.
    pub previous: Option<PackageId>,
}

/// Why a bound document has no package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// The binding file is unreadable or wrong.
    Binding(String),
    /// The archive it names cannot be read.
    Source(String),
    /// The archive is not the one the binding pins.
    HashMismatch { expected: String, found: String },
    /// The archive is unsafe or broken.
    Archive(String),
    /// The package's own data is invalid or of another format.
    Package(PackageError),
    /// The binding and the package disagree on who this is.
    IdentityMismatch { binding: PackageId, package: PackageId },
    /// The local store cannot be read or written.
    Store(String),
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::Binding(e) => write!(f, "The binding file is not valid: {e}"),
            Problem::Source(e) => write!(f, "The package file cannot be read: {e}"),
            Problem::HashMismatch { expected, found } => write!(
                f,
                "The package file is not the one the binding names: sha256 {found}, expected {expected}."
            ),
            Problem::Archive(e) => write!(f, "The package file was refused: {e}"),
            Problem::Package(e) => write!(f, "The package is not valid: {e}"),
            Problem::IdentityMismatch { binding, package } => write!(
                f,
                "The binding names {} {}, the package says it is {} {}.",
                binding.library, binding.version, package.library, package.version
            ),
            Problem::Store(e) => write!(f, "The local package store failed: {e}"),
        }
    }
}

/// What a document's package state is.
#[derive(Clone, Debug)]
pub enum Resolution {
    /// No binding applies. The common case; nothing else was touched.
    Unbound,
    /// Ask the user.
    Ask(Pending),
    Ready(InstalledPackage),
    /// The binding file, if there is one, and what is wrong.
    Failed {
        binding: Option<PathBuf>,
        problem: Problem,
    },
}

/// Finds what `doc` is bound to. Never installs or records anything for a package the user has
/// not accepted: a document can only cause reads until they say yes.
pub fn resolve(doc: &Path, store: &Store, home: Option<&Path>) -> Resolution {
    let Some(file) = find_binding(doc, home) else {
        return Resolution::Unbound;
    };
    let failed = |problem| Resolution::Failed {
        binding: Some(file.clone()),
        problem,
    };
    let binding = match read_binding(&file) {
        Ok(b) => b,
        Err(e) => return failed(Problem::Binding(e)),
    };
    let result = if store.is_accepted(&binding.library, &binding.sha256) {
        ready(&binding, store).map(Resolution::Ready)
    } else {
        inspect(&binding, store).map(Resolution::Ask)
    };
    result.unwrap_or_else(failed)
}

/// The user said yes: verifies the archive again, unpacks it and records the consent.
pub fn accept(pending: &Pending, store: &Store) -> Result<InstalledPackage, Problem> {
    let verified = verified_source(&pending.binding)?;
    if verified.index.id() != &pending.id {
        return Err(identity_problem(&pending.binding, verified.index.id()));
    }
    store
        .install(&pending.binding.sha256, &verified.files)
        .map_err(Problem::Store)?;
    store
        .accept(&pending.id, &pending.binding.sha256)
        .map_err(Problem::Store)?;
    Ok(installed(&pending.binding, store, verified.index))
}

fn installed(binding: &Binding, store: &Store, index: PackageIndex) -> InstalledPackage {
    InstalledPackage {
        binding: binding.clone(),
        sha256: binding.sha256.clone(),
        root: store.files_dir(&binding.sha256),
        index: Arc::new(index),
    }
}

/// An accepted package: from the cache, or unpacked again when the cache lost or damaged it.
fn ready(binding: &Binding, store: &Store) -> Result<InstalledPackage, Problem> {
    let cached = store.load(&binding.sha256);
    if let Ok(Some(files)) = cached {
        let index = PackageIndex::from_files(&files).map_err(Problem::Package)?;
        check_identity(binding, &index)?;
        return Ok(installed(binding, store, index));
    }
    if cached.is_err() {
        store.remove_package(&binding.sha256).map_err(Problem::Store)?;
    }
    let verified = verified_source(binding)?;
    store
        .install(&binding.sha256, &verified.files)
        .map_err(Problem::Store)?;
    Ok(installed(binding, store, verified.index))
}

fn inspect(binding: &Binding, store: &Store) -> Result<Pending, Problem> {
    let verified = verified_source(binding)?;
    let previous = store
        .accepted()
        .into_iter()
        .rfind(|a| a.id.library == binding.library)
        .map(|a| a.id);
    Ok(Pending {
        binding: binding.clone(),
        id: verified.index.id().clone(),
        sha256: binding.sha256.clone(),
        block_count: verified.index.block_count(),
        previous,
    })
}

struct Verified {
    files: Files,
    index: PackageIndex,
}

/// The binding's archive, hashed and validated from one read of its bytes.
fn verified_source(binding: &Binding) -> Result<Verified, Problem> {
    let bytes = read_source(binding).map_err(Problem::Source)?;
    let found = sha256_hex(&bytes);
    if found != binding.sha256 {
        return Err(Problem::HashMismatch {
            expected: binding.sha256.clone(),
            found,
        });
    }
    let files = read_archive(&bytes).map_err(Problem::Archive)?;
    let index = PackageIndex::from_files(&files).map_err(Problem::Package)?;
    check_identity(binding, &index)?;
    Ok(Verified { files, index })
}

fn check_identity(binding: &Binding, index: &PackageIndex) -> Result<(), Problem> {
    if index.id().library == binding.library && index.id().version == binding.version {
        Ok(())
    } else {
        Err(identity_problem(binding, index.id()))
    }
}

fn identity_problem(binding: &Binding, package: &PackageId) -> Problem {
    Problem::IdentityMismatch {
        binding: PackageId {
            library: binding.library.clone(),
            version: binding.version.clone(),
        },
        package: package.clone(),
    }
}

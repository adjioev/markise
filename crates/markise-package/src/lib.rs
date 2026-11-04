//! `markise-package` — block packages (layer L2, ADR 0019).
//!
//! A block package is data a site owns: JavaScript, CSS, one JSON schema per block, a palette
//! and optional view settings. This crate validates and indexes that data. It has no vocabulary
//! of any site and it executes nothing: script and style files stay opaque.
//!
//! It also finds the package a document is bound to, verifies it, keeps it in a per-user store
//! and records the user's consent. [`resolve`] and [`accept`] block on the file system: call
//! them off the UI thread.

mod archive;
mod binding;
mod error;
mod index;
pub mod limits;
mod palette;
mod paths;
mod resolve;
mod schema;
mod store;
mod value;
mod views;

pub use archive::{Files, read_archive};
pub use binding::{BINDING_FILE, Binding, find_binding, parse_binding, read_binding, read_source};
pub use error::{Issue, PackageError};
pub use index::{BlockEntry, PackageIndex};
pub use palette::{Palette, PaletteBlock, PaletteGroup, Preset};
pub use paths::clean_relative;
pub use resolve::{InstalledPackage, Pending, Problem, Resolution, accept, resolve};
pub use schema::{BlockSchema, FieldKind, FieldSchema};
pub use store::{Accepted, Store, sha256_hex};
pub use value::Localized;
pub use views::{ViewConfig, ViewKind};

/// The one package format this build reads, as `manifest.json` names it.
pub const FORMAT: &str = "markise-block-package/2";
/// The stem every package format name starts with.
pub const FORMAT_STEM: &str = "markise-block-package/";

/// Who a package says it is: `manifest.json`'s `library` and `version`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackageId {
    pub library: String,
    pub version: String,
}

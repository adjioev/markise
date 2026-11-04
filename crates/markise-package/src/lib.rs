//! `markise-package` — block packages (layer L2, ADR 0019).
//!
//! A block package is data a site owns: JavaScript, CSS, one JSON schema per block, a palette
//! and optional view settings. This crate validates and indexes that data. It has no vocabulary
//! of any site and it executes nothing: script and style files stay opaque.

mod error;
mod index;
pub mod limits;
mod palette;
mod schema;
mod value;
mod views;

pub use error::{Issue, PackageError};
pub use index::{BlockEntry, PackageIndex};
pub use palette::{Palette, PaletteBlock, PaletteGroup, Preset};
pub use schema::{BlockSchema, FieldKind, FieldSchema};
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

//! Validating a package's files into the index later code asks.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::error::{Issue, Issues, PackageError};
use crate::limits::{MAX_BLOCKS, MAX_NAME_BYTES};
use crate::palette::{self, Palette, PaletteBlock, PaletteGroup, Preset};
use crate::schema::{self, BlockSchema};
use crate::value::{is_block_name, is_token, parse_object};
use crate::views::{self, ViewConfig};
use crate::{FORMAT, FORMAT_STEM, PackageId};

pub(crate) const MANIFEST: &str = "manifest.json";
const PALETTE: &str = "editor/palette.json";
const VIEWS: &str = "editor/views.json";

/// A valid package: name to schema, view and palette entry.
#[derive(Clone, Debug, PartialEq)]
pub struct PackageIndex {
    id: PackageId,
    blocks: BTreeMap<String, BlockSchema>,
    palette: Palette,
    views: BTreeMap<String, ViewConfig>,
}

/// Everything the package says about one block.
#[derive(Clone, Copy, Debug)]
pub struct BlockEntry<'a> {
    pub schema: &'a BlockSchema,
    pub view: Option<&'a ViewConfig>,
    pub palette: Option<&'a PaletteBlock>,
}

impl PackageIndex {
    /// Validates a package given as `path -> content`, with `/` separated paths relative to the
    /// package root. Only the manifest, the schemas, the palette and the views are read; every
    /// other file is opaque here.
    pub fn from_files(files: &BTreeMap<String, Vec<u8>>) -> Result<PackageIndex, PackageError> {
        let mut issues = Issues::default();
        let refuse = |message: String| {
            PackageError::Invalid(vec![Issue {
                file: MANIFEST.to_string(),
                message,
            }])
        };
        let manifest = files
            .get(MANIFEST)
            .ok_or_else(|| refuse("the package has no manifest".to_string()))?;
        let Some(manifest) = parse_object(MANIFEST, manifest, &mut issues) else {
            return issues.into_result(PackageIndex::empty);
        };
        // The format decides how to read everything else, so it is checked before anything else.
        match manifest.get("format").and_then(Value::as_str) {
            Some(FORMAT) => {}
            Some(found) if found.starts_with(FORMAT_STEM) => {
                return Err(PackageError::UnsupportedFormat {
                    found: found.chars().take(MAX_NAME_BYTES).collect(),
                });
            }
            Some(found) => return Err(refuse(format!("\"{found}\" is not a package format"))),
            None => return Err(refuse("the manifest does not name a format".to_string())),
        }

        let library = manifest_token(&manifest, "library", &mut issues);
        let version = manifest_token(&manifest, "version", &mut issues);
        let names = manifest_blocks(&manifest, &mut issues);
        let known: BTreeSet<String> = names.iter().cloned().collect();

        let mut blocks = BTreeMap::new();
        for name in &names {
            let file = format!("components/{name}/{name}.json");
            match files.get(&file) {
                Some(bytes) => {
                    if let Some(map) = parse_object(&file, bytes, &mut issues) {
                        blocks.insert(name.clone(), schema::parse(&file, name, map, &mut issues));
                    }
                }
                None => issues.push(&file, "the manifest lists this block, the file is missing"),
            }
        }
        for path in files.keys() {
            if let Some(name) = undeclared_schema(path, &known) {
                issues.push(path, format!("block \"{name}\" is not in the manifest"));
            }
        }

        let mut palette = Palette::default();
        match files.get(PALETTE) {
            Some(bytes) => {
                if let Some(map) = parse_object(PALETTE, bytes, &mut issues) {
                    palette = palette::parse(PALETTE, map, &known, &mut issues);
                }
            }
            None => issues.push(PALETTE, "the package has no palette"),
        }
        let mut view_map = BTreeMap::new();
        if let Some(bytes) = files.get(VIEWS)
            && let Some(map) = parse_object(VIEWS, bytes, &mut issues)
        {
            view_map = views::parse(VIEWS, map, &known, &mut issues);
        }

        issues.into_result(|| PackageIndex {
            id: PackageId { library, version },
            blocks,
            palette,
            views: view_map,
        })
    }

    /// Never returned to a caller: `parse_object` pushed an issue on this path.
    fn empty() -> PackageIndex {
        PackageIndex {
            id: PackageId {
                library: String::new(),
                version: String::new(),
            },
            blocks: BTreeMap::new(),
            palette: Palette::default(),
            views: BTreeMap::new(),
        }
    }

    pub fn id(&self) -> &PackageId {
        &self.id
    }

    /// The block's entry; `None` for a name the package does not define.
    pub fn block(&self, name: &str) -> Option<BlockEntry<'_>> {
        let schema = self.blocks.get(name)?;
        Some(BlockEntry {
            schema,
            view: self.views.get(name),
            palette: self.palette.blocks.get(name),
        })
    }

    /// The block names, sorted.
    pub fn block_names(&self) -> impl Iterator<Item = &str> {
        self.blocks.keys().map(String::as_str)
    }

    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    pub fn groups(&self) -> &[PaletteGroup] {
        &self.palette.groups
    }

    /// The presets, in the palette's order.
    pub fn presets(&self) -> &[Preset] {
        &self.palette.presets
    }

    /// The presets of one block, in the palette's order.
    pub fn presets_of<'a>(&'a self, block: &'a str) -> impl Iterator<Item = &'a Preset> {
        self.palette.presets.iter().filter(move |p| p.block == block)
    }
}

fn manifest_token(manifest: &serde_json::Map<String, Value>, key: &str, issues: &mut Issues) -> String {
    match manifest.get(key).and_then(Value::as_str) {
        Some(s) if is_token(s) || (key == "version" && is_version(s)) => s.to_string(),
        Some(s) => {
            issues.push(MANIFEST, format!("{key} \"{s}\" is not valid"));
            String::new()
        }
        None => {
            issues.push(MANIFEST, format!("the manifest has no {key}"));
            String::new()
        }
    }
}

/// A token that may also carry SemVer's `+` build part.
fn is_version(s: &str) -> bool {
    s.len() <= MAX_NAME_BYTES && s.split('+').count() <= 2 && s.split('+').all(is_token)
}

fn manifest_blocks(manifest: &serde_json::Map<String, Value>, issues: &mut Issues) -> Vec<String> {
    let Some(items) = manifest.get("blocks").and_then(Value::as_array) else {
        issues.push(MANIFEST, "blocks must be an array of names");
        return Vec::new();
    };
    if items.is_empty() {
        issues.push(MANIFEST, "the package defines no blocks");
    }
    if items.len() > MAX_BLOCKS {
        issues.push(MANIFEST, format!("{} blocks, the limit is {MAX_BLOCKS}", items.len()));
        return Vec::new();
    }
    let mut seen = BTreeSet::new();
    let mut names = Vec::new();
    for item in items {
        match item.as_str() {
            Some(name) if is_block_name(name) => {
                if seen.insert(name) {
                    names.push(name.to_string());
                } else {
                    issues.push(MANIFEST, format!("block \"{name}\" appears twice"));
                }
            }
            Some(name) => issues.push(MANIFEST, format!("\"{name}\" is not a valid block name")),
            None => issues.push(MANIFEST, "a block name must be a string"),
        }
    }
    names
}

/// The block name if `path` is a schema file of a block the manifest does not list.
fn undeclared_schema<'a>(path: &'a str, known: &BTreeSet<String>) -> Option<&'a str> {
    let rest = path.strip_prefix("components/")?;
    let (name, file) = rest.split_once('/')?;
    let stem = file.strip_suffix(".json")?;
    (stem == name && !known.contains(name)).then_some(name)
}

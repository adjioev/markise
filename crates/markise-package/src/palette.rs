//! `editor/palette.json`: how a package's blocks are offered for insertion.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::error::Issues;
use crate::limits::{MAX_GROUPS, MAX_PRESET_MARKDOWN_BYTES, MAX_PRESETS, MAX_TEXT_BYTES};
use crate::value::{Localized, opt_localized, opt_text, opt_token};

/// The palette format this build reads, as the file's optional `format` key names it.
const PALETTE_FORMAT: u64 = 1;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Palette {
    pub groups: Vec<PaletteGroup>,
    pub blocks: BTreeMap<String, PaletteBlock>,
    pub presets: Vec<Preset>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaletteGroup {
    pub id: String,
    pub label: Option<Localized>,
    pub blocks: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaletteBlock {
    pub description: Option<Localized>,
    /// An icon name; the package supplies what it means.
    pub icon: Option<String>,
}

/// A ready piece of Markdown to insert.
#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub id: String,
    pub block: String,
    pub label: Option<Localized>,
    pub keywords: Option<String>,
    pub markdown: String,
    /// Every other key of the preset, kept as the package wrote it.
    pub extra: serde_json::Map<String, Value>,
}

/// Parses the palette. `known` holds the package's block names; a reference to any other name
/// is an issue.
pub(crate) fn parse(
    file: &str,
    map: serde_json::Map<String, Value>,
    known: &BTreeSet<String>,
    issues: &mut Issues,
) -> Palette {
    match map.get("format") {
        None => {}
        Some(v) if v.as_u64() == Some(PALETTE_FORMAT) => {}
        Some(v) => issues.push(file, format!("unsupported palette format {v}")),
    }
    Palette {
        groups: parse_groups(file, map.get("groups"), known, issues),
        blocks: parse_blocks(file, map.get("blocks"), known, issues),
        presets: parse_presets(file, map.get("presets"), known, issues),
    }
}

fn check_block(file: &str, what: &str, name: &str, known: &BTreeSet<String>, issues: &mut Issues) {
    if !known.contains(name) {
        issues.push(file, format!("{what} names the unknown block \"{name}\""));
    }
}

fn parse_groups(file: &str, v: Option<&Value>, known: &BTreeSet<String>, issues: &mut Issues) -> Vec<PaletteGroup> {
    let items = match v {
        None | Some(Value::Null) => return Vec::new(),
        Some(Value::Array(items)) if items.len() <= MAX_GROUPS => items,
        Some(Value::Array(items)) => {
            issues.push(file, format!("{} groups, the limit is {MAX_GROUPS}", items.len()));
            return Vec::new();
        }
        Some(_) => {
            issues.push(file, "groups must be an array");
            return Vec::new();
        }
    };
    let mut out: Vec<PaletteGroup> = Vec::new();
    let mut listed = BTreeSet::new();
    for (i, item) in items.iter().enumerate() {
        let what = format!("group {i}");
        let Some(id) = opt_token(file, &what, item.get("id"), issues) else {
            issues.push(file, format!("{what} has no id"));
            continue;
        };
        if out.iter().any(|g| g.id == id) {
            issues.push(file, format!("group id \"{id}\" appears twice"));
        }
        let label = opt_localized(file, &format!("group \"{id}\" label"), item.get("label"), issues);
        let mut blocks = Vec::new();
        match item.get("blocks") {
            Some(Value::Array(names)) => {
                for name in names {
                    match name.as_str() {
                        Some(name) => {
                            check_block(file, &format!("group \"{id}\""), name, known, issues);
                            if !listed.insert(name.to_string()) {
                                issues.push(file, format!("group \"{id}\": a block is listed more than once"));
                                continue;
                            }
                            blocks.push(name.to_string());
                        }
                        None => {
                            issues.push(file, format!("group \"{id}\": a block must be a string"));
                        }
                    }
                }
            }
            _ => issues.push(file, format!("group \"{id}\": blocks must be an array")),
        }
        out.push(PaletteGroup { id, label, blocks });
    }
    out
}

fn parse_blocks(
    file: &str,
    v: Option<&Value>,
    known: &BTreeSet<String>,
    issues: &mut Issues,
) -> BTreeMap<String, PaletteBlock> {
    let map = match v {
        None | Some(Value::Null) => return BTreeMap::new(),
        Some(Value::Object(map)) => map,
        Some(_) => {
            issues.push(file, "blocks must be an object");
            return BTreeMap::new();
        }
    };
    let mut out = BTreeMap::new();
    for (name, def) in map {
        // known names are short; an unknown one must not be copied into a message per language
        if !known.contains(name) {
            let shown: String = name.chars().take(64).collect();
            issues.push(file, format!("blocks names the unknown block \"{shown}\""));
            continue;
        }
        let what = format!("block \"{name}\"");
        if !def.is_object() {
            issues.push(file, format!("{what} must be an object"));
            continue;
        }
        out.insert(
            name.clone(),
            PaletteBlock {
                description: opt_localized(file, &format!("{what} description"), def.get("description"), issues),
                icon: opt_token(file, &format!("{what} icon"), def.get("icon"), issues),
            },
        );
    }
    out
}

fn parse_presets(file: &str, v: Option<&Value>, known: &BTreeSet<String>, issues: &mut Issues) -> Vec<Preset> {
    let items = match v {
        None | Some(Value::Null) => return Vec::new(),
        Some(Value::Array(items)) if items.len() <= MAX_PRESETS => items,
        Some(Value::Array(items)) => {
            issues.push(file, format!("{} presets, the limit is {MAX_PRESETS}", items.len()));
            return Vec::new();
        }
        Some(_) => {
            issues.push(file, "presets must be an array");
            return Vec::new();
        }
    };
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let what = format!("preset {i}");
        let Some(map) = item.as_object() else {
            issues.push(file, format!("{what} must be an object"));
            continue;
        };
        let Some(id) = opt_token(file, &what, map.get("id"), issues) else {
            issues.push(file, format!("{what} has no id"));
            continue;
        };
        let what = format!("preset \"{id}\"");
        if !seen.insert(id.clone()) {
            issues.push(file, format!("{what} appears twice"));
        }
        let Some(block) = map.get("block").and_then(Value::as_str) else {
            issues.push(file, format!("{what} has no block"));
            continue;
        };
        check_block(file, &what, block, known, issues);
        let markdown = opt_text(
            file,
            &format!("{what} markdown"),
            map.get("markdown"),
            MAX_PRESET_MARKDOWN_BYTES,
            issues,
        );
        let Some(markdown) = markdown else {
            issues.push(file, format!("{what} has no markdown"));
            continue;
        };
        if !opens_block(&markdown, block) {
            issues.push(file, format!("{what}: markdown must start with :::{block}"));
        }
        let label = opt_localized(file, &format!("{what} label"), map.get("label"), issues);
        let keywords = opt_text(
            file,
            &format!("{what} keywords"),
            map.get("keywords"),
            MAX_TEXT_BYTES,
            issues,
        );
        let extra = map
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "id" | "block" | "label" | "keywords" | "markdown"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        out.push(Preset {
            id,
            block: block.to_string(),
            label,
            keywords,
            markdown,
            extra,
        });
    }
    out
}

/// Whether the first line of `markdown` opens `:::block` (the fence may be longer).
fn opens_block(markdown: &str, block: &str) -> bool {
    let line = markdown.lines().next().unwrap_or("");
    let rest = line.trim_start_matches(':');
    line.len() - rest.len() >= 3
        && rest
            .strip_prefix(block)
            .is_some_and(|tail| tail.is_empty() || tail.starts_with(|c: char| c.is_whitespace()))
}

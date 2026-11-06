//! `editor/views.json`: which generic view shows a block. Optional, per block.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::error::Issues;
use crate::value::opt_token;

/// The generic view kinds. Markise has no others, and no per-site ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewKind {
    Callout,
    Card,
    Choices,
    Media,
}

impl ViewKind {
    fn parse(s: &str) -> Option<ViewKind> {
        Some(match s {
            "callout" => ViewKind::Callout,
            "card" => ViewKind::Card,
            "choices" => ViewKind::Choices,
            "media" => ViewKind::Media,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ViewConfig {
    pub kind: ViewKind,
    /// A color name, its meaning is up to the view.
    pub accent: Option<String>,
    /// An icon name; the package supplies what it means.
    pub icon: Option<String>,
    /// `false`: no tinted ground, a thin bar instead. Default `true`.
    pub fill: bool,
    /// `false`: no label row; the block's own heading is enough. Default `true`.
    pub label: bool,
    /// Every other key, kept as the package wrote it.
    pub extra: serde_json::Map<String, Value>,
}

/// An optional `true` or `false` of a view; both default to what a view does without the key.
fn opt_switch(file: &str, name: &str, key: &str, def: &serde_json::Map<String, Value>, issues: &mut Issues) -> bool {
    match def.get(key) {
        None | Some(Value::Bool(true)) => true,
        Some(Value::Bool(false)) => false,
        Some(_) => {
            issues.push(file, format!("the view of \"{name}\": \"{key}\" must be true or false"));
            true
        }
    }
}

pub(crate) fn parse(
    file: &str,
    map: serde_json::Map<String, Value>,
    known: &BTreeSet<String>,
    issues: &mut Issues,
) -> BTreeMap<String, ViewConfig> {
    let mut out = BTreeMap::new();
    for (name, def) in map {
        if !known.contains(&name) {
            issues.push(file, format!("a view names the unknown block \"{name}\""));
        }
        let Some(def) = def.as_object() else {
            issues.push(file, format!("the view of \"{name}\" must be an object"));
            continue;
        };
        let Some(kind_name) = def.get("kind").and_then(Value::as_str) else {
            issues.push(file, format!("the view of \"{name}\" has no kind"));
            continue;
        };
        let Some(kind) = ViewKind::parse(kind_name) else {
            issues.push(
                file,
                format!("the view of \"{name}\" has the unknown kind \"{kind_name}\""),
            );
            continue;
        };
        let accent = opt_token(file, &format!("\"{name}\" accent"), def.get("accent"), issues);
        let icon = opt_token(file, &format!("\"{name}\" icon"), def.get("icon"), issues);
        let fill = opt_switch(file, &name, "fill", def, issues);
        let label = opt_switch(file, &name, "label", def, issues);
        let extra = def
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "kind" | "accent" | "icon" | "fill" | "label"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        out.insert(
            name,
            ViewConfig {
                kind,
                accent,
                icon,
                fill,
                label,
                extra,
            },
        );
    }
    out
}

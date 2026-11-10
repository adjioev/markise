//! `editor/views.json`: which generic view shows a block. Optional, per block.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::error::Issues;
use crate::schema::{BlockSchema, FieldKind};
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
    /// The block can be practised (ADR 0024).
    pub practice: Option<PracticeConfig>,
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

/// `"practice"` of a view: what a block shows while someone answers it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PracticeConfig {
    /// The fields hidden until the answers are checked, by name.
    pub reveal: Vec<String>,
    /// A block of this view opens in Practice, until someone chooses otherwise.
    pub default: bool,
}

/// `"practice"` as far as it can be read without the block's schema ([`check_practice`] has the rest).
fn parse_practice(
    file: &str,
    name: &str,
    kind: ViewKind,
    value: Option<&Value>,
    issues: &mut Issues,
) -> Option<PracticeConfig> {
    let value = value?;
    if kind == ViewKind::Media {
        issues.push(
            file,
            format!("\"{name}\" has `practice`, which a `media` view cannot have: it shows only pictures"),
        );
        return None;
    }
    let Some(def) = value.as_object() else {
        issues.push(file, format!("`practice` of \"{name}\" must be an object"));
        return None;
    };
    issues.unknown_keys(file, &format!("`practice` of \"{name}\""), def, &["reveal", "default"]);
    let mut reveal = Vec::new();
    match def.get("reveal") {
        None => {}
        Some(Value::Array(names)) => {
            for field in names {
                let Some(field) = field.as_str() else {
                    issues.push(
                        file,
                        format!("`practice.reveal` of \"{name}\" must list field names, found {field}"),
                    );
                    continue;
                };
                if reveal.iter().any(|r| r == field) {
                    issues.push(file, format!("`practice.reveal` of \"{name}\" names \"{field}\" twice"));
                } else {
                    reveal.push(field.to_string());
                }
            }
        }
        Some(_) => issues.push(file, format!("`practice.reveal` of \"{name}\" must be a list")),
    }
    let default = match def.get("default") {
        None => false,
        Some(Value::Bool(default)) => *default,
        Some(_) => {
            issues.push(file, format!("`practice.default` of \"{name}\" must be true or false"));
            false
        }
    };
    Some(PracticeConfig { reveal, default })
}

/// What `"practice"` asks of the block's schema: answers to check or something to reveal, and fields
/// that can be hidden.
pub(crate) fn check_practice(
    file: &str,
    name: &str,
    practice: &PracticeConfig,
    schema: &BlockSchema,
    issues: &mut Issues,
) {
    if practice.reveal.is_empty() && !schema.fields.iter().any(|f| f.kind == FieldKind::Choices) {
        issues.push(
            file,
            format!("\"{name}\" has `practice` with no `choices` field to check and nothing in `reveal` to show"),
        );
    }
    for field in &practice.reveal {
        match schema.fields.iter().find(|f| f.name == *field) {
            None => issues.push(
                file,
                format!("`practice.reveal` of \"{name}\" names the unknown field \"{field}\""),
            ),
            Some(f) if f.kind == FieldKind::Choices => issues.push(
                file,
                format!("`practice.reveal` of \"{name}\" names \"{field}\", the answers themselves"),
            ),
            // a heading stands in the document's outline whatever the view shows
            Some(f) if f.kind == FieldKind::Heading => issues.push(
                file,
                format!("`practice.reveal` of \"{name}\" names \"{field}\", a heading, which the outline shows"),
            ),
            Some(_) => {}
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
        issues.unknown_keys(
            file,
            &format!("the view of \"{name}\""),
            def,
            &["kind", "accent", "icon", "fill", "label", "practice"],
        );
        let accent = opt_token(file, &format!("\"{name}\" accent"), def.get("accent"), issues);
        let icon = opt_token(file, &format!("\"{name}\" icon"), def.get("icon"), issues);
        let fill = opt_switch(file, &name, "fill", def, issues);
        let label = opt_switch(file, &name, "label", def, issues);
        let practice = parse_practice(file, &name, kind, def.get("practice"), issues);
        let extra = def
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "kind" | "accent" | "icon" | "fill" | "label" | "practice"))
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
                practice,
                extra,
            },
        );
    }
    out
}

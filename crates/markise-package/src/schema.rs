//! `components/<name>/<name>.json`: one block's schema.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::error::Issues;
use crate::limits::MAX_FIELDS;
use crate::value::{Localized, is_block_name, opt_localized};

/// What a field holds in the block's Markdown. These are generic primitives; a package names
/// them and Markise knows them, so a site adds no type of its own (ADR 0019).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Heading,
    Text,
    List,
    Choices,
    Quote,
    Image,
}

impl FieldKind {
    fn parse(s: &str) -> Option<FieldKind> {
        Some(match s {
            "heading" => FieldKind::Heading,
            "text" => FieldKind::Text,
            "list" => FieldKind::List,
            "choices" => FieldKind::Choices,
            "quote" => FieldKind::Quote,
            "image" => FieldKind::Image,
            _ => return None,
        })
    }
}

/// One field. `raw` holds the field's whole JSON object: the constraints in it (`min`, `level`
/// and so on) are read by the field model, not here.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldSchema {
    pub name: String,
    pub kind: FieldKind,
    pub raw: Value,
}

/// A block's schema. `raw` holds the file's whole JSON object.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockSchema {
    pub name: String,
    pub label: Option<Localized>,
    pub fields: Vec<FieldSchema>,
    pub raw: Value,
}

pub(crate) fn parse(file: &str, name: &str, map: serde_json::Map<String, Value>, issues: &mut Issues) -> BlockSchema {
    let label = opt_localized(file, "label", map.get("label"), issues);
    if let Some(variants) = map.get("variants") {
        match variants {
            Value::Object(variants) => {
                for (variant, text) in variants {
                    let what = format!("variant \"{variant}\"");
                    opt_localized(file, &what, Some(text), issues);
                }
            }
            _ => issues.push(file, "variants must be an object"),
        }
    }
    if let Some(argument) = map.get("argument")
        && !argument.is_object()
    {
        issues.push(file, "argument must be an object");
    }
    let mut fields = Vec::new();
    match map.get("fields") {
        Some(Value::Array(items)) if items.len() > MAX_FIELDS => {
            issues.push(file, format!("{} fields, the limit is {MAX_FIELDS}", items.len()));
        }
        Some(Value::Array(items)) => {
            let mut seen = BTreeSet::new();
            for (i, item) in items.iter().enumerate() {
                if let Some(f) = parse_field(file, i, item, issues) {
                    if !seen.insert(f.name.clone()) {
                        issues.push(file, format!("field \"{}\" appears twice", f.name));
                    }
                    fields.push(f);
                }
            }
        }
        _ => issues.push(file, "fields must be an array"),
    }
    BlockSchema {
        name: name.to_string(),
        label,
        fields,
        raw: Value::Object(map),
    }
}

fn parse_field(file: &str, i: usize, item: &Value, issues: &mut Issues) -> Option<FieldSchema> {
    let Some(map) = item.as_object() else {
        issues.push(file, format!("field {i} must be an object"));
        return None;
    };
    let Some(name) = map.get("name").and_then(Value::as_str) else {
        issues.push(file, format!("field {i} has no name"));
        return None;
    };
    if !is_block_name(name) {
        issues.push(file, format!("field {i}: \"{name}\" is not a valid name"));
        return None;
    }
    let Some(type_name) = map.get("type").and_then(Value::as_str) else {
        issues.push(file, format!("field \"{name}\" has no type"));
        return None;
    };
    let Some(kind) = FieldKind::parse(type_name) else {
        issues.push(file, format!("field \"{name}\" has the unknown type \"{type_name}\""));
        return None;
    };
    Some(FieldSchema {
        name: name.to_string(),
        kind,
        raw: item.clone(),
    })
}

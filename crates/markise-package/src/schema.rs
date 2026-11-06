//! `components/<name>/<name>.json`: one block's schema.

use std::collections::BTreeSet;

use schema_form_core::Pattern;
use serde_json::Value;

use crate::error::Issues;
use crate::limits::{MAX_FIELDS, MAX_NAME_BYTES};
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
                    // before the name is copied into a message per language below
                    if variant.len() > MAX_NAME_BYTES {
                        issues.push(file, format!("a variant name is longer than {MAX_NAME_BYTES} bytes"));
                        continue;
                    }
                    let what = format!("variant \"{variant}\"");
                    opt_localized(file, &what, Some(text), issues);
                }
            }
            _ => issues.push(file, "variants must be an object"),
        }
    }
    match map.get("argument") {
        None => {}
        Some(Value::Object(argument)) => check_argument_pattern(file, argument, issues),
        Some(_) => issues.push(file, "argument must be an object"),
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

/// A pattern that could not be run within its bounds is the package's mistake, and it is said here,
/// not on the first block that would use it.
fn check_argument_pattern(file: &str, argument: &serde_json::Map<String, Value>, issues: &mut Issues) {
    match argument.get("pattern") {
        None => {}
        Some(Value::String(pattern)) => {
            if let Err(why) = Pattern::new(pattern) {
                issues.push(file, format!("the argument's pattern {why}"));
            }
        }
        Some(_) => issues.push(file, "the argument's pattern must be a string"),
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

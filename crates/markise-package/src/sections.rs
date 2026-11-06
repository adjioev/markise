//! `editor/sections.json`: what a site says about its sections, as data (ADR 0026). Optional. Markise knows
//! only "a section"; what the site calls one and what metadata it wants are the site's.

use serde_json::Value;

use crate::error::Issues;
use crate::limits::MAX_TEXT_BYTES;
use crate::value::{Localized, opt_localized, opt_text, opt_token};

const SECTIONS_FORMAT: u64 = 1;
/// A section's metadata has at most 20 keys, `id` among them (ADR 0026).
const MAX_FIELDS: usize = 19;
const MAX_VALUES: usize = 256;
const MAX_VALUE_CHARS: usize = 200;

/// What `editor/sections.json` says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SectionConfig {
    /// What the library's item says, and the form's title.
    pub label: Option<Localized>,
    pub description: Option<Localized>,
    pub keywords: Option<String>,
    /// An icon name, as a block's: `assets/<name>.svg` (ADR 0019).
    pub icon: Option<String>,
    /// The metadata a new section's form asks for, in this order. None: the form asks for none.
    pub fields: Vec<SectionField>,
}

/// One key of a section's metadata the site declares.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionField {
    pub key: String,
    pub label: Option<Localized>,
    /// The values to choose from. None: any one line.
    pub values: Vec<SectionValue>,
}

/// A value to choose for a key.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionValue {
    pub value: String,
    pub label: Option<Localized>,
}

/// The grammar of a metadata key (ADR 0026).
pub(crate) fn is_section_key(s: &str) -> bool {
    let mut chars = s.chars();
    s.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The grammar of a metadata value, which is literal: one line, not empty, with nothing around it.
pub fn is_section_value(s: &str) -> bool {
    !s.is_empty() && s.trim() == s && s.chars().count() <= MAX_VALUE_CHARS && !s.chars().any(char::is_control)
}

pub(crate) fn parse(file: &str, map: serde_json::Map<String, Value>, issues: &mut Issues) -> SectionConfig {
    match map.get("format") {
        None => {}
        Some(v) if v.as_u64() == Some(SECTIONS_FORMAT) => {}
        Some(v) => issues.push(file, format!("unsupported sections format {v}")),
    }
    SectionConfig {
        label: opt_localized(file, "label", map.get("label"), issues),
        description: opt_localized(file, "description", map.get("description"), issues),
        keywords: opt_text(file, "keywords", map.get("keywords"), MAX_TEXT_BYTES, issues),
        icon: opt_token(file, "icon", map.get("icon"), issues),
        fields: parse_fields(file, map.get("fields"), issues),
    }
}

fn parse_fields(file: &str, v: Option<&Value>, issues: &mut Issues) -> Vec<SectionField> {
    let items = match v {
        None | Some(Value::Null) => return Vec::new(),
        Some(Value::Array(items)) if items.len() <= MAX_FIELDS => items,
        Some(Value::Array(items)) => {
            issues.push(file, format!("{} fields, the limit is {MAX_FIELDS}", items.len()));
            return Vec::new();
        }
        Some(_) => {
            issues.push(file, "fields must be an array");
            return Vec::new();
        }
    };
    let mut out: Vec<SectionField> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let Some(key) = item.get("key").and_then(Value::as_str) else {
            issues.push(file, format!("field {i} has no key"));
            continue;
        };
        if !is_section_key(key) {
            issues.push(file, format!("\"{key}\" is not a metadata key"));
            continue;
        }
        // written on the section's heading, by the form's own field: two would repeat it
        if key == "id" {
            issues.push(file, "`id` cannot be a field: a new section's form has its own");
            continue;
        }
        if out.iter().any(|f| f.key == key) {
            issues.push(file, format!("field \"{key}\" appears twice"));
            continue;
        }
        out.push(SectionField {
            key: key.to_string(),
            label: opt_localized(file, &format!("field \"{key}\" label"), item.get("label"), issues),
            values: parse_values(file, key, item.get("values"), issues),
        });
    }
    out
}

fn parse_values(file: &str, key: &str, v: Option<&Value>, issues: &mut Issues) -> Vec<SectionValue> {
    let items = match v {
        None | Some(Value::Null) => return Vec::new(),
        Some(Value::Array(items)) if items.len() <= MAX_VALUES => items,
        Some(Value::Array(items)) => {
            issues.push(
                file,
                format!("{} values of \"{key}\", the limit is {MAX_VALUES}", items.len()),
            );
            return Vec::new();
        }
        Some(_) => {
            issues.push(file, format!("values of \"{key}\" must be an array"));
            return Vec::new();
        }
    };
    let mut out: Vec<SectionValue> = Vec::new();
    for item in items {
        let Some(value) = item.get("value").and_then(Value::as_str) else {
            issues.push(file, format!("a value of \"{key}\" has no value"));
            continue;
        };
        if !is_section_value(value) {
            issues.push(file, format!("\"{value}\" of \"{key}\" is not one line of text"));
            continue;
        }
        if out.iter().any(|o| o.value == value) {
            issues.push(file, format!("\"{value}\" of \"{key}\" appears twice"));
            continue;
        }
        out.push(SectionValue {
            value: value.to_string(),
            label: opt_localized(file, &format!("value \"{value}\" label"), item.get("label"), issues),
        });
    }
    out
}

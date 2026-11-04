//! Small checked readers over parsed JSON, shared by the file formats.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::error::Issues;
use crate::limits::{MAX_JSON_BYTES, MAX_NAME_BYTES, MAX_TEXT_BYTES};

/// Text for the user: one string, or a string per language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Localized {
    Plain(String),
    ByLanguage(BTreeMap<String, String>),
}

impl Localized {
    /// The text for `language`, else English, else the first language there is.
    pub fn get(&self, language: &str) -> &str {
        match self {
            Localized::Plain(s) => s,
            Localized::ByLanguage(m) => m
                .get(language)
                .or_else(|| m.get("en"))
                .or_else(|| m.values().next())
                .map_or("", String::as_str),
        }
    }
}

/// Reads a file's bytes as a JSON object.
pub(crate) fn parse_object(file: &str, bytes: &[u8], issues: &mut Issues) -> Option<serde_json::Map<String, Value>> {
    if bytes.len() > MAX_JSON_BYTES {
        issues.push(
            file,
            format!("the file has {} bytes, the limit is {MAX_JSON_BYTES}", bytes.len()),
        );
        return None;
    }
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(map)) => Some(map),
        Ok(_) => {
            issues.push(file, "expected a JSON object");
            None
        }
        Err(e) => {
            issues.push(file, format!("not valid JSON: {e}"));
            None
        }
    }
}

/// A block or field name: the rule of `:::name` (ADR 0018), letters and digits of any script
/// plus `-` and `_`, starting with a letter or digit.
pub(crate) fn is_block_name(s: &str) -> bool {
    s.len() <= MAX_NAME_BYTES
        && s.chars()
            .enumerate()
            .all(|(i, c)| c.is_alphanumeric() || (i > 0 && (c == '-' || c == '_')))
        && !s.is_empty()
}

/// A name used as a key by later code: ASCII letters, digits, `-`, `_` and `.`, starting with a
/// letter or digit. It cannot leave a folder or carry markup.
pub(crate) fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_NAME_BYTES
        && s.bytes()
            .enumerate()
            .all(|(i, b)| b.is_ascii_alphanumeric() || (i > 0 && (b == b'-' || b == b'_' || b == b'.')))
}

/// An optional string field of limited length.
pub(crate) fn opt_text(file: &str, what: &str, v: Option<&Value>, max: usize, issues: &mut Issues) -> Option<String> {
    match v {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.len() <= max => Some(s.clone()),
        Some(Value::String(_)) => {
            issues.push(file, format!("{what} is longer than {max} bytes"));
            None
        }
        Some(_) => {
            issues.push(file, format!("{what} must be a string"));
            None
        }
    }
}

/// An optional token (see [`is_token`]).
pub(crate) fn opt_token(file: &str, what: &str, v: Option<&Value>, issues: &mut Issues) -> Option<String> {
    let s = opt_text(file, what, v, MAX_NAME_BYTES, issues)?;
    if is_token(&s) {
        Some(s)
    } else {
        issues.push(
            file,
            format!(
                "{what} \"{s}\" may hold only ASCII letters, digits, '-', '_' and '.', and must start with a letter or digit"
            ),
        );
        None
    }
}

/// An optional [`Localized`] text.
pub(crate) fn opt_localized(file: &str, what: &str, v: Option<&Value>, issues: &mut Issues) -> Option<Localized> {
    match v {
        None | Some(Value::Null) => None,
        Some(Value::String(_)) => opt_text(file, what, v, MAX_TEXT_BYTES, issues).map(Localized::Plain),
        Some(Value::Object(map)) => {
            let mut out = BTreeMap::new();
            for (language, text) in map {
                let what = format!("{what} ({language})");
                match text {
                    Value::String(_) if is_token(language) => {
                        if let Some(s) = opt_text(file, &what, Some(text), MAX_TEXT_BYTES, issues) {
                            out.insert(language.clone(), s);
                        }
                    }
                    Value::String(_) => {
                        issues.push(file, format!("{what}: not a language code"));
                    }
                    _ => issues.push(file, format!("{what} must be a string")),
                }
            }
            Some(Localized::ByLanguage(out))
        }
        Some(_) => {
            issues.push(
                file,
                format!("{what} must be a string or an object of strings per language"),
            );
            None
        }
    }
}

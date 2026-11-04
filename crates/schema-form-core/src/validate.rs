//! Values against a [`FormSpec`]. The result is data: every reason names its field and says what
//! is wrong; what to do about it is the caller's.

use std::collections::HashSet;

use serde_json::{Map, Value};

use crate::{CorrectRule, FormSpec, JsonType, Limits, PropRule, ValueRule};

/// What is wrong with a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Required,
    WrongType,
    TooShort { min: u64 },
    TooLong { max: u64 },
    TooFew { min: u64 },
    TooMany { max: u64 },
    ExactlyOneCorrect,
    AtLeastOneCorrect,
    MissingId,
    DuplicateId { id: String },
    UnknownKey,
}

impl std::fmt::Display for ErrorKind {
    /// The English message; a host with other languages maps [`ErrorKind`] itself.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorKind::Required => write!(f, "Required"),
            ErrorKind::WrongType => write!(f, "The value has the wrong type"),
            ErrorKind::TooShort { min } => write!(f, "At least {min} characters"),
            ErrorKind::TooLong { max } => write!(f, "At most {max} characters"),
            ErrorKind::TooFew { min } => write!(f, "At least {min} needed"),
            ErrorKind::TooMany { max } => write!(f, "At most {max} allowed"),
            ErrorKind::ExactlyOneCorrect => write!(f, "Mark exactly one"),
            ErrorKind::AtLeastOneCorrect => write!(f, "Mark at least one"),
            ErrorKind::MissingId => write!(f, "Has no id"),
            ErrorKind::DuplicateId { id } => write!(f, "The id `{id}` is used twice"),
            ErrorKind::UnknownKey => write!(f, "Not a field of this form"),
        }
    }
}

/// One reason. `path` is `key`, `key/2` or `key/2/inner`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldError {
    pub path: String,
    pub kind: ErrorKind,
}

impl FormSpec {
    /// Every reason `values` does not fit, in field order. A key absent from the values counts
    /// as empty. An empty string or array as a field's value fails `required` and passes
    /// everything else: when a value comes from a document, empty is how missing content looks.
    /// An entry of an array exists, so its strings answer to their limits even when empty.
    pub fn validate(&self, values: &Value) -> Vec<FieldError> {
        let mut out = Vec::new();
        match values.as_object() {
            Some(o) => check_object(&self.rule, o, "", false, &mut out),
            None => out.push(FieldError {
                path: String::new(),
                kind: ErrorKind::WrongType,
            }),
        }
        out
    }
}

fn join(path: &str, key: impl std::fmt::Display) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}/{key}")
    }
}

fn check_object(rule: &ValueRule, o: &Map<String, Value>, path: &str, in_item: bool, out: &mut Vec<FieldError>) {
    for p in &rule.props {
        check_prop(p, o.get(&p.key), &join(path, &p.key), in_item, out);
    }
    if rule.closed {
        for k in o.keys().filter(|k| !rule.props.iter().any(|p| p.key == **k)) {
            out.push(FieldError {
                path: join(path, k),
                kind: ErrorKind::UnknownKey,
            });
        }
    }
}

fn check_prop(p: &PropRule, v: Option<&Value>, path: &str, in_item: bool, out: &mut Vec<FieldError>) {
    let empty = match v {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.trim().is_empty(),
        Some(Value::Array(a)) => a.is_empty(),
        _ => false,
    };
    if empty && p.required {
        out.push(FieldError {
            path: path.to_string(),
            kind: ErrorKind::Required,
        });
        return;
    }
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return;
    };
    let empty_string_in_item = in_item && v.is_string();
    if !empty || empty_string_in_item {
        check_value(&p.rule, v, path, in_item, out);
    } else if !type_fits(p.rule.json_type, v) {
        // empty is exempt from the limits, not from the type
        out.push(FieldError {
            path: path.to_string(),
            kind: ErrorKind::WrongType,
        });
    }
}

fn type_fits(ty: JsonType, v: &Value) -> bool {
    match ty {
        JsonType::String => v.is_string(),
        JsonType::Boolean => v.is_boolean(),
        JsonType::Array => v.is_array(),
        JsonType::Object => v.is_object(),
        JsonType::Any => true,
    }
}

fn check_string(s: &str, l: &Limits, path: &str, out: &mut Vec<FieldError>) {
    let n = s.chars().count() as u64;
    if let Some(min) = l.min_length.filter(|m| n < *m) {
        let kind = if n == 0 {
            ErrorKind::Required
        } else {
            ErrorKind::TooShort { min }
        };
        out.push(FieldError {
            path: path.to_string(),
            kind,
        });
    }
    if let Some(max) = l.max_length.filter(|m| n > *m) {
        out.push(FieldError {
            path: path.to_string(),
            kind: ErrorKind::TooLong { max },
        });
    }
}

fn check_value(rule: &ValueRule, v: &Value, path: &str, in_item: bool, out: &mut Vec<FieldError>) {
    if !type_fits(rule.json_type, v) {
        out.push(FieldError {
            path: path.to_string(),
            kind: ErrorKind::WrongType,
        });
        return;
    }
    match v {
        Value::String(s) if rule.json_type == JsonType::String => check_string(s, &rule.limits, path, out),
        Value::Object(o) if rule.json_type == JsonType::Object => check_object(rule, o, path, in_item, out),
        Value::Array(a) if rule.json_type == JsonType::Array => {
            let mut push = |kind| {
                out.push(FieldError {
                    path: path.to_string(),
                    kind,
                })
            };
            if let Some(min) = rule.limits.min_items.filter(|m| (a.len() as u64) < *m) {
                push(ErrorKind::TooFew { min });
            }
            if let Some(max) = rule.limits.max_items.filter(|m| (a.len() as u64) > *m) {
                push(ErrorKind::TooMany { max });
            }
            if let Some(item) = &rule.items {
                for (i, e) in a.iter().enumerate() {
                    check_value(item, e, &join(path, i), true, out);
                }
            }
            if let Some(correct) = rule.choices {
                check_choices(a, correct, path, out);
            }
        }
        _ => {}
    }
}

fn check_choices(a: &[Value], rule: CorrectRule, path: &str, out: &mut Vec<FieldError>) {
    let marked = (a.iter())
        .filter(|e| e.get("correct") == Some(&Value::Bool(true)))
        .count();
    let kind = match rule {
        CorrectRule::ExactlyOne if marked != 1 => Some(ErrorKind::ExactlyOneCorrect),
        CorrectRule::AtLeastOne if marked == 0 => Some(ErrorKind::AtLeastOneCorrect),
        _ => None,
    };
    if let Some(kind) = kind {
        out.push(FieldError {
            path: path.to_string(),
            kind,
        });
    }
    let mut seen: HashSet<&str> = HashSet::with_capacity(a.len());
    for (i, e) in a.iter().enumerate() {
        match e.get("id").and_then(Value::as_str).unwrap_or("") {
            "" => out.push(FieldError {
                path: join(path, i),
                kind: ErrorKind::MissingId,
            }),
            id if !seen.insert(id) => {
                out.push(FieldError {
                    path: join(path, i),
                    kind: ErrorKind::DuplicateId { id: id.to_string() },
                });
            }
            _ => {}
        }
    }
}

//! Values against a [`FormSpec`]. The result is data: every reason names its field and says what
//! is wrong; what to do about it is the caller's.

use serde_json::{Map, Value};

use crate::{Control, CorrectRule, FieldSpec, FormSpec, JsonType, Limits};

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
    /// as empty. An empty string or array fails `required` and passes everything else: when a
    /// value comes from a document, empty is how missing content looks.
    pub fn validate(&self, values: &Value) -> Vec<FieldError> {
        let mut out = Vec::new();
        match values.as_object() {
            Some(o) => check_object(&self.fields, o, "", self.closed, &mut out),
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

fn check_object(fields: &[FieldSpec], o: &Map<String, Value>, path: &str, closed: bool, out: &mut Vec<FieldError>) {
    for f in fields {
        check_field(f, o.get(&f.key), &join(path, &f.key), out);
    }
    if closed {
        for k in o.keys().filter(|k| !fields.iter().any(|f| f.key == **k)) {
            out.push(FieldError {
                path: join(path, k),
                kind: ErrorKind::UnknownKey,
            });
        }
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

fn check_field(f: &FieldSpec, v: Option<&Value>, path: &str, out: &mut Vec<FieldError>) {
    let mut push = |kind| {
        out.push(FieldError {
            path: path.to_string(),
            kind,
        })
    };
    let empty = match v {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.trim().is_empty(),
        Some(Value::Array(a)) => a.is_empty(),
        _ => false,
    };
    if empty && f.required {
        push(ErrorKind::Required);
        return;
    }
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return;
    };
    let type_ok = match f.json_type {
        JsonType::String => v.is_string(),
        JsonType::Boolean => v.is_boolean(),
        JsonType::Array => v.is_array(),
        JsonType::Object => v.is_object(),
        JsonType::Any => true,
    };
    if !type_ok {
        push(ErrorKind::WrongType);
        return;
    }
    if let Some(s) = v.as_str() {
        if !s.is_empty() {
            check_string(s, &f.limits, path, out);
        }
        return;
    }
    let Some(a) = v.as_array() else {
        if let (Control::Group { fields }, Some(o)) = (&f.control, v.as_object()) {
            check_object(fields, o, path, false, out);
        }
        return;
    };
    if let Some(min) = f.limits.min_items.filter(|m| (a.len() as u64) < *m) {
        push(ErrorKind::TooFew { min });
    }
    if let Some(max) = f.limits.max_items.filter(|m| (a.len() as u64) > *m) {
        push(ErrorKind::TooMany { max });
    }
    for (i, e) in a.iter().enumerate() {
        let p = join(path, i);
        match &f.control {
            Control::StringList { .. } => match e.as_str() {
                Some(s) => check_string(s, &f.item_limits, &p, out),
                None => out.push(FieldError {
                    path: p,
                    kind: ErrorKind::WrongType,
                }),
            },
            Control::Array { fields } => match e.as_object() {
                Some(o) => check_object(fields, o, &p, false, out),
                None => out.push(FieldError {
                    path: p,
                    kind: ErrorKind::WrongType,
                }),
            },
            Control::Choices { .. } | Control::Images if !e.is_object() => {
                out.push(FieldError {
                    path: p,
                    kind: ErrorKind::WrongType,
                });
            }
            _ => {}
        }
    }
    if let Control::Choices { correct, .. } = &f.control {
        check_choices(a, *correct, path, out);
    }
}

fn check_choices(a: &[Value], rule: CorrectRule, path: &str, out: &mut Vec<FieldError>) {
    let marked = a
        .iter()
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
    let mut seen: Vec<&str> = Vec::new();
    for (i, e) in a.iter().enumerate() {
        match e.get("id").and_then(Value::as_str).unwrap_or("") {
            "" => out.push(FieldError {
                path: join(path, i),
                kind: ErrorKind::MissingId,
            }),
            id if seen.contains(&id) => {
                out.push(FieldError {
                    path: join(path, i),
                    kind: ErrorKind::DuplicateId { id: id.to_string() },
                });
            }
            id => seen.push(id),
        }
    }
}

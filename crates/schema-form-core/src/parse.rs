//! Schema and hints to a [`FormSpec`], refusing what is outside the subset.

use serde_json::{Map, Value};

use crate::{Control, CorrectRule, FieldSpec, FormSpec, JsonType, Limits, SPEC_VERSION, Selection, SpecError};

/// Keys of a schema that describe and never constrain; ignored.
const ANNOTATIONS: [&str; 6] = ["$schema", "$id", "$comment", "title", "description", "examples"];

fn err<T>(path: &str, reason: impl Into<String>) -> Result<T, SpecError> {
    Err(SpecError {
        path: path.to_string(),
        reason: reason.into(),
    })
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}/{key}")
    }
}

fn check_keys(path: &str, obj: &Map<String, Value>, known: &[&str], what: &str) -> Result<(), SpecError> {
    for k in obj.keys() {
        if !known.contains(&k.as_str()) && !k.starts_with("x-") && !ANNOTATIONS.contains(&k.as_str()) {
            return err(
                &join(path, k),
                format!("not in the supported subset of {what} (version {SPEC_VERSION})"),
            );
        }
    }
    Ok(())
}

fn object<'a>(path: &str, v: &'a Value, what: &str) -> Result<&'a Map<String, Value>, SpecError> {
    match v.as_object() {
        Some(o) => Ok(o),
        None => err(path, format!("{what} must be an object")),
    }
}

fn limit(path: &str, o: &Map<String, Value>, key: &str) -> Result<Option<u64>, SpecError> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) => Ok(Some(n)),
            None => err(&join(path, key), "must be a non-negative integer"),
        },
    }
}

fn limits(path: &str, o: &Map<String, Value>) -> Result<Limits, SpecError> {
    Ok(Limits {
        min_length: limit(path, o, "minLength")?,
        max_length: limit(path, o, "maxLength")?,
        min_items: limit(path, o, "minItems")?,
        max_items: limit(path, o, "maxItems")?,
    })
}

const SCHEMA_KEYS: [&str; 9] = [
    "type",
    "properties",
    "required",
    "items",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "additionalProperties",
];
const HINT_KEYS: [&str; 10] = [
    "label",
    "help",
    "widget",
    "custom",
    "multiline",
    "add_label",
    "selection",
    "correct",
    "order",
    "fields",
];

fn type_of<'a>(path: &str, s: &'a Map<String, Value>) -> Result<&'a str, SpecError> {
    match s.get("type").and_then(Value::as_str) {
        Some(t) => Ok(t),
        None => err(path, "needs a single `type`"),
    }
}

/// The fields of an object schema, in the hints' order and then by key.
fn fields_of(path: &str, schema: &Map<String, Value>, hints: &Map<String, Value>) -> Result<Vec<FieldSpec>, SpecError> {
    let empty = Map::new();
    let props = match schema.get("properties") {
        Some(p) => object(&join(path, "properties"), p, "`properties`")?,
        None => &empty,
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if let Some(r) = required.iter().find(|r| !props.contains_key(**r)) {
        return err(&join(path, "required"), format!("`{r}` is not in `properties`"));
    }
    let field_hints = match hints.get("fields") {
        Some(h) => object(&join(path, "fields"), h, "`fields`")?,
        None => &empty,
    };
    if let Some(k) = field_hints.keys().find(|k| !props.contains_key(*k)) {
        return err(&join(path, k), "has hints but is not in the schema");
    }
    let mut order: Vec<&str> = Vec::new();
    for k in hints.get("order").and_then(Value::as_array).into_iter().flatten() {
        match k.as_str().filter(|k| props.contains_key(*k)) {
            Some(k) if !order.contains(&k) => order.push(k),
            _ => return err(&join(path, "order"), format!("{k} is not a field, or twice")),
        }
    }
    let rest: Vec<&str> = props
        .keys()
        .map(String::as_str)
        .filter(|k| !order.contains(k))
        .collect();
    order.extend(rest);
    order
        .into_iter()
        .map(|key| {
            let p = join(path, key);
            let hint = match field_hints.get(key) {
                Some(h) => object(&p, h, "a field's hints")?,
                None => &empty,
            };
            let sch = object(&p, &props[key], "a property")?;
            field(&p, key, sch, hint, required.contains(&key))
        })
        .collect()
}

fn field(
    path: &str,
    key: &str,
    s: &Map<String, Value>,
    h: &Map<String, Value>,
    required: bool,
) -> Result<FieldSpec, SpecError> {
    check_keys(path, s, &SCHEMA_KEYS, "JSON Schema")?;
    check_keys(path, h, &HINT_KEYS, "the hints")?;
    let widget = h.get("widget").and_then(Value::as_str);
    let hint_str = |k: &str| h.get(k).and_then(Value::as_str).map(str::to_string);
    let multiline = h.get("multiline").and_then(Value::as_bool).unwrap_or(false);
    let ty = s.get("type").and_then(Value::as_str);
    let mut item_limits = Limits::default();
    let mut json_type = JsonType::Array;
    let items = s.get("items").and_then(Value::as_object);
    let check_item = |it: &Map<String, Value>, want: &[(&str, &str)]| -> Result<(), SpecError> {
        let props = it.get("properties").and_then(Value::as_object);
        for (k, t) in want {
            let got = props
                .and_then(|p| p.get(*k))
                .and_then(|p| p.get("type"))
                .and_then(Value::as_str);
            if got != Some(t) {
                return err(&join(path, "items"), format!("needs `{k}` of type {t}"));
            }
        }
        Ok(())
    };
    let control = match (widget, ty) {
        (Some("custom"), t) => {
            json_type = match t {
                Some("string") => JsonType::String,
                Some("boolean") => JsonType::Boolean,
                Some("array") => JsonType::Array,
                Some("object") => JsonType::Object,
                _ => JsonType::Any,
            };
            match hint_str("custom") {
                Some(name) if !name.is_empty() => Control::Custom { name },
                _ => return err(path, "a custom widget needs its name in `custom`"),
            }
        }
        (None | Some("text"), Some("string")) => {
            json_type = JsonType::String;
            Control::Text { multiline: false }
        }
        (Some("multiline"), Some("string")) => {
            json_type = JsonType::String;
            Control::Text { multiline: true }
        }
        (None | Some("checkbox"), Some("boolean")) => {
            json_type = JsonType::Boolean;
            Control::Checkbox
        }
        (None | Some("list"), Some("array")) if items.is_some_and(|i| i.get("type") == Some(&"string".into())) => {
            let it = items.expect("checked");
            check_keys(&join(path, "items"), it, &SCHEMA_KEYS, "JSON Schema")?;
            item_limits = limits(&join(path, "items"), it)?;
            Control::StringList { multiline }
        }
        (Some("choices"), Some("array")) => {
            let Some(it) = items else {
                return err(path, "a choices field needs `items`");
            };
            check_item(it, &[("id", "string"), ("text", "string"), ("correct", "boolean")])?;
            let correct = match h.get("correct").and_then(Value::as_str) {
                None | Some("any") => CorrectRule::Any,
                Some("exactly-one") => CorrectRule::ExactlyOne,
                Some("at-least-one") => CorrectRule::AtLeastOne,
                Some(o) => return err(&join(path, "correct"), format!("unknown rule `{o}`")),
            };
            let selection = match h.get("selection").and_then(Value::as_str) {
                Some("single") => Selection::Single,
                Some("multiple") => Selection::Multiple,
                None if correct == CorrectRule::ExactlyOne => Selection::Single,
                None => Selection::Multiple,
                Some(o) => return err(&join(path, "selection"), format!("unknown selection `{o}`")),
            };
            Control::Choices { selection, correct }
        }
        (Some("images"), Some("array")) => {
            let Some(it) = items else {
                return err(path, "an images field needs `items`");
            };
            check_item(it, &[("alt", "string"), ("path", "string")])?;
            Control::Images
        }
        (None | Some("array"), Some("array")) if items.is_some_and(|i| i.get("type") == Some(&"object".into())) => {
            let it = items.expect("checked");
            check_keys(&join(path, "items"), it, &SCHEMA_KEYS, "JSON Schema")?;
            Control::Array {
                fields: fields_of(path, it, h)?,
            }
        }
        (None | Some("group"), Some("object")) => {
            json_type = JsonType::Object;
            Control::Group {
                fields: fields_of(path, s, h)?,
            }
        }
        (w, t) => {
            return err(
                path,
                format!(
                    "no widget `{}` for type `{}`",
                    w.unwrap_or("(default)"),
                    t.unwrap_or("(none)")
                ),
            );
        }
    };
    let mut extensions = Map::new();
    for (k, v) in h.iter().filter(|(k, _)| k.starts_with("x-")) {
        extensions.insert(k.clone(), v.clone());
    }
    Ok(FieldSpec {
        key: key.to_string(),
        label: hint_str("label")
            .or_else(|| s.get("title").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| key.to_string()),
        help: hint_str("help").or_else(|| s.get("description").and_then(Value::as_str).map(str::to_string)),
        add_label: hint_str("add_label"),
        required,
        limits: limits(path, s)?,
        item_limits,
        control,
        json_type,
        extensions,
    })
}

/// Replaces in `base` what `over` says, object by object.
fn overlay(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                overlay(a.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (a, b) => *a = b.clone(),
    }
}

impl FormSpec {
    /// `variant` picks the hints' `variants[variant]` overlay: a host's way of saying this block
    /// allows one mark and that one many. Unknown variants use the base hints.
    pub fn parse(schema: &Value, hints: &Value, variant: Option<&str>) -> Result<FormSpec, SpecError> {
        let h = object("hints", hints, "hints")?;
        match h.get("version").and_then(Value::as_u64) {
            Some(SPEC_VERSION) => {}
            Some(v) => return err("hints/version", format!("version {v}, this reads {SPEC_VERSION}")),
            None => return err("hints/version", "missing"),
        }
        check_keys("hints", h, &["version", "order", "fields", "variants"], "the hints")?;
        let mut merged = hints.clone();
        if let Some(over) = variant.and_then(|n| hints.get("variants")?.get(n)) {
            overlay(&mut merged, over);
        }
        let s = object("schema", schema, "the schema")?;
        check_keys("schema", s, &SCHEMA_KEYS, "JSON Schema")?;
        if type_of("schema", s)? != "object" {
            return err("schema", "the root must be an object");
        }
        let closed = match s.get("additionalProperties") {
            None | Some(Value::Bool(true)) => false,
            Some(Value::Bool(false)) => true,
            Some(_) => return err("schema/additionalProperties", "only true or false"),
        };
        let m = merged.as_object().expect("an object overlaid on an object stays one");
        Ok(FormSpec {
            fields: fields_of("", s, m)?,
            closed,
        })
    }
}

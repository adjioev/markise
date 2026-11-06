//! Schema and hints to a [`FormSpec`], refusing what is outside the subset.

use std::collections::HashSet;

use serde_json::{Map, Value};

use crate::{
    ChoiceOption, ChoiceShape, Control, CorrectRule, FieldSpec, FormSpec, JsonType, Limits, OptionsRule, Pattern,
    PropRule, SPEC_VERSION, Selection, SpecError, ValueRule,
};

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

const SCHEMA_KEYS: [&str; 11] = [
    "type",
    "enum",
    "pattern",
    "properties",
    "required",
    "items",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "additionalProperties",
];
const HINT_KEYS: [&str; 12] = [
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
    "options",
    "columns",
];
const STRING_HINTS: [&str; 7] = ["label", "help", "widget", "custom", "add_label", "selection", "correct"];

/// The keywords that mean something for each type; on another type they would be ignored.
fn check_applies(path: &str, s: &Map<String, Value>, ty: &str) -> Result<(), SpecError> {
    let all = [
        "enum",
        "pattern",
        "properties",
        "required",
        "additionalProperties",
        "items",
        "minLength",
        "maxLength",
        "minItems",
        "maxItems",
    ];
    let allowed: &[&str] = match ty {
        "string" => &["enum", "pattern", "minLength", "maxLength"],
        "array" => &["items", "minItems", "maxItems"],
        "object" => &["properties", "required", "additionalProperties"],
        _ => &[],
    };
    match all.iter().find(|k| s.contains_key(**k) && !allowed.contains(k)) {
        Some(k) => err(&join(path, k), format!("means nothing on type `{ty}`")),
        None => Ok(()),
    }
}

fn required_of<'a>(path: &str, schema: &'a Map<String, Value>) -> Result<Vec<&'a str>, SpecError> {
    let p = join(path, "required");
    let Some(r) = schema.get("required") else {
        return Ok(Vec::new());
    };
    let Some(a) = r.as_array() else {
        return err(&p, "must be an array of property names");
    };
    let names: Option<Vec<&str>> = a.iter().map(Value::as_str).collect();
    match names {
        Some(n) => Ok(n),
        None => err(&p, "must be an array of property names"),
    }
}

/// The keys of an object schema, in the hints' order and then by key.
fn ordered_keys<'a>(
    path: &str,
    props: &'a Map<String, Value>,
    hints: &Map<String, Value>,
) -> Result<Vec<&'a str>, SpecError> {
    let order_path = join(path, "order");
    let mut order: Vec<&str> = Vec::new();
    let listed: &[Value] = match hints.get("order") {
        None => &[],
        Some(Value::Array(a)) => a,
        Some(_) => return err(&order_path, "must be an array of field names"),
    };
    for k in listed {
        match k.as_str().and_then(|k| props.get_key_value(k)) {
            Some((k, _)) if !order.contains(&k.as_str()) => order.push(k),
            _ => return err(&order_path, format!("{k} is not a field, or twice")),
        }
    }
    let rest: Vec<&str> = props
        .keys()
        .map(String::as_str)
        .filter(|k| !order.contains(k))
        .collect();
    order.extend(rest);
    Ok(order)
}

fn props_of<'a>(
    path: &str,
    schema: &'a Map<String, Value>,
    empty: &'a Map<String, Value>,
) -> Result<&'a Map<String, Value>, SpecError> {
    match schema.get("properties") {
        Some(p) => object(&join(path, "properties"), p, "`properties`"),
        None => Ok(empty),
    }
}

fn correct_rule(path: &str, h: &Map<String, Value>) -> Result<CorrectRule, SpecError> {
    match h.get("correct").and_then(Value::as_str) {
        None | Some("any") => Ok(CorrectRule::Any),
        Some("exactly-one") => Ok(CorrectRule::ExactlyOne),
        Some("at-least-one") => Ok(CorrectRule::AtLeastOne),
        Some(o) => err(&join(path, "correct"), format!("unknown rule `{o}`")),
    }
}

fn columns_of(path: &str, h: &Map<String, Value>) -> Result<Option<u32>, SpecError> {
    match h.get("columns") {
        None => Ok(None),
        Some(v) => match v.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0) {
            Some(n) => Ok(Some(n)),
            None => err(&join(path, "columns"), "must be a positive integer"),
        },
    }
}

/// The widget that draws choice buttons.
const BUTTONS: &str = "buttons";

fn selection_of(path: &str, h: &Map<String, Value>, correct: CorrectRule) -> Result<Selection, SpecError> {
    match h.get("selection").and_then(Value::as_str) {
        Some("single") => Ok(Selection::Single),
        Some("multiple") => Ok(Selection::Multiple),
        None if correct == CorrectRule::ExactlyOne => Ok(Selection::Single),
        None => Ok(Selection::Multiple),
        Some(o) => err(&join(path, "selection"), format!("unknown selection `{o}`")),
    }
}

/// `enum`: distinct strings, at least one.
fn enum_of(path: &str, s: &Map<String, Value>) -> Result<Option<Vec<String>>, SpecError> {
    let Some(e) = s.get("enum") else {
        return Ok(None);
    };
    let p = join(path, "enum");
    let names: Option<Vec<&str>> = e
        .as_array()
        .map(|a| a.iter().map(Value::as_str).collect())
        .unwrap_or(None);
    let Some(names) = names.filter(|n| !n.is_empty()) else {
        return err(&p, "must be an array of strings, at least one");
    };
    let mut seen = HashSet::new();
    if let Some(twice) = names.iter().find(|n| !seen.insert(**n)) {
        return err(&p, format!("`{twice}` is there twice"));
    }
    Ok(Some(names.into_iter().map(str::to_string).collect()))
}

/// `pattern`: a string the engine can run within its bounds.
fn pattern_of(path: &str, s: &Map<String, Value>) -> Result<Option<Pattern>, SpecError> {
    let Some(p) = s.get("pattern") else {
        return Ok(None);
    };
    let here = join(path, "pattern");
    let Some(text) = p.as_str() else {
        return err(&here, "must be a string");
    };
    Pattern::new(text).map(Some).or_else(|why| err(&here, why))
}

/// The options of choice buttons: the hints' `options`, then what else `allowed` holds. With
/// `allowed` (an `enum`) an option must be one of those; without it there must be options.
fn options_of(path: &str, h: &Map<String, Value>, allowed: Option<&[String]>) -> Result<Vec<ChoiceOption>, SpecError> {
    let p = join(path, "options");
    let listed: &[Value] = match h.get("options") {
        None => &[],
        Some(Value::Array(a)) => a,
        Some(_) => return err(&p, "must be an array of options"),
    };
    let mut out: Vec<ChoiceOption> = Vec::with_capacity(listed.len());
    for (i, o) in listed.iter().enumerate() {
        let here = join(&p, &i.to_string());
        let o = object(&here, o, "an option")?;
        check_keys(&here, o, &["value", "label", "icon", "exclusive"], "an option")?;
        let text = |k: &str| match o.get(k) {
            None => Ok(None),
            Some(Value::String(s)) if !s.is_empty() => Ok(Some(s.clone())),
            Some(_) => err(&join(&here, k), "must be a string, not empty"),
        };
        let Some(value) = text("value")? else {
            return err(&here, "needs a `value`");
        };
        if out.iter().any(|x| x.value == value) {
            return err(&here, format!("`{value}` is there twice"));
        }
        if allowed.is_some_and(|a| !a.contains(&value)) {
            return err(&here, format!("`{value}` is not in the `enum`"));
        }
        let exclusive = match o.get("exclusive") {
            None => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return err(&join(&here, "exclusive"), "must be true or false"),
        };
        out.push(ChoiceOption {
            label: text("label")?.unwrap_or_else(|| value.clone()),
            icon: text("icon")?,
            exclusive,
            value,
        });
    }
    for value in allowed.unwrap_or(&[]) {
        if !out.iter().any(|x| x.value == *value) {
            out.push(ChoiceOption::plain(value));
        }
    }
    if out.is_empty() {
        return err(&p, "choice buttons need options");
    }
    Ok(out)
}

/// What a value must look like, checking the whole schema below it against the subset. Only a
/// custom widget's own value may leave `type` out.
fn rule_of(
    path: &str,
    s: &Map<String, Value>,
    h: &Map<String, Value>,
    may_be_any: bool,
) -> Result<ValueRule, SpecError> {
    check_keys(path, s, &SCHEMA_KEYS, "JSON Schema")?;
    let ty = match s.get("type") {
        None if may_be_any => None,
        None => return err(path, "needs a single `type`"),
        Some(Value::String(t)) => Some(t.as_str()),
        Some(_) => return err(&join(path, "type"), "must be one type"),
    };
    let json_type = match ty {
        None => JsonType::Any,
        Some("string") => JsonType::String,
        Some("boolean") => JsonType::Boolean,
        Some("array") => JsonType::Array,
        Some("object") => JsonType::Object,
        Some(t) => return err(&join(path, "type"), format!("`{t}` is not in the supported subset")),
    };
    check_applies(path, s, ty.unwrap_or("(none)"))?;
    let mut rule = ValueRule {
        json_type,
        limits: limits(path, s)?,
        items: None,
        props: Vec::new(),
        closed: false,
        choices: None,
        one_of: enum_of(path, s)?,
        pattern: pattern_of(path, s)?,
        options: None,
    };
    if json_type == JsonType::Array {
        if let Some(it) = s.get("items") {
            let p = join(path, "items");
            let it = object(&p, it, "`items`")?;
            let empty = Map::new();
            // an array's hints speak of the properties of its entries
            let item_hints = if it.get("type").is_some_and(|t| t == "object") {
                h
            } else {
                &empty
            };
            rule.items = Some(Box::new(rule_of(&p, it, item_hints, false)?));
        }
        if h.get("widget").is_some_and(|w| w == "choices") {
            rule.choices = Some(correct_rule(path, h)?);
        }
        let buttons = h.get("widget").is_some_and(|w| w == BUTTONS);
        let items_enum = rule.items.as_ref().and_then(|i| i.one_of.as_deref());
        if buttons || (h.get("widget").is_none() && items_enum.is_some()) {
            let options = options_of(path, h, items_enum)?;
            rule.options = Some(OptionsRule {
                values: options.iter().map(|o| o.value.clone()).collect(),
                exclusive: (options.iter().filter(|o| o.exclusive))
                    .map(|o| o.value.clone())
                    .collect(),
            });
            if rule.items.as_ref().is_some_and(|i| i.json_type == JsonType::Object) {
                rule.choices = Some(correct_rule(path, h)?);
            }
        }
    }
    if json_type == JsonType::Object {
        let empty = Map::new();
        let props = props_of(path, s, &empty)?;
        let required = required_of(path, s)?;
        if let Some(r) = required.iter().find(|r| !props.contains_key(**r)) {
            return err(&join(path, "required"), format!("`{r}` is not in `properties`"));
        }
        let field_hints = match h.get("fields") {
            Some(f) => object(&join(path, "fields"), f, "`fields`")?,
            None => &empty,
        };
        for key in ordered_keys(path, props, h)? {
            let p = join(path, key);
            let hint = match field_hints.get(key) {
                Some(x) => object(&p, x, "a field's hints")?,
                None => &empty,
            };
            let custom = hint.get("widget").is_some_and(|w| w == "custom");
            let inner = if custom { &empty } else { hint };
            rule.props.push(PropRule {
                key: key.to_string(),
                required: required.contains(&key),
                rule: rule_of(&p, object(&p, &props[key], "a property")?, inner, custom)?,
            });
        }
        rule.closed = match s.get("additionalProperties") {
            None | Some(Value::Bool(true)) => false,
            Some(Value::Bool(false)) => true,
            Some(_) => return err(&join(path, "additionalProperties"), "only true or false"),
        };
    }
    Ok(rule)
}

/// The fields of an object schema.
fn fields_of(path: &str, schema: &Map<String, Value>, hints: &Map<String, Value>) -> Result<Vec<FieldSpec>, SpecError> {
    let empty = Map::new();
    let props = props_of(path, schema, &empty)?;
    let required = required_of(path, schema)?;
    let field_hints = match hints.get("fields") {
        Some(h) => object(&join(path, "fields"), h, "`fields`")?,
        None => &empty,
    };
    if let Some(k) = field_hints.keys().find(|k| !props.contains_key(*k)) {
        return err(&join(path, k), "has hints but is not in the schema");
    }
    ordered_keys(path, props, hints)?
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
    check_keys(path, h, &HINT_KEYS, "the hints")?;
    for k in STRING_HINTS {
        if h.get(k).is_some_and(|v| !v.is_string()) {
            return err(&join(path, k), "must be a string");
        }
    }
    if h.get("multiline").is_some_and(|v| !v.is_boolean()) {
        return err(&join(path, "multiline"), "must be true or false");
    }
    let widget = h.get("widget").and_then(Value::as_str);
    let is_custom = widget == Some("custom");
    let no_hints = Map::new();
    let rule = rule_of(path, s, if is_custom { &no_hints } else { h }, is_custom)?;
    let hint_str = |k: &str| h.get(k).and_then(Value::as_str).map(str::to_string);
    let multiline = h.get("multiline").and_then(Value::as_bool).unwrap_or(false);
    let ty = s.get("type").and_then(Value::as_str);
    let items = s.get("items").and_then(Value::as_object);
    let items_of = |t: &str| items.is_some_and(|i| i.get("type").is_some_and(|x| x == t));
    let check_item = |want: &[(&str, &str)]| -> Result<(), SpecError> {
        let props = items.and_then(|it| it.get("properties")).and_then(Value::as_object);
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
    let has_enum = s.contains_key("enum");
    let items_enum = items.and_then(|i| enum_of(path, i).transpose()).transpose()?;
    let control = match (widget, ty) {
        (Some("custom"), _) => match hint_str("custom") {
            Some(name) if !name.is_empty() => Control::Custom { name },
            _ => return err(path, "a custom widget needs its name in `custom`"),
        },
        (None | Some(BUTTONS), Some("string")) if has_enum => {
            if h.contains_key("selection") {
                return err(&join(path, "selection"), "a string holds one choice");
            }
            Control::ChoiceButtons {
                options: options_of(path, h, rule.one_of.as_deref())?,
                selection: Selection::Single,
                shape: ChoiceShape::String,
                columns: columns_of(path, h)?,
            }
        }
        (None | Some("text"), Some("string")) => Control::Text { multiline: false },
        (Some("multiline"), Some("string")) => Control::Text { multiline: true },
        (None | Some("checkbox"), Some("boolean")) => Control::Checkbox,
        (None | Some(BUTTONS), Some("array")) if items_of("string") && items_enum.is_some() => {
            if h.contains_key("selection") {
                return err(
                    &join(path, "selection"),
                    "an array of strings holds any number of choices",
                );
            }
            Control::ChoiceButtons {
                options: options_of(path, h, items_enum.as_deref())?,
                selection: Selection::Multiple,
                shape: ChoiceShape::Strings,
                columns: columns_of(path, h)?,
            }
        }
        (Some(BUTTONS), Some("array")) if items_of("object") => {
            check_item(&[("id", "string"), ("text", "string"), ("correct", "boolean")])?;
            Control::ChoiceButtons {
                options: options_of(path, h, None)?,
                selection: selection_of(path, h, correct_rule(path, h)?)?,
                shape: ChoiceShape::Marks,
                columns: columns_of(path, h)?,
            }
        }
        (None | Some("list"), Some("array")) if items_of("string") => Control::StringList { multiline },
        (Some("choices"), Some("array")) => {
            if items.is_none() {
                return err(path, "a choices field needs `items`");
            }
            check_item(&[("id", "string"), ("text", "string"), ("correct", "boolean")])?;
            let correct = correct_rule(path, h)?;
            let selection = selection_of(path, h, correct)?;
            Control::Choices { selection, correct }
        }
        (Some("images"), Some("array")) => {
            if items.is_none() {
                return err(path, "an images field needs `items`");
            }
            check_item(&[("alt", "string"), ("path", "string")])?;
            Control::Images
        }
        (None | Some("array"), Some("array")) if items_of("object") => Control::Array {
            fields: fields_of(path, items.expect("checked"), h)?,
        },
        (None | Some("group"), Some("object")) => Control::Group {
            fields: fields_of(path, s, h)?,
        },
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
        control,
        rule,
        extensions,
    })
}

fn uses_choice_rules(r: &ValueRule) -> bool {
    r.one_of.is_some()
        || r.options.is_some()
        || r.items.as_deref().is_some_and(uses_choice_rules)
        || r.props.iter().any(|p| uses_choice_rules(&p.rule))
}

fn uses_pattern(r: &ValueRule) -> bool {
    r.pattern.is_some() || r.items.as_deref().is_some_and(uses_pattern) || r.props.iter().any(|p| uses_pattern(&p.rule))
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
    /// allows one mark and that one many. No variant picks the overlay named `""`, if any, which
    /// no named variant inherits. Unknown variants use the base hints. Every variant is checked,
    /// picked or not.
    pub fn parse(schema: &Value, hints: &Value, variant: Option<&str>) -> Result<FormSpec, SpecError> {
        let h = object("hints", hints, "hints")?;
        let version = match h.get("version").and_then(Value::as_u64) {
            Some(v @ 1..=SPEC_VERSION) => v,
            Some(v) => return err("hints/version", format!("version {v}, this reads up to {SPEC_VERSION}")),
            None => return err("hints/version", "missing"),
        };
        check_keys("hints", h, &["version", "order", "fields", "variants"], "the hints")?;
        if let Some(vs) = h.get("variants") {
            for (name, over) in object("hints/variants", vs, "`variants`")? {
                let p = format!("hints/variants/{name}");
                check_keys(&p, object(&p, over, "a variant")?, &["order", "fields"], "a variant")?;
            }
        }
        let mut merged = hints.clone();
        if let Some(over) = hints.get("variants").and_then(|v| v.get(variant.unwrap_or(""))) {
            overlay(&mut merged, over);
        }
        let s = object("schema", schema, "the schema")?;
        if s.get("type").is_none_or(|t| t != "object") {
            return err("schema", "the root must be an object");
        }
        let m = merged.as_object().expect("an object overlaid with an object stays one");
        let fields = fields_of("", s, m)?;
        let rule = rule_of("", s, m, false)?;
        if version < 2 && uses_choice_rules(&rule) {
            return err("hints/version", "`enum` and choice buttons need version 2");
        }
        if version < 3 && uses_pattern(&rule) {
            return err("hints/version", "`pattern` needs version 3");
        }
        Ok(FormSpec { fields, rule })
    }
}

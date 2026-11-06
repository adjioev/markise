//! From a block's schema to the input of a form (ADR 0020): a package declares `fields: [{name,
//! type, ...}]`, and `schema-form-core` reads JSON Schema plus hints. Here is the one place that
//! knows both. The result is plain JSON, so this crate draws no form; `x-markdown` in the hints
//! tells the field model where each field lives, so the field declarations come from the same
//! place and cannot disagree with the form.
//!
//! What a package may say and what a form can check are not the same list, so a known key with a
//! value the form could not honour is refused by name rather than dropped. Keys not listed here
//! are data for later versions (ADR 0019) and are ignored.

use serde_json::{Map, Value, json};

use crate::index::PackageIndex;
use crate::palette::Preset;
use crate::schema::{BlockSchema, FieldKind, FieldSchema};

/// The form's key for a block's argument: not a field's name, so no field can be called this (a field
/// named like the argument is another thing).
pub const ARGUMENT_KEY: &str = ":argument";

/// The version of the hints this writes, as `schema-form-core` reads them.
const HINTS_VERSION: u64 = 3;

/// What a preset's `widget` says when it asks for a choice drawn as icon buttons.
const ICON_CHOICES: &str = "icon-choices";

/// The input of `FormSpec::parse`.
#[derive(Clone, Debug, PartialEq)]
pub struct FormInput {
    pub schema: Value,
    pub hints: Value,
}

/// Why a block has no form: `field` names the place, empty for the block as a whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormRefusal {
    pub field: String,
    pub reason: String,
}

impl std::fmt::Display for FormRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.field.is_empty() {
            write!(f, "{}", self.reason)
        } else {
            write!(f, "field `{}`: {}", self.field, self.reason)
        }
    }
}

fn refuse<T>(field: &str, reason: impl Into<String>) -> Result<T, FormRefusal> {
    Err(FormRefusal {
        field: field.to_string(),
        reason: reason.into(),
    })
}

/// Text for the user, a string or a string per language: the language, English, the first there is.
fn text_of<'a>(v: Option<&'a Value>, language: &str) -> Option<&'a str> {
    match v? {
        Value::String(s) => Some(s),
        Value::Object(m) => m
            .get(language)
            .or_else(|| m.get("en"))
            .or_else(|| m.values().next())?
            .as_str(),
        _ => None,
    }
}

/// `prompt_text` reads as `Prompt text`: a field without a label still says what it is.
fn name_as_label(name: &str) -> String {
    let spaced = name.replace(['_', '-'], " ");
    let mut c = spaced.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => spaced,
    }
}

fn count(f: &FieldSchema, key: &str) -> Result<Option<u64>, FormRefusal> {
    match f.raw.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) => Ok(Some(n)),
            None => refuse(&f.name, format!("`{key}` must be a non-negative integer or null")),
        },
    }
}

/// `min` and `max` of a field that is one piece of text: present or not, nothing in between.
fn required_text(f: &FieldSchema) -> Result<bool, FormRefusal> {
    if let Some(n) = count(f, "max")? {
        return refuse(
            &f.name,
            format!("`max` {n}: a form cannot limit how much text a field holds"),
        );
    }
    match count(f, "min")? {
        None | Some(1) => Ok(true),
        Some(0) => Ok(false),
        Some(n) => refuse(&f.name, format!("`min` {n}: a text field holds one value")),
    }
}

/// `minItems`, `maxItems`, or `min` and `max` as an image field writes them.
fn item_limits(f: &FieldSchema, schema: &mut Map<String, Value>) -> Result<bool, FormRefusal> {
    let min = count(f, "minItems")?.or(count(f, "min")?);
    let max = count(f, "maxItems")?.or(count(f, "max")?);
    if let Some(n) = min {
        schema.insert("minItems".into(), json!(n));
    }
    if let Some(n) = max {
        schema.insert("maxItems".into(), json!(n));
    }
    Ok(min.is_some_and(|n| n > 0))
}

struct Converted {
    schema: Map<String, Value>,
    hint: Map<String, Value>,
    required: bool,
}

fn convert_field(f: &FieldSchema) -> Result<Converted, FormRefusal> {
    let mut schema = Map::new();
    let mut hint = Map::new();
    let required;
    match f.kind {
        FieldKind::Heading => {
            required = required_text(f)?;
            schema.insert("type".into(), json!("string"));
            let mut place = json!({"kind": "heading"});
            if let Some(level) = count(f, "level")? {
                if !(1..=6).contains(&level) {
                    return refuse(&f.name, format!("`level` {level}: a heading has levels 1 to 6"));
                }
                place["level"] = json!(level);
            }
            hint.insert("x-markdown".into(), place);
        }
        FieldKind::Text | FieldKind::Quote => {
            required = required_text(f)?;
            schema.insert("type".into(), json!("string"));
            hint.insert("widget".into(), json!("multiline"));
            let kind = if f.kind == FieldKind::Text { "text" } else { "quote" };
            hint.insert("x-markdown".into(), json!({"kind": kind}));
        }
        FieldKind::List => {
            schema.insert("type".into(), json!("array"));
            required = item_limits(f, &mut schema)?;
            schema.insert("items".into(), json!({"type": "string", "minLength": 1}));
            hint.insert("x-markdown".into(), json!({"kind": "list"}));
        }
        FieldKind::Choices => {
            match f.raw.get("ids") {
                None => {}
                Some(v) if v == "required" => {}
                Some(v) => return refuse(&f.name, format!("`ids` {v}: an answer always has an id")),
            }
            schema.insert("type".into(), json!("array"));
            required = item_limits(f, &mut schema)?;
            schema.insert(
                "items".into(),
                json!({"type": "object", "properties": {
                    "id": {"type": "string"},
                    "text": {"type": "string", "minLength": 1},
                    "correct": {"type": "boolean"},
                }}),
            );
            hint.insert("widget".into(), json!("choices"));
            hint.insert("x-markdown".into(), json!({"kind": "choices"}));
        }
        FieldKind::Image => {
            schema.insert("type".into(), json!("array"));
            required = item_limits(f, &mut schema)?;
            schema.insert(
                "items".into(),
                json!({"type": "object", "properties": {
                    "alt": {"type": "string"},
                    "path": {"type": "string", "minLength": 1},
                }}),
            );
            hint.insert("widget".into(), json!("images"));
            hint.insert("x-markdown".into(), json!({"kind": "image"}));
        }
    }
    Ok(Converted { schema, hint, required })
}

/// A hints object for one field inside `variants[variant].fields`, or the base hints.
fn variant_field<'a>(variants: &'a mut Map<String, Value>, variant: &str, field: &str) -> &'a mut Map<String, Value> {
    let v = variants
        .entry(variant.to_string())
        .or_insert_with(|| json!({"fields": {}}));
    let hint = v["fields"]
        .as_object_mut()
        .expect("built here as an object")
        .entry(field.to_string())
        .or_insert_with(|| json!({}));
    hint.as_object_mut().expect("built here as an object")
}

/// `correct`, one rule or a rule per variant, where `""` is the block without one.
fn apply_correct(
    f: &FieldSchema,
    hint: &mut Map<String, Value>,
    variants: &mut Map<String, Value>,
) -> Result<(), FormRefusal> {
    match f.raw.get("correct") {
        None | Some(Value::Null) => {}
        Some(Value::String(rule)) => {
            hint.insert("correct".into(), json!(rule));
        }
        Some(Value::Object(per_variant)) => {
            for (variant, rule) in per_variant {
                if !rule.is_string() {
                    return refuse(&f.name, format!("`correct` of \"{variant}\" must be a string"));
                }
                if variant.is_empty() {
                    hint.insert("correct".into(), rule.clone());
                } else {
                    variant_field(variants, variant, &f.name).insert("correct".into(), rule.clone());
                }
            }
        }
        Some(_) => return refuse(&f.name, "`correct` must be a rule or a rule per variant"),
    }
    Ok(())
}

/// What a preset asking for icon choices adds. An option's value is its label: that is the text
/// the answer holds in the Markdown, written in one language by the preset.
fn icon_choice_hints(
    p: &Preset,
    field: &FieldSchema,
    language: &str,
    hint: &mut Map<String, Value>,
) -> Result<(), FormRefusal> {
    let name = &field.name;
    let Some(answers) = p.extra.get("answers").and_then(Value::as_array) else {
        return refuse(name, format!("preset \"{}\" has no `answers`", p.id));
    };
    let mut options = Vec::with_capacity(answers.len());
    for (i, a) in answers.iter().enumerate() {
        let Some(label) = a.get("label").and_then(Value::as_str) else {
            return refuse(name, format!("preset \"{}\": answer {i} has no `label`", p.id));
        };
        let mut o = json!({"value": label, "label": label});
        if let Some(icon) = a.get("icon").filter(|v| !v.is_null()) {
            if !icon.is_string() {
                return refuse(name, format!("preset \"{}\": answer {i}: `icon` must be a file", p.id));
            }
            o["icon"] = icon.clone();
        }
        if let Some(e) = a.get("exclusive").filter(|v| !v.is_null()) {
            if !e.is_boolean() {
                return refuse(
                    name,
                    format!("preset \"{}\": answer {i}: `exclusive` must be true or false", p.id),
                );
            }
            o["exclusive"] = e.clone();
        }
        options.push(o);
    }
    hint.insert("widget".into(), json!("buttons"));
    hint.insert("options".into(), Value::Array(options));
    if let Some(c) = p.extra.get("columns").filter(|v| !v.is_null()) {
        hint.insert("columns".into(), c.clone());
    }
    if let Some(label) = text_of(p.extra.get("field_label"), language) {
        hint.insert("label".into(), json!(label));
    }
    Ok(())
}

/// A key of a preset that names something: absent or null, or a string; anything else would
/// quietly mean something wider than was written.
fn preset_text<'a>(p: &'a Preset, key: &str) -> Result<Option<&'a str>, FormRefusal> {
    match p.extra.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(v) => refuse("", format!("preset \"{}\": `{key}` {v} must be a string", p.id)),
    }
}

/// The field for the block's `argument`, if it declares one: a line of text. `x-markdown` says it
/// lives in the opener, so it takes nothing from the body.
fn argument_hint(argument: &Map<String, Value>, language: &str) -> Map<String, Value> {
    let label = text_of(argument.get("label"), language).map_or_else(
        || {
            let name = argument.get("name").and_then(Value::as_str).unwrap_or("argument");
            name_as_label(name)
        },
        str::to_string,
    );
    let mut hint = Map::new();
    hint.insert("label".into(), json!(label));
    if let Some(help) = text_of(argument.get("help"), language) {
        hint.insert("help".into(), json!(help));
    }
    hint.insert("x-markdown".into(), json!({"kind": "argument"}));
    hint
}

impl BlockSchema {
    /// The form of this block. `presets` are the block's own: one can ask that a variant's choice
    /// is drawn as icon buttons.
    pub fn form_input<'a>(
        &self,
        presets: impl Iterator<Item = &'a Preset>,
        language: &str,
    ) -> Result<FormInput, FormRefusal> {
        let mut properties = Map::new();
        let mut required = Vec::new();
        let mut order = Vec::with_capacity(self.fields.len() + 1);
        let mut fields = Map::new();
        let mut variants = Map::new();
        if let Some(argument) = self.raw.get("argument").and_then(Value::as_object) {
            // in a schema with variants the arguments name one, and that is their check
            let checked = self.variants().next().is_none();
            let mut schema = json!({"type": "string"});
            if checked {
                required.push(json!(ARGUMENT_KEY));
                if let Some(pattern) = argument.get("pattern") {
                    schema["pattern"] = pattern.clone();
                }
            }
            properties.insert(ARGUMENT_KEY.into(), schema);
            fields.insert(ARGUMENT_KEY.into(), Value::Object(argument_hint(argument, language)));
            order.push(json!(ARGUMENT_KEY));
        }
        for f in &self.fields {
            let mut c = convert_field(f)?;
            let label = text_of(f.raw.get("label"), language).map_or_else(|| name_as_label(&f.name), str::to_string);
            c.hint.insert("label".into(), json!(label));
            for (from, to) in [("help", "help"), ("add_label", "add_label")] {
                if let Some(t) = text_of(f.raw.get(from), language) {
                    c.hint.insert(to.into(), json!(t));
                }
            }
            apply_correct(f, &mut c.hint, &mut variants)?;
            if c.required {
                required.push(json!(f.name));
            }
            order.push(json!(f.name));
            properties.insert(f.name.clone(), Value::Object(c.schema));
            fields.insert(f.name.clone(), Value::Object(c.hint));
        }
        for p in presets {
            if p.extra.get("widget").and_then(Value::as_str) != Some(ICON_CHOICES) {
                continue;
            }
            let wanted = preset_text(p, "field")?;
            let field = self
                .fields
                .iter()
                .find(|f| f.kind == FieldKind::Choices && wanted.is_none_or(|w| w == f.name));
            let Some(field) = field else {
                return refuse(
                    wanted.unwrap_or(""),
                    format!("preset \"{}\" asks for icon choices on no choices field", p.id),
                );
            };
            // no key: every variant, through the base hints; `""`: the block without one
            match preset_text(p, "variant")? {
                Some(v) => icon_choice_hints(p, field, language, variant_field(&mut variants, v, &field.name))?,
                None => {
                    let hint = fields[&field.name].as_object_mut().expect("built above");
                    icon_choice_hints(p, field, language, hint)?;
                }
            }
        }
        let mut hints = json!({"version": HINTS_VERSION, "order": order, "fields": fields});
        if !variants.is_empty() {
            hints["variants"] = Value::Object(variants);
        }
        Ok(FormInput {
            schema: json!({"type": "object", "required": required, "properties": properties}),
            hints,
        })
    }

    /// The block's variant names, `""` included if the schema names it.
    pub fn variants(&self) -> impl Iterator<Item = &str> {
        self.raw
            .get("variants")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|m| m.keys().map(String::as_str))
    }

    /// The text for the user of a variant.
    pub fn variant_label(&self, variant: &str, language: &str) -> Option<&str> {
        text_of(self.raw.get("variants")?.get(variant), language)
    }
}

impl PackageIndex {
    /// The form of block `name`; `None` when the package has no schema for it.
    pub fn form_input(&self, name: &str, language: &str) -> Option<Result<FormInput, FormRefusal>> {
        let entry = self.block(name)?;
        Some(entry.schema.form_input(self.presets_of(name), language))
    }
}

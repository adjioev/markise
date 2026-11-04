//! The renderer-independent field model: a list of fields with id, kind, label, options and
//! validation, parsed from a JSON Schema + a UI Schema. No GUI types here, so that another
//! renderer (for example editing in place on a card) can share it.
//!
//! Supported vocabulary: `VerticalLayout`/`HorizontalLayout`/`Group` (flattened), `Label`,
//! `Control` with a `#/properties/<key>` scope and `options` (`multi`, `format`). This is not a
//! general JSON Forms implementation: anything else becomes [`FieldKind::Unsupported`].

use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    /// A text line without a value (`options.section`: a group's title).
    Label,
    /// A string; `multi` allows line breaks.
    Text { multi: bool },
    /// An array of strings: add, remove, up, down.
    StringList,
    /// `options.format = "choices"`: an array of `{id, text, correct}`; `options.single` makes
    /// it a radio group, `options.correct` (`exactly-one`, `at-least-one`) is validated.
    Choices,
    /// `options.format = <name>`: drawn by a widget the host registered under that name.
    Custom(String),
    /// Nothing here can edit this; the reason is shown.
    Unsupported(String),
}

#[derive(Clone, Debug)]
pub struct Field {
    /// The key in the values object (empty for labels).
    pub id: String,
    pub kind: FieldKind,
    pub label: String,
    pub options: Value,
    /// The JSON Schema of the value.
    pub schema: Value,
}

/// An error of a field; `path` is `key`, `key/2` or `key/2/text`.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldError {
    pub path: String,
    pub message: String,
}

/// The form's fixed texts.
#[derive(Clone, Debug)]
pub struct Texts {
    pub lang: String,
    pub add: &'static str,
    pub add_answer: &'static str,
    pub remove: &'static str,
    pub up: &'static str,
    pub down: &'static str,
    pub answer: &'static str,
    pub correct: &'static str,
    pub required: &'static str,
    pub min_items: &'static str,
    pub max_items: &'static str,
    pub exactly_one: &'static str,
    pub at_least_one: &'static str,
    pub unsupported: &'static str,
}

impl Texts {
    pub fn for_lang(lang: &str) -> Self {
        if lang == "ru" {
            Texts {
                lang: lang.into(),
                add: "Добавить",
                add_answer: "Добавить ответ",
                remove: "Удалить",
                up: "Выше (⌥↑)",
                down: "Ниже (⌥↓)",
                answer: "Текст ответа",
                correct: "верный",
                required: "Заполните поле",
                min_items: "Нужно не меньше {}",
                max_items: "Можно не больше {}",
                exactly_one: "Отметьте ровно один верный ответ",
                at_least_one: "Отметьте хотя бы один верный ответ",
                unsupported: "Это поле здесь не редактируется",
            }
        } else {
            Texts {
                lang: lang.into(),
                add: "Add",
                add_answer: "Add answer",
                remove: "Remove",
                up: "Up (⌥↑)",
                down: "Down (⌥↓)",
                answer: "Answer text",
                correct: "correct",
                required: "Required",
                min_items: "At least {} needed",
                max_items: "At most {} allowed",
                exactly_one: "Mark exactly one correct answer",
                at_least_one: "Mark at least one correct answer",
                unsupported: "This field cannot be edited here",
            }
        }
    }
}

/// A parsed form; build it once when the form opens.
#[derive(Clone, Debug)]
pub struct FormSchema {
    pub fields: Vec<Field>,
    pub texts: Texts,
}

impl FormSchema {
    pub fn parse(schema: &Value, ui_schema: &Value, lang: &str) -> Self {
        let mut fields = Vec::new();
        element(schema, ui_schema, &mut fields);
        FormSchema {
            fields,
            texts: Texts::for_lang(lang),
        }
    }

    /// Every error of `values`, in field order.
    pub fn validate(&self, values: &Value) -> Vec<FieldError> {
        let mut out = Vec::new();
        for f in &self.fields {
            if matches!(f.kind, FieldKind::Label | FieldKind::Unsupported(_)) {
                continue;
            }
            let v = &values[f.id.as_str()];
            check(&f.schema, v, &f.id, &self.texts, &mut out);
            if f.options.get("correct").is_some() {
                let n = v
                    .as_array()
                    .map_or(0, |a| a.iter().filter(|x| x["correct"] == true).count());
                let msg = match f.options["correct"].as_str() {
                    Some("exactly-one") if n != 1 => Some(self.texts.exactly_one),
                    Some("at-least-one") if n == 0 => Some(self.texts.at_least_one),
                    _ => None,
                };
                if let Some(m) = msg {
                    out.push(FieldError {
                        path: f.id.clone(),
                        message: m.to_string(),
                    });
                }
            }
        }
        out
    }
}

fn element(schema: &Value, e: &Value, out: &mut Vec<Field>) {
    let plain = |label: &str, section: bool| Field {
        id: String::new(),
        kind: FieldKind::Label,
        label: label.to_string(),
        options: serde_json::json!({"section": section}),
        schema: Value::Null,
    };
    match e["type"].as_str().unwrap_or("") {
        "VerticalLayout" | "HorizontalLayout" | "Group" => {
            if let Some(l) = e["label"].as_str() {
                out.push(plain(l, true));
            }
            for c in e["elements"].as_array().into_iter().flatten() {
                element(schema, c, out);
            }
        }
        "Label" => out.push(plain(e["text"].as_str().unwrap_or(""), false)),
        "Control" => {
            let scope = e["scope"].as_str().unwrap_or("");
            let id = scope.strip_prefix("#/properties/").unwrap_or(scope).to_string();
            let sch = schema["properties"][id.as_str()].clone();
            let options = e.get("options").cloned().unwrap_or(Value::Null);
            let kind = match (options["format"].as_str(), sch["type"].as_str()) {
                (Some("choices"), _) => FieldKind::Choices,
                (Some(name), _) => FieldKind::Custom(name.to_string()),
                (None, Some("string")) => FieldKind::Text {
                    multi: options["multi"] == true,
                },
                (None, Some("array")) if sch["items"]["type"] == "string" => FieldKind::StringList,
                (None, t) => FieldKind::Unsupported(format!("type {}", t.unwrap_or("?"))),
            };
            out.push(Field {
                label: e["label"].as_str().unwrap_or(&id).to_string(),
                id,
                kind,
                options,
                schema: sch,
            });
        }
        other => out.push(Field {
            id: String::new(),
            kind: FieldKind::Unsupported(format!("layout {other}")),
            label: String::new(),
            options: Value::Null,
            schema: Value::Null,
        }),
    }
}

/// The subset of JSON Schema the forms use: `minLength`, `minItems`, `maxItems`, `items`,
/// `properties`.
fn check(s: &Value, v: &Value, path: &str, t: &Texts, out: &mut Vec<FieldError>) {
    let mut err = |message: String| {
        out.push(FieldError {
            path: path.to_string(),
            message,
        })
    };
    match v {
        Value::String(x) => {
            if s["minLength"].as_u64().is_some_and(|n| (x.chars().count() as u64) < n) {
                err(t.required.to_string());
            }
        }
        Value::Array(a) => {
            if let Some(n) = s["minItems"].as_u64().filter(|n| (a.len() as u64) < *n) {
                err(t.min_items.replace("{}", &n.to_string()));
            }
            if let Some(n) = s["maxItems"].as_u64().filter(|n| (a.len() as u64) > *n) {
                err(t.max_items.replace("{}", &n.to_string()));
            }
            for (i, x) in a.iter().enumerate() {
                check(&s["items"], x, &format!("{path}/{i}"), t, out);
            }
        }
        Value::Object(o) => {
            for (k, x) in o {
                if let Some(ks) = s["properties"].get(k) {
                    check(ks, x, &format!("{path}/{k}"), t, out);
                }
            }
        }
        _ => {}
    }
}

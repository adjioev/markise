use egui_kittest::Harness;
use egui_schema_form::schema_form_core::FormSpec;
use egui_schema_form::{FormOutput, SchemaForm};
use serde_json::{Value, json};

pub struct S {
    pub form: SchemaForm,
    /// How many frames reported a change.
    pub changes: usize,
    pub last: FormOutput,
}

pub fn schema() -> Value {
    json!({
        "type": "object",
        "required": ["title", "answers"],
        "properties": {
            "title": {"type": "string", "minLength": 1},
            "body": {"type": "string"},
            "items": {"type": "array", "minItems": 1, "maxItems": 3, "items": {"type": "string", "minLength": 1}},
            "answers": {
                "type": "array",
                "minItems": 2,
                "maxItems": 4,
                "items": {"type": "object", "properties": {
                    "id": {"type": "string"},
                    "text": {"type": "string", "minLength": 1},
                    "correct": {"type": "boolean"},
                }},
            },
            "rows": {"type": "array", "items": {"type": "object", "properties": {
                "k": {"type": "string"},
                "on": {"type": "boolean"},
            }}},
        },
    })
}

pub fn hints(selection: &str) -> Value {
    json!({
        "version": 1,
        "order": ["title", "body", "items", "answers", "rows"],
        "fields": {
            "title": {"label": "Title"},
            "body": {"label": "Body", "widget": "multiline", "help": "Longer text"},
            "items": {"label": "Items", "add_label": "Add item"},
            "answers": {
                "label": "Answers",
                "widget": "choices",
                "selection": selection,
                "correct": if selection == "single" { "exactly-one" } else { "at-least-one" },
                "add_label": "Add answer",
            },
            "rows": {"label": "Rows", "add_label": "Add row", "fields": {"k": {"label": "Key"}, "on": {"label": "On"}}},
        },
    })
}

pub fn values() -> Value {
    json!({
        "title": "A title",
        "body": "Some words",
        "items": ["one", "two"],
        "answers": [
            {"id": "a1", "text": "Red", "correct": false},
            {"id": "a2", "text": "Green", "correct": true},
            {"id": "a3", "text": "Blue", "correct": false},
        ],
        "rows": [{"k": "x", "on": false}, {"k": "y", "on": true}],
    })
}

pub fn harness_with(schema: &Value, hints: &Value, values: Value) -> Harness<'static, S> {
    let spec = FormSpec::parse(schema, hints, None).expect("in the subset");
    let state = S {
        form: SchemaForm::new(spec, values, "test"),
        changes: 0,
        last: FormOutput::default(),
    };
    Harness::builder().with_size(egui::vec2(480.0, 900.0)).build_ui_state(
        |ui, s: &mut S| {
            s.last = s.form.ui(ui);
            s.changes += usize::from(s.last.changed);
        },
        state,
    )
}

pub fn harness(selection: &str) -> Harness<'static, S> {
    harness_with(&schema(), &hints(selection), values())
}

pub fn value_of(h: &Harness<'_, S>, key: &str) -> Value {
    h.state().form.values()[key].clone()
}

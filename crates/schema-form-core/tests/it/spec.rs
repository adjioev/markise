//! What parses is what is promised: everything outside the subset is refused by name.

use schema_form_core::{ChoiceShape, Control, CorrectRule, ErrorKind, FormSpec, SPEC_VERSION, Selection};
use serde_json::{Value, json};

fn schema() -> Value {
    json!({
        "type": "object",
        "required": ["title", "answers"],
        "properties": {
            "title": {"type": "string", "minLength": 1, "title": "Schema title"},
            "body": {"type": "string"},
            "tags": {"type": "array", "items": {"type": "string", "minLength": 1}, "maxItems": 4},
            "answers": {
                "type": "array",
                "minItems": 2,
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "text": {"type": "string", "minLength": 1},
                        "correct": {"type": "boolean"},
                    },
                },
            },
            "rows": {
                "type": "array",
                "items": {"type": "object", "properties": {"k": {"type": "string"}, "v": {"type": "string"}}},
            },
            "meta": {"type": "object", "properties": {"note": {"type": "string"}}},
        },
    })
}

fn hints() -> Value {
    json!({
        "version": SPEC_VERSION,
        "order": ["title", "answers", "body"],
        "fields": {
            "body": {"widget": "multiline", "label": "Body", "help": "More words", "x-host": 7},
            "answers": {"widget": "choices", "correct": "exactly-one", "add_label": "Add answer"},
            "rows": {"fields": {"v": {"label": "Value"}}, "order": ["v", "k"]},
        },
        "variants": {
            "several": {"fields": {"answers": {"correct": "at-least-one", "selection": "multiple"}}},
        },
    })
}

#[test]
fn the_tree_follows_the_hints() {
    let spec = FormSpec::parse(&schema(), &hints(), None).expect("in the subset");
    let keys: Vec<&str> = spec.fields.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(keys, ["title", "answers", "body", "meta", "rows", "tags"]);
    assert_eq!(spec.fields[0].label, "Schema title");
    assert!(spec.fields[0].required);
    assert_eq!(spec.fields[1].add_label.as_deref(), Some("Add answer"));
    assert_eq!(
        spec.fields[1].control,
        Control::Choices {
            selection: Selection::Single,
            correct: CorrectRule::ExactlyOne
        }
    );
    assert_eq!(spec.fields[2].control, Control::Text { multiline: true });
    assert_eq!(spec.fields[2].extensions["x-host"], json!(7));
    let Control::Array { fields } = &spec.fields[4].control else {
        panic!("an array of groups")
    };
    assert_eq!(fields[0].label, "Value");
    assert!(matches!(spec.fields[3].control, Control::Group { .. }));
    assert!(matches!(spec.fields[5].control, Control::StringList { .. }));
}

#[test]
fn a_variant_overlays_the_hints() {
    let spec = FormSpec::parse(&schema(), &hints(), Some("several")).expect("in the subset");
    assert_eq!(
        spec.fields[1].control,
        Control::Choices {
            selection: Selection::Multiple,
            correct: CorrectRule::AtLeastOne
        }
    );
    let same = FormSpec::parse(&schema(), &hints(), Some("no such")).expect("in the subset");
    assert_eq!(same, FormSpec::parse(&schema(), &hints(), None).expect("in the subset"));
}

#[test]
fn another_version_is_refused() {
    for h in [json!({}), json!({"version": SPEC_VERSION + 1})] {
        let e = FormSpec::parse(&schema(), &h, None).expect_err("refused");
        assert_eq!(e.path, "hints/version");
    }
}

#[test]
fn keywords_outside_the_subset_are_refused_by_name() {
    for bad in [
        "pattern", "oneOf", "anyOf", "allOf", "$ref", "if", "format", "const", "default",
    ] {
        let mut s = schema();
        s["properties"]["title"][bad] = json!("x");
        let e = FormSpec::parse(&s, &hints(), None).expect_err(bad);
        assert_eq!(e.path, format!("title/{bad}"));
    }
    let mut s = schema();
    s["$ref"] = json!("#/x");
    assert!(FormSpec::parse(&s, &hints(), None).is_err());
}

#[test]
fn hints_outside_the_format_are_refused() {
    let cases = [
        json!({"version": SPEC_VERSION, "elements": []}),
        json!({"version": SPEC_VERSION, "fields": {"title": {"rule": {}}}}),
        json!({"version": SPEC_VERSION, "fields": {"nope": {}}}),
        json!({"version": SPEC_VERSION, "order": ["nope"]}),
        json!({"version": SPEC_VERSION, "order": ["title", "title"]}),
        json!({"version": SPEC_VERSION, "fields": {"title": {"widget": "choices"}}}),
        json!({"version": SPEC_VERSION, "fields": {"title": {"widget": "direction"}}}),
        json!({"version": SPEC_VERSION, "fields": {"title": {"widget": "custom"}}}),
        json!({"version": SPEC_VERSION, "fields": {"tags": {"widget": "choices"}}}),
        json!({"version": SPEC_VERSION, "fields": {"answers": {"widget": "choices", "correct": "two"}}}),
        json!({"version": SPEC_VERSION, "fields": {"answers": {"widget": "images"}}}),
    ];
    for h in cases {
        assert!(FormSpec::parse(&schema(), &h, None).is_err(), "{h}");
    }
}

#[test]
fn a_custom_widget_is_a_name_and_nothing_else() {
    let h = json!({"version": SPEC_VERSION, "fields": {"title": {"widget": "custom", "custom": "my-picker"}}});
    let spec = FormSpec::parse(&schema(), &h, None).expect("in the subset");
    assert_eq!(
        spec.fields.iter().find(|f| f.key == "title").expect("title").control,
        Control::Custom {
            name: "my-picker".into()
        }
    );
}

#[test]
fn a_variant_that_is_no_object_is_refused_not_a_panic() {
    let cases = [
        json!({"version": SPEC_VERSION, "variants": {"bad": null}}),
        json!({"version": SPEC_VERSION, "variants": {"bad": 3}}),
        json!({"version": SPEC_VERSION, "variants": 3}),
        json!({"version": SPEC_VERSION, "variants": {"bad": {"version": 2}}}),
        json!({"version": SPEC_VERSION, "variants": {"bad": {"elements": []}}}),
    ];
    for h in cases {
        for variant in [Some("bad"), None] {
            assert!(FormSpec::parse(&schema(), &h, variant).is_err(), "{h} with {variant:?}");
        }
    }
}

#[test]
fn the_subset_holds_at_any_depth() {
    let inner = |item: Value, widget: &str| {
        let schema = json!({"type": "object", "properties": {"a": {"type": "array", "items": item}}});
        let hints = json!({"version": 1, "fields": {"a": {"widget": widget}}});
        FormSpec::parse(&schema, &hints, None)
    };
    let props = |extra: Value| json!({"id": {"type": "string"}, "text": extra, "correct": {"type": "boolean"}});
    let choices = |extra: Value| json!({"type": "object", "properties": props(extra)});
    let refused = inner(choices(json!({"type": "string", "pattern": "x"})), "choices").expect_err("refused");
    assert_eq!(refused.path, "a/items/text/pattern");
    assert!(inner(choices(json!({"type": "string", "const": "x"})), "choices").is_err());
    let images = json!({"type": "object", "pattern": "x", "properties": {
        "alt": {"type": "string"}, "path": {"type": "string"},
    }});
    assert!(inner(images, "images").is_err());
    assert!(inner(choices(json!({"type": "string"})), "choices").is_ok());
    // a keyword that would mean nothing on its type is no quiet promise
    let bad = json!({"type": "object", "properties": {"t": {"type": "string", "minItems": 1}}});
    assert!(FormSpec::parse(&bad, &json!({"version": 1}), None).is_err());
}

#[test]
fn required_must_be_a_list_of_names() {
    for required in [
        json!("title"),
        json!([1]),
        json!(["title", null]),
        json!({"title": true}),
    ] {
        let s = json!({"type": "object", "required": required, "properties": {"title": {"type": "string"}}});
        let e = FormSpec::parse(&s, &json!({"version": 1}), None).expect_err("refused");
        assert_eq!(e.path, "required", "{s}");
    }
}

#[test]
fn a_custom_widget_cannot_declare_what_the_subset_lacks() {
    let hints = json!({"version": 1, "fields": {"c": {"widget": "custom", "custom": "w"}}});
    for c in [
        json!({"type": "integer"}),
        json!({"type": ["string", "null"]}),
        json!({"type": "string", "pattern": "x"}),
        json!({"type": "object", "properties": {"n": {"type": "integer"}}}),
        json!({"type": "array", "items": {"type": "number"}}),
    ] {
        let s = json!({"type": "object", "properties": {"c": c}});
        assert!(FormSpec::parse(&s, &hints, None).is_err(), "{s}");
    }
}

#[test]
fn a_custom_widget_without_a_type_cannot_have_constraints() {
    let hints = json!({"version": 1, "fields": {"c": {"widget": "custom", "custom": "w"}}});
    for constraint in ["maxLength", "minLength", "minItems", "maxItems"] {
        let s = json!({"type": "object", "properties": {"c": {constraint: 1}}});
        let e = FormSpec::parse(&s, &hints, None).expect_err(constraint);
        assert_eq!(e.path, format!("c/{constraint}"));
    }
    for (key, value) in [
        ("required", json!([])),
        ("properties", json!({})),
        ("items", json!({"type": "string"})),
    ] {
        let s = json!({"type": "object", "properties": {"c": {key: value}}});
        assert!(FormSpec::parse(&s, &hints, None).is_err(), "{s}");
    }
    let bare = json!({"type": "object", "properties": {"c": {"description": "anything goes"}}});
    assert!(
        FormSpec::parse(&bare, &hints, None).is_ok(),
        "an annotation constrains nothing"
    );
}

fn buttons_schema() -> Value {
    json!({"type": "object", "properties": {
        "one": {"type": "string", "enum": ["a", "b"]},
        "many": {"type": "array", "items": {"type": "string", "enum": ["a", "b", "c"]}},
        "marks": {"type": "array", "items": {"type": "object", "properties": {
            "id": {"type": "string"}, "text": {"type": "string"}, "correct": {"type": "boolean"},
        }}},
    }})
}

fn buttons_hints() -> Value {
    json!({"version": SPEC_VERSION, "fields": {
        "one": {"options": [{"value": "b", "label": "Bee", "icon": "bee"}]},
        "many": {"columns": 2, "options": [{"value": "c", "exclusive": true}]},
        "marks": {"widget": "buttons", "correct": "at-least-one", "options": [
            {"value": "Left", "icon": "left"},
            {"value": "None", "label": "Neither", "exclusive": true},
        ]},
    }})
}

#[test]
fn choice_buttons_read_every_shape() {
    let spec = FormSpec::parse(&buttons_schema(), &buttons_hints(), None).expect("in the subset");
    let control = |key: &str| &spec.fields.iter().find(|f| f.key == key).expect("field").control;
    let Control::ChoiceButtons {
        options,
        selection,
        shape,
        columns,
    } = control("one")
    else {
        panic!("{:?}", control("one"));
    };
    assert_eq!(
        (*shape, *selection, *columns),
        (ChoiceShape::String, Selection::Single, None)
    );
    // the hints' options first, then the rest of the enum under its own name
    assert_eq!(options[0].label, "Bee");
    assert_eq!(options[0].icon.as_deref(), Some("bee"));
    assert_eq!(options[1].value, "a");
    assert_eq!(options[1].label, "a");
    let Control::ChoiceButtons {
        options,
        shape,
        columns,
        selection,
    } = control("many")
    else {
        panic!();
    };
    assert_eq!(
        (*shape, *selection, *columns),
        (ChoiceShape::Strings, Selection::Multiple, Some(2))
    );
    assert!(options[0].exclusive);
    let Control::ChoiceButtons { options, shape, .. } = control("marks") else {
        panic!();
    };
    assert_eq!(*shape, ChoiceShape::Marks);
    assert_eq!(options[1].label, "Neither");
}

#[test]
fn enum_means_buttons_unless_a_widget_says_otherwise() {
    let schema = json!({"type": "object", "properties": {"x": {"type": "string", "enum": ["a"]}}});
    let hints = json!({"version": SPEC_VERSION, "fields": {"x": {"widget": "text"}}});
    let spec = FormSpec::parse(&schema, &hints, None).expect("in the subset");
    assert_eq!(spec.fields[0].control, Control::Text { multiline: false });
    assert_eq!(spec.validate(&json!({"x": "b"}))[0].kind, ErrorKind::NotAnOption);
}

#[test]
fn version_1_hints_keep_their_meaning() {
    let plain = json!({"type": "object", "properties": {"x": {"type": "string"}}});
    FormSpec::parse(&plain, &json!({"version": 1}), None).expect("still read");
    let with_enum = json!({"type": "object", "properties": {"x": {"type": "string", "enum": ["a"]}}});
    let e = FormSpec::parse(&with_enum, &json!({"version": 1}), None).expect_err("needs 2");
    assert_eq!(e.path, "hints/version");
    FormSpec::parse(&with_enum, &json!({"version": 2}), None).expect("in the subset");
}

#[test]
fn choices_outside_the_subset_are_refused_by_name() {
    let refused = |schema: Value, hints: Value| {
        let h = json!({"version": SPEC_VERSION, "fields": {"x": hints}});
        let s = json!({"type": "object", "properties": {"x": schema}});
        FormSpec::parse(&s, &h, None).expect_err("refused").path
    };
    let en = json!({"type": "string", "enum": ["a"]});
    assert_eq!(refused(json!({"type": "boolean", "enum": ["a"]}), json!({})), "x/enum");
    assert_eq!(refused(json!({"type": "string", "enum": []}), json!({})), "x/enum");
    assert_eq!(
        refused(json!({"type": "string", "enum": ["a", "a"]}), json!({})),
        "x/enum"
    );
    assert_eq!(refused(json!({"type": "string", "enum": [1]}), json!({})), "x/enum");
    assert_eq!(refused(en.clone(), json!({"options": [{"value": "z"}]})), "x/options/0");
    assert_eq!(refused(en.clone(), json!({"options": [{}]})), "x/options/0");
    assert_eq!(
        refused(en.clone(), json!({"options": [{"value": "a", "site": 1}]})),
        "x/options/0/site"
    );
    assert_eq!(
        refused(en.clone(), json!({"options": [{"value": "a", "exclusive": "yes"}]})),
        "x/options/0/exclusive"
    );
    assert_eq!(refused(en.clone(), json!({"columns": 0})), "x/columns");
    assert_eq!(refused(en.clone(), json!({"selection": "multiple"})), "x/selection");
    assert_eq!(refused(json!({"type": "string"}), json!({"widget": "buttons"})), "x");
    let answers = json!({"type": "array", "items": {"type": "object", "properties": {
        "id": {"type": "string"}, "text": {"type": "string"}, "correct": {"type": "boolean"},
    }}});
    // answers have nothing that lists their options
    assert_eq!(refused(answers, json!({"widget": "buttons"})), "x/options");
}

#[test]
fn the_overlay_named_empty_is_for_no_variant_alone() {
    let s = json!({"type": "object", "properties": {"x": {"type": "string"}}});
    let h = json!({"version": SPEC_VERSION, "variants": {
        "": {"fields": {"x": {"label": "Plain"}}},
        "other": {"fields": {"x": {"help": "Other"}}},
    }});
    let label = |variant| {
        FormSpec::parse(&s, &h, variant).expect("in the subset").fields[0]
            .label
            .clone()
    };
    assert_eq!(label(None), "Plain");
    assert_eq!(label(Some("")), "Plain");
    assert_eq!(label(Some("other")), "x", "not inherited");
    assert_eq!(label(Some("unknown")), "x");
}

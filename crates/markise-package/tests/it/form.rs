//! From a package's declared fields to the input of a form (ADR 0020).

use std::collections::BTreeMap;

use markise_package::{ARGUMENT_KEY, FormInput, FormRefusal, PackageIndex};
use schema_form_core::{ChoiceShape, Control, CorrectRule, ErrorKind, FormSpec, Selection};
use serde_json::{Value, json};

/// A package with one block `b`, and presets if any.
fn index(block: &Value, presets: &[Value]) -> PackageIndex {
    let mut files = BTreeMap::new();
    files.insert(
        "manifest.json".to_string(),
        br#"{"format":"markise-block-package/2","library":"sample","version":"1","blocks":["b"]}"#.to_vec(),
    );
    files.insert("components/b/b.json".to_string(), block.to_string().into_bytes());
    files.insert(
        "editor/palette.json".to_string(),
        json!({"format": 1, "presets": presets}).to_string().into_bytes(),
    );
    PackageIndex::from_files(&files).expect("a valid package")
}

fn input(block: &Value, presets: &[Value]) -> Result<FormInput, FormRefusal> {
    index(block, presets).form_input("b", "en").expect("a schema")
}

fn spec_of(block: &Value, presets: &[Value], variant: Option<&str>) -> FormSpec {
    let i = input(block, presets).expect("converted");
    FormSpec::parse(&i.schema, &i.hints, variant).expect("in the subset")
}

fn all_types() -> Value {
    json!({"label": "All", "fields": [
        {"name": "title", "type": "heading", "level": 2, "min": 0},
        {"name": "first_line", "type": "heading"},
        {"name": "body", "type": "text", "min": 1, "max": null, "label": {"en": "Body text", "fr": "Corps"}},
        {"name": "more", "type": "text", "min": 0, "help": "Optional"},
        {"name": "items", "type": "list", "minItems": 1, "maxItems": 8, "add_label": "Add item"},
        {"name": "answers", "type": "choices", "ids": "required", "minItems": 2},
        {"name": "why", "type": "quote"},
        {"name": "pictures", "type": "image", "min": 0, "max": null},
    ]})
}

#[test]
fn every_type_becomes_a_field_of_the_form() {
    let spec = spec_of(&all_types(), &[], None);
    let keys: Vec<&str> = spec.fields.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "title",
            "first_line",
            "body",
            "more",
            "items",
            "answers",
            "why",
            "pictures"
        ],
        "in the declared order"
    );
    let f = |k: &str| spec.fields.iter().find(|f| f.key == k).expect("field");
    assert_eq!(f("title").control, Control::Text { multiline: false });
    assert!(!f("title").required, "min 0");
    assert!(f("first_line").required, "no min: a heading is expected");
    assert_eq!(f("first_line").label, "First line");
    assert_eq!(f("body").control, Control::Text { multiline: true });
    assert_eq!(f("body").label, "Body text");
    assert!(f("body").required);
    assert!(!f("more").required);
    assert_eq!(f("more").help.as_deref(), Some("Optional"));
    assert_eq!(f("items").control, Control::StringList { multiline: false });
    assert_eq!(f("items").add_label.as_deref(), Some("Add item"));
    assert_eq!(f("items").rule.limits.max_items, Some(8));
    assert!(f("items").required);
    assert!(matches!(f("answers").control, Control::Choices { .. }));
    assert_eq!(f("why").control, Control::Text { multiline: true });
    assert_eq!(f("pictures").control, Control::Images);
    assert!(!f("pictures").required);
    assert_eq!(f("pictures").rule.limits.max_items, None);
    assert_eq!(
        f("title").extensions["x-markdown"],
        json!({"kind": "heading", "level": 2})
    );
    assert_eq!(f("why").extensions["x-markdown"], json!({"kind": "quote"}));
}

fn with_variants() -> Value {
    json!({
        "label": "Poll",
        "variants": {"": "one", "multi": {"en": "several", "fr": "plusieurs"}, "icons": "with icons"},
        "fields": [{"name": "answers", "type": "choices", "minItems": 2, "correct": {
            "": "exactly-one", "multi": "at-least-one", "icons": "at-least-one",
        }}],
    })
}

#[test]
fn correctness_follows_the_variant() {
    let control = |variant| spec_of(&with_variants(), &[], variant).fields[0].control.clone();
    let choices = |selection, correct| Control::Choices { selection, correct };
    assert_eq!(control(None), choices(Selection::Single, CorrectRule::ExactlyOne));
    assert_eq!(
        control(Some("multi")),
        choices(Selection::Multiple, CorrectRule::AtLeastOne)
    );
    let spec = spec_of(&with_variants(), &[], Some("multi"));
    let v = json!({"answers": [
        {"id": "a", "text": "x", "correct": false},
        {"id": "b", "text": "y", "correct": false},
    ]});
    assert_eq!(spec.validate(&v)[0].kind, ErrorKind::AtLeastOneCorrect);
}

#[test]
fn one_rule_for_all_variants() {
    let block = json!({"fields": [{"name": "a", "type": "choices", "correct": "exactly-one"}]});
    assert!(matches!(
        spec_of(&block, &[], None).fields[0].control,
        Control::Choices {
            correct: CorrectRule::ExactlyOne,
            ..
        }
    ));
}

#[test]
fn variants_answer_by_name_and_language() {
    let i = index(&with_variants(), &[]);
    let s = i.block("b").expect("a block").schema;
    assert_eq!(s.variant_label("multi", "fr"), Some("plusieurs"));
    assert_eq!(s.variant_label("nope", "fr"), None);
    let names: Vec<&str> = s.variants().collect();
    assert_eq!(names, ["", "icons", "multi"]);
}

fn icon_preset() -> Value {
    json!({
        "id": "p", "block": "b", "label": "With icons",
        "markdown": ":::b icons\n\n- [ ] Left {#a1}\n\n:::\n",
        "variant": "icons", "widget": "icon-choices", "field": "answers", "columns": 3,
        "field_label": {"fr": "Sens", "en": "Way"},
        "answers": [
            {"label": "Left", "icon": "assets/left.svg"},
            {"label": "Right", "icon": "assets/right.svg"},
            {"label": "No way", "exclusive": true},
        ],
    })
}

#[test]
fn a_preset_draws_its_variants_choice_as_icon_buttons() {
    let presets = [icon_preset()];
    let plain = spec_of(&with_variants(), &presets, None);
    assert!(
        matches!(plain.fields[0].control, Control::Choices { .. }),
        "only in its variant"
    );
    let spec = spec_of(&with_variants(), &presets, Some("icons"));
    let f = &spec.fields[0];
    assert_eq!(f.label, "Way");
    assert_eq!(
        f.extensions["x-markdown"],
        json!({"kind": "choices"}),
        "from the base hints"
    );
    let Control::ChoiceButtons {
        options,
        selection,
        shape,
        columns,
    } = &f.control
    else {
        panic!("{:?}", f.control);
    };
    assert_eq!(
        (*shape, *selection, *columns),
        (ChoiceShape::Marks, Selection::Multiple, Some(3))
    );
    assert_eq!(options[0].value, "Left");
    assert_eq!(options[0].icon.as_deref(), Some("assets/left.svg"));
    assert!(options[2].exclusive && options[2].icon.is_none());
}

#[test]
fn a_preset_for_the_block_without_a_variant_stays_there() {
    let mut p = icon_preset();
    p["variant"] = json!("");
    let presets = [p];
    let is_buttons = |variant| {
        matches!(
            spec_of(&with_variants(), &presets, variant).fields[0].control,
            Control::ChoiceButtons { .. }
        )
    };
    assert!(is_buttons(None));
    assert!(!is_buttons(Some("multi")), "a named variant has its own form");
    assert!(!is_buttons(Some("icons")));
}

#[test]
fn a_preset_with_no_variant_key_is_for_every_variant() {
    let mut p = icon_preset();
    p.as_object_mut().expect("object").remove("variant");
    let spec = spec_of(&with_variants(), &[p], Some("multi"));
    assert!(matches!(spec.fields[0].control, Control::ChoiceButtons { .. }));
}

#[test]
fn a_preset_key_of_the_wrong_type_is_refused() {
    for (key, value) in [
        ("field", json!(42)),
        ("field", json!(false)),
        ("variant", json!(42)),
        ("variant", json!(false)),
    ] {
        let mut p = icon_preset();
        p[key] = value;
        let e = input(&with_variants(), &[p]).expect_err(key);
        assert!(e.reason.contains(&format!("`{key}`")), "{e}");
    }
}

#[test]
fn a_preset_of_another_widget_is_left_for_later() {
    let mut p = icon_preset();
    p["widget"] = json!("something-new");
    assert!(matches!(
        spec_of(&with_variants(), &[p], Some("icons")).fields[0].control,
        Control::Choices { .. }
    ));
}

#[test]
fn what_a_form_cannot_honour_is_refused_by_name() {
    let refused = |field: Value| {
        input(&json!({"fields": [field]}), &[])
            .expect_err("refused")
            .to_string()
    };
    for (field, needle) in [
        (json!({"name": "a", "type": "text", "max": 3}), "`max` 3"),
        (json!({"name": "a", "type": "text", "min": 2}), "`min` 2"),
        (json!({"name": "a", "type": "heading", "level": 9}), "`level` 9"),
        (json!({"name": "a", "type": "list", "minItems": "x"}), "`minItems`"),
        (json!({"name": "a", "type": "choices", "ids": "optional"}), "`ids`"),
        (json!({"name": "a", "type": "choices", "correct": 1}), "`correct`"),
    ] {
        let why = refused(field);
        assert!(why.contains("field `a`") && why.contains(needle), "{why}");
    }
    let mut p = icon_preset();
    p["answers"] = json!([{"icon": "x"}]);
    assert!(
        input(&with_variants(), &[p])
            .expect_err("refused")
            .reason
            .contains("no `label`")
    );
    let mut p = icon_preset();
    p["field"] = json!("nope");
    let e = input(&with_variants(), &[p]).expect_err("refused");
    assert_eq!(e.field, "nope");
}

#[test]
fn keys_it_does_not_know_are_data_for_later() {
    let block = json!({"fields": [{"name": "a", "type": "text", "someday": {"x": 1}}]});
    assert_eq!(spec_of(&block, &[], None).fields.len(), 1);
}

#[test]
fn a_name_outside_the_package_has_no_form() {
    assert!(index(&all_types(), &[]).form_input("other", "en").is_none());
}

#[test]
fn an_argument_is_the_first_field_with_a_key_no_field_can_have() {
    let block = json!({"label": "Plate", "argument": {"name": "image_set"}, "fields": [
        {"name": "image_set", "type": "text", "min": 0},
    ]});
    let spec = spec_of(&block, &[], None);
    let keys: Vec<&str> = spec.fields.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(keys, [ARGUMENT_KEY, "image_set"]);
    let field = &spec.fields[0];
    assert_eq!(field.label, "Image set");
    assert_eq!(field.control, Control::Text { multiline: false });
    assert_eq!(field.extensions["x-markdown"]["kind"], "argument");
}

#[test]
fn an_argument_s_label_comes_from_the_package_or_a_plain_default() {
    let label_of = |argument: Value| {
        let block = json!({"label": "Plate", "argument": argument, "fields": []});
        spec_of(&block, &[], None).fields[0].label.clone()
    };
    assert_eq!(label_of(json!({"name": "set", "label": "Which set"})), "Which set");
    assert_eq!(
        label_of(json!({"label": {"en": "Which set", "de": "Welche"}})),
        "Which set"
    );
    assert_eq!(label_of(json!({"name": "set"})), "Set");
    assert_eq!(label_of(json!({})), "Argument");
    let block = json!({"label": "Plate", "argument": {"help": "As set/name"}, "fields": []});
    assert_eq!(
        spec_of(&block, &[], None).fields[0].help.as_deref(),
        Some("As set/name")
    );
}

#[test]
fn a_block_with_no_argument_has_no_such_field() {
    let spec = spec_of(&all_types(), &[], None);
    assert!(spec.fields.iter().all(|f| f.key != ARGUMENT_KEY));
    let block = json!({"label": "V", "variants": {"": "one", "many": "several"}, "fields": []});
    assert!(
        spec_of(&block, &[], Some("many")).fields.is_empty(),
        "a variant stays what it was"
    );
}

#[test]
fn an_argument_is_required_and_answers_to_its_pattern() {
    let block = json!({"label": "Plate", "argument": {"name": "set", "pattern": "^[a-z]+/[a-z]+$"}, "fields": []});
    let spec = spec_of(&block, &[], None);
    assert!(spec.fields[0].required);
    let kinds = |v: Value| spec.validate(&v).into_iter().map(|e| e.kind).collect::<Vec<_>>();
    assert!(kinds(json!({ARGUMENT_KEY: "a/b"})).is_empty());
    assert_eq!(kinds(json!({ARGUMENT_KEY: ""})), [ErrorKind::Required]);
    assert_eq!(
        kinds(json!({ARGUMENT_KEY: "bad arg!"})),
        [ErrorKind::NoMatch {
            pattern: "^[a-z]+/[a-z]+$".to_string()
        }]
    );
    // no pattern: anything goes, as long as something is there
    let block = json!({"label": "Plate", "argument": {"name": "set"}, "fields": []});
    let spec = spec_of(&block, &[], None);
    assert!(spec.validate(&json!({ARGUMENT_KEY: "any thing!"})).is_empty());
    assert_eq!(spec.validate(&json!({ARGUMENT_KEY: ""}))[0].kind, ErrorKind::Required);
}

#[test]
fn beside_variants_an_argument_is_checked_by_naming_one() {
    let block = json!({"label": "Sign", "variants": {"": "plain", "loud": "loud"},
        "argument": {"name": "how", "pattern": "^x$"}, "fields": []});
    let spec = spec_of(&block, &[], Some("loud"));
    assert!(!spec.fields[0].required);
    assert!(spec.validate(&json!({ARGUMENT_KEY: "loud"})).is_empty());
    assert!(
        spec_of(&block, &[], None)
            .validate(&json!({ARGUMENT_KEY: ""}))
            .is_empty()
    );
}

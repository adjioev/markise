//! Reasons per field.

use proptest::prelude::*;
use schema_form_core::{ErrorKind, FieldError, FormSpec, SPEC_VERSION};
use serde_json::{Value, json};

fn spec(correct: &str) -> FormSpec {
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["title", "answers"],
        "properties": {
            "title": {"type": "string", "minLength": 3, "maxLength": 5},
            "tags": {"type": "array", "items": {"type": "string", "minLength": 1, "maxLength": 2}, "maxItems": 2},
            "answers": {
                "type": "array",
                "minItems": 2,
                "items": {"type": "object", "properties": {
                    "id": {"type": "string"},
                    "text": {"type": "string"},
                    "correct": {"type": "boolean"},
                }},
            },
            "rows": {"type": "array", "items": {
                "type": "object",
                "required": ["k"],
                "properties": {"k": {"type": "string"}},
            }},
            "meta": {"type": "object", "required": ["n"], "properties": {"n": {"type": "string"}}},
        },
    });
    let hints = json!({
        "version": SPEC_VERSION,
        "fields": {"answers": {"widget": "choices", "correct": correct}},
    });
    FormSpec::parse(&schema, &hints, None).expect("in the subset")
}

fn a(id: &str, correct: bool) -> Value {
    json!({"id": id, "text": "t", "correct": correct})
}

fn kinds(v: Value, correct: &str) -> Vec<(String, ErrorKind)> {
    let e = spec(correct).validate(&v);
    e.into_iter().map(|FieldError { path, kind }| (path, kind)).collect()
}

fn ok_values() -> Value {
    json!({"title": "abcd", "answers": [a("x", true), a("y", false)], "meta": {"n": "v"}})
}

#[test]
fn a_value_that_fits_has_no_reasons() {
    assert_eq!(kinds(ok_values(), "exactly-one"), []);
}

#[test]
fn required_fields() {
    let e = kinds(json!({"meta": {"n": "v"}}), "any");
    assert_eq!(
        e,
        [
            ("answers".to_string(), ErrorKind::Required),
            ("title".to_string(), ErrorKind::Required)
        ]
    );
    let mut v = ok_values();
    v["title"] = json!("  ");
    assert_eq!(kinds(v, "any"), [("title".to_string(), ErrorKind::Required)]);
    let mut v = ok_values();
    v["meta"] = json!({});
    assert_eq!(kinds(v, "any"), [("meta/n".to_string(), ErrorKind::Required)]);
}

#[test]
fn lengths_and_array_limits() {
    let mut v = ok_values();
    v["title"] = json!("ab");
    v["tags"] = json!(["abc", "", "ok"]);
    v["answers"] = json!([a("x", true)]);
    assert_eq!(
        kinds(v, "any"),
        [
            ("answers".to_string(), ErrorKind::TooFew { min: 2 }),
            ("tags".to_string(), ErrorKind::TooMany { max: 2 }),
            ("tags/0".to_string(), ErrorKind::TooLong { max: 2 }),
            ("tags/1".to_string(), ErrorKind::Required),
            ("title".to_string(), ErrorKind::TooShort { min: 3 }),
        ]
    );
    // lengths count characters, not bytes
    let mut v = ok_values();
    v["title"] = json!("ёёё");
    assert_eq!(kinds(v, "any"), []);
}

#[test]
fn correctness_rules() {
    let with = |x: bool, y: bool| {
        let mut v = ok_values();
        v["answers"] = json!([a("x", x), a("y", y)]);
        v
    };
    let wrong_one = [("answers".to_string(), ErrorKind::ExactlyOneCorrect)];
    assert_eq!(kinds(with(true, true), "exactly-one"), wrong_one);
    assert_eq!(kinds(with(false, false), "exactly-one"), wrong_one);
    assert_eq!(kinds(with(false, true), "exactly-one"), []);
    assert_eq!(
        kinds(with(false, false), "at-least-one"),
        [("answers".to_string(), ErrorKind::AtLeastOneCorrect)]
    );
    assert_eq!(kinds(with(true, true), "at-least-one"), []);
    assert_eq!(kinds(with(false, false), "any"), []);
}

#[test]
fn answer_ids() {
    let mut v = ok_values();
    v["answers"] = json!([a("x", true), a("x", false), a("", false)]);
    assert_eq!(
        kinds(v, "any"),
        [
            ("answers/1".to_string(), ErrorKind::DuplicateId { id: "x".into() }),
            ("answers/2".to_string(), ErrorKind::MissingId),
        ]
    );
}

#[test]
fn wrong_types_and_unknown_keys_are_named() {
    let mut v = ok_values();
    v["title"] = json!(1);
    v["tags"] = json!([1]);
    v["rows"] = json!(["x"]);
    v["extra"] = json!(true);
    assert_eq!(
        kinds(v, "any"),
        [
            ("rows/0".to_string(), ErrorKind::WrongType),
            ("tags/0".to_string(), ErrorKind::WrongType),
            ("title".to_string(), ErrorKind::WrongType),
            ("extra".to_string(), ErrorKind::UnknownKey),
        ]
    );
    assert_eq!(
        kinds(json!("no object"), "any"),
        [(String::new(), ErrorKind::WrongType)]
    );
}

fn any_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i32>().prop_map(Value::from),
        "[a-z ]{0,6}".prop_map(Value::from),
    ];
    leaf.prop_recursive(4, 30, 5, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..5).prop_map(Value::from),
            proptest::collection::btree_map(
                prop_oneof![
                    Just("title"),
                    Just("tags"),
                    Just("answers"),
                    Just("rows"),
                    Just("meta"),
                    Just("id")
                ],
                inner,
                0..5,
            )
            .prop_map(|m| m
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect::<serde_json::Map<_, _>>())
            .prop_map(Value::Object),
        ]
    })
}

proptest! {
    /// Any JSON gets reasons, never a panic, and every reason names a place in the value's form.
    #[test]
    fn any_value_gets_reasons_not_a_panic(v in any_json()) {
        for e in spec("exactly-one").validate(&v) {
            let root = e.path.split('/').next().unwrap_or("").to_string();
            prop_assert!(
                ["title", "tags", "answers", "rows", "meta", "id", ""].contains(&root.as_str()),
                "{:?}", e
            );
        }
    }
}

fn parse(schema: Value, hints: Value) -> FormSpec {
    FormSpec::parse(&schema, &hints, None).expect("in the subset")
}

fn reasons(spec: &FormSpec, v: Value) -> Vec<(String, ErrorKind)> {
    spec.validate(&v).into_iter().map(|e| (e.path, e.kind)).collect()
}

fn s(path: &str, kind: ErrorKind) -> (String, ErrorKind) {
    (path.to_string(), kind)
}

#[test]
fn entries_of_choices_and_images_answer_to_their_schemas() {
    let spec = parse(
        json!({"type": "object", "properties": {
            "a": {"type": "array", "items": {"type": "object", "required": ["id"], "properties": {
                "id": {"type": "string"},
                "text": {"type": "string", "minLength": 2},
                "correct": {"type": "boolean"},
            }}},
            "p": {"type": "array", "items": {"type": "object", "properties": {
                "alt": {"type": "string", "maxLength": 1},
                "path": {"type": "string"},
            }}},
        }}),
        json!({"version": 1, "fields": {"a": {"widget": "choices"}, "p": {"widget": "images"}}}),
    );
    assert_eq!(
        reasons(&spec, json!({"a": [{"id": "x", "text": 42, "correct": "yes"}]})),
        [
            s("a/0/correct", ErrorKind::WrongType),
            s("a/0/text", ErrorKind::WrongType)
        ]
    );
    assert_eq!(
        reasons(
            &spec,
            json!({"a": [{"id": "x", "text": "t", "correct": true}, {"id": "y", "text": "", "correct": false}]})
        ),
        [
            s("a/0/text", ErrorKind::TooShort { min: 2 }),
            s("a/1/text", ErrorKind::Required)
        ]
    );
    assert_eq!(
        reasons(&spec, json!({"p": [{"alt": "long", "path": 1}, 7]})),
        [
            s("p/0/alt", ErrorKind::TooLong { max: 1 }),
            s("p/0/path", ErrorKind::WrongType),
            s("p/1", ErrorKind::WrongType),
        ]
    );
}

#[test]
fn additional_properties_hold_at_any_depth() {
    let spec = parse(
        json!({"type": "object", "properties": {
            "g": {"type": "object", "additionalProperties": false, "properties": {"k": {"type": "string"}}},
            "r": {"type": "array", "items": {
                "type": "object", "additionalProperties": false, "properties": {"k": {"type": "string"}},
            }},
            "open": {"type": "object", "properties": {"k": {"type": "string"}}},
        }}),
        json!({"version": 1}),
    );
    assert_eq!(
        reasons(
            &spec,
            json!({"g": {"x": 1}, "r": [{"k": "v", "y": 2}], "open": {"z": 3}, "top": 4})
        ),
        [s("g/x", ErrorKind::UnknownKey), s("r/0/y", ErrorKind::UnknownKey)]
    );
}

#[test]
fn a_custom_widgets_schema_still_holds() {
    let hints = json!({"version": 1, "fields": {
        "o": {"widget": "custom", "custom": "w"},
        "l": {"widget": "custom", "custom": "w"},
        "t": {"widget": "custom", "custom": "w"},
    }});
    let spec = parse(
        json!({"type": "object", "properties": {
            "o": {"type": "object", "required": ["n"], "properties": {"n": {"type": "string"}}},
            "l": {"type": "array", "minItems": 2, "items": {"type": "string"}},
            "t": {"type": "string", "maxLength": 1},
        }}),
        hints,
    );
    assert_eq!(
        reasons(&spec, json!({"o": {}, "l": [1], "t": "long"})),
        [
            s("l", ErrorKind::TooFew { min: 2 }),
            s("l/0", ErrorKind::WrongType),
            s("o/n", ErrorKind::Required),
            s("t", ErrorKind::TooLong { max: 1 }),
        ]
    );
    assert_eq!(reasons(&spec, json!({"o": 1})), [s("o", ErrorKind::WrongType)]);
}

#[test]
fn an_empty_optional_value_passes_its_limits() {
    let spec = parse(
        json!({"type": "object", "properties": {
            "a": {"type": "array", "minItems": 2, "items": {"type": "string"}},
            "t": {"type": "string", "minLength": 2},
        }}),
        json!({"version": 1}),
    );
    assert_eq!(reasons(&spec, json!({"a": [], "t": ""})), []);
    assert_eq!(
        reasons(&spec, json!({"a": ["x"], "t": "x"})),
        [
            s("a", ErrorKind::TooFew { min: 2 }),
            s("t", ErrorKind::TooShort { min: 2 }),
        ]
    );
}

#[test]
fn a_large_list_of_answers_names_its_one_duplicate() {
    // 20,000 entries stay cheap only through a set; the assertion is on the result, not on time
    let schema = json!({"type": "object", "properties": {"a": {"type": "array", "items": {"type": "object", "properties": {
        "id": {"type": "string"}, "text": {"type": "string"}, "correct": {"type": "boolean"},
    }}}}});
    let spec = parse(schema, json!({"version": 1, "fields": {"a": {"widget": "choices"}}}));
    let mut list: Vec<Value> = (0..20_000).map(|i| a(&format!("i{i}"), false)).collect();
    list.push(a("i7", false));
    assert_eq!(
        reasons(&spec, json!({ "a": list })),
        [s("a/20000", ErrorKind::DuplicateId { id: "i7".into() })]
    );
}

#[test]
fn an_empty_value_of_the_wrong_type_is_still_the_wrong_type() {
    let spec = parse(
        json!({"type": "object", "properties": {
            "enabled": {"type": "boolean"},
            "title": {"type": "string", "minLength": 2},
            "tags": {"type": "array", "minItems": 2, "items": {"type": "string"}},
            "meta": {"type": "object", "properties": {}},
        }}),
        json!({"version": 1}),
    );
    for empty in [json!([]), json!(""), json!("  ")] {
        assert_eq!(
            reasons(&spec, json!({ "enabled": empty })),
            [s("enabled", ErrorKind::WrongType)],
            "{empty}"
        );
        assert_eq!(
            reasons(&spec, json!({ "meta": empty })),
            [s("meta", ErrorKind::WrongType)],
            "{empty}"
        );
    }
    assert_eq!(reasons(&spec, json!({"title": []})), [s("title", ErrorKind::WrongType)]);
    assert_eq!(reasons(&spec, json!({"tags": ""})), [s("tags", ErrorKind::WrongType)]);
    // the right type, empty: still exempt from the limits
    assert_eq!(reasons(&spec, json!({"title": "", "tags": []})), []);
}

mod choice_options {
    use schema_form_core::{
        ChoiceOption, ChoiceShape, Control, ErrorKind, FormSpec, SPEC_VERSION, Selection, selected_options,
        toggle_option, unknown_rows, write_selected,
    };
    use serde_json::{Value, json};

    fn spec() -> FormSpec {
        let schema = json!({"type": "object", "properties": {
            "one": {"type": "string", "enum": ["a", "b"]},
            "many": {"type": "array", "items": {"type": "string", "enum": ["a", "b", "z"]}},
            "marks": {"type": "array", "minItems": 1, "items": {"type": "object", "properties": {
                "id": {"type": "string"}, "text": {"type": "string"}, "correct": {"type": "boolean"},
            }}},
        }});
        let hints = json!({"version": SPEC_VERSION, "fields": {
            "many": {"options": [{"value": "z", "exclusive": true}]},
            "marks": {"widget": "buttons", "correct": "at-least-one", "options": [
                {"value": "L"}, {"value": "R"}, {"value": "No", "exclusive": true},
            ]},
        }});
        FormSpec::parse(&schema, &hints, None).expect("in the subset")
    }

    fn options(key: &str) -> Vec<ChoiceOption> {
        let spec = spec();
        let f = spec.fields.iter().find(|f| f.key == key).expect("field");
        match &f.control {
            Control::ChoiceButtons { options, .. } => options.clone(),
            c => panic!("{c:?}"),
        }
    }

    fn kinds(v: Value) -> Vec<(String, ErrorKind)> {
        (spec().validate(&v).into_iter()).map(|e| (e.path, e.kind)).collect()
    }

    fn mark(id: &str, text: &str, correct: bool) -> Value {
        json!({"id": id, "text": text, "correct": correct})
    }

    #[test]
    fn a_value_outside_the_options_has_a_reason() {
        assert_eq!(
            kinds(json!({"one": "c", "many": ["a", "c"], "marks": [mark("x", "Odd", true)]})),
            [
                ("many/1".to_string(), ErrorKind::NotAnOption),
                ("marks/0".to_string(), ErrorKind::NotAnOption),
                ("one".to_string(), ErrorKind::NotAnOption),
            ]
        );
        assert_eq!(kinds(json!({"one": ""})), [], "empty is how no choice looks");
    }

    #[test]
    fn an_exclusive_option_with_others_has_a_reason() {
        let exclusive = |value: &str| ErrorKind::ExclusiveWithOthers { value: value.into() };
        assert_eq!(
            kinds(json!({"many": ["a", "z"], "marks": [mark("x", "L", true), mark("y", "No", true)]})),
            [
                ("many".to_string(), exclusive("z")),
                ("marks".to_string(), exclusive("No")),
            ]
        );
        assert_eq!(
            kinds(json!({"many": ["z"], "marks": [mark("x", "L", false), mark("y", "No", true)]})),
            []
        );
    }

    #[test]
    fn a_new_answer_without_an_id_key_is_not_missing_one() {
        let new = json!({"text": "L", "correct": true});
        assert_eq!(kinds(json!({"marks": [new]})), []);
        assert_eq!(
            kinds(json!({"marks": [mark("", "L", true)]})),
            [("marks/0".to_string(), ErrorKind::MissingId)]
        );
    }

    #[test]
    fn the_correct_rule_holds_for_marks() {
        assert_eq!(
            kinds(json!({"marks": [mark("x", "L", false)]})),
            [("marks".to_string(), ErrorKind::AtLeastOneCorrect)]
        );
    }

    #[test]
    fn exclusive_and_the_others_clear_each_other() {
        let o = options("marks");
        let press = |sel: &[bool], i: usize| {
            let mut s = sel.to_vec();
            toggle_option(&o, Selection::Multiple, &mut s, i);
            s
        };
        assert_eq!(press(&[false, false, false], 0), [true, false, false]);
        assert_eq!(press(&[true, false, false], 1), [true, true, false], "others stay");
        assert_eq!(press(&[true, true, false], 2), [false, false, true], "exclusive clears");
        assert_eq!(
            press(&[false, false, true], 0),
            [true, false, false],
            "another clears it"
        );
        assert_eq!(press(&[true, true, false], 0), [false, true, false], "pressed again");
        assert_eq!(press(&[false, false, true], 2), [false, false, false]);
    }

    #[test]
    fn one_selection_stays_on_what_was_pressed() {
        let o = options("one");
        let mut s = vec![true, false];
        toggle_option(&o, Selection::Single, &mut s, 1);
        assert_eq!(s, [false, true]);
        toggle_option(&o, Selection::Single, &mut s, 1);
        assert_eq!(s, [false, true]);
    }

    #[test]
    fn writing_a_string_and_strings() {
        let o = options("one");
        let mut v = json!("a");
        assert_eq!(selected_options(ChoiceShape::String, &o, &v), [true, false]);
        write_selected(ChoiceShape::String, &o, &[false, true], &mut v);
        assert_eq!(v, json!("b"));

        let o = options("many");
        // z, then the rest of the enum in its order
        assert_eq!(o.iter().map(|x| x.value.as_str()).collect::<Vec<_>>(), ["z", "a", "b"]);
        let mut v = json!(["other", "b"]);
        assert_eq!(selected_options(ChoiceShape::Strings, &o, &v), [false, false, true]);
        write_selected(ChoiceShape::Strings, &o, &[false, true, true], &mut v);
        assert_eq!(
            v,
            json!(["other", "a", "b"]),
            "keeps what it cannot show, in the options' order"
        );
        write_selected(ChoiceShape::Strings, &o, &[false, false, false], &mut v);
        assert_eq!(v, json!(["other"]));
    }

    #[test]
    fn writing_marks_touches_only_what_changed() {
        let o = options("marks");
        let mut v = json!([mark("a2", "R", true), mark("a9", "Odd", true), mark("a3", "No", false)]);
        assert_eq!(selected_options(ChoiceShape::Marks, &o, &v), [false, true, false]);
        assert_eq!(unknown_rows(ChoiceShape::Marks, &o, &v), [1]);
        write_selected(ChoiceShape::Marks, &o, &[false, false, true], &mut v);
        assert_eq!(
            v,
            json!([mark("a2", "R", false), mark("a9", "Odd", true), mark("a3", "No", true)])
        );
    }

    #[test]
    fn a_selected_option_without_a_row_gets_one_in_order() {
        let o = options("marks");
        let mut v = json!([mark("a2", "R", true)]);
        write_selected(ChoiceShape::Marks, &o, &[true, true, false], &mut v);
        assert_eq!(v, json!([{"text": "L", "correct": true}, mark("a2", "R", true)]));
        write_selected(ChoiceShape::Marks, &o, &[true, true, true], &mut v);
        assert_eq!(v[2], json!({"text": "No", "correct": true}));
    }

    #[test]
    fn a_second_row_of_an_option_is_unknown_and_a_mark_it_cannot_read_is_kept() {
        let o = options("marks");
        let v = json!([mark("a", "L", false), mark("b", "L", false)]);
        assert_eq!(unknown_rows(ChoiceShape::Marks, &o, &v), [1]);
        let mut v = json!([{"id": "a", "text": "L", "correct": "yes"}]);
        write_selected(ChoiceShape::Marks, &o, &[true, false, false], &mut v);
        assert_eq!(v[0]["correct"], json!("yes"));
    }
}

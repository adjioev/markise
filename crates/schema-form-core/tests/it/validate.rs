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

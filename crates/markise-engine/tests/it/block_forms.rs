//! A package's declared fields, the form they become and the field model agree on what the
//! values look like (ADR 0020): the three are written apart, and only JSON holds them together.

use std::collections::BTreeMap;

use markise_engine::block_fields::{BlockFields, decls_from_spec};
use markise_md::ParseOptions;
use markise_package::PackageIndex;
use schema_form_core::{ErrorKind, FormSpec, JsonType, ValueRule};
use serde_json::{Value, json};

fn index() -> PackageIndex {
    let block = json!({
        "label": "Everything",
        "variants": {"": "plain", "many": "several", "icons": "with icons"},
        "fields": [
            {"name": "title", "type": "heading", "level": 2, "min": 0},
            {"name": "body", "type": "text", "min": 0},
            {"name": "items", "type": "list"},
            {"name": "answers", "type": "choices", "ids": "required", "correct": {
                "": "exactly-one", "many": "at-least-one", "icons": "at-least-one",
            }},
            {"name": "why", "type": "quote"},
            {"name": "pictures", "type": "image", "min": 0},
        ],
    });
    let palette = json!({"format": 1, "presets": [{
        "id": "p", "block": "all", "markdown": ":::all icons\n:::\n",
        "variant": "icons", "widget": "icon-choices", "field": "answers",
        "answers": [{"label": "Red", "icon": "red.svg"}, {"label": "None", "exclusive": true}],
    }]});
    let mut files = BTreeMap::new();
    files.insert(
        "manifest.json".to_string(),
        br#"{"format":"markise-block-package/2","library":"sample","version":"1","blocks":["all"]}"#.to_vec(),
    );
    files.insert("components/all/all.json".to_string(), block.to_string().into_bytes());
    files.insert("editor/palette.json".to_string(), palette.to_string().into_bytes());
    PackageIndex::from_files(&files).expect("a valid package")
}

fn spec(variant: Option<&str>) -> FormSpec {
    let i = index().form_input("all", "en").expect("a schema").expect("converted");
    FormSpec::parse(&i.schema, &i.hints, variant).expect("in the subset")
}

/// The value has the type of its rule at every depth, and an object has every declared property.
fn assert_shape(rule: &ValueRule, v: &Value, at: &str) {
    match rule.json_type {
        JsonType::String => assert!(v.is_string(), "{at}: {v}"),
        JsonType::Boolean => assert!(v.is_boolean(), "{at}: {v}"),
        JsonType::Any => {}
        JsonType::Array => {
            let a = v.as_array().unwrap_or_else(|| panic!("{at}: {v}"));
            let item = rule.items.as_deref().unwrap_or_else(|| panic!("{at}: no items"));
            for (i, e) in a.iter().enumerate() {
                assert_shape(item, e, &format!("{at}/{i}"));
            }
        }
        JsonType::Object => {
            let o = v.as_object().unwrap_or_else(|| panic!("{at}: {v}"));
            for p in &rule.props {
                let e = o.get(&p.key).unwrap_or_else(|| panic!("{at}: no {}", p.key));
                assert_shape(&p.rule, e, &format!("{at}/{}", p.key));
            }
            for k in o.keys() {
                assert!(rule.props.iter().any(|p| p.key == *k), "{at}: {k} is not in the spec");
            }
        }
    }
}

const FULL: &str = ":::all\n\n## A title {#t}\n\nSome text.\n\n* One\n* Two\n\n- [ ] Red {#a1}\n- [x] None {#a2}\n\n> Because.\n\n![An alt](a/b.png)\n\n:::";
const EMPTY: &str = ":::all\n:::";

#[test]
fn engine_values_have_the_shape_the_spec_declares() {
    for variant in [None, Some("many"), Some("icons")] {
        let spec = spec(variant);
        let decls = decls_from_spec(&spec).expect("every field has a place");
        for source in [FULL, EMPTY] {
            let model = BlockFields::decode(source, 0, &decls, &ParseOptions::default()).expect("a block");
            let values = model.values();
            assert_shape(&spec.rule, &values, &format!("{variant:?} {source:?}"));
            let wrong: Vec<_> = (spec.validate(&values).into_iter())
                .filter(|e| matches!(e.kind, ErrorKind::WrongType | ErrorKind::UnknownKey))
                .collect();
            assert!(wrong.is_empty(), "{wrong:?}");
            let edits = model.edits_from_values(&values).expect("its own values");
            assert!(edits.is_empty(), "{edits:?}");
        }
    }
}

#[test]
fn the_full_block_reads_field_by_field() {
    let spec = spec(None);
    let decls = decls_from_spec(&spec).expect("every field has a place");
    let model = BlockFields::decode(FULL, 0, &decls, &ParseOptions::default()).expect("a block");
    assert_eq!(
        model.values(),
        json!({
            "title": "A title",
            "body": "Some text.",
            "items": ["One", "Two"],
            "answers": [
                {"id": "a1", "text": "Red", "correct": false},
                {"id": "a2", "text": "None", "correct": true},
            ],
            "why": "Because.",
            "pictures": [{"alt": "An alt", "path": "a/b.png"}],
        })
    );
    assert!(model.check(&spec).fits(), "{:?}", model.check(&spec));
    assert!(model.check(&spec).extras.is_empty());
}

#[test]
fn a_new_row_of_a_form_stores_with_a_fresh_id() {
    let spec = spec(Some("icons"));
    let decls = decls_from_spec(&spec).expect("every field has a place");
    let model = BlockFields::decode(FULL, 0, &decls, &ParseOptions::default()).expect("a block");
    let mut values = model.values();
    values["answers"]
        .as_array_mut()
        .expect("an array")
        .push(json!({"text": "Blue", "correct": true}));
    assert_eq!(
        spec.validate(&values)
            .iter()
            .filter(|e| e.kind == ErrorKind::MissingId)
            .count(),
        0
    );
    let applied = model
        .apply(&model.edits_from_values(&values).expect("edits"))
        .expect("applied");
    let after = applied.fields.values();
    assert_eq!(
        after["answers"][2],
        json!({"id": "a3", "text": "Blue", "correct": true})
    );
}

fn with_argument(argument: Value, fields: Value) -> (PackageIndex, FormSpec) {
    let block = json!({"label": "Plate", "argument": argument, "fields": fields});
    let mut files = BTreeMap::new();
    files.insert(
        "manifest.json".to_string(),
        br#"{"format":"markise-block-package/2","library":"sample","version":"1","blocks":["plate"]}"#.to_vec(),
    );
    files.insert(
        "components/plate/plate.json".to_string(),
        block.to_string().into_bytes(),
    );
    files.insert("editor/palette.json".to_string(), br#"{"format": 1}"#.to_vec());
    let index = PackageIndex::from_files(&files).expect("a valid package");
    let i = index.form_input("plate", "en").expect("a schema").expect("converted");
    let spec = FormSpec::parse(&i.schema, &i.hints, None).expect("in the subset");
    (index, spec)
}

#[test]
fn an_argument_has_the_shape_the_spec_declares_and_no_markup_of_its_own() {
    let fields = json!([{"name": "title", "type": "heading", "min": 0}, {"name": "body", "type": "text", "min": 0}]);
    let (_, spec) = with_argument(json!({"name": "set"}), fields);
    let decls = decls_from_spec(&spec).expect("every field has a place");
    for source in [":::plate a/b\n\n## T\n\nText\n\n:::", ":::plate\n:::"] {
        let model = BlockFields::decode(source, 0, &decls, &ParseOptions::default()).expect("a block");
        assert_shape(&spec.rule, &model.values(), source);
        assert!(model.edits_from_values(&model.values()).expect("its own").is_empty());
    }
    let model = BlockFields::decode(":::plate a/b\n\nText\n\n:::", 0, &decls, &ParseOptions::default()).unwrap();
    assert_eq!(model.values()[markise_package::ARGUMENT_KEY], "a/b");
    let html = markise_engine::block_page::markup(
        &model,
        &spec,
        false,
        true,
        markise_engine::block_page::BlockRoot::Section,
    );
    assert_eq!(html.matches("a/b").count(), 1, "only in its attribute: {html}");
    assert!(html.contains(r#"data-argument="a/b""#), "{html}");
    assert!(!html.contains(markise_package::ARGUMENT_KEY), "{html}");
}

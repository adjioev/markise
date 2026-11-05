//! A package's views as the view gets them (ADR 0021), on a small neutral package.

use std::collections::BTreeMap;
use std::sync::Arc;

use markise_engine::block_view::PackageViews;
use markise_layout::{BlockLook, BlockViews, NativeKind, NativeRole};
use markise_package::PackageIndex;
use serde_json::json;

pub fn index() -> Arc<PackageIndex> {
    let tip = json!({"label": {"en": "Tip", "de": "Tipp"}, "fields": [
        {"name": "title", "type": "heading", "level": 3, "min": 0},
        {"name": "body", "type": "text", "min": 1},
    ]});
    let poll = json!({"label": "Poll", "variants": {"": "one", "many": "several"}, "fields": [
        {"name": "question", "type": "heading", "level": 2},
        {"name": "answers", "type": "choices", "ids": "required", "minItems": 2,
         "correct": {"": "exactly-one", "many": "at-least-one"}},
        {"name": "why", "type": "quote", "min": 0},
    ]});
    let plain = json!({"label": "Plain", "fields": [{"name": "body", "type": "text"}]});
    let steps = json!({"label": "Steps", "fields": [{"name": "items", "type": "list", "minItems": 1}]});
    let views = json!({
        "tip": {"kind": "callout", "accent": "green", "icon": "bulb"},
        "poll": {"kind": "choices", "accent": "purple"},
        "steps": {"kind": "card", "accent": "blue"},
    });
    let mut files = BTreeMap::new();
    let manifest = r#"{"format":"markise-block-package/2","library":"sample","version":"1","blocks":["tip","poll","plain","steps"]}"#;
    files.insert("manifest.json".to_string(), manifest.as_bytes().to_vec());
    for (name, schema) in [("tip", &tip), ("poll", &poll), ("plain", &plain), ("steps", &steps)] {
        files.insert(
            format!("components/{name}/{name}.json"),
            schema.to_string().into_bytes(),
        );
    }
    files.insert("editor/palette.json".to_string(), br#"{"format": 1}"#.to_vec());
    files.insert("editor/views.json".to_string(), views.to_string().into_bytes());
    Arc::new(PackageIndex::from_files(&files).expect("a valid package"))
}

fn views() -> PackageViews {
    PackageViews::new(index(), "en")
}

const POLL: &str = ":::poll\n\n## Which?\n\n- [ ] One {#a1}\n- [x] Two {#a2}\n\n> Because.\n\n:::";

fn native(look: BlockLook) -> markise_layout::NativeBlock {
    match look {
        BlockLook::Native(b) => b,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_fitting_block_has_its_view_and_its_parts() {
    let b = native(views().look("poll", "", POLL));
    assert_eq!(b.view.kind, NativeKind::Choices);
    assert_eq!(b.view.accent.as_deref(), Some("purple"));
    assert_eq!(&*b.view.label, "Poll");
    assert_eq!(b.view.icon, None);
    assert!(b.view.single_answer, "exactly one");
    let role = |needle: &str| b.role_at(POLL.find(needle).unwrap() as u32);
    assert_eq!(role("Which"), Some(NativeRole::Title));
    assert_eq!(role("One"), Some(NativeRole::Answer));
    assert_eq!(role("Two"), Some(NativeRole::Answer));
    assert_eq!(role("Because"), Some(NativeRole::Quote));
    assert_eq!(role(":::poll"), None);
}

#[test]
fn a_variant_says_how_answers_are_marked_and_how_it_is_called() {
    let text = POLL.replace(":::poll", ":::poll many");
    let b = native(views().look("poll", "many", &text));
    assert!(!b.view.single_answer);
    assert_eq!(&*b.view.label, "Poll · several");
}

fn outside(look: BlockLook) -> Vec<String> {
    match look {
        BlockLook::Outside { reasons, .. } => reasons,
        other => panic!("{other:?}"),
    }
}

#[test]
fn what_does_not_fit_says_why() {
    let v = views();
    let two_marks = POLL.replace("[ ] One", "[x] One");
    assert_eq!(outside(v.look("poll", "", &two_marks)), ["Answers: Mark exactly one"]);
    let no_id = POLL.replace(" {#a1}", "");
    assert_eq!(outside(v.look("poll", "", &no_id)), ["Answers 1: Has no id"]);
    let unnamed = outside(v.look("poll", "nope", POLL));
    assert_eq!(unnamed, ["The block has no variant `nope`."]);
    let extra = POLL.replace("> Because.", "> Because.\n\n```\ncode\n```");
    let why = outside(v.look("poll", "", &extra));
    assert_eq!(why.len(), 1);
    assert!(why[0].starts_with("In no field:"), "{why:?}");
}

#[test]
fn what_is_only_not_filled_in_stays_native_with_its_reason() {
    let v = views();
    let unfilled = |look: BlockLook| match look {
        BlockLook::Unfilled { reasons, .. } => reasons,
        other => panic!("{other:?}"),
    };
    assert_eq!(unfilled(v.look("tip", "", ":::tip\n\n:::")), ["Body: Required"]);
    let no_mark = POLL.replace("[x]", "[ ]");
    assert_eq!(unfilled(v.look("poll", "", &no_mark)), ["Answers: Mark exactly one"]);
    // both kinds at once are about the structure
    let both = no_mark.replace(" {#a1}", "");
    assert_eq!(outside(v.look("poll", "", &both)).len(), 2);
}

#[test]
fn no_view_means_generic_and_no_name_means_no_view() {
    let v = views();
    assert!(v.has_view("tip") && v.has_view("poll"));
    assert!(!v.has_view("plain") && !v.has_view("unknown"));
    assert!(v.has_schema("plain") && !v.has_schema("unknown"));
    assert_eq!(v.look("plain", "", ":::plain\n\nBody.\n\n:::"), BlockLook::Generic);
    assert_eq!(v.look("unknown", "", ":::unknown\n\nBody.\n\n:::"), BlockLook::Generic);
    // a schema is enough to be outside it, with or without a view
    assert_eq!(outside(v.look("plain", "", ":::plain\n\n:::")), ["Body: Required"]);
}

#[test]
fn another_package_is_another_id() {
    assert_ne!(
        PackageViews::new(index(), "en").id(),
        PackageViews::new(index(), "en").id()
    );
}

#[test]
fn the_label_follows_the_language() {
    let b = native(PackageViews::new(index(), "de").look("tip", "", ":::tip\n\nBody.\n\n:::"));
    assert_eq!(&*b.view.label, "Tipp");
    assert_eq!(b.view.icon.as_deref(), Some("assets/bulb.svg"), "a file of the package");
}

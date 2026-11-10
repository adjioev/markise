//! The field model of `:::` blocks (ADR 0020): source to values, edits to minimal patches.

use std::sync::Arc;

use markise_doc::DocumentFormat;
use markise_engine::block_fields::{
    BlockFields, FieldDecl, FieldEdit, FieldKind, FieldValue, FieldsError, ItemData, ItemId, block_range_at,
    decls_from_spec,
};
use markise_engine::editor::{EditorSession, HistoryLimits, ManualClock};
use markise_md::{MarkdownFormat, ParseOptions};
use proptest::prelude::*;
use schema_form_core::{ErrorKind, FormSpec};
use serde_json::{Value, json};

fn poll_decls() -> Vec<FieldDecl> {
    vec![
        FieldDecl::new("prompt", FieldKind::Heading { level: Some(2) }),
        FieldDecl::new("setup", FieldKind::Text),
        FieldDecl::new("answers", FieldKind::Choices),
        FieldDecl::new("why", FieldKind::Quote),
    ]
}

fn figure_decls() -> Vec<FieldDecl> {
    vec![
        FieldDecl::new("title", FieldKind::Heading { level: None }),
        FieldDecl::new("caption", FieldKind::Text),
        FieldDecl::new("pictures", FieldKind::Image),
    ]
}

fn list_decls() -> Vec<FieldDecl> {
    vec![FieldDecl::new("items", FieldKind::List)]
}

const POLL: &str = ":::poll several\n\n## Which colour? {#q}\n\nA line of context.\n\n- [ ] Red {#a1}\n- [x] Green {#a2}\n- [ ] Blue {#a3}\n\n> Because green\n> means go.\n\n:::";

fn dec(src: &str, decls: &[FieldDecl]) -> BlockFields {
    BlockFields::decode(src, 0, decls, &ParseOptions::default()).expect("a block")
}

/// Applies `edits`; the patches must produce the source of the returned model.
fn run(b: &BlockFields, edits: &[FieldEdit]) -> BlockFields {
    let out = b.apply(edits).expect("edits apply");
    assert_eq!(
        out.patches.apply_to(b.source()),
        out.fields.source(),
        "patches give the new source"
    );
    out.fields
}

fn ids(b: &BlockFields, field: &str) -> Vec<ItemId> {
    match &b.field(field).expect("field").value {
        FieldValue::Items(v) => v.iter().map(|i| i.id).collect(),
        FieldValue::Text(_) => panic!("not items"),
    }
}

fn set_values(b: &BlockFields, v: Value) -> BlockFields {
    let edits = b.edits_from_values(&v).expect("values fit");
    let next = run(b, &edits);
    for (k, want) in v.as_object().expect("object") {
        let got = &next.values()[k];
        if want.is_array() {
            // new answers get an id
            let got: Vec<Value> = got.as_array().expect("array").clone();
            assert_eq!(got.len(), want.as_array().expect("array").len(), "{k}");
        } else {
            assert_eq!(got, want, "{k}");
        }
    }
    next
}

#[test]
fn reads_fields_without_their_syntax() {
    let b = dec(POLL, &poll_decls());
    assert_eq!(b.name(), "poll");
    assert_eq!(b.args(), "several");
    assert_eq!(
        b.values(),
        json!({
            "prompt": "Which colour?",
            "setup": "A line of context.",
            "answers": [
                {"id": "a1", "text": "Red", "correct": false},
                {"id": "a2", "text": "Green", "correct": true},
                {"id": "a3", "text": "Blue", "correct": false},
            ],
            "why": "Because green\nmeans go.",
        })
    );
    assert!(b.extras().is_empty());
}

#[test]
fn no_change_gives_no_patches_on_any_spelling() {
    let cases: &[(&str, Vec<FieldDecl>)] = &[
        (POLL, poll_decls()),
        (
            ":::poll\r\n\r\n## Q ##\r\n\r\n* [X] A {#a}\r\n* [ ] B   {#b}\r\n\r\n>no space\r\nlazy\r\n\r\n:::",
            poll_decls(),
        ),
        (":::poll\n## Tight\ntext\n- [ ] A {#a}\n> q\n:::", poll_decls()),
        (":::poll\nSetext\n---\n\n:::", poll_decls()),
        (":::poll\n\n- [ ] {#only}\n\n:::", poll_decls()),
        ("::::poll\n\n:::inner\ntext\n:::\n\n- [ ] A {#a}\n\n::::", poll_decls()),
        (
            ":::l\n\n1. one\n2. two\n\n   more of two\n3. three\n   - nested\n\n:::",
            list_decls(),
        ),
        (
            ":::l\n\n- loose\n\n- items  \n\n- [ ] with a box\n-\n\n:::",
            list_decls(),
        ),
        (
            ":::f\n\n# T\n\nCap.\n\n![a b](<p q.png> \"title\")\n![[x]](r.png){.c}\n\n![](s.png)\n\n:::",
            figure_decls(),
        ),
        (
            ":::f\n\n\\# not a heading\n\n1\\. not a list\n\\- nor this\n\n:::",
            figure_decls(),
        ),
    ];
    for (src, decls) in cases {
        let b = dec(src, decls);
        let out = b.apply(&[]).expect("applies");
        assert!(out.patches.is_empty(), "{src:?}: {:?}", out.patches);
        let same = b.edits_from_values(&b.values()).expect("fits");
        assert!(same.is_empty(), "{src:?}: {same:?}");
    }
}

#[test]
fn a_text_edit_patches_only_the_changed_characters() {
    let b = dec(POLL, &poll_decls());
    let id = ids(&b, "answers")[1];
    let out = b
        .apply(&[FieldEdit::SetItemText {
            field: "answers".into(),
            item: id,
            text: "Greener".into(),
        }])
        .expect("applies");
    let [p] = out.patches.patches() else {
        panic!("{:?}", out.patches)
    };
    assert_eq!(p.insert, "er");
    assert!(p.range.is_empty());
    assert!(out.fields.source().contains("- [x] Greener {#a2}\n"));
}

#[test]
fn correctness_changes_only_the_box() {
    let b = dec(POLL, &poll_decls());
    let id = ids(&b, "answers")[0];
    let out = b
        .apply(&[FieldEdit::SetCorrect {
            field: "answers".into(),
            item: id,
            correct: true,
        }])
        .expect("applies");
    let [p] = out.patches.patches() else {
        panic!("{:?}", out.patches)
    };
    assert_eq!((p.range.len(), p.insert.as_str()), (1, "x"));
}

#[test]
fn reorder_moves_one_line_and_keeps_ids_and_identities() {
    let b = dec(POLL, &poll_decls());
    let before = ids(&b, "answers");
    let edits = [FieldEdit::MoveItem {
        field: "answers".into(),
        item: before[2],
        to: 0,
    }];
    let out = b.apply(&edits).expect("applies");
    assert_eq!(out.patches.apply_to(b.source()), out.fields.source());
    assert_eq!(out.patches.len(), 2, "{:?}", out.patches);
    assert!(
        out.fields
            .source()
            .contains("- [ ] Blue {#a3}\n- [ ] Red {#a1}\n- [x] Green {#a2}\n")
    );
    assert_eq!(ids(&out.fields, "answers"), vec![before[2], before[0], before[1]]);
}

#[test]
fn a_new_answer_gets_a_new_id_and_a_removed_id_does_not_come_back() {
    let b = dec(POLL, &poll_decls());
    let a3 = ids(&b, "answers")[2];
    let insert = FieldEdit::InsertItem {
        field: "answers".into(),
        at: 2,
        item: ItemData::Choice {
            anchor: None,
            text: "Mauve".into(),
            correct: false,
        },
    };
    let out = b
        .apply(&[
            FieldEdit::RemoveItem {
                field: "answers".into(),
                item: a3,
            },
            insert.clone(),
        ])
        .expect("applies");
    assert!(
        out.fields.source().contains("- [ ] Mauve {#a4}\n"),
        "{}",
        out.fields.source()
    );
    assert!(!out.fields.source().contains("{#a3}"));
    assert_eq!(out.inserted.len(), 1);
    // the successor still remembers
    let next = run(
        &out.fields,
        &[FieldEdit::RemoveItem {
            field: "answers".into(),
            item: out.inserted[0],
        }],
    );
    let next = run(&next, &[insert]);
    assert!(next.source().contains("{#a5}"), "{}", next.source());
}

#[test]
fn values_round_trip_through_json_with_identity() {
    let b = dec(POLL, &poll_decls());
    let before = ids(&b, "answers");
    let next = set_values(
        &b,
        json!({"answers": [
            {"id": "a2", "text": "Green!", "correct": true},
            {"id": "", "text": "New", "correct": false},
            {"id": "a1", "text": "Red", "correct": false},
        ]}),
    );
    assert!(
        next.source()
            .contains("- [x] Green! {#a2}\n- [ ] New {#a4}\n- [ ] Red {#a1}\n"),
        "{}",
        next.source()
    );
    let after = ids(&next, "answers");
    assert_eq!((after[0], after[2]), (before[1], before[0]));
    assert!(!before.contains(&after[1]));
}

#[test]
fn markup_typed_into_a_field_stays_text() {
    let frags = [
        "# not a heading",
        "- not a list",
        "+ nor",
        "* this",
        "> nor a quote",
        "1. nor numbered",
        "2) nor",
        "---",
        "***",
        "===",
        "--",
        "```",
        "~~~ x",
        ":::x",
        ":::",
        "::::",
        "    code",
        "\ttab",
        "[x]: y",
        "[^1]: z",
        "| a |",
        "|---|",
        "<div>",
        "$$",
        "[toc]",
        "\\# already",
        "1\\. already",
        "\\\\# twice",
    ];
    for f in frags {
        let want = f.trim_start();
        let b = dec(":::f\n\n# T\n\nCap.\n\n:::", &figure_decls());
        let next = run(
            &b,
            &[FieldEdit::SetText {
                field: "caption".into(),
                text: format!("first\n{f}\nlast"),
            }],
        );
        assert_eq!(
            next.values()["caption"],
            json!(format!("first\n{want}\nlast")),
            "text: {}",
            next.source()
        );
        assert!(next.extras().is_empty(), "{f}: {}", next.source());

        let next = run(
            &b,
            &[FieldEdit::SetText {
                field: "caption".into(),
                text: f.to_string(),
            }],
        );
        assert_eq!(next.values()["caption"], json!(want), "alone: {}", next.source());

        let b = dec(":::l\n\n- a\n- b\n\n:::", &list_decls());
        let id = ids(&b, "items")[1];
        let next = run(
            &b,
            &[FieldEdit::SetItemText {
                field: "items".into(),
                item: id,
                text: f.to_string(),
            }],
        );
        assert_eq!(next.values()["items"], json!(["a", want]), "item: {}", next.source());

        let b = dec(POLL, &poll_decls());
        let next = run(
            &b,
            &[FieldEdit::SetText {
                field: "why".into(),
                text: format!("{f}\nb"),
            }],
        );
        assert_eq!(
            next.values()["why"],
            json!(format!("{want}\nb")),
            "quote: {}",
            next.source()
        );

        let id = ids(&b, "answers")[1];
        let next = run(
            &b,
            &[FieldEdit::SetItemText {
                field: "answers".into(),
                item: id,
                text: f.to_string(),
            }],
        );
        assert_eq!(
            next.values()["answers"][1]["text"],
            json!(want),
            "answer: {}",
            next.source()
        );
        assert_eq!(next.values()["answers"][1]["id"], json!("a2"));
    }
}

#[test]
fn a_typed_id_or_closing_hashes_stay_text() {
    let b = dec(POLL, &poll_decls());
    let next = run(
        &b,
        &[FieldEdit::SetText {
            field: "prompt".into(),
            text: "Q {#fake}".into(),
        }],
    );
    assert_eq!(next.values()["prompt"], json!("Q {#fake}"));
    assert!(next.source().contains("## Q \\{#fake} {#q}"), "{}", next.source());
    let next = run(
        &b,
        &[FieldEdit::SetText {
            field: "prompt".into(),
            text: "Q ##".into(),
        }],
    );
    assert_eq!(next.values()["prompt"], json!("Q ##"));

    let id = ids(&b, "answers")[0];
    let next = run(
        &b,
        &[FieldEdit::SetItemText {
            field: "answers".into(),
            item: id,
            text: "A {#fake}".into(),
        }],
    );
    assert_eq!(
        next.values()["answers"][0],
        json!({"id": "a1", "text": "A {#fake}", "correct": false})
    );
}

#[test]
fn an_absent_field_is_written_at_its_place_and_an_emptied_one_goes_away() {
    let src = ":::poll\n\n## Q\n\n- [ ] A {#a}\n\n:::";
    let b = dec(src, &poll_decls());
    let next = run(
        &b,
        &[FieldEdit::SetText {
            field: "setup".into(),
            text: "Context.".into(),
        }],
    );
    assert_eq!(next.source(), ":::poll\n\n## Q\n\nContext.\n\n- [ ] A {#a}\n\n:::");
    let next = run(
        &next,
        &[FieldEdit::SetText {
            field: "why".into(),
            text: "Why.\n\nMore.".into(),
        }],
    );
    assert_eq!(
        next.source(),
        ":::poll\n\n## Q\n\nContext.\n\n- [ ] A {#a}\n\n> Why.\n>\n> More.\n\n:::"
    );
    assert_eq!(next.values()["why"], json!("Why.\n\nMore."));
    let next = run(
        &next,
        &[
            FieldEdit::SetText {
                field: "setup".into(),
                text: String::new(),
            },
            FieldEdit::SetText {
                field: "why".into(),
                text: "  \n".into(),
            },
        ],
    );
    assert_eq!(next.source(), src);

    // a tight block stays readable; the new field does not join the next one
    let b = dec(":::poll\nsome text\n:::", &poll_decls());
    let next = run(
        &b,
        &[FieldEdit::SetText {
            field: "prompt".into(),
            text: "Q".into(),
        }],
    );
    assert_eq!(next.source(), ":::poll\n## Q\n\nsome text\n:::");
    let next = run(
        &next,
        &[FieldEdit::SetText {
            field: "setup".into(),
            text: String::new(),
        }],
    );
    assert_eq!(next.values()["setup"], json!(""));

    // from nothing to everything
    let b = dec(":::poll\n:::", &poll_decls());
    let next = set_values(
        &b,
        json!({"prompt": "Q", "setup": "S", "why": "W", "answers": [{"id": "", "text": "A", "correct": true}]}),
    );
    assert_eq!(next.source(), ":::poll\n## Q\n\nS\n\n- [x] A {#a1}\n\n> W\n:::");
}

#[test]
fn removing_every_item_removes_the_list_and_adding_brings_it_back() {
    let b = dec(":::l\n\n- a\n\n:::", &list_decls());
    let next = set_values(&b, json!({"items": []}));
    assert_eq!(next.source(), ":::l\n\n:::");
    let next = set_values(&next, json!({"items": ["x", "y"]}));
    assert_eq!(next.source(), ":::l\n\n- x\n- y\n\n:::");
}

#[test]
fn ordered_lists_renumber_and_keep_their_style() {
    let b = dec(":::l\n\n3. a\n4. b\n5. c\n\n:::", &list_decls());
    let i = ids(&b, "items");
    let next = run(
        &b,
        &[FieldEdit::MoveItem {
            field: "items".into(),
            item: i[2],
            to: 0,
        }],
    );
    assert_eq!(next.source(), ":::l\n\n3. c\n4. a\n5. b\n\n:::");
    let b = dec(":::l\n\n1. a\n1. b\n\n:::", &list_decls());
    let next = run(
        &b,
        &[FieldEdit::InsertItem {
            field: "items".into(),
            at: 2,
            item: ItemData::Text("c".into()),
        }],
    );
    assert_eq!(next.source(), ":::l\n\n1. a\n1. b\n1. c\n\n:::");
}

#[test]
fn loose_lists_stay_loose_and_multi_line_items_keep_their_lines() {
    let src = ":::l\n\n- a\n  more\n\n- b\n\n- c\n\n:::";
    let b = dec(src, &list_decls());
    assert_eq!(b.values()["items"], json!(["a\nmore", "b", "c"]));
    let i = ids(&b, "items");
    let next = run(
        &b,
        &[FieldEdit::MoveItem {
            field: "items".into(),
            item: i[0],
            to: 2,
        }],
    );
    assert_eq!(next.source(), ":::l\n\n- b\n\n- c\n\n- a\n  more\n\n:::");
    let next = run(
        &b,
        &[FieldEdit::SetItemText {
            field: "items".into(),
            item: i[0],
            text: "a\nmore\nand more".into(),
        }],
    );
    assert_eq!(next.source(), ":::l\n\n- a\n  more\n  and more\n\n- b\n\n- c\n\n:::");
}

#[test]
fn line_endings_of_untouched_lines_survive_in_a_mixed_file() {
    let src = ":::poll\r\n\r\n## Q\n\r\n- [ ] A {#a}\r\n- [ ] B {#b}\n\n:::";
    let b = dec(src, &poll_decls());
    let id = ids(&b, "answers")[0];
    let next = run(
        &b,
        &[FieldEdit::SetItemText {
            field: "answers".into(),
            item: id,
            text: "AA".into(),
        }],
    );
    assert_eq!(next.source(), src.replace("] A {", "] AA {"));
}

#[test]
fn images_edit_their_alt_and_path_only() {
    let src = ":::f\n\n![old alt](old.png \"t\"){.c}\n\n:::";
    let b = dec(src, &figure_decls());
    assert_eq!(b.values()["pictures"], json!([{"alt": "old alt", "path": "old.png"}]));
    let next = set_values(
        &b,
        json!({"pictures": [{"alt": "new [alt]", "path": "my pic (1).png"}]}),
    );
    assert_eq!(
        next.source(),
        ":::f\n\n![new \\[alt\\]](<my pic (1).png> \"t\"){.c}\n\n:::"
    );
    assert_eq!(
        next.values()["pictures"],
        json!([{"alt": "new [alt]", "path": "my pic (1).png"}])
    );
    let next = set_values(
        &next,
        json!({"pictures": [
            {"alt": "new [alt]", "path": "my pic (1).png"},
            {"alt": "b", "path": "b.png"},
        ]}),
    );
    assert!(
        next.source().ends_with(" \"t\"){.c}\n\n![b](b.png)\n\n:::"),
        "{}",
        next.source()
    );
    let next = set_values(&next, json!({"pictures": []}));
    assert_eq!(next.source(), ":::f\n\n:::");
}

#[test]
fn what_no_field_takes_stays_untouched_and_is_reported() {
    let src = ":::poll\n\n## Q\n\n## second heading\n\n```\ncode\n```\n\n- [ ] A {#a}\n\n:::";
    let decls = vec![
        FieldDecl::new("prompt", FieldKind::Heading { level: Some(2) }),
        FieldDecl::new("answers", FieldKind::Choices),
    ];
    let b = dec(src, &decls);
    let extras: Vec<&str> = b.extras().into_iter().map(|r| &src[r]).collect();
    assert_eq!(extras, ["## second heading", "```\ncode\n```"]);
    let next = run(
        &b,
        &[FieldEdit::SetText {
            field: "prompt".into(),
            text: "New".into(),
        }],
    );
    assert_eq!(next.source(), src.replace("## Q", "## New"));
}

#[test]
fn text_takes_what_has_no_field_and_keeps_it_while_other_lines_change() {
    let src = ":::a\n\nFirst.\n\n- a list\n\n| t |\n|---|\n\nLast.\n\n:::";
    let b = dec(src, &[FieldDecl::new("text", FieldKind::Text)]);
    assert!(b.extras().is_empty());
    assert_eq!(b.values()["text"], json!("First.\n\n- a list\n\n| t |\n|---|\n\nLast."));
    let next = run(
        &b,
        &[FieldEdit::SetText {
            field: "text".into(),
            text: "First!\n\n- a list\n\n| t |\n|---|\n\nLast.".into(),
        }],
    );
    assert_eq!(next.source(), src.replace("First.", "First!"));
}

#[test]
fn refreshing_keeps_identities_across_an_outside_change() {
    let b = dec(POLL, &poll_decls());
    let before = ids(&b, "answers");
    let changed = POLL.replace(
        "- [ ] Red {#a1}\n- [x] Green {#a2}\n",
        "- [x] Green {#a2}\n- [ ] Scarlet {#a1}\n",
    );
    let next = b.refreshed(&changed, 5).expect("a block");
    assert_eq!(next.base(), 5);
    assert_eq!(ids(&next, "answers"), vec![before[1], before[0], before[2]]);
}

#[test]
fn not_a_block_is_an_error() {
    for src in [":::a\nno closer", "text", ":::a\n:::\n:::b\n:::", "text\n:::a\n:::"] {
        assert!(
            BlockFields::decode(src, 0, &[], &ParseOptions::default()).is_err(),
            "{src}"
        );
    }
}

fn session(text: &str) -> EditorSession {
    let format: Arc<dyn DocumentFormat> = Arc::new(MarkdownFormat::default());
    EditorSession::with(format, text, HistoryLimits::default(), Box::new(ManualClock::default()))
}

#[test]
fn a_set_of_edits_is_one_undo_step_in_the_session() {
    let doc = format!("Before\n\n{POLL}\n\nAfter\n");
    let mut s = session(&doc);
    let at = doc.find("Red").expect("in the text");
    let range = block_range_at(s.doc(), at).expect("in a block");
    assert_eq!(&doc[range.clone()], POLL);
    let b = BlockFields::decode(
        &doc[range.clone()],
        range.start,
        &poll_decls(),
        &ParseOptions::default(),
    )
    .expect("a block");
    let i = ids(&b, "answers");
    let edits = [
        FieldEdit::SetText {
            field: "prompt".into(),
            text: "Which?".into(),
        },
        FieldEdit::MoveItem {
            field: "answers".into(),
            item: i[2],
            to: 0,
        },
        FieldEdit::SetCorrect {
            field: "answers".into(),
            item: i[0],
            correct: true,
        },
        FieldEdit::SetText {
            field: "why".into(),
            text: "Just.".into(),
        },
    ];
    let out = b.apply_to_session(&mut s, &edits, "Edit Block").expect("applies");
    assert_eq!(s.text(), format!("Before\n\n{}\n\nAfter\n", out.fields.source()));
    assert!(s.undo());
    assert_eq!(s.text(), doc);
    assert!(!s.undo(), "one step");
    assert!(s.redo());
    assert_eq!(s.text(), format!("Before\n\n{}\n\nAfter\n", out.fields.source()));
}

#[test]
fn a_stale_model_does_not_touch_the_document() {
    let mut s = session(POLL);
    let b = dec(POLL, &poll_decls());
    let edit = [FieldEdit::SetText {
        field: "prompt".into(),
        text: "X".into(),
    }];
    let mut changed = session(&POLL.replace("Red", "Rouge"));
    assert!(b.apply_to_session(&mut changed, &edit, "Edit Block").is_err());
    assert!(changed.text().contains("Rouge"));
    assert!(b.apply_to_session(&mut s, &edit, "Edit Block").is_ok());
}

// ---------------------------------------------------------------- Markdown nested under an item

const NESTED: &str = ":::poll\n\n## Which?\n\n- [ ] One {#a1}\n  > Said of one.\n- [x] Two {#a2}\n\n  More on two.\n\n  ![two](two.png)\n- [ ] Three {#a3}\n\n:::";

#[test]
fn markdown_nested_under_an_answer_is_the_answers_and_no_value() {
    let b = dec(NESTED, &poll_decls());
    assert_eq!(
        b.values()["answers"],
        json!([
            {"id": "a1", "text": "One", "correct": false},
            {"id": "a2", "text": "Two", "correct": true},
            {"id": "a3", "text": "Three", "correct": false},
        ])
    );
    assert!(b.extras().is_empty());
    let v = ids(&b, "answers");
    assert_eq!(b.nested_under(v[0]), Some("  > Said of one."));
    assert_eq!(b.nested_under(v[1]), Some("  More on two.\n\n  ![two](two.png)"));
    assert_eq!(b.nested_under(v[2]), None);
    assert!(
        b.check(&poll_spec()).errors.is_empty(),
        "{:?}",
        b.check(&poll_spec()).errors
    );
    assert!(b.apply(&[]).expect("applies").patches.is_empty());
    assert!(b.edits_from_values(&b.values()).expect("fits").is_empty());
    // in place, an answer's slot is its own text
    let slot = b.slot_at(NESTED.find("One").unwrap()).expect("a slot");
    assert_eq!(slot.value(), "One");
    assert!(b.slot_at(NESTED.find("Said").unwrap()).is_none());
}

#[test]
fn nested_markdown_moves_and_goes_with_its_item() {
    let b = dec(NESTED, &poll_decls());
    let v = ids(&b, "answers");
    let edit = |e: FieldEdit| run(&b, &[e]);
    let text = edit(FieldEdit::SetItemText {
        field: "answers".into(),
        item: v[0],
        text: "Uno".into(),
    });
    assert_eq!(text.source(), NESTED.replace("One {#a1}", "Uno {#a1}"));
    let moved = edit(FieldEdit::MoveItem {
        field: "answers".into(),
        item: v[0],
        to: 2,
    });
    assert!(
        moved
            .source()
            .contains("- [ ] Three {#a3}\n- [ ] One {#a1}\n  > Said of one.\n\n:::"),
        "{}",
        moved.source()
    );
    assert_eq!(moved.nested_under(v[0]), Some("  > Said of one."));
    let removed = edit(FieldEdit::RemoveItem {
        field: "answers".into(),
        item: v[1],
    });
    assert!(!removed.source().contains("two"), "{}", removed.source());
    assert_eq!(removed.nested_under(v[0]), Some("  > Said of one."));
    let emptied = edit(FieldEdit::SetItemText {
        field: "answers".into(),
        item: v[0],
        text: String::new(),
    });
    assert!(
        emptied.source().contains("- [ ] {#a1}\n  > Said of one.\n"),
        "{}",
        emptied.source()
    );
    assert_eq!(emptied.nested_under(v[0]), Some("  > Said of one."));
}

#[test]
fn rows_given_by_identity_keep_nested_markdown_on_their_own_item() {
    let src = ":::l\n\n- Same\n  > first\n- Same\n  > second\n\n:::";
    let b = dec(src, &list_decls());
    let v = ids(&b, "items");
    let rows = |r: Vec<Option<ItemId>>| std::collections::HashMap::from([("items".to_string(), r)]);
    // matched by value, the first of two that read the same is the one that stays
    let by_value = run(&b, &b.edits_from_values(&json!({"items": ["Same"]})).expect("fits"));
    assert_eq!(by_value.source(), ":::l\n\n- Same\n  > first\n\n:::");
    // by identity, the one the caller kept stays
    let edits = b
        .edits_from_rows(&json!({"items": ["Same"]}), &rows(vec![Some(v[1])]))
        .expect("fits");
    assert_eq!(run(&b, &edits).source(), ":::l\n\n- Same\n  > second\n\n:::");
    // two that read the same change places: a move, though the values are as they were
    let edits =
        (b.edits_from_rows(&json!({"items": ["Same", "Same"]}), &rows(vec![Some(v[1]), Some(v[0])]))).expect("fits");
    assert_eq!(
        run(&b, &edits).source(),
        ":::l\n\n- Same\n  > second\n- Same\n  > first\n\n:::"
    );
    // a new row among them, and rows that do not fit the field fall back to matching
    let edits = (b.edits_from_rows(&json!({"items": ["New", "Same"]}), &rows(vec![None, Some(v[1])]))).expect("fits");
    assert_eq!(run(&b, &edits).source(), ":::l\n\n- New\n- Same\n  > second\n\n:::");
    for bad in [vec![Some(v[1])], vec![Some(v[0]), Some(v[0])]] {
        let edits = b
            .edits_from_rows(&json!({"items": ["Same", "Same"]}), &rows(bad))
            .expect("fits");
        assert!(edits.is_empty(), "{edits:?}");
    }
}

#[test]
fn nested_markdown_follows_a_marker_of_another_width() {
    let src = ":::l\n\n8. eight\n9. nine\n   > of nine\n\n:::";
    let b = dec(src, &list_decls());
    let v = ids(&b, "items");
    assert_eq!(b.values()["items"], json!(["eight", "nine"]));
    // a new first item makes the second `10.`: what is under it moves one column on
    let next = run(
        &b,
        &[FieldEdit::InsertItem {
            field: "items".into(),
            at: 0,
            item: ItemData::Text("seven".into()),
        }],
    );
    assert_eq!(
        next.source(),
        ":::l\n\n8. seven\n9. eight\n10. nine\n    > of nine\n\n:::"
    );
    assert_eq!(next.nested_under(v[1]), Some("    > of nine"));
    let back = run(
        &next,
        &[FieldEdit::RemoveItem {
            field: "items".into(),
            item: ids(&next, "items")[0],
        }],
    );
    assert_eq!(back.source(), src);
}

#[test]
fn a_list_item_keeps_a_nested_list_and_paragraph_out_of_its_text() {
    let src = ":::l\n\n1. one\n2. two\n\n   more of two\n3. three\n   - nested\n\n:::";
    let b = dec(src, &list_decls());
    assert_eq!(b.values()["items"], json!(["one", "two", "three"]));
    let v = ids(&b, "items");
    assert_eq!(b.nested_under(v[1]), Some("   more of two"));
    assert_eq!(b.nested_under(v[2]), Some("   - nested"));
    let next = run(
        &b,
        &[FieldEdit::SetItemText {
            field: "items".into(),
            item: v[2],
            text: "THREE".into(),
        }],
    );
    assert_eq!(next.source(), src.replace("three", "THREE"));
}

#[test]
fn nested_markdown_indented_by_a_tab_follows_a_wider_marker() {
    let src = ":::l\n\n98. first\n99. second\n\t> attachment\n\n:::";
    let b = dec(src, &list_decls());
    let v = ids(&b, "items");
    assert_eq!(b.nested_under(v[1]), Some("\t> attachment"));
    let next = run(
        &b,
        &[FieldEdit::InsertItem {
            field: "items".into(),
            at: 0,
            item: ItemData::Text("zero".into()),
        }],
    );
    assert!(next.extras().is_empty(), "{}", next.source());
    assert_eq!(
        next.values()["items"],
        json!(["zero", "first", "second"]),
        "{}",
        next.source()
    );
    assert_eq!(next.nested_under(v[1]), Some("     > attachment"), "{}", next.source());
}

// ---------------------------------------------------------------- properties

fn word() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-zа-яё]{1,8}",
        Just("- x".to_string()),
        Just("# y".to_string()),
        Just("> z".to_string()),
        Just("1. n".to_string()),
        Just(":::q".to_string()),
        Just("---".to_string()),
        Just("[ ] box".to_string()),
        Just("**b** w".to_string()),
    ]
}

fn value() -> impl Strategy<Value = String> {
    proptest::collection::vec(word(), 1..4).prop_map(|v| v.join("\n"))
}

fn source() -> impl Strategy<Value = String> {
    let eol = prop_oneof![Just("\n"), Just("\r\n")];
    let sep = prop_oneof![Just(1usize), Just(2)];
    (
        eol,
        sep,
        proptest::option::of("[a-z]{1,6}"),
        proptest::option::of("[a-z]{1,6}"),
        proptest::collection::vec(("[a-z]{1,6}", any::<bool>(), proptest::option::of(0usize..4)), 0..5),
        proptest::option::of("[a-z]{1,6}"),
    )
        .prop_map(|(eol, sep, head, text, answers, quote)| {
            let mut parts = Vec::new();
            if let Some(h) = head {
                parts.push(format!("## {h}"));
            }
            if let Some(t) = text {
                parts.push(t);
            }
            if !answers.is_empty() {
                let lines: Vec<String> = (answers.iter().enumerate())
                    .map(|(i, (t, c, under))| {
                        let item = format!("- [{}] {t} {{#a{i}}}", if *c { "x" } else { " " });
                        format!("{item}{}", under.map_or(String::new(), |k| nested(k, eol, t)))
                    })
                    .collect();
                parts.push(lines.join(eol));
            }
            if let Some(q) = quote {
                parts.push(format!("> {q}"));
            }
            let gap = eol.repeat(sep);
            let mut out = format!(":::poll{gap}");
            for p in parts {
                out.push_str(&p);
                out.push_str(&gap);
            }
            out.push_str(":::");
            out
        })
}

/// Markdown nested under an answer, of kind `k`: a quote, a paragraph, an image, a list.
fn nested(k: usize, eol: &str, t: &str) -> String {
    match k {
        0 => format!("{eol}  > under {t}"),
        1 => format!("{eol}{eol}  more on {t}"),
        2 => format!("{eol}{eol}  ![{t}](p.png)"),
        _ => format!("{eol}  - sub {t}"),
    }
}

#[derive(Clone, Debug)]
enum Op {
    Prompt(String),
    Setup(String),
    Why(String),
    Text(usize, String),
    Correct(usize, bool),
    Insert(usize, String),
    Remove(usize),
    Move(usize, usize),
}

fn op() -> impl Strategy<Value = Op> {
    let n = 0usize..8;
    prop_oneof![
        "[a-z ]{0,6}".prop_map(Op::Prompt),
        value().prop_map(Op::Setup),
        value().prop_map(Op::Why),
        (n.clone(), value()).prop_map(|(i, v)| Op::Text(i, v)),
        (n.clone(), any::<bool>()).prop_map(|(i, c)| Op::Correct(i, c)),
        (n.clone(), value()).prop_map(|(i, v)| Op::Insert(i, v)),
        n.clone().prop_map(Op::Remove),
        (n.clone(), n).prop_map(|(i, j)| Op::Move(i, j)),
    ]
}

fn to_edit(b: &BlockFields, op: &Op) -> Option<FieldEdit> {
    let f = |s: &str| s.to_string();
    let v = ids(b, "answers");
    let pick = |i: usize| (!v.is_empty()).then(|| v[i % v.len()]);
    Some(match op {
        Op::Prompt(t) => FieldEdit::SetText {
            field: f("prompt"),
            text: t.clone(),
        },
        Op::Setup(t) => FieldEdit::SetText {
            field: f("setup"),
            text: t.clone(),
        },
        Op::Why(t) => FieldEdit::SetText {
            field: f("why"),
            text: t.clone(),
        },
        Op::Text(i, t) => FieldEdit::SetItemText {
            field: f("answers"),
            item: pick(*i)?,
            text: t.clone(),
        },
        Op::Correct(i, c) => FieldEdit::SetCorrect {
            field: f("answers"),
            item: pick(*i)?,
            correct: *c,
        },
        Op::Insert(i, t) => FieldEdit::InsertItem {
            field: f("answers"),
            at: i % (v.len() + 1),
            item: ItemData::Choice {
                anchor: None,
                text: t.clone(),
                correct: false,
            },
        },
        Op::Remove(i) => FieldEdit::RemoveItem {
            field: f("answers"),
            item: pick(*i)?,
        },
        Op::Move(i, j) => FieldEdit::MoveItem {
            field: f("answers"),
            item: pick(*i)?,
            to: j % v.len().max(1),
        },
    })
}

fn anchors(b: &BlockFields) -> Vec<String> {
    match &b.field("answers").expect("field").value {
        FieldValue::Items(v) => (v.iter())
            .filter_map(|i| match &i.data {
                ItemData::Choice { anchor, .. } => anchor.clone(),
                _ => None,
            })
            .collect(),
        FieldValue::Text(_) => Vec::new(),
    }
}

proptest! {
    /// No edit, no patch; values read back unchanged give no edits.
    #[test]
    fn decode_then_encode_changes_nothing(src in source()) {
        let b = dec(&src, &poll_decls());
        prop_assert!(b.apply(&[]).expect("applies").patches.is_empty());
        prop_assert!(b.edits_from_values(&b.values()).expect("fits").is_empty());
    }

    /// Any edits: the patches give the new source, it reads back as the edited values, the model
    /// after the edits equals a fresh decode, ids stay unique and no kept id changes.
    #[test]
    fn edits_read_back_as_edited(src in source(), ops in proptest::collection::vec(op(), 1..6)) {
        let mut b = dec(&src, &poll_decls());
        for o in &ops {
            let Some(e) = to_edit(&b, o) else { continue };
            let kept = anchors(&b);
            let removed = match &e {
                FieldEdit::RemoveItem { item, .. } => {
                    let i = ids(&b, "answers").iter().position(|x| x == item).expect("item");
                    Some(kept[i].clone())
                }
                _ => None,
            };
            let next = run(&b, std::slice::from_ref(&e));
            let fresh = dec(next.source(), &poll_decls());
            prop_assert_eq!(fresh.values(), next.values());
            prop_assert!(fresh.extras().is_empty(), "extras in {:?}", next.source());
            let after = anchors(&next);
            let mut unique = after.clone();
            unique.sort();
            unique.dedup();
            prop_assert_eq!(unique.len(), after.len());
            for a in &kept {
                prop_assert!(after.contains(a) || removed.as_ref() == Some(a), "lost {} in {:?}", a, next.source());
            }
            // the edited value reads back
            match &e {
                FieldEdit::SetText { field, text } if field != "prompt" => {
                    prop_assert_eq!(&next.values()[field.as_str()], &json!(text));
                }
                FieldEdit::SetItemText { item, text, .. } => {
                    let i = ids(&next, "answers").iter().position(|x| x == item).expect("item");
                    prop_assert_eq!(&next.values()["answers"][i]["text"], &json!(text));
                }
                _ => {}
            }
            b = next;
        }
    }

    /// What is nested under an answer stays with that answer, byte for byte, through any edits,
    /// until the answer itself goes.
    #[test]
    fn nested_markdown_goes_with_its_answer(src in source(), ops in proptest::collection::vec(op(), 1..6)) {
        let mut b = dec(&src, &poll_decls());
        prop_assert!(b.extras().is_empty(), "{:?}", b.source());
        prop_assert!(b.check(&poll_spec()).errors.iter().all(|e| e.kind != ErrorKind::MissingId), "{:?}", b.source());
        let under = |b: &BlockFields| -> Vec<(ItemId, Option<String>)> {
            ids(b, "answers").into_iter().map(|i| (i, b.nested_under(i).map(str::to_string))).collect()
        };
        for o in &ops {
            let Some(e) = to_edit(&b, o) else { continue };
            let before = under(&b);
            let next = run(&b, std::slice::from_ref(&e));
            let after = under(&next);
            for (id, n) in &before {
                if let Some((_, m)) = after.iter().find(|(j, _)| j == id) {
                    prop_assert_eq!(n, m, "{:?} -> {:?}", b.source(), next.source());
                } else {
                    let gone = matches!(&e, FieldEdit::RemoveItem { item, .. } if item == id);
                    prop_assert!(gone, "{:?} lost an item", e);
                }
            }
            b = next;
        }
    }

    /// Nested Markdown indented by spaces or tabs stays nested when a numbered marker changes width
    /// either way, and keeps its lines but for their leading blanks.
    #[test]
    fn nested_markdown_stays_nested_when_markers_change_width(
        start in prop_oneof![Just(8u64), Just(9), Just(98), Just(99), Just(998)],
        items in proptest::collection::vec(("[a-z]{1,5}", proptest::option::of((any::<bool>(), 0usize..3))), 1..5),
        insert in any::<bool>(),
    ) {
        let mut src = ":::l\n\n".to_string();
        for (i, (t, under)) in items.iter().enumerate() {
            let marker = format!("{}. ", start + i as u64);
            src.push_str(&format!("{marker}{t}\n"));
            if let Some((tab, k)) = under {
                // at the item's content column, by a tab where one reaches it, or by spaces
                let width = marker.len();
                let indent = if *tab && width <= 4 { "\t".to_string() } else { " ".repeat(width) };
                let body = ["> under", "- sub", "```\ncode\n```"][*k].replace('\n', &format!("\n{indent}"));
                src.push_str(&format!("{indent}{body}\n"));
            }
        }
        src.push_str("\n:::");
        let b = dec(&src, &list_decls());
        prop_assert!(b.extras().is_empty(), "{:?}", src);
        let v = ids(&b, "items");
        prop_assert_eq!(v.len(), items.len(), "{:?}", src);
        let edit = if insert {
            FieldEdit::InsertItem { field: "items".into(), at: 0, item: ItemData::Text("new".into()) }
        } else {
            FieldEdit::RemoveItem { field: "items".into(), item: v[0] }
        };
        let next = run(&b, &[edit]);
        let fresh = dec(next.source(), &list_decls());
        prop_assert!(fresh.extras().is_empty(), "{:?}", next.source());
        prop_assert_eq!(fresh.values(), next.values());
        let trimmed = |t: &str| t.lines().map(str::trim_start).collect::<Vec<_>>().join("\n");
        for id in v.iter().skip(usize::from(!insert)) {
            let (a, z) = (b.nested_under(*id), next.nested_under(*id));
            prop_assert_eq!(a.map(trimmed), z.map(trimmed), "{:?} -> {:?}", src, next.source());
        }
    }

    /// A value edit touches nothing outside that value: other fields' bytes are all still
    /// there, in order, and the removed bytes are at most the old value's.
    #[test]
    fn a_value_edit_is_minimal(src in source(), new in "[a-z]{1,6}") {
        let b = dec(&src, &poll_decls());
        prop_assume!(!ids(&b, "answers").is_empty());
        let id = ids(&b, "answers")[0];
        let old_text = b.values()["answers"][0]["text"].as_str().expect("text").to_string();
        let out = b.apply(&[FieldEdit::SetItemText { field: "answers".into(), item: id, text: new.clone() }]).expect("applies");
        let removed: usize = out.patches.patches().iter().map(|p| p.range.len()).sum();
        let added: usize = out.patches.patches().iter().map(|p| p.insert.len()).sum();
        prop_assert!(removed <= old_text.len(), "{:?}", out.patches);
        prop_assert!(added <= new.len(), "{:?}", out.patches);
        let line_end = src.find('\n').unwrap_or(src.len());
        prop_assert!(out.patches.patches().iter().all(|p| p.range.start > line_end));
    }

    /// Reordering through JSON keeps every source line's bytes; only moved lines appear in patches.
    #[test]
    fn reorder_through_values_keeps_lines(src in source(), seed in 0usize..100) {
        let b = dec(&src, &poll_decls());
        let mut arr = b.values()["answers"].as_array().expect("array").clone();
        prop_assume!(arr.len() > 1);
        let n = arr.len();
        arr.rotate_left(seed % n);
        let edits = b.edits_from_values(&json!({ "answers": arr })).expect("fits");
        let next = run(&b, &edits);
        let mut a: Vec<&str> = src.lines().collect();
        let mut c: Vec<&str> = next.source().lines().collect();
        a.sort();
        c.sort();
        prop_assert_eq!(a, c);
        prop_assert_eq!(&next.values()["answers"], &json!(arr));
    }
}

// ---------------------------------------------------------------- against a form spec

fn poll_spec() -> FormSpec {
    let schema = json!({
        "type": "object",
        "required": ["prompt", "answers"],
        "properties": {
            "prompt": {"type": "string"},
            "setup": {"type": "string"},
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
            "why": {"type": "string"},
        },
    });
    let hints = json!({
        "version": 1,
        "order": ["prompt", "setup", "answers", "why"],
        "fields": {
            "prompt": {"x-markdown": {"kind": "heading", "level": 2}},
            "setup": {"widget": "multiline"},
            "answers": {"widget": "choices", "correct": "exactly-one"},
            "why": {"widget": "multiline", "x-markdown": "quote"},
        },
    });
    FormSpec::parse(&schema, &hints, None).expect("in the subset")
}

#[test]
fn the_spec_declares_the_fields() {
    assert_eq!(decls_from_spec(&poll_spec()).expect("all have a place"), poll_decls());
    let schema = json!({"type": "object", "properties": {"m": {"type": "object"}}});
    let spec = FormSpec::parse(&schema, &json!({"version": 1}), None).expect("in the subset");
    assert_eq!(decls_from_spec(&spec), Err("m".to_string()));
}

#[test]
fn a_block_that_fits_has_no_reasons() {
    let src = POLL
        .replace("- [x] Green", "- [ ] Green")
        .replace("- [ ] Blue", "- [x] Blue");
    let fit = dec(&src, &poll_decls()).check(&poll_spec());
    assert!(fit.fits(), "{fit:?}");
}

#[test]
fn a_block_outside_its_schema_reports_per_field_and_stays_editable() {
    // no heading, one answer without an id and none marked, content no field takes
    let src = ":::poll\n\n- [ ] Only\n\n> why\n\n> a second quote\n\n:::";
    let spec = poll_spec();
    let decls = decls_from_spec(&spec).expect("all have a place");
    let b = dec(src, &decls);
    let fit = b.check(&spec);
    assert!(!fit.fits());
    let reasons: Vec<(&str, &ErrorKind)> = fit.errors.iter().map(|e| (e.path.as_str(), &e.kind)).collect();
    assert_eq!(
        reasons,
        [
            ("prompt", &ErrorKind::Required),
            ("answers", &ErrorKind::TooFew { min: 2 }),
            ("answers", &ErrorKind::ExactlyOneCorrect),
            ("answers/0", &ErrorKind::MissingId),
        ]
    );
    assert_eq!(
        fit.extras.iter().map(|r| &src[r.clone()]).collect::<Vec<_>>(),
        ["> a second quote"]
    );

    // still editable; nothing the check said blocks a change, and bad content stays
    let next = run(
        &b,
        &[FieldEdit::SetText {
            field: "prompt".into(),
            text: "Now".into(),
        }],
    );
    assert!(next.source().contains("> a second quote"));
    let mut fixed = next.check(&spec).errors;
    fixed.retain(|e| e.path == "prompt");
    assert!(fixed.is_empty());
}

// ---------------------------------------------------------------- review regressions

fn heading_decl() -> Vec<FieldDecl> {
    vec![FieldDecl::new("h", FieldKind::Heading { level: Some(2) })]
}

#[test]
fn a_tab_after_the_marker_is_columns_not_bytes() {
    let src = ":::x\n\n-\t中\n\n:::";
    let b = dec(src, &list_decls());
    assert_eq!(b.values()["items"], json!(["中"]));
    assert!(b.apply(&[]).expect("applies").patches.is_empty());
    // a continuation line hangs at the tab's column, so it survives a blank line: past one, it is
    // Markdown nested under the item, which stays the item's and is not its text
    let id = ids(&b, "items")[0];
    let next = run(
        &b,
        &[FieldEdit::SetItemText {
            field: "items".into(),
            item: id,
            text: "中\n\nmore".into(),
        }],
    );
    assert_eq!(next.values()["items"], json!(["中"]), "{}", next.source());
    assert_eq!(next.source(), ":::x\n\n-\t中\n\n    more\n\n:::");
    assert!(next.extras().is_empty());
    let b = dec(":::x\n\n1.\tab\n\tcd\n\n:::", &list_decls());
    assert_eq!(b.values()["items"], json!(["ab\ncd"]));
}

#[test]
fn an_empty_heading_with_an_id_survives_when_nobody_edits_it() {
    let src = ":::x\n\n## {#q}\n\ntext\n\n:::";
    let decls = [heading_decl(), vec![FieldDecl::new("t", FieldKind::Text)]].concat();
    let b = dec(src, &decls);
    assert_eq!(b.values()["h"], json!(""));
    assert!(b.apply(&[]).expect("applies").patches.is_empty());
    let next = run(
        &b,
        &[FieldEdit::SetText {
            field: "t".into(),
            text: "other".into(),
        }],
    );
    assert_eq!(next.source(), src.replace("text", "other"));
    let next = run(
        &b,
        &[FieldEdit::SetText {
            field: "h".into(),
            text: "Now".into(),
        }],
    );
    assert_eq!(next.source(), src.replace("## {#q}", "## Now {#q}"));
    // an empty quote the same way
    let src = ":::x\n\n>\n\n:::";
    let b = dec(src, &[FieldDecl::new("q", FieldKind::Quote)]);
    assert!(b.apply(&[]).expect("applies").patches.is_empty());
}

#[test]
fn an_empty_marker_gets_its_space_with_its_first_text() {
    let b = dec(":::x\n\n-\n\n:::", &list_decls());
    let id = ids(&b, "items")[0];
    let next = run(
        &b,
        &[FieldEdit::SetItemText {
            field: "items".into(),
            item: id,
            text: "hello".into(),
        }],
    );
    assert_eq!(next.source(), ":::x\n\n- hello\n\n:::");
    assert_eq!(ids(&next, "items"), vec![id], "still that item");
    // a new item copying that marker too
    let b = dec(":::x\n\n-\n\n:::", &list_decls());
    let next = run(
        &b,
        &[FieldEdit::InsertItem {
            field: "items".into(),
            at: 1,
            item: ItemData::Text("b".into()),
        }],
    );
    assert_eq!(next.values()["items"], json!(["", "b"]), "{}", next.source());
}

#[test]
fn a_new_id_is_unique_in_the_whole_block() {
    let src = ":::x\n\n## Q {#a7}\n\n- [ ] A {#a1}\n\n> w\n\n- [ ] B {#a2}\n- [ ] C {#a5}\n\n:::";
    let decls = vec![
        FieldDecl::new("h", FieldKind::Heading { level: Some(2) }),
        FieldDecl::new("one", FieldKind::Choices),
        FieldDecl::new("why", FieldKind::Quote),
        FieldDecl::new("two", FieldKind::Choices),
    ];
    let b = dec(src, &decls);
    let insert = |wanted: Option<&str>| FieldEdit::InsertItem {
        field: "one".into(),
        at: 1,
        item: ItemData::Choice {
            anchor: wanted.map(str::to_string),
            text: "N".into(),
            correct: false,
        },
    };
    let next = run(&b, &[insert(None)]);
    assert_eq!(
        anchors_of(&next, "one"),
        ["a1", "a8"],
        "above the other field's and the heading's"
    );
    // an id wanted but taken elsewhere in the block is not granted
    let next = run(&b, &[insert(Some("a5"))]);
    assert_eq!(anchors_of(&next, "one"), ["a1", "a8"]);
    let next = run(&b, &[insert(Some("q"))]);
    assert_eq!(anchors_of(&next, "one"), ["a1", "q"], "a free one is");
}

fn anchors_of(b: &BlockFields, field: &str) -> Vec<String> {
    (b.values()[field].as_array().expect("array").iter())
        .map(|a| a["id"].as_str().expect("id").to_string())
        .collect()
}

#[test]
fn a_typed_backslash_before_closing_hashes_stays() {
    let b = dec(":::x\n\n## Q\n\n:::", &heading_decl());
    for typed in ["Q ##", "Q \\##", "Q \\\\##", "\\#", "#", "Q#", "Q \\# {#x}"] {
        let next = run(
            &b,
            &[FieldEdit::SetText {
                field: "h".into(),
                text: typed.into(),
            }],
        );
        assert_eq!(next.values()["h"], json!(typed), "{}", next.source());
        let fresh = dec(next.source(), &heading_decl());
        assert_eq!(fresh.values()["h"], json!(typed));
        assert!(fresh.apply(&[]).expect("applies").patches.is_empty());
    }
}

#[test]
fn the_largest_number_in_an_id_does_not_overflow_the_next_one() {
    let src = ":::x\n\n## Q {#a18446744073709551615}\n\n- [ ] A {#a18446744073709551614}\n\n:::";
    let decls = vec![
        FieldDecl::new("h", FieldKind::Heading { level: Some(2) }),
        FieldDecl::new("one", FieldKind::Choices),
    ];
    let b = dec(src, &decls);
    let insert = FieldEdit::InsertItem {
        field: "one".into(),
        at: 1,
        item: ItemData::Choice {
            anchor: None,
            text: "N".into(),
            correct: false,
        },
    };
    let next = run(&b, std::slice::from_ref(&insert));
    let got = anchors_of(&next, "one");
    assert_eq!(got.len(), 2);
    assert!(!got[1].is_empty());
    assert!(!src.contains(&format!("{{#{}}}", got[1])), "{}", next.source());
    assert_eq!(got[0], "a18446744073709551614");
    // the next number after the largest u64 can itself be taken
    let src = src.replace("{#a18446744073709551614}", "{#a18446744073709551616}");
    let next = run(&dec(&src, &decls), &[insert]);
    assert_eq!(
        anchors_of(&next, "one")[1],
        "a18446744073709551617",
        "{}",
        next.source()
    );
}

fn plate_decls() -> Vec<FieldDecl> {
    vec![
        FieldDecl::new("which", FieldKind::Argument),
        FieldDecl::new("title", FieldKind::Heading { level: None }),
        FieldDecl::new("body", FieldKind::Text),
    ]
}

fn set_argument(text: &str) -> FieldEdit {
    FieldEdit::SetText {
        field: "which".to_string(),
        text: text.to_string(),
    }
}

#[test]
fn an_edit_of_the_argument_changes_only_its_bytes() {
    for (src, new, want) in [
        (":::plate a/b\n\nText\n\n:::", "c/d", ":::plate c/d\n\nText\n\n:::"),
        // the fence, what stands around the arguments and the line endings stay
        ("::::plate a/b\n\nText\n\n::::", "c", "::::plate c\n\nText\n\n::::"),
        (
            ":::plate\t  a b  \t\n\nText\n\n:::",
            "c",
            ":::plate\t  c  \t\n\nText\n\n:::",
        ),
        (
            ":::plate a\r\n\r\nText\r\n\r\n:::",
            "bc",
            ":::plate bc\r\n\r\nText\r\n\r\n:::",
        ),
        (":::plate a\n:::", "ю/я", ":::plate ю/я\n:::"),
        // none yet: one blank before it
        (":::plate\n\nText\n\n:::", "c", ":::plate c\n\nText\n\n:::"),
        (":::plate  \n\nText\n\n:::", "c", ":::plate c  \n\nText\n\n:::"),
        // none any more: what stood before it goes with it
        (":::plate a/b\n\nText\n\n:::", "", ":::plate\n\nText\n\n:::"),
        (":::plate\t a/b  \n\nText\n\n:::", "", ":::plate  \n\nText\n\n:::"),
        // the ends of a typed value are not part of an argument
        (":::plate a\n\nText\n\n:::", "  c d\t", ":::plate c d\n\nText\n\n:::"),
        // what could read as markup anywhere else is plain text here
        (":::plate a\n:::", ":::x {#y} # *z*", ":::plate :::x {#y} # *z*\n:::"),
    ] {
        let next = run(&dec(src, &plate_decls()), &[set_argument(new)]);
        assert_eq!(next.source(), want, "{src:?}");
        assert_eq!(next.args(), new.trim_matches([' ', '\t']));
        assert_eq!(next.values()["which"], next.args());
    }
}

#[test]
fn the_patch_of_an_argument_is_inside_the_opener() {
    let b = dec(":::plate old/name\n\n## T\n\nText\n\n:::", &plate_decls());
    let out = b.apply(&[set_argument("old/other")]).expect("applies");
    let [p] = out.patches.patches() else {
        panic!("{:?}", out.patches)
    };
    assert_eq!(&b.source()[p.range.clone()], "name");
    assert_eq!(p.insert.as_str(), "other");
    // with a field in the same change, each stays where it is
    let both = [
        set_argument("x"),
        FieldEdit::SetText {
            field: "body".to_string(),
            text: "Other".to_string(),
        },
    ];
    assert_eq!(run(&b, &both).source(), ":::plate x\n\n## T\n\nOther\n\n:::");
}

#[test]
fn an_argument_is_one_line_and_reads_back_or_nothing_happens() {
    let b = dec(":::plate a\n\nText\n\n:::", &plate_decls());
    for bad in ["two\nlines", "two\r\nlines", "end\r"] {
        assert!(matches!(
            b.apply(&[set_argument(bad)]),
            Err(markise_engine::block_fields::FieldsError::WrongKind { .. })
        ));
    }
    // not what the parser calls a blank: as it stands in the file, it stays
    let nbsp = dec(":::plate \u{a0}a\u{a0}\n:::", &plate_decls());
    assert_eq!(nbsp.args(), "\u{a0}a\u{a0}");
    assert!(nbsp.edits_from_values(&nbsp.values()).unwrap().is_empty());
}

#[test]
fn an_argument_is_a_value_of_the_form_and_no_place_to_type() {
    let b = dec(":::plate a/b\n\n## T\n\nText\n\n:::", &plate_decls());
    assert_eq!(b.values(), json!({"which": "a/b", "title": "T", "body": "Text"}));
    assert!(b.edits_from_values(&b.values()).unwrap().is_empty());
    assert_eq!(
        b.edits_from_values(&json!({"which": "  a/b "})).unwrap(),
        [],
        "only blanks around it: no change"
    );
    assert_eq!(
        b.edits_from_values(&json!({"which": "c"})).unwrap(),
        [set_argument("c")]
    );
    assert_eq!(b.edits_from_values(&json!({"which": ""})).unwrap(), [set_argument("")]);
    // no slot, no region: the keyboard gets nowhere near it, and an empty one neither
    assert!(b.slots().iter().all(|s| s.kind != FieldKind::Argument));
    assert!(b.field_regions().iter().all(|(_, k)| *k != FieldKind::Argument));
    let empty = dec(":::plate\n:::", &plate_decls());
    assert!(empty.slots().is_empty());
    // the body goes to the fields as it did, whatever the argument
    assert_eq!(b.field("title").unwrap().value, FieldValue::Text("T".into()));
    assert!(b.extras().is_empty());
}

#[test]
fn a_slot_edit_keeps_the_argument_and_an_argument_is_one_undo_step() {
    let src = "Before.\n\n:::plate a\n\nText\n\n:::\n";
    let mut s = session(src);
    let r = block_range_at(s.doc(), src.find(":::plate").unwrap()).expect("a block");
    let b = BlockFields::decode(&src[r.clone()], r.start, &plate_decls(), &ParseOptions::default()).expect("a block");
    let undo = s.history().undo_len();
    let edits = [
        set_argument("c/d"),
        FieldEdit::SetText {
            field: "body".to_string(),
            text: "Other".to_string(),
        },
    ];
    b.apply_to_session(&mut s, &edits, "Edit Block").expect("applies");
    assert_eq!(s.text(), "Before.\n\n:::plate c/d\n\nOther\n\n:::\n");
    assert_eq!(s.history().undo_len(), undo + 1);
    assert!(s.undo());
    assert_eq!(s.text(), src);
    // and a block that changed refuses, as any edit of it
    let mut changed = session(&src.replace(":::plate a", ":::plate z"));
    assert!(matches!(
        b.apply_to_session(&mut changed, &[set_argument("c")], "Edit Block"),
        Err(markise_engine::block_fields::FieldsError::Stale)
    ));
}

/// Issue #231: a code line edited through a text field lost its indent, and a fence line got a `\`.
/// A text field holding a code block keeps it byte for byte: an edit that would change it is refused.
#[test]
fn a_code_block_in_a_text_field_is_never_rewritten() {
    let src = ":::f\n\n# T\n\nBefore\n\n```rust title\n  let x = 1;\n\\# not an escape\n```\n\n    indented code\n\nAfter\n\n:::";
    let b = dec(src, &figure_decls());
    let caption = b.values()["caption"].as_str().unwrap().to_string();
    // the value shows the code as written, escapes and indents included
    assert!(
        caption.contains("```rust title\n  let x = 1;\n\\# not an escape\n```"),
        "{caption}"
    );
    assert!(caption.contains("\n    indented code\n"), "{caption}");
    assert!(b.holds_code("caption"));
    assert!(!b.holds_code("title"));
    // editing a code line, the info string or the indent is refused, and nothing is written
    for (from, to) in [
        ("  let x = 1;", "  let x = 2;"),
        ("```rust title", "```python"),
        ("    indented code", "    indented code!"),
        ("\\# not an escape\n", "\\# not an escape\nmore\n"),
    ] {
        let text = caption.replacen(from, to, 1);
        let e = b.apply(&[FieldEdit::SetText {
            field: "caption".into(),
            text: text.clone(),
        }]);
        assert!(
            matches!(e, Err(FieldsError::Code { ref field }) if field == "caption"),
            "{from:?} -> {to:?}: {:?}",
            e.map(|a| a.fields.source().to_string())
        );
        let e = b.edits_from_values(&json!({"title": "T", "caption": text, "pictures": []}));
        if let Ok(edits) = e {
            assert!(b.apply(&edits).is_err(), "{from:?}");
        }
    }
    // emptying the field would take the code away: refused too
    assert!(
        b.apply(&[FieldEdit::SetText {
            field: "caption".into(),
            text: String::new()
        }])
        .is_err()
    );
    // the prose around it is text, and the code keeps its bytes
    let next = run(
        &b,
        &[FieldEdit::SetText {
            field: "caption".into(),
            text: caption.replace("Before", "# Before").replace("After", "After!"),
        }],
    );
    assert_eq!(
        next.source(),
        src.replace("Before", "\\# Before").replace("After", "After!")
    );
}

/// The code, math and HTML blocks of a block's source, as written.
fn code_blocks(src: &str) -> Vec<String> {
    fn walk(s: &str, blocks: &[markise_md::Block], out: &mut Vec<String>) {
        for b in blocks {
            match b.kind {
                markise_md::BlockKind::FencedCode { .. } | markise_md::BlockKind::IndentedCode => {
                    out.push(s[b.span.start..b.span.end].to_string())
                }
                _ => walk(s, &b.children, out),
            }
        }
    }
    let doc = markise_md::Document::parse(src, ParseOptions::default());
    let mut out = Vec::new();
    walk(src, &doc.tree(), &mut out);
    out
}

fn code_source() -> impl Strategy<Value = String> {
    let part = prop_oneof![
        "[a-z]{1,6}( [a-z]{1,4})?".prop_map(|w| w),
        ("[a-z]{0,4}", proptest::collection::vec(code_line(), 1..4))
            .prop_map(|(info, lines)| format!("```{info}\n{}\n```", lines.join("\n"))),
        proptest::collection::vec(code_line(), 1..3).prop_map(|lines| lines
            .iter()
            .map(|l| format!("    {l}"))
            .collect::<Vec<_>>()
            .join("\n")),
    ];
    (
        prop_oneof![Just("\n"), Just("\r\n")],
        proptest::collection::vec(part, 1..5),
    )
        .prop_map(|(eol, parts)| {
            // two indented code blocks in a row would be one
            let body = parts.join("\n\nx\n\n");
            format!(":::f\n\n# T\n\n{body}\n\n:::").replace('\n', eol)
        })
}

fn code_line() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z]{1,5}".prop_map(|w| w),
        Just("  indented".to_string()),
        Just("\\# escaped".to_string()),
        Just("# hash".to_string()),
        Just("- dash".to_string()),
        Just("\tTab".to_string()),
    ]
}

proptest! {
    /// Issue #231: whatever a text field's value becomes, a code block in it keeps its bytes, or
    /// the edit is refused as touching code. An edit that goes through reads back as typed.
    #[test]
    fn code_in_a_text_field_round_trips(
        src in code_source(),
        at in 0usize..40,
        line in prop_oneof![code_line(), word(), Just(String::new())],
        how in 0usize..3,
    ) {
        let b = dec(&src, &figure_decls());
        prop_assert!(b.apply(&[]).expect("applies").patches.is_empty());
        prop_assert!(b.edits_from_values(&b.values()).expect("fits").is_empty());
        let caption = b.values()["caption"].as_str().expect("text").to_string();
        let mut lines: Vec<String> = caption.split('\n').map(str::to_string).collect();
        let i = at % (lines.len() + 1);
        match how {
            0 if i < lines.len() => lines[i] = line,
            1 if i < lines.len() => {
                lines.remove(i);
            }
            _ => lines.insert(i, line),
        }
        let text = lines.join("\n");
        match b.apply(&[FieldEdit::SetText { field: "caption".into(), text }]) {
            Ok(out) => {
                prop_assert_eq!(out.patches.apply_to(b.source()), out.fields.source());
                prop_assert_eq!(code_blocks(out.fields.source()), code_blocks(b.source()), "{:?}", out.fields.source());
                let fresh = dec(out.fields.source(), &figure_decls());
                prop_assert_eq!(fresh.values(), out.fields.values());
            }
            Err(e) => prop_assert!(matches!(e, FieldsError::Code { .. }), "{:?}", e),
        }
    }
}

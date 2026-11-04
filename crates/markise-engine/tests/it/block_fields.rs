//! The field model of `:::` blocks (ADR 0020): source to values, edits to minimal patches.

use std::sync::Arc;

use markise_doc::DocumentFormat;
use markise_engine::block_fields::{
    BlockFields, FieldDecl, FieldEdit, FieldKind, FieldValue, ItemData, ItemId, block_range_at,
};
use markise_engine::editor::{EditorSession, HistoryLimits, ManualClock};
use markise_md::{MarkdownFormat, ParseOptions};
use proptest::prelude::*;
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
        proptest::collection::vec(("[a-z]{1,6}", any::<bool>()), 0..5),
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
                    .map(|(i, (t, c))| format!("- [{}] {t} {{#a{i}}}", if *c { "x" } else { " " }))
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
    // a continuation line hangs at the tab's column, so it survives a blank line
    let id = ids(&b, "items")[0];
    let next = run(
        &b,
        &[FieldEdit::SetItemText {
            field: "items".into(),
            item: id,
            text: "中\n\nmore".into(),
        }],
    );
    assert_eq!(next.values()["items"], json!(["中\n\nmore"]), "{}", next.source());
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

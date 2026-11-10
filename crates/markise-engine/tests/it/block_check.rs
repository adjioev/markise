//! A document against its package with no window: reasons in the words of the view, and their lines.

use markise_engine::block_check::{BlockChecker, Problem, ProblemKind};
use markise_engine::block_view::PackageViews;
use markise_layout::{BlockLook, BlockViews};

use super::block_views::index;

fn problems(text: &str) -> Vec<Problem> {
    BlockChecker::new(index()).check(text).problems
}

fn short(p: &Problem) -> (usize, Option<&str>, &'static str, &str) {
    (p.line, p.block.as_deref(), p.kind.name(), p.reason.as_str())
}

#[test]
fn a_mix_of_good_and_bad_blocks_reports_the_bad_ones_on_their_lines() {
    let text = "\
---
lang: en
---

# A document

:::tip

Fine.

:::

:::tip

:::

:::poll

## Which?

- [ ] One {#a1}
- [x] Two {#a2}

> Because.

:::

:::poll nope

## Which?

- [ ] One {#a1}
- [x] Two {#a2}

:::

:::poll

## Which?

- [ ] One {#a1}
- [x] Two {#a1}

```
code
```

:::

:::plate wrong

:::

:::plain

:::

:::elsewhere

Body.

:::

::: spaced

:::orphan

Text.
";
    let mut checker = BlockChecker::new(index());
    let check = checker.check(text);
    let found: Vec<_> = check.problems.iter().map(short).collect();
    assert_eq!(check.language, "en");
    assert_eq!(check.blocks, 8, "{found:#?}");
    assert_eq!(
        found[..7],
        [
            (13, Some("tip"), "not-filled", "Body: Required"),
            (28, Some("poll"), "structure", "The block has no variant `nope`."),
            (37, Some("poll"), "structure", "Answers 2: The id `a1` is used twice"),
            (44, Some("poll"), "structure", "In no field: ```…"),
            (
                50,
                Some("plate"),
                "not-filled",
                "Image set: Does not have the form `^[a-z]+/[a-z]+$`"
            ),
            (54, Some("plain"), "structure", "Body: Required"),
            (
                58,
                Some("elsewhere"),
                "unknown-block",
                "The package has no block `elsewhere`."
            ),
        ],
        "{found:#?}"
    );
    assert_eq!(found[7].0, 64);
    assert_eq!(found[7].1, None);
    assert!(found[7].3.starts_with("`::: spaced` is not a marker"), "{found:#?}");
    assert_eq!((found[8].0, found[8].1, found[8].2), (66, Some("orphan"), "marker"));
    assert!(found[8].3.starts_with("`:::orphan` has no closer"), "{found:#?}");
    assert_eq!(found.len(), 9);
    // a reason about a piece of the block says where, and which block
    let extra = &check.problems[3];
    assert_eq!((extra.line, extra.block_line), (44, 37));
}

/// The same words as the view, so the two cannot tell different stories.
#[test]
fn the_reasons_are_those_of_the_view() {
    let views = PackageViews::new(index(), "en");
    for (text, unfilled) in [
        (":::tip\n\n:::", true),
        (":::poll\n\n## Q\n\n- [x] A {#a1}\n- [x] B {#a2}\n\n:::", false),
        (":::poll nope\n\n:::", false),
        (":::plain\n\n:::", false),
    ] {
        let found = problems(&format!("{text}\n"));
        let (name, args) = {
            let opener = text.lines().next().unwrap().trim_start_matches(':');
            opener.split_once(' ').unwrap_or((opener, ""))
        };
        let reasons = match views.look(name, args, text) {
            BlockLook::Unfilled { reasons, .. } => {
                assert!(unfilled, "{text}");
                reasons
            }
            BlockLook::Outside { reasons, .. } => {
                assert!(!unfilled, "{text}");
                reasons
            }
            other => panic!("{other:?}"),
        };
        let words: Vec<_> = found.iter().map(|p| p.reason.clone()).collect();
        assert_eq!(words, reasons, "{text}");
        let kind = if unfilled {
            ProblemKind::NotFilled
        } else {
            ProblemKind::Structure
        };
        assert!(found.iter().all(|p| p.kind == kind), "{text}");
    }
}

#[test]
fn lines_count_as_an_editor_counts_them() {
    for eol in ["\n", "\r\n", "\r"] {
        let text = ["# T", "", "x", "", ":::tip", "", ":::", ""].join(eol);
        let found = problems(&text);
        assert_eq!(found.len(), 1, "{eol:?}");
        assert_eq!(found[0].line, 5, "{eol:?}");
    }
}

#[test]
fn a_block_the_view_would_not_read_says_so() {
    let body = "A line of text to fill the block.\n".repeat(2500);
    let found = problems(&format!(":::tip\n\n{body}\n:::\n"));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, ProblemKind::Structure);
    assert_eq!(found[0].reason, "Over 64 KiB, so it stays a card.");
    // one the package does not know is that first
    let found = problems(&format!(":::elsewhere\n\n{body}\n:::\n"));
    assert_eq!(found[0].kind, ProblemKind::UnknownBlock);
}

#[test]
fn the_language_of_the_document_names_the_fields() {
    let mut files = std::collections::BTreeMap::new();
    let schema =
        r#"{"label": "Tip", "fields": [{"name": "body", "type": "text", "label": {"en": "Text", "de": "Inhalt"}}]}"#;
    files.insert(
        "manifest.json".to_string(),
        br#"{"format":"markise-block-package/2","library":"s","version":"1","blocks":["tip"]}"#.to_vec(),
    );
    files.insert("components/tip/tip.json".to_string(), schema.as_bytes().to_vec());
    files.insert("editor/palette.json".to_string(), b"{}".to_vec());
    let index = markise_package::PackageIndex::from_files(&files).unwrap();
    let mut checker = BlockChecker::new(std::sync::Arc::new(index));
    let reason = |c: &mut BlockChecker, text: &str| c.check(text).problems[0].reason.clone();
    assert_eq!(reason(&mut checker, ":::tip\n\n:::\n"), "Text: Required");
    assert_eq!(
        reason(&mut checker, "---\nlang: de\n---\n\n:::tip\n\n:::\n"),
        "Inhalt: Required"
    );
}

#[test]
fn a_block_with_no_form_gives_the_reason_of_the_panel() {
    let mut files = std::collections::BTreeMap::new();
    files.insert(
        "manifest.json".to_string(),
        br#"{"format":"markise-block-package/2","library":"s","version":"1","blocks":["tip"]}"#.to_vec(),
    );
    files.insert(
        "components/tip/tip.json".to_string(),
        br#"{"label": "Tip", "fields": [{"name": "title", "type": "heading", "level": 7}]}"#.to_vec(),
    );
    files.insert("editor/palette.json".to_string(), b"{}".to_vec());
    let index = markise_package::PackageIndex::from_files(&files).unwrap();
    let found = BlockChecker::new(std::sync::Arc::new(index))
        .check(":::tip\n\n# T\n\n:::\n")
        .problems;
    assert_eq!(found[0].kind, ProblemKind::NoForm);
    assert_eq!(found[0].reason, "field `title`: `level` 7: a heading has levels 1 to 6");
}

#[test]
fn a_nested_block_answers_for_itself_and_is_in_no_field_of_the_one_around_it() {
    let found = problems("::::tip\n\nText.\n\n:::tip\n\n:::\n\n::::\n");
    let found: Vec<_> = found.iter().map(short).collect();
    assert_eq!(found[0], (5, Some("tip"), "structure", "In no field: :::tip…"));
    assert_eq!(found[1], (5, Some("tip"), "not-filled", "Body: Required"));
}

#[test]
fn a_document_with_no_blocks_is_clean_and_a_marker_is_read_without_blank_lines() {
    let mut checker = BlockChecker::new(index());
    let check = checker.check("# Title\n\nText with ::: in it.\n");
    assert_eq!((check.blocks, check.problems.len()), (0, 0));
    assert_eq!(checker.check("Text.\n:::tip\nBody.\n:::\nText.\n").problems, []);
}

#[test]
fn a_sections_metadata_block_is_unknown_only_outside_the_dialect() {
    let body = ":::section-metadata\n\nid: one\n\n:::\n";
    assert_eq!(problems(body)[0].kind, ProblemKind::UnknownBlock);
    assert_eq!(problems(&format!("---\nauthoring: blocks-v3\n---\n\n{body}")), []);
    assert_eq!(
        problems("---\nauthoring: blocks-v3\n---\n\n:::elsewhere\n\n:::\n")[0].kind,
        ProblemKind::UnknownBlock
    );
}

/// What `blocks check` says of a block and what the view does with it in Read agree (ADR 0024).
#[test]
fn the_check_and_practice_agree() {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use markise_layout::NativeBlock;
    use markise_package::PackageIndex;

    let files: BTreeMap<String, Vec<u8>> = [
        (
            "manifest.json",
            r#"{"format":"markise-block-package/2","library":"s","version":"1","blocks":["task","pick"]}"#,
        ),
        (
            "components/task/task.json",
            r#"{"fields":[{"name":"do","type":"text"},{"name":"done","type":"quote","min":0}]}"#,
        ),
        (
            "components/pick/pick.json",
            r#"{"fields":[{"name":"do","type":"text"},{"name":"ones","type":"choices","minItems":0},{"name":"why","type":"quote","min":0}]}"#,
        ),
        ("editor/palette.json", r#"{"format":1}"#),
        (
            "editor/views.json",
            r#"{"task":{"kind":"card","practice":{"reveal":["done"]}},"pick":{"kind":"choices","practice":{"reveal":["why"]}}}"#,
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
    .collect();
    let index = Arc::new(PackageIndex::from_files(&files).unwrap());
    let views = PackageViews::new(index.clone(), "en");
    let parts = |source: &str, name: &str| {
        let block: NativeBlock = match views.look(name, "", source) {
            BlockLook::Native(b) => b,
            _ => panic!("{source}"),
        };
        block.practice.unwrap()
    };
    let says = |source: &str| BlockChecker::new(index.clone()).check(source).problems;
    // nothing to reveal: clean, and not practised
    for (name, source) in [("task", ":::task\n\nDo.\n\n:::"), ("pick", ":::pick\n\nDo.\n\n:::")] {
        assert!(says(source).is_empty(), "{source}");
        assert!(parts(source, name).nothing_to_practise(), "{source}");
    }
    // something to practise: clean, and ready
    for (name, source) in [
        ("task", ":::task\n\nDo.\n\n> Done.\n\n:::"),
        ("pick", ":::pick\n\nDo.\n\n- [x] One {#a1}\n- [ ] Two {#a2}\n\n:::"),
    ] {
        assert!(says(source).is_empty(), "{source}");
        assert!(parts(source, name).is_ready(), "{source}");
    }
    // a mark no field reads as an answer: not ready in the view, and so in the check
    for (name, source) in [
        ("task", ":::task\n\nDo.\n\n- [x] One {#a1}\n- Two {#a2}\n\n:::"),
        ("pick", ":::pick\n\nDo.\n\n- [x] One {#a1}\n- Two {#a2}\n\n:::"),
        ("task", ":::task\n\nDo.\n\n- [ ] One\n\n:::"),
    ] {
        let p = parts(source, name);
        assert!(!p.is_ready() && !p.nothing_to_practise(), "{source}");
        let problems = says(source);
        assert_eq!(problems.len(), 1, "{source}: {problems:?}");
        assert!(problems[0].reason.contains("not read as answers"), "{problems:?}");
    }
    // a mark in code is text, an example, and no answer: for the view, Practice and the check alike
    for (name, source) in [
        (
            "task",
            ":::task\n\nDo.\n\n```md\n- [x] One\n- [ ] Two\n```\n\n> Done.\n\n:::",
        ),
        ("task", ":::task\n\nDo.\n\n    - [ ] indented code\n\n> Done.\n\n:::"),
        (
            "pick",
            ":::pick\n\nDo.\n\n```\n- [ ] in code\n```\n\n- [x] One {#a1}\n- [ ] Two {#a2}\n\n:::",
        ),
    ] {
        let p = parts(source, name);
        assert!(p.is_ready() && !p.stray_marks, "{source}");
        assert!(says(source).is_empty(), "{source}: {:?}", says(source));
    }
    // and out of code it still is one
    let source = ":::task\n\nDo.\n\n- [x] One\n\n```\ncode\n```\n\n> Done.\n\n:::";
    assert!(parts(source, "task").stray_marks, "{source}");
    // answers and no mark: the view says not ready, and so does the check
    let source = ":::pick\n\nDo.\n\n- [ ] One {#a1}\n- [ ] Two {#a2}\n\n> Why.\n\n:::";
    assert!(!parts(source, "pick").is_ready() && !parts(source, "pick").nothing_to_practise());
    let problems = says(source);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].reason.contains("No answer is marked"), "{problems:?}");
}

//! Property test (Codex review of PR #35): list edits across a marker-width boundary
//! (`9.` → `10.`, `99.` → `100.`) keep every item's children inside it, and the
//! incremental projection equals a full parse after every step.

use std::sync::Arc;

use markise_doc::{Params, Selection};
use markise_engine::editor::command::Registry;
use markise_engine::editor::{EditorSession, HistoryLimits, ManualClock};
use markise_md::{Document, MarkdownFormat, ParseOptions};
use proptest::prelude::*;

/// Item `k`'s first block: a paragraph, an ATX heading (with a closing sequence) or a setext
/// heading (Codex review round 4: joins must keep heading syntax inside items).
fn doc(n: usize, start: usize, kinds: &[u8]) -> String {
    let mut s = String::new();
    for k in 0..n {
        let num = start + k;
        let w = num.to_string().len() + 2;
        let pad = " ".repeat(w);
        match kinds.get(k).copied().unwrap_or(0) % 3 {
            0 => s.push_str(&format!("{num}. a{k}\n")),
            1 => s.push_str(&format!("{num}. ## a{k} ##\n")),
            _ => s.push_str(&format!("{num}. a{k}\n{pad}===\n")),
        }
        s.push_str(&format!("{pad}- chïld\n"));
    }
    s
}

#[derive(Clone, Debug)]
enum Op {
    EnterEnd(prop::sample::Index),
    BackspaceStart(prop::sample::Index),
    DeleteEnd(prop::sample::Index),
    Tab(prop::sample::Index),
    ShiftTab(prop::sample::Index),
    MoveUp(prop::sample::Index),
    MoveDown(prop::sample::Index),
    Undo,
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => any::<prop::sample::Index>().prop_map(Op::EnterEnd),
        2 => any::<prop::sample::Index>().prop_map(Op::BackspaceStart),
        2 => any::<prop::sample::Index>().prop_map(Op::DeleteEnd),
        1 => any::<prop::sample::Index>().prop_map(Op::Tab),
        1 => any::<prop::sample::Index>().prop_map(Op::ShiftTab),
        1 => any::<prop::sample::Index>().prop_map(Op::MoveUp),
        1 => any::<prop::sample::Index>().prop_map(Op::MoveDown),
        1 => Just(Op::Undo),
    ]
}

fn cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(128)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases()))]

    #[test]
    fn children_stay_inside_items_across_marker_widths(
        (n, start) in prop::sample::select(&[(3usize, 8usize), (4, 7), (3, 98), (4, 97)][..]),
        ops in prop::collection::vec(arb_op(), 1..10),
        kinds in prop::collection::vec(0u8..3, 4),
    ) {
        let src = doc(n, start, &kinds);
        let reg = Registry::for_format(&MarkdownFormat::default());
        let mut s = EditorSession::with(Arc::new(MarkdownFormat::default()), src, HistoryLimits::default(), Box::new(ManualClock::default()));
        for op in &ops {
            // the item labels present now (`a3`, …)
            let text = s.text().to_string();
            let labels: Vec<usize> = (0..n).filter(|k| text.contains(&format!("a{k}"))).collect();
            let pick = |ix: &prop::sample::Index| labels[ix.index(labels.len())];
            let at_end = |k: usize| text.find(&format!("a{k}")).map(|p| p + format!("a{k}").len());
            let at_start = |k: usize| text.find(&format!("a{k}"));
            let (pos, id) = match op {
                Op::EnterEnd(ix) => (at_end(pick(ix)), "edit.newline"),
                Op::BackspaceStart(ix) => (at_start(pick(ix)), "edit.deleteBackward"),
                Op::DeleteEnd(ix) => (at_end(pick(ix)), "edit.deleteForward"),
                Op::Tab(ix) => (at_start(pick(ix)), "edit.indentOrNextCell"),
                Op::ShiftTab(ix) => (at_start(pick(ix)), "edit.outdentOrPrevCell"),
                Op::MoveUp(ix) => (at_start(pick(ix)), "edit.moveBlockUp"),
                Op::MoveDown(ix) => (at_start(pick(ix)), "edit.moveBlockDown"),
                Op::Undo => (None, "edit.undo"),
            };
            if let Some(p) = pos {
                s.set_selection(Selection::caret(p));
            }
            let top_lists = |t: &str| t.lines().filter(|l| *l == "ul").count();
            let before_text = s.text().to_string();
            let before_top = top_lists(&s.doc().debug_tree());
            if id == "edit.deleteBackward" {
                s.delete_backward().unwrap();
            } else if id == "edit.deleteForward" {
                s.delete_forward().unwrap();
            } else {
                let _ = reg.execute(&mut s, id, &Params::new());
            }
            if id == "edit.newline" {
                s.insert_text("z").unwrap();
            }
            let full = Document::parse(s.text(), ParseOptions::default());
            let mut full = full;
            full.set_hints(s.hints());
            full.set_options(ParseOptions::default());
            let tree = s.doc().debug_tree();
            prop_assert_eq!(&tree, &markise_md::debug_tree(&full), "incremental == full after {:?}\n{}", op, s.text());
            // every child is still a list item's child list (never absorbed or detached)
            // (C48 may merge a child's text into the item's paragraph: the text stays in paragraphs)
            prop_assert_eq!(tree.matches("chïld").count(), s.text().matches("chïld").count(), "{:?}\nbefore:\n{}\nafter:\n{}\n{}", op, before_text, s.text(), tree);
            prop_assert!(!tree.lines().any(|l| l.trim_start().starts_with("icode")), "{:?}\n{}\n{}", op, s.text(), tree);
            // heading syntax never dangles: every `===` line underlines a setext heading, every
            // `## ` line is an ATX heading (no join drops or strands a heading's syntax)
            let underlines = s.text().lines().filter(|l| l.trim() == "===").count();
            prop_assert_eq!(underlines, tree.matches("h1s").count(), "setext after {:?}\nbefore:\n{}\nafter:\n{}\n{}", op, before_text, s.text(), tree);
            let atx = s.text().lines().filter(|l| l.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ').starts_with("## ")).count();
            prop_assert_eq!(atx, tree.matches("h2 ").count(), "atx after {:?}\nbefore:\n{}\nafter:\n{}\n{}", op, before_text, s.text(), tree);
            // a child list never falls out of its item (C46 unwrap and outdent may move blocks out)
            if !matches!(op, Op::BackspaceStart(_) | Op::DeleteEnd(_) | Op::ShiftTab(_) | Op::Undo) {
                prop_assert!(top_lists(&tree) <= before_top, "child list detached by {:?}\n{}\n{}", op, s.text(), tree);
            }
        }
    }
}

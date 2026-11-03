//! Property tests (ADR 0006 §8.3): random edit/command sequences on both formats — undo all →
//! original bytes and hints; redo all → final bytes and hints; mixed CRLF/LF preserved;
//! Markdown incremental projection == full parse with the same hints; rollback == no-op.

use std::sync::Arc;
use std::time::Duration;

use markise_doc::{DocumentFormat, PatchSet, PlanStep, Selection};
use markise_engine::editor::command::Registry;
use markise_engine::editor::{EditError, EditorSession, HistoryLimits, ManualClock, TxKind};
use markise_md::{Document, MarkdownFormat, ParseOptions};
use markise_testfmt::TestFormat;
use proptest::prelude::*;

#[derive(Clone, Debug)]
enum Op {
    Type(&'static str),
    Backspace,
    Delete,
    Newline,
    Select(prop::sample::Index, prop::sample::Index),
    Pause,
    Command(&'static str),
    Undo,
    Redo,
    Expire,
    FailingTx(prop::sample::Index),
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        6 => prop::sample::select(&["a", "b", " ", "- ", "-", "#", "# ", "```", "*", "é", "日", "\n", "\r\n", "= ", "x y", "> ", "1. ", "[ ] ", "(", ")", "[!NOTE]", "\"", "[x]: y"][..]).prop_map(Op::Type),
        2 => Just(Op::Backspace),
        1 => Just(Op::Delete),
        2 => Just(Op::Newline),
        2 => (any::<prop::sample::Index>(), any::<prop::sample::Index>()).prop_map(|(a, b)| Op::Select(a, b)),
        1 => Just(Op::Pause),
        3 => prop::sample::select(&[
            "block.heading1", "block.heading2", "block.paragraph", "block.toggleTask",
            "format.strong", "format.emph", "format.code", "format.link", "format.clear",
            "edit.indentOrNextCell", "edit.outdentOrPrevCell", "block.headingUp", "block.headingDown",
            "edit.softBreak", "edit.moveBlockUp", "edit.moveBlockDown", "block.quote", "block.bulletList",
            "block.orderedList", "block.taskList", "block.insertRule", "select.word", "select.scope",
            "select.line", "delete.word", "delete.block", "delete.scope", "format.underline",
        ][..]).prop_map(Op::Command),
        2 => Just(Op::Undo),
        1 => Just(Op::Redo),
        1 => Just(Op::Expire),
        1 => any::<prop::sample::Index>().prop_map(Op::FailingTx),
    ]
}

const STARTS: &[&str] = &[
    "",
    "alpha\r\nbeta\ngamma\r\n",
    "# T\n\n- [ ] a\r\n- b\n\npara\r\n",
    "= H\r\n\r\n* i\nplain\n",
    "x\ry\r\n\r\n```\ncode\n```\n",
    "- a\n- b\n  - c\n\n1. x\n2. y\n",
    "> - q\r\n> - r\r\n>\r\n> **s** t\r\n",
    "- [ ] t\n\n  para\n- \n",
];

fn boundary(text: &str, ix: prop::sample::Index) -> usize {
    let mut p = ix.index(text.len() + 1);
    while !text.is_char_boundary(p) {
        p -= 1;
    }
    // never between CR and LF
    if p > 0 && text.as_bytes()[p - 1] == b'\r' && text.as_bytes().get(p) == Some(&b'\n') {
        p -= 1;
    }
    p
}

fn full_md_tree(s: &EditorSession) -> String {
    let mut d = Document::parse(s.text(), ParseOptions::default());
    d.set_hints(s.hints());
    d.set_options(ParseOptions::default());
    markise_md::debug_tree(&d)
}

fn run(format: Arc<dyn DocumentFormat>, start: &str, ops: &[Op], check_md: bool) -> Result<(), TestCaseError> {
    let clock = ManualClock::default();
    let reg = Registry::for_format(format.as_ref());
    let mut s = EditorSession::with(format, start, HistoryLimits::default(), Box::new(clock.clone()));
    for op in ops {
        match op {
            Op::Type(t) => s.insert_text(t).unwrap(),
            Op::Backspace => s.delete_backward().unwrap(),
            Op::Delete => s.delete_forward().unwrap(),
            Op::Newline => s.newline().unwrap(),
            Op::Select(a, b) => {
                let (a, b) = (boundary(s.text(), *a), boundary(s.text(), *b));
                s.set_selection(Selection::range(a, b));
            }
            Op::Pause => clock.advance(Duration::from_secs(2)),
            Op::Command(id) => {
                let _ = reg.execute(&mut s, id, &Default::default());
            }
            Op::Undo => {
                s.undo();
            }
            Op::Redo => {
                s.redo();
            }
            Op::Expire => s.expire_hints().unwrap(),
            Op::FailingTx(ix) => {
                let before = (
                    s.text().to_string(),
                    s.hints().clone(),
                    s.selection().clone(),
                    s.state_id(),
                );
                let at = boundary(s.text(), *ix);
                let r = s.transact("Fails", TxKind::Structural, |tx| {
                    tx.step(PlanStep::patches(PatchSet::insert(at, "zz\n")))?;
                    tx.step(PlanStep::patches(PatchSet::delete(0..at.min(1))))?;
                    Err::<(), _>(EditError::Aborted("test".into()))
                });
                prop_assert!(r.is_err());
                let after = (
                    s.text().to_string(),
                    s.hints().clone(),
                    s.selection().clone(),
                    s.state_id(),
                );
                prop_assert_eq!(before, after, "rollback is a no-op");
            }
        }
        if check_md {
            prop_assert_eq!(
                s.doc().debug_tree(),
                full_md_tree(&s),
                "incremental != full after {:?}\n{:?}",
                op,
                s.text()
            );
        }
    }
    let final_text = s.text().to_string();
    let final_hints = s.hints().clone();
    let pending_redo = s.history().redo_len();
    let mut n = 0;
    while s.undo() {
        n += 1;
    }
    prop_assert_eq!(s.text(), start, "undo all → original bytes (mixed endings included)");
    prop_assert!(s.hints().is_empty());
    prop_assert!(!s.is_dirty(), "the opened state is clean again");
    for _ in 0..n + pending_redo {
        prop_assert!(s.redo());
    }
    for _ in 0..pending_redo {
        prop_assert!(s.undo());
    }
    prop_assert_eq!(s.text(), final_text.as_str(), "redo all → final bytes");
    prop_assert_eq!(s.hints(), &final_hints);
    if check_md {
        prop_assert_eq!(s.doc().debug_tree(), full_md_tree(&s));
    }
    Ok(())
}

/// Case count: `PROPTEST_CASES` when set (local stress runs), else 192.
fn cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(192)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases()))]

    #[test]
    fn markdown_undo_redo_roundtrip(start in prop::sample::select(STARTS), ops in prop::collection::vec(arb_op(), 0..40)) {
        run(Arc::new(MarkdownFormat::default()), start, &ops, true)?;
    }

    #[test]
    fn testfmt_undo_redo_roundtrip(start in prop::sample::select(STARTS), ops in prop::collection::vec(arb_op(), 0..40)) {
        run(Arc::new(TestFormat), start, &ops, false)?;
    }
}

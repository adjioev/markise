//! Editing session: transactions, undo/redo, hints, grouping, rollback, budgets (ADR 0006 §2.6,
//! §4) on Markdown and on the toy test format (seam proof, §1.2).

use std::sync::Arc;
use std::time::Duration;

use markise_doc::{DocPos, DocumentFormat, HintOrigin, ParamValue, Params, PatchSet, PlanStep, Selection, planners};
use markise_engine::editor::command::{CommandError, Registry};
use markise_engine::editor::{EditError, EditorSession, HistoryLimits, ManualClock, SessionEffect, TxKind};
use markise_md::MarkdownFormat;
use markise_testfmt::TestFormat;

fn md_session(text: &str) -> (EditorSession, ManualClock) {
    let clock = ManualClock::default();
    let s = EditorSession::with(
        Arc::new(MarkdownFormat::default()),
        text,
        HistoryLimits::default(),
        Box::new(clock.clone()),
    );
    (s, clock)
}

fn tf_session(text: &str) -> (EditorSession, ManualClock) {
    let clock = ManualClock::default();
    let s = EditorSession::with(
        Arc::new(TestFormat),
        text,
        HistoryLimits::default(),
        Box::new(clock.clone()),
    );
    (s, clock)
}

fn type_str(s: &mut EditorSession, text: &str) {
    for c in text.chars() {
        s.insert_text(c.encode_utf8(&mut [0; 4])).unwrap();
    }
}

fn caret(s: &EditorSession) -> usize {
    s.selection().primary().focus.offset
}

/// One observable state: bytes, projected tree, hints, selection, StateId, dirty.
#[derive(Debug, Clone, PartialEq)]
struct Snap {
    text: String,
    tree: String,
    hints: Vec<(usize, HintOrigin)>,
    sel: Selection,
    state: u64,
    dirty: bool,
}

fn snap(s: &EditorSession) -> Snap {
    Snap {
        text: s.text().to_string(),
        tree: s.doc().debug_tree(),
        hints: s.hints().iter().map(|h| (h.line, h.origin)).collect(),
        sel: s.selection().clone(),
        state: s.state_id().0,
        dirty: s.is_dirty(),
    }
}

// ---- §11 / ADR 0006 §2.6: literal `- ` undo with hint states S0, S1, S2 ----

#[test]
fn spec02_s11_bullet_rule_undo_restores_literal_paragraph() {
    let (mut s, _) = md_session("");
    let empty = snap(&s);
    type_str(&mut s, "-");
    // S0: the state before the triggering keystroke (`-` stays a literal paragraph)
    let s0 = snap(&s);
    assert_eq!(s0.tree, "para \"-\"\n");
    type_str(&mut s, " ");
    let s2 = snap(&s);
    assert_eq!(s2.text, "- ");
    assert_eq!(s2.tree, "ul\n  li\n", "converted to a list");
    assert!(s2.hints.is_empty());
    assert_eq!(s.history().undo_len(), 3, "`-` group, ` ` group (C56), conversion T2");
    assert_eq!(s.history().last().unwrap().kind, TxKind::Auto);

    assert!(s.undo());
    let s1 = snap(&s);
    assert_eq!(s1.text, "- ", "Undo of the conversion changes no bytes");
    assert_eq!(s1.tree, "para \"- \"\n", "…but restores the literal paragraph");
    assert_eq!(s1.hints, vec![(0, HintOrigin::RuleTrigger)]);
    assert_eq!(caret(&s), 2);

    assert!(s.undo());
    assert_eq!(snap(&s), s0, "Undo of the typing restores S0 exactly");

    // repeated cycles S0 ⇄ S1 ⇄ S2
    for _ in 0..3 {
        assert!(s.redo());
        assert_eq!(snap(&s), s1);
        assert!(s.redo());
        assert_eq!(snap(&s), s2);
        assert!(s.undo());
        assert_eq!(snap(&s), s1);
        assert!(s.undo());
        assert_eq!(snap(&s), s0);
    }
    assert_ne!(s1.state, s2.state, "equal bytes, different StateId (hints differ)");
    assert!(s.undo());
    assert_eq!(snap(&s), empty);
}

#[test]
fn spec02_s11_dirty_identity_across_undo_redo_after_save() {
    let (mut s, _) = md_session("");
    assert!(!s.is_dirty(), "opened state is clean");
    type_str(&mut s, "- ");
    let snap_save = s.prepare_save().unwrap();
    assert_eq!(&*snap_save.text, "- ");
    s.mark_saved(&snap_save);
    assert!(!s.is_dirty());
    assert!(s.undo(), "→ S1");
    assert!(
        s.is_dirty(),
        "S1 has the same bytes but a hint: saving would write `\\- `"
    );
    assert!(s.redo());
    assert!(!s.is_dirty(), "back at the saved state");
    // edit, undo back to the saved state: clean again (saved state reachable via undo)
    type_str(&mut s, "x");
    assert!(s.is_dirty());
    assert!(s.undo());
    assert!(!s.is_dirty());
}

#[test]
fn hint_expiry_materialises_as_own_transaction_and_undoes_to_s1() {
    let (mut s, _) = md_session("");
    type_str(&mut s, "- ");
    assert!(s.undo(), "→ S1: literal `- ` paragraph");
    let s1 = snap(&s);
    // caret leaves the block → hints expire (no typing group open: own Auto transaction)
    s.expire_hints().unwrap();
    assert_eq!(s.text(), "\\- ");
    assert_eq!(s.doc().debug_tree(), "para \"\\\\- \"\n");
    assert_eq!(s.history().last().unwrap().label, "Escape Literal Syntax");
    assert!(s.undo());
    assert_eq!(snap(&s), s1, "a further Undo returns exactly to S1");
}

#[test]
fn save_materialises_hints_into_the_open_typing_transaction() {
    let (mut s, _) = md_session("");
    type_str(&mut s, "# T");
    assert_eq!(
        s.doc().debug_tree(),
        "para \"# T\"\n",
        "Enter-only syntax stays a paragraph"
    );
    let before = s.history().undo_len();
    let snap_save = s.prepare_save().unwrap();
    assert_eq!(&*snap_save.text, "\\# T", "the saved bytes parse as what the user saw");
    assert_eq!(s.history().undo_len(), before, "joined the open typing transaction");
    assert!(s.hints().is_empty());
    s.mark_saved(&snap_save);
    assert!(s.undo());
    assert_eq!(
        s.text(),
        "#",
        "one Undo returns to `#` (hinted), not to an escaped text"
    );
    assert_eq!(s.doc().debug_tree(), "para \"#\"\n");
}

#[test]
fn s2_1_heading_is_paragraph_until_enter() {
    let (mut s, _) = md_session("");
    type_str(&mut s, "## Plan");
    assert_eq!(s.doc().debug_tree(), "para \"## Plan\"\n");
    assert_eq!(s.history().undo_len(), 2, "typing groups `##`, ` Plan` (C56)");
    s.newline().unwrap();
    // the heading is created by dropping the hint; the caret goes to the virtual paragraph
    // after it (M2b-1), no blank line is written (M2c)
    assert_eq!(s.text(), "## Plan");
    assert_eq!(s.doc().debug_tree(), "h2 \"Plan\"\n");
    assert_eq!(s.selection().primary().focus, DocPos::with_virt(7, 1));
    assert!(s.undo());
    assert_eq!(
        s.doc().debug_tree(),
        "para \"## Plan\"\n",
        "Undo of Enter → hinted paragraph"
    );
    assert_eq!(caret(&s), 7);
    assert!(s.undo());
    assert_eq!(s.doc().debug_tree(), "para \"##\"\n", "`##` keeps its hint");
    assert!(s.undo());
    assert_eq!(s.text(), "");
    assert!(s.hints().is_empty());
}

#[test]
fn s2_5_code_fence_on_enter_without_swallowing() {
    let (mut s, _) = md_session("a\n\n\n\nb\n");
    s.set_selection(Selection::caret(3));
    type_str(&mut s, "```python");
    assert_eq!(
        s.doc().debug_tree(),
        "para \"a\"\npara \"```python\"\npara \"b\"\n",
        "no swallowing"
    );
    s.newline().unwrap();
    assert_eq!(s.text(), "a\n\n```python\n\n```\n\nb\n");
    assert!(s.doc().debug_tree().contains("fence[python]"));
    assert_eq!(caret(&s), 13, "caret on code line 1");
    assert!(s.undo());
    assert!(s.undo());
    assert_eq!(s.text(), "a\n\n\n\nb\n");
}

// ---- grouping (ADR 0006 §4.2, C13) ----

#[test]
fn c13_typing_groups() {
    let (mut s, clock) = md_session("");
    type_str(&mut s, "hel");
    clock.advance(Duration::from_millis(900));
    type_str(&mut s, "lo");
    assert_eq!(s.history().undo_len(), 1, "no pause > 1 s: one group");
    clock.advance(Duration::from_millis(1500));
    type_str(&mut s, "!");
    assert_eq!(s.history().undo_len(), 2, "pause boundary");
    s.delete_backward().unwrap();
    assert_eq!(s.history().undo_len(), 3, "insert → delete switch");
    s.delete_backward().unwrap();
    assert_eq!(s.history().undo_len(), 3, "consecutive backspaces coalesce");
    s.set_selection(Selection::caret(0));
    type_str(&mut s, "a");
    assert_eq!(s.history().undo_len(), 4, "caret jump");
    s.newline().unwrap();
    assert_eq!(s.history().undo_len(), 5, "Enter is its own transaction");
    type_str(&mut s, "x");
    assert_eq!(s.history().undo_len(), 6, "boundary after Enter");
    let reg = Registry::for_format(&MarkdownFormat::default());
    reg.execute(&mut s, "block.heading1", &Params::new()).unwrap();
    type_str(&mut s, "y");
    assert_eq!(
        s.history().undo_len(),
        8,
        "formatting command: boundaries before and after"
    );
    // undo walks back group by group to the empty document
    while s.undo() {}
    assert_eq!(s.text(), "");
}

#[test]
fn c13_pause_is_configurable() {
    let clock = ManualClock::default();
    let limits = HistoryLimits {
        typing_pause: Duration::from_millis(100),
        ..Default::default()
    };
    let mut s = EditorSession::with(Arc::new(MarkdownFormat::default()), "", limits, Box::new(clock.clone()));
    type_str(&mut s, "a");
    clock.advance(Duration::from_millis(150));
    type_str(&mut s, "b");
    assert_eq!(s.history().undo_len(), 2);
}

#[test]
fn c56_word_boundaries() {
    // Type `foo bar` without pauses. Undo → `foo`. Undo → empty.
    let (mut s, _) = md_session("");
    type_str(&mut s, "foo bar");
    assert!(s.undo());
    assert_eq!(s.text(), "foo");
    assert!(s.undo());
    assert_eq!(s.text(), "");
    // `foo barr`, Backspace, `.` → undo: `foo bar` → `foo barr` → `foo` → empty
    let (mut s, _) = md_session("");
    type_str(&mut s, "foo barr");
    s.delete_backward().unwrap();
    type_str(&mut s, ".");
    assert_eq!(s.text(), "foo bar.");
    for want in ["foo bar", "foo barr", "foo", ""] {
        assert!(s.undo());
        assert_eq!(s.text(), want);
    }
    // `a  b` (two spaces): consecutive whitespace stays in one group. Undo → `a`.
    let (mut s, _) = md_session("");
    type_str(&mut s, "a  b");
    assert!(s.undo());
    assert_eq!(s.text(), "a");
}

#[test]
fn coalesced_typing_is_one_patch_and_byte_exact() {
    let (mut s, _) = md_session("x\r\ny\n");
    s.set_selection(Selection::caret(1));
    type_str(&mut s, "abc");
    let tx = s.history().last().unwrap();
    assert_eq!(tx.steps.len(), 1);
    assert_eq!(tx.steps[0].forward.patches()[0].insert, "abc");
    // newly typed line break takes the document's line ending (CRLF, C23); others untouched
    s.newline().unwrap();
    assert_eq!(s.text(), "xabc\r\n\r\ny\n");
    assert!(s.undo());
    assert!(s.undo());
    assert_eq!(s.text(), "x\r\ny\n");
    assert!(s.redo());
    assert!(s.redo());
    assert_eq!(s.text(), "xabc\r\n\r\ny\n");
}

#[test]
fn backspace_and_delete_coalesce_with_exact_removed_bytes() {
    let (mut s, _) = md_session("héllo wörld\r\nend");
    s.set_selection(Selection::caret(6)); // after "héllo"
    for _ in 0..3 {
        s.delete_backward().unwrap();
    }
    assert_eq!(s.text(), "hé wörld\r\nend");
    let tx = s.history().last().unwrap();
    assert_eq!(tx.steps.len(), 1);
    assert_eq!(&*tx.steps[0].removed[0], "llo");
    s.set_selection(Selection::caret(10)); // before CRLF
    s.delete_forward().unwrap();
    s.delete_forward().unwrap();
    assert_eq!(s.text(), "hé wörldnd", "CRLF deleted as one");
    assert!(s.undo());
    assert!(s.undo());
    assert_eq!(s.text(), "héllo wörld\r\nend");
}

// ---- immutable historical selections (ADR 0006 §2.2) ----

#[test]
fn selection_inside_later_deleted_text_is_restored_exactly() {
    let (mut s, _) = md_session("alpha beta gamma delta\n");
    // T1: with "beta" selected, type over it
    let sel1 = Selection::range(6, 10);
    s.set_selection(sel1.clone());
    type_str(&mut s, "BETA");
    let sel1_after = s.selection().clone();
    // T2: delete a range containing T1's selection interior
    let sel2 = Selection::range(2, 16);
    s.set_selection(sel2.clone());
    s.delete_backward().unwrap();
    assert_eq!(s.text(), "al delta\n");
    assert!(s.undo());
    assert_eq!(s.selection(), &sel2);
    assert!(s.undo());
    assert_eq!(
        s.selection(),
        &sel1,
        "T1's sel_before restored verbatim, not transformed"
    );
    assert_eq!(s.text(), "alpha beta gamma delta\n");
    assert!(s.redo());
    assert_eq!(s.selection(), &sel1_after);
}

// ---- atomic multi-step commands, journal, rollback (ADR 0006 §4.4) ----

#[test]
fn failed_multi_step_command_rolls_back_marks_selection_hints_and_state() {
    let (mut s, _) = md_session("one two three four\n\n");
    s.set_selection(Selection::caret(20));
    type_str(&mut s, "- "); // hints + history in place
    s.undo(); // S1: a hint is live
    assert!(!s.hints().is_empty());
    let undo_len = s.history().undo_len();
    let mark = s.add_mark(DocPos::new(9)); // inside "two three" region below
    let sel = Selection::range(6, 12);
    s.set_selection(sel.clone());
    let before = snap(&s);
    let counter = s.revision().counter;
    s.take_effects();
    let r = s.transact("Two Steps", TxKind::Structural, |tx| {
        // step 1 deletes the mark's surroundings (the mark collapses)
        tx.step(PlanStep::patches(PatchSet::delete(6..16)))?;
        assert_eq!(tx.mark(mark).unwrap().offset, 6);
        tx.defer(SessionEffect::Custom("autosave".into()));
        // step 2 depends on step 1 and fails (out of bounds)
        tx.step(PlanStep::patches(PatchSet::single(500..501, "x")))?;
        Ok(())
    });
    assert!(matches!(r, Err(EditError::Patch(_))));
    assert_eq!(snap(&s), before, "bytes, tree, hints, selection, StateId restored");
    assert_eq!(
        s.mark(mark),
        Some(DocPos::new(9)),
        "collapsed mark restored from the journal"
    );
    assert!(s.revision().counter > counter, "counter bumped, never reused");
    assert!(s.take_effects().is_empty(), "deferred effects discarded");
    assert_eq!(s.history().undo_len(), undo_len, "nothing recorded");
}

#[test]
fn overlapping_sequential_steps_undo_redo_byte_exact() {
    let (mut s, _) = md_session("abc\r\ndef\n");
    s.transact("Overlap", TxKind::Structural, |tx| {
        tx.step(PlanStep::patches(PatchSet::insert(3, "XYZ")))?;
        // step 2 edits step 1's insert and crosses the CRLF
        tx.step(PlanStep::patches(PatchSet::single(4..8, "q\nr")))?;
        tx.defer(SessionEffect::Custom("done".into()));
        Ok(())
    })
    .unwrap();
    assert_eq!(s.text(), "abcXq\r\nrdef\n");
    assert!(s.take_effects().contains(&SessionEffect::Custom("done".into())));
    assert!(s.undo());
    assert_eq!(s.text(), "abc\r\ndef\n");
    assert!(s.redo());
    assert_eq!(s.text(), "abcXq\r\nrdef\n");
}

// ---- budgets (ADR 0006 §4.5) ----

#[test]
fn history_budget_evicts_oldest_first() {
    let clock = ManualClock::default();
    let limits = HistoryLimits {
        max_steps: 5,
        ..Default::default()
    };
    let mut s = EditorSession::with(Arc::new(MarkdownFormat::default()), "", limits, Box::new(clock.clone()));
    for i in 0..8 {
        type_str(&mut s, &format!("w{i},"));
        clock.advance(Duration::from_secs(2));
    }
    assert_eq!(s.history().undo_len(), 5);
    assert_eq!(s.history().evicted(), 3);
    while s.undo() {}
    assert_eq!(s.text(), "w0,w1,w2,", "the three oldest transactions are gone");

    // byte budget: the newest transaction is always kept, even when it alone exceeds it
    let limits = HistoryLimits {
        max_bytes: 4096,
        ..Default::default()
    };
    let mut s = EditorSession::with(Arc::new(MarkdownFormat::default()), "", limits, Box::new(clock.clone()));
    type_str(&mut s, "small");
    s.close_typing();
    let big = "x".repeat(10_000);
    s.insert_text(&big).unwrap();
    assert_eq!(s.history().undo_len(), 1, "older transaction evicted by bytes");
    assert!(s.history().bytes() > 4096);
    assert!(s.undo());
    assert_eq!(s.text(), "small");
}

#[test]
fn history_budget_default_is_500_and_32mb() {
    let l = HistoryLimits::default();
    assert_eq!(
        (l.max_steps, l.max_bytes, l.typing_pause),
        (500, 32 << 20, Duration::from_secs(1))
    );
}

// ---- registry (ADR 0006 §3.1) ----

#[test]
fn registry_assembles_engine_and_format_commands() {
    let reg = Registry::for_format(&MarkdownFormat::default());
    let ids: Vec<&str> = reg.describe().map(|d| d.id).collect();
    for id in [
        "edit.undo",
        "edit.insertText",
        "edit.newline",
        "block.heading2",
        "block.toggleTask",
    ] {
        assert!(ids.contains(&id), "{id}");
    }
    let (mut s, _) = md_session("- [ ] task\n");
    s.set_selection(Selection::caret(8));
    assert!(reg.enabled(&s, "block.toggleTask").is_ok());
    assert!(reg.enabled(&s, "edit.undo").is_err(), "nothing to undo");
    reg.execute(&mut s, "block.toggleTask", &Params::new()).unwrap();
    assert_eq!(s.text(), "- [x] task\n");
    assert_eq!(s.history().last().unwrap().label, "Toggle Task Status");
    reg.execute(&mut s, "edit.insertText", &Params::text("text", "!"))
        .unwrap();
    assert_eq!(
        reg.execute(
            &mut s,
            "edit.insertText",
            &Params::new().with("text", ParamValue::Int(3))
        ),
        Err(CommandError::Params("parameter `text`: expected Text".into()))
    );
    assert!(matches!(
        reg.execute(&mut s, "nope", &Params::new()),
        Err(CommandError::Unknown(_))
    ));
    reg.execute(&mut s, "edit.undo", &Params::new()).unwrap();
    reg.execute(&mut s, "edit.undo", &Params::new()).unwrap();
    assert_eq!(s.text(), "- [ ] task\n");
    // a format without a planner for a well-known id: the command is disabled
    s.set_selection(Selection::caret(0));
    let plain = Registry::new();
    assert!(plain.get("block.heading1").is_none());
}

// ---- seam: the same engine on the toy format (ADR 0006 §1.2) ----

#[test]
fn testfmt_seam_rules_undo_hints_and_commands() {
    let (mut s, _) = tf_session("");
    type_str(&mut s, "*");
    let s0 = snap(&s);
    type_str(&mut s, " ");
    assert_eq!(s.doc().debug_tree(), "item \"\"\n");
    assert!(s.undo());
    assert_eq!(s.doc().debug_tree(), "para \"* \"\n", "literal trigger restored");
    assert_eq!(s.hints().iter().next().unwrap().origin, HintOrigin::RuleTrigger);
    assert!(s.undo());
    assert_eq!(snap(&s), s0);
    assert!(s.redo());
    assert!(s.redo());
    assert_eq!(s.doc().debug_tree(), "item \"\"\n");

    // Enter rule `= x`
    let (mut s, _) = tf_session("");
    type_str(&mut s, "= Title");
    assert_eq!(s.doc().debug_tree(), "para \"= Title\"\n");
    s.newline().unwrap();
    assert_eq!(s.doc().debug_tree(), "h1 \"Title\"\n");
    assert!(s.undo());
    assert_eq!(s.doc().debug_tree(), "para \"= Title\"\n");
    let snap_save = s.prepare_save().unwrap();
    assert_eq!(&*snap_save.text, "\\= Title", "toy materialisation");

    // format command through the registry
    let reg = Registry::for_format(&TestFormat);
    let (mut s, _) = tf_session("plain\r\n");
    reg.execute(&mut s, "block.heading1", &Params::new()).unwrap();
    assert_eq!(s.text(), "= plain\r\n");
    assert_eq!(s.doc().debug_tree(), "h1 \"plain\"\n");
    assert!(s.undo());
    assert_eq!(s.text(), "plain\r\n");
    // a planner the toy format does not implement is disabled
    assert!(!s.doc().can_plan(planners::TOGGLE_TASK, &s.request(Params::new())));
    assert_eq!(TestFormat.id(), "testfmt");
}

#[test]
fn effects_report_monotonic_counters() {
    let (mut s, _) = md_session("");
    type_str(&mut s, "ab");
    s.undo();
    s.redo();
    let counters: Vec<u64> = s
        .take_effects()
        .into_iter()
        .filter_map(|e| match e {
            SessionEffect::TextChanged { counter } => Some(counter),
            _ => None,
        })
        .collect();
    assert_eq!(counters.len(), 4);
    assert!(counters.windows(2).all(|w| w[0] < w[1]));
}

// ---- Codex review of PR #9: rule eligibility (P1, CRLF), atomic hints ----

#[test]
fn editing_existing_blocks_at_document_start_adds_no_hint() {
    // P1: typing into an existing heading must not demote it
    let (mut s, _) = md_session("# Title");
    s.set_selection(Selection::caret(7));
    type_str(&mut s, "!");
    assert!(s.hints().is_empty());
    assert_eq!(s.doc().debug_tree(), "h1 \"Title!\"\n");
    assert_eq!(&*s.prepare_save().unwrap().text, "# Title!");
    // an existing list item, a quote and a code fence at the start
    for (src, at) in [("- item", 6), ("> quote", 7), ("```\ncode\n```", 8), ("1. one", 6)] {
        let (mut s, _) = md_session(src);
        let tree = s.doc().debug_tree();
        s.set_selection(Selection::caret(at));
        type_str(&mut s, "x");
        assert!(s.hints().is_empty(), "{src:?}");
        let kind = |t: &str| {
            t.lines()
                .next()
                .unwrap_or("")
                .split(' ')
                .next()
                .unwrap_or("")
                .to_string()
        };
        assert_eq!(kind(&s.doc().debug_tree()), kind(&tree), "{src:?}");
    }
}

#[test]
fn continuing_a_freshly_converted_bullet_keeps_the_list() {
    let (mut s, _) = md_session("");
    type_str(&mut s, "- item");
    assert!(s.hints().is_empty());
    assert_eq!(s.doc().debug_tree(), "ul\n  li\n    para \"item\"\n");
    assert_eq!(&*s.prepare_save().unwrap().text, "- item");
    // also below other content
    let (mut s, _) = md_session("intro\n\n");
    s.set_selection(Selection::caret(7));
    type_str(&mut s, "- a");
    assert_eq!(s.doc().debug_tree(), "para \"intro\"\nul\n  li\n    para \"a\"\n");
    assert!(s.hints().is_empty());
}

#[test]
fn input_rules_do_not_depend_on_line_endings() {
    for eol in ["\n", "\r\n", "\r"] {
        // previous line non-blank: no rule (soft-broken line, C9); the line keeps an
        // EnterPending hint so `foo⏎- ` stays paragraph text (M2c, ADR 0006 §15)
        let src = format!("foo{eol}");
        let (mut s, _) = md_session(&src);
        s.set_selection(Selection::caret(src.len()));
        type_str(&mut s, "- ");
        assert!(
            s.hints().iter().all(|h| h.origin == HintOrigin::EnterPending),
            "{eol:?}"
        );
        assert!(s.doc().debug_tree().starts_with("para"), "{eol:?}");
        assert_eq!(s.history().undo_len(), 2, "{eol:?}: `-`, ` ` typing groups only");
        assert!(s.history().undo_stack().all(|t| t.kind != TxKind::Auto), "{eol:?}");
        // previous line blank: the rule fires the same way for every ending
        let src = format!("foo{eol}{eol}");
        let (mut s, _) = md_session(&src);
        s.set_selection(Selection::caret(src.len()));
        type_str(&mut s, "- ");
        assert_eq!(s.history().last().unwrap().kind, TxKind::Auto, "{eol:?}");
        assert!(
            s.doc().debug_tree().ends_with("ul\n  li\n"),
            "{eol:?}: {}",
            s.doc().debug_tree()
        );
    }
}

#[test]
fn testfmt_rule_eligibility_matches() {
    for eol in ["\n", "\r\n"] {
        let src = format!("text{eol}");
        let (mut s, _) = tf_session(&src);
        s.set_selection(Selection::caret(src.len()));
        type_str(&mut s, "* ");
        assert!(s.history().undo_stack().all(|t| t.kind != TxKind::Auto), "{eol:?}");
    }
    let (mut s, _) = tf_session("= H");
    s.set_selection(Selection::caret(3));
    type_str(&mut s, " x");
    assert!(s.hints().is_empty(), "existing heading untouched");
    assert_eq!(s.doc().debug_tree(), "h1 \"H x\"\n");
}

#[test]
fn completing_an_unclosed_fence_never_parses_the_suffix_as_code() {
    // The keystroke that completes ``` adds the hint in the same step; text and hint are applied
    // atomically, so the result (and every intermediate parse) keeps the rest as paragraphs.
    let mut src = String::from("a\n\n``\n\n");
    for i in 0..2000 {
        src.push_str(&format!("para {i}\n\n"));
    }
    let (mut s, _) = md_session(&src);
    s.set_selection(Selection::caret(5));
    type_str(&mut s, "`");
    assert_eq!(s.hints().iter().next().map(|h| h.line), Some(3));
    assert!(
        s.doc()
            .debug_tree()
            .starts_with("para \"a\"\npara \"```\"\npara \"para 0\"")
    );
    assert!(s.undo());
    assert!(s.redo());
    assert!(s.doc().debug_tree().starts_with("para \"a\"\npara \"```\"\n"));
}

// ---- saved states stay undo boundaries (Codex review round 3 of PR #21) ----

mod saved_boundaries {
    use super::*;
    use markise_doc::{FormatDoc, FormatOptions};
    use proptest::prelude::*;

    #[derive(Clone, Debug)]
    enum Op {
        /// Install a new text as a joinable "Source Edit".
        Install(u8),
        /// The current state is saved.
        Save,
        /// A save of text the session does not have yet finished (no state is on disk).
        ClearSaved,
        /// A save starts (`prepare_save`); it completes or fails later.
        SaveStart,
        /// The oldest running save completes.
        SaveDone,
        /// The oldest running save fails.
        SaveFail,
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            4 => any::<u8>().prop_map(Op::Install),
            2 => Just(Op::Save),
            1 => Just(Op::ClearSaved),
            2 => Just(Op::SaveStart),
            2 => Just(Op::SaveDone),
            1 => Just(Op::SaveFail),
        ]
    }

    fn install(s: &mut EditorSession, text: &str) {
        let doc: Arc<dyn FormatDoc> = Arc::from(s.format().parse(text.into(), &FormatOptions::default()));
        s.replace_doc(doc, "Source Edit", TxKind::Bulk, true).unwrap();
    }

    proptest! {
        /// Whatever the order of installs, saves (immediate, delayed, failed) and
        /// save-before-install completions, undoing to the start passes through every text that
        /// was ever saved — including saves that complete after later edits.
        #[test]
        fn every_saved_text_is_reachable_by_undo(ops in proptest::collection::vec(op(), 1..24)) {
            let (mut s, _) = md_session("t0\n");
            let mut saved = vec![s.text().to_string()];
            let mut n = 0u32;
            let mut running: std::collections::VecDeque<(markise_engine::editor::SaveSnapshot, String)> =
                Default::default();
            for o in &ops {
                match o {
                    Op::Install(k) => {
                        n += 1;
                        install(&mut s, &format!("t{n} {k}\n"));
                    }
                    Op::Save => {
                        s.mark_current_saved();
                        saved.push(s.text().to_string());
                    }
                    Op::ClearSaved => s.clear_saved_state(),
                    Op::SaveStart => {
                        let snap = s.prepare_save().unwrap();
                        running.push_back((snap, s.text().to_string()));
                    }
                    Op::SaveDone => {
                        if let Some((snap, text)) = running.pop_front() {
                            s.mark_saved(&snap);
                            saved.push(text);
                        }
                    }
                    Op::SaveFail => {
                        running.pop_front();
                    }
                }
            }
            let mut seen = vec![s.text().to_string()];
            while s.undo() {
                seen.push(s.text().to_string());
            }
            for t in &saved {
                prop_assert!(seen.contains(t), "saved {t:?} not reachable; seen {seen:?}, ops {ops:?}");
            }
        }
    }

    #[test]
    fn a_running_save_protects_its_state_from_joining() {
        // A → B, save B starts, C is installed before the save completes (join requested)
        let (mut s, _) = md_session("A\n");
        install(&mut s, "B\n");
        let snap = s.prepare_save().unwrap();
        install(&mut s, "C\n");
        s.mark_saved(&snap);
        assert!(s.is_dirty(), "C is not on disk");
        assert!(s.undo());
        assert_eq!(s.text(), "B\n", "B is still a boundary");
        assert!(!s.is_dirty());
    }

    #[test]
    fn earlier_saved_boundary_survives_a_later_save_before_install() {
        // A → B (installed, saved) → C saved before its install (no state on disk) → C
        // installed with join: must not merge into B's transaction
        let (mut s, _) = md_session("A\n");
        install(&mut s, "B\n");
        s.mark_current_saved();
        s.clear_saved_state();
        install(&mut s, "C\n");
        s.mark_current_saved();
        assert!(!s.is_dirty());
        assert!(s.undo());
        assert_eq!(s.text(), "B\n", "B stays reachable");
        assert!(s.is_dirty(), "C is on disk now");
        assert!(s.undo());
        assert_eq!(s.text(), "A\n");
    }
}

/// `set_selection` from an API caller clamps offsets past the end or inside a UTF-8
/// sequence (#20); the editing keys after it must not panic.
#[test]
fn set_selection_clamps_out_of_range_offsets() {
    let (mut s, _) = md_session("- a\n- b\n");
    s.set_selection(Selection::range(3, 999));
    let p = s.selection().primary();
    assert_eq!((p.anchor.offset, p.focus.offset), (3, 8));
    s.set_selection(Selection::caret(999));
    assert_eq!(s.selection().primary().focus.offset, 8);
    s.newline().unwrap();
    s.delete_backward().unwrap();
    s.insert_text("x").unwrap();
    let (mut s, _) = md_session("мир\n");
    s.set_selection(Selection::caret(1));
    assert_eq!(s.selection().primary().focus.offset, 0);
    s.insert_text("x").unwrap();
    assert_eq!(s.text(), "xмир\n");
}

/// Select All (first to last caret stop) replaced or deleted takes the hidden block syntax
/// at the document's edges with it (M2b-3; #20 f03); a partial selection does not.
#[test]
fn whole_document_selection_replaces_every_byte() {
    let src = "# H\n\n- a\n- b\n\n> q\n";
    let (mut s, _) = md_session(src);
    s.set_selection(Selection::range(2, src.len() - 1));
    s.insert_text("new").unwrap();
    assert_eq!(s.text(), "new");
    assert!(s.undo());
    assert_eq!(s.text(), src);
    s.set_selection(Selection::range(src.len() - 1, 2));
    s.delete_backward().unwrap();
    assert_eq!(s.text(), "");
    let (mut s, _) = md_session(src);
    s.set_selection(Selection::range(3, src.len() - 1));
    s.insert_text("x").unwrap();
    assert_eq!(s.text(), "# Hx\n");
}

/// Spec 02 §5.3 rule 2: Tab / Shift+Tab in a table move between cells (skipping the
/// delimiter row) selecting the content; never a literal tab (#20). Adding a row at the last
/// cell is M2d: until then a no-op.
#[test]
fn tab_in_a_table_moves_between_cells() {
    let src = "| a | b |\n|---|---|\n| x | y |\n";
    let (mut s, _) = md_session(src);
    let sel = |s: &EditorSession| {
        let p = s.selection().primary();
        (p.anchor.offset, p.focus.offset)
    };
    s.set_selection(Selection::caret(3));
    s.tab(false).unwrap();
    assert_eq!(sel(&s), (6, 7));
    s.tab(false).unwrap();
    assert_eq!(sel(&s), (22, 23));
    s.tab(true).unwrap();
    assert_eq!(sel(&s), (6, 7));
    s.set_selection(Selection::caret(27));
    s.tab(false).unwrap();
    assert_eq!(sel(&s), (27, 27));
    s.set_selection(Selection::caret(2));
    s.tab(true).unwrap();
    assert_eq!(s.text(), src);
}

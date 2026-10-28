//! M2b (ADR 0006 §8.1): caret, selection, reveal, plain typing, paragraph split, virtual empty
//! paragraph, undo grouping, dirty state and reload on the headless editing surface
//! (`EditorView` = session + layout view, CPU test fonts).

use std::sync::Arc;
use std::time::Duration;

use epaint::text::Fonts;
use markise_doc::{DocPos, DocumentFormat, FormatDoc, Params, Selection};
use markise_engine::editor::{EditorSession, EditorView, HistoryLimits, ManualClock, TxKind};
use markise_layout::block::NoImages;
use markise_layout::{DocModel, View, testing};
use markise_md::MarkdownFormat;
use markise_theme::Theme;

struct T {
    ed: EditorView,
    fonts: Fonts,
    clock: ManualClock,
}

impl T {
    fn new(text: &str) -> T {
        let clock = ManualClock::default();
        let format: Arc<dyn DocumentFormat> = Arc::new(MarkdownFormat::default());
        let session = EditorSession::with(format, text, HistoryLimits::default(), Box::new(clock.clone()));
        let model = DocModel::build(session.doc(), None);
        let view = View::new(session.doc_arc().clone(), model, Theme::light(), 600.0, 1.0);
        let mut t = T {
            ed: EditorView::new(session, view, None),
            fonts: testing::fonts(),
            clock,
        };
        t.frame();
        t
    }

    fn frame(&mut self) {
        self.ed.refresh_focus();
        let mut f = self.fonts.with_pixels_per_point(1.0);
        self.ed.view.frame(800.0, 400.0, &mut f, &NoImages);
    }

    fn run(&mut self, id: &str) {
        self.run_p(id, Params::new());
    }

    fn run_p(&mut self, id: &str, p: Params) {
        {
            let mut f = self.fonts.with_pixels_per_point(1.0);
            let r = self.ed.execute(id, &p, &mut f, &NoImages, 800.0);
            if let Err(e) = r {
                assert!(
                    matches!(e, markise_engine::editor::command::CommandError::Disabled(_)),
                    "{id}: {e}"
                );
            }
        }
        self.frame();
    }

    fn typ(&mut self, s: &str) {
        for c in s.chars() {
            self.run_p("edit.insertText", Params::text("text", c.to_string()));
        }
    }

    fn text(&self) -> &str {
        self.ed.session.text()
    }

    fn caret(&self) -> DocPos {
        self.ed.session.selection().primary().focus
    }

    fn set(&mut self, off: usize) {
        self.ed.set_caret(DocPos::new(off), false);
        self.frame();
    }

    fn revealed(&self) -> bool {
        self.ed.view.focus().is_some_and(|(_, f)| !f.reveal.is_empty())
    }

    /// The incremental model equals a full rebuild.
    fn check_model(&self) {
        let full = DocModel::build(self.ed.session.doc(), None);
        self.ed
            .view
            .model()
            .diff(&full)
            .expect("incremental model == full build");
    }
}

#[test]
fn s3_1_enter_inside_a_styled_span_keeps_the_style_on_both_halves() {
    let src = "This is **very bold** text";
    let mut t = T::new(src);
    let at = src.find("very").unwrap() + 4;
    t.set(at);
    t.run("edit.newline");
    assert_eq!(t.text(), "This is **very**\n\n**bold** text");
    assert_eq!(t.ed.session.doc().debug_tree().matches("para").count(), 2);
    // the caret starts the second paragraph (before its reopened marker)
    assert_eq!(t.caret().offset, "This is **very**\n\n".len());
    t.check_model();
    t.run("edit.undo");
    assert_eq!(t.text(), src);
    assert_eq!(t.caret(), DocPos::new(at), "sel_before restored");
    t.run("edit.redo");
    assert_eq!(t.text(), "This is **very**\n\n**bold** text");
}

#[test]
fn enter_splits_paragraphs_in_quotes_and_list_items() {
    let mut t = T::new("> ab cd\n");
    t.set(4);
    t.run("edit.newline");
    assert_eq!(t.text(), "> ab\n>\n> cd\n");
    let mut t = T::new("- one two\n- three\n");
    t.set(5);
    t.run("edit.newline");
    assert_eq!(t.text(), "- one\n- two\n- three\n");
    let mut t = T::new("1. one\n");
    t.set(6);
    t.run("edit.newline");
    assert_eq!(t.text(), "1. one\n2. \n");
    t.check_model();
}

#[test]
fn spec02_s3_2_markers_reveal_at_the_caret_and_arrows_step_through_them() {
    let src = "a **b** c";
    let mut t = T::new(src);
    t.set(0);
    assert!(!t.revealed());
    let mut seen = vec![0];
    for _ in 0..12 {
        t.run("caret.right");
        seen.push(t.caret().offset);
    }
    assert_eq!(seen, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 9, 9, 9]);
    // revealed exactly while the caret is inside or touching the span (2..=7)
    t.set(5);
    assert!(t.revealed());
    t.set(7);
    assert!(t.revealed());
    t.set(8);
    assert!(!t.revealed());
    // and back to the left
    t.set(9);
    let mut back = vec![9];
    for _ in 0..10 {
        t.run("caret.left");
        back.push(t.caret().offset);
    }
    assert_eq!(back, [9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 0]);
}

#[test]
fn collapsed_markers_are_not_caret_stops() {
    // the caret never lands inside hidden markers when they are collapsed: from outside the
    // span, Right moves to the boundary (touching: revealed) and then through the markers
    let mut t = T::new("xy **b** z\n\nnext");
    t.set(10); // end of the first paragraph
    t.run("caret.right");
    assert_eq!(t.caret().offset, 12, "into the next paragraph in one step");
    t.run("caret.left");
    assert_eq!(t.caret().offset, 10);
}

#[test]
fn heading_markers_are_skipped_and_backspace_converts_the_heading() {
    let mut t = T::new("para\n\n## Notes");
    t.set(4);
    t.run("caret.right");
    assert_eq!(t.caret().offset, 9, "the hidden `## ` is not a stop");
    // S-5.3: Backspace at the start of a heading's text → paragraph "Notes"
    t.run("edit.deleteBackward");
    assert_eq!(t.text(), "para\n\nNotes");
    assert!(t.ed.session.doc().debug_tree().contains("para"));
    // spec 02 §5.2 rule 5: a second Backspace merges with the previous paragraph
    t.run("edit.deleteBackward");
    assert_eq!(t.text(), "paraNotes");
    assert_eq!(t.caret().offset, 4);
    t.check_model();
    t.run("edit.undo");
    t.run("edit.undo");
    assert_eq!(t.text(), "para\n\n## Notes");
}

#[test]
fn delete_at_the_end_of_a_paragraph_merges_the_next_one() {
    let mut t = T::new("one\n\ntwo");
    t.set(3);
    t.run("edit.deleteForward");
    assert_eq!(t.text(), "onetwo");
    let mut t = T::new("one\n\n```\ncode\n```\n");
    t.set(3);
    t.run("edit.deleteForward");
    assert_eq!(t.text(), "one\n\n```\ncode\n```\n", "no merge into code (C48)");
}

#[test]
fn virtual_empty_paragraph_after_enter_at_the_end() {
    let mut t = T::new("foo\n\nbar");
    t.set(3);
    let undo = t.ed.session.history().undo_len();
    t.run("edit.newline");
    assert_eq!(t.text(), "foo\n\nbar", "no source change");
    assert_eq!(t.caret(), DocPos::with_virt(3, 1));
    assert_eq!(t.ed.session.history().undo_len(), undo, "no history entry");
    assert!(
        t.ed.view.focus().is_some_and(|(_, f)| f.virtual_after),
        "the item shows the empty paragraph"
    );
    // typing materialises it
    t.typ("x");
    assert_eq!(t.text(), "foo\n\nx\n\nbar");
    assert_eq!(t.caret().offset, 6);
    t.check_model();
    t.run("edit.undo");
    assert_eq!(t.text(), "foo\n\nbar");
    assert_eq!(
        t.caret(),
        DocPos::with_virt(3, 1),
        "the virtual caret comes back verbatim"
    );
    // Backspace / moving away drop it without a source change
    t.run("edit.deleteBackward");
    assert_eq!(t.caret(), DocPos::new(3));
    assert_eq!(t.text(), "foo\n\nbar");
}

#[test]
fn enter_at_the_start_of_a_paragraph_opens_an_empty_one_above() {
    let mut t = T::new("one\n\ntwo");
    t.set(5);
    t.run("edit.newline");
    assert_eq!(t.caret(), DocPos::with_virt(3, 1));
    t.typ("z");
    assert_eq!(t.text(), "one\n\nz\n\ntwo");
}

#[test]
fn vertical_line_and_document_moves() {
    let mut t = T::new("# Title\n\nfirst line\n\nsecond line\n\n---\n\nthird");
    t.set(9); // start of "first line"
    t.run("caret.down");
    assert_eq!(t.caret().offset, 21, "start of `second line`");
    t.run("caret.down");
    assert_eq!(t.caret().offset, 39, "the rule has no stops: `third`");
    t.run("caret.up");
    assert_eq!(t.caret().offset, 21);
    t.run("caret.lineEnd");
    assert_eq!(t.caret().offset, 32);
    t.run("caret.lineStart");
    assert_eq!(t.caret().offset, 21);
    t.run("nav.bottom");
    assert_eq!(t.caret().offset, t.text().len());
    t.run("nav.top");
    assert_eq!(t.caret().offset, 2, "first stop: after the hidden `# `");
    t.run("select.down");
    let p = t.ed.session.selection().primary();
    assert_eq!((p.anchor.offset, p.focus.offset), (2, 9));
    t.run("select.all");
    let p = t.ed.session.selection().primary();
    assert_eq!((p.anchor.offset, p.focus.offset), (2, t.text().len()));
    // typing over Select All replaces the whole document, hidden `# ` included (#20 f03)
    t.typ("Z");
    assert_eq!(t.text(), "Z");
}

#[test]
fn soft_wrapped_rows_have_their_own_line_start_and_end() {
    let long = "word ".repeat(60);
    let mut t = T::new(long.trim_end());
    t.set(0);
    t.run("caret.lineEnd");
    let end1 = t.caret();
    assert!(
        end1.offset > 0 && end1.offset < long.trim_end().len(),
        "end of the first row"
    );
    t.run("caret.down");
    t.run("caret.lineStart");
    let start2 = t.caret();
    assert!(start2.offset >= end1.offset);
    assert!(start2.offset <= end1.offset + 1);
}

#[test]
fn word_moves_and_word_deletion() {
    let mut t = T::new("alpha beta gamma");
    t.set(16);
    t.run("caret.wordLeft");
    assert_eq!(t.caret().offset, 11);
    t.run("caret.wordLeft");
    assert_eq!(t.caret().offset, 6);
    t.run("caret.wordRight");
    assert_eq!(t.caret().offset, 10);
    t.run("edit.deleteWordBackward");
    assert_eq!(t.text(), "alpha  gamma");
}

#[test]
fn c13_pause_starts_a_new_undo_group_with_an_injected_clock() {
    let mut t = T::new("");
    t.typ("abc");
    t.clock.advance(Duration::from_millis(1500));
    t.typ("def");
    assert_eq!(t.text(), "abcdef");
    t.run("edit.undo");
    assert_eq!(t.text(), "abc");
    t.run("edit.undo");
    assert_eq!(t.text(), "");
    // without a pause (and no whitespace) it is one group
    t.run("edit.redo");
    t.run("edit.redo");
    let mut t = T::new("");
    t.typ("abc");
    t.clock.advance(Duration::from_millis(500));
    t.typ("def");
    t.run("edit.undo");
    assert_eq!(t.text(), "");
}

#[test]
fn caret_jump_closes_the_typing_group() {
    let mut t = T::new("one two");
    t.set(3);
    t.typ("X");
    t.set(8);
    t.typ("Y");
    assert_eq!(t.text(), "oneX twoY");
    t.run("edit.undo");
    assert_eq!(t.text(), "oneX two");
}

#[test]
fn undo_returns_to_the_saved_state_and_reads_clean() {
    let mut t = T::new("text");
    t.set(4);
    t.typ("a");
    let snap = t.ed.edit(|s| s.prepare_save().unwrap());
    t.ed.session.mark_saved(&snap);
    assert!(!t.ed.session.is_dirty());
    t.typ("b");
    assert!(t.ed.session.is_dirty());
    t.run("edit.undo");
    assert_eq!(t.text(), "texta");
    assert!(!t.ed.session.is_dirty(), "undo to the saved content is clean");
    t.run("edit.undo");
    assert!(t.ed.session.is_dirty());
    t.run("edit.redo");
    assert!(!t.ed.session.is_dirty());
}

#[test]
fn reload_from_disk_is_one_undoable_transaction() {
    let mut t = T::new("line one\n\nline two\n");
    t.set(4);
    let format = t.ed.session.format().clone();
    let disk: Arc<dyn FormatDoc> =
        Arc::from(format.parse("line ONE\n\nline two\n\nadded\n".into(), &Default::default()));
    t.ed.release();
    t.ed.session
        .replace_doc(disk, "Reload from Disk", TxKind::Bulk, false)
        .unwrap();
    t.ed.session.mark_current_saved();
    t.ed.sync();
    t.frame();
    assert_eq!(t.text(), "line ONE\n\nline two\n\nadded\n");
    assert!(!t.ed.session.is_dirty());
    assert_eq!(t.ed.session.history().last().unwrap().label, "Reload from Disk");
    t.check_model();
    t.run("edit.undo");
    assert_eq!(t.text(), "line one\n\nline two\n");
    assert!(t.ed.session.is_dirty(), "the pre-reload text is not what is on disk");
    t.check_model();
    t.run("edit.redo");
    assert!(!t.ed.session.is_dirty());
}

#[test]
fn typing_edits_the_shared_document_in_place() {
    let mut t = T::new(&"Some paragraph text.\n\n".repeat(200));
    t.set(5);
    t.typ("hello world");
    t.run("edit.deleteBackward");
    t.run("edit.newline");
    t.run("edit.undo");
    assert_eq!(
        t.ed.session.copies, 0,
        "the view released the document before each edit"
    );
    t.check_model();
}

#[test]
fn click_hit_testing_maps_points_to_source_offsets() {
    let mut t = T::new("first paragraph\n\nsecond **bold** paragraph");
    let vis = {
        let mut f = t.fonts.with_pixels_per_point(1.0);
        t.ed.view.frame(800.0, 400.0, &mut f, &NoImages)
    };
    let second = &vis[1];
    let r = second.layout.prims.iter().find_map(|p| match p {
        markise_layout::Prim::Text { pos, text, .. } => Some(text.rect().translate(pos.to_vec2())),
        _ => None,
    });
    let r = r.unwrap();
    let len = t.ed.session.text().len();
    let mut f = t.fonts.with_pixels_per_point(1.0);
    let mut g = t.ed.geometry(&mut f, &NoImages);
    // far left of the second paragraph → its first stop; far right → its end
    let p = g.hit(1, epaint::pos2(r.min.x - 50.0, r.center().y)).unwrap();
    assert_eq!(p.offset, 17);
    let p = g.hit(1, epaint::pos2(r.max.x + 50.0, r.center().y)).unwrap();
    assert_eq!(p.offset, len);
    // selection rectangles of a range inside the item
    let rects = g.selection_rects(1, 17..23);
    assert_eq!(rects.len(), 1);
    assert!(rects[0].width() > 10.0);
}

#[test]
fn enter_over_a_selection_replaces_it_in_one_transaction() {
    let mut t = T::new("abcdef");
    t.ed.edit(|s| s.set_selection(Selection::range(2, 4)));
    t.run("edit.newline");
    assert_eq!(t.text(), "ab\n\nef");
    t.run("edit.undo");
    assert_eq!(t.text(), "abcdef");
    assert_eq!(t.ed.session.selection(), &Selection::range(2, 4));
}

#[test]
fn enter_in_atx_headings_never_creates_indented_code() {
    // Codex review PR #21 P2: the heading marker must not become indentation
    for level in 1..=6 {
        let hashes = "#".repeat(level);
        let src = format!("{hashes} ab cd");
        let mut t = T::new(&src);
        t.set(level + 3); // after "ab"
        t.run("edit.newline");
        assert_eq!(t.text(), format!("{hashes} ab\n\ncd"), "level {level}");
        let tree = t.ed.session.doc().debug_tree();
        assert!(tree.contains(&format!("h{level}")) && tree.contains("para"), "{tree}");
        assert!(!tree.contains("code"), "{tree}");
    }
}

#[test]
fn enter_in_setext_and_nested_headings() {
    let mut t = T::new("ab cd\n=====\n\nnext");
    t.set(2);
    t.run("edit.newline");
    assert_eq!(t.text(), "ab\n=====\n\ncd\n\nnext");
    assert_eq!(t.caret().offset, "ab\n=====\n\n".len());
    t.check_model();
    t.run("edit.undo");
    assert_eq!(t.text(), "ab cd\n=====\n\nnext");
    // headings inside a quote and a list item keep the container prefix only
    let mut t = T::new("> ### ab cd\n");
    t.set(8);
    t.run("edit.newline");
    assert_eq!(t.text(), "> ### ab\n>\n> cd\n");
    let mut t = T::new("- #### ab cd\n");
    t.set(9);
    t.run("edit.newline");
    let tree = t.ed.session.doc().debug_tree();
    assert!(!tree.contains("code"), "{}: {tree}", t.text());
    assert!(t.text().starts_with("- #### ab\n"), "{}", t.text());
}

#[test]
fn backspace_and_delete_never_remove_structure() {
    // Codex review PR #21 P1: table pipes, list markers, quote prefixes, fences, front matter
    let table = "| a | b |\n|---|---|\n| c | d |\n";
    let mut t = T::new(table);
    t.set(3); // after `a`
    t.run("edit.deleteForward");
    assert_eq!(t.text(), table, "Delete at the end of a cell");
    t.set(6); // before `b`
    t.run("edit.deleteBackward");
    assert_eq!(t.text(), table, "Backspace at the start of a cell");
    t.set(3);
    t.run("edit.deleteBackward");
    assert_eq!(
        t.text(),
        "|  | b |\n|---|---|\n| c | d |\n",
        "cell content is still editable"
    );
    for (src, at, want) in [
        ("- one\n- two\n", 8, "- one\n\n  two\n"), // S-5.4 (M2c): joins the item above
        ("> one\n> two\n", 8, "> onetwo\n"),       // soft line join in a quote
        ("```\ncode\n```\n", 4, "```\ncode\n```\n"), // first code line
        ("text\n\n```\ncode\n```\n", 10, "text\n\n```\ncode\n```\n"),
        ("---\ntitle: x\n---\n\nbody", 4, "---\ntitle: x\n---\n\nbody"),
        ("1. one\n2. two\n", 10, "1. one\n\n   two\n"),
    ] {
        let mut t = T::new(src);
        t.ed.edit(|s| s.set_selection(Selection::caret(at)));
        t.frame();
        t.run("edit.deleteBackward");
        assert_eq!(t.text(), want, "Backspace in {src:?} at {at}");
    }
    let mut t = T::new("- one\n- two\n");
    t.ed.edit(|s| s.set_selection(Selection::caret(5)));
    t.run("edit.deleteForward");
    assert_eq!(
        t.text(),
        "- onetwo\n",
        "Delete at the end of an item joins the next (C48)"
    );
    // the end of a code line: Delete does not eat the closing fence
    let src = "```\ncode\n```\n";
    let mut t = T::new(src);
    t.ed.edit(|s| s.set_selection(Selection::caret(8)));
    t.run("edit.deleteForward");
    assert_eq!(t.text(), src);
}

#[test]
fn pointer_hits_land_on_grapheme_stops() {
    // Codex review PR #21 P2: ZWJ sequence, flag, combining mark
    use unicode_segmentation::UnicodeSegmentation;
    let src = "a\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}b\u{1F1E9}\u{1F1EA}c e\u{301}x";
    let mut t = T::new(src);
    let stops: Vec<usize> = src.grapheme_indices(true).map(|(i, _)| i).chain([src.len()]).collect();
    let mut f = t.fonts.with_pixels_per_point(1.0);
    let mut g = t.ed.geometry(&mut f, &NoImages);
    let (_, r) = g.caret_rect(DocPos::new(0)).unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for k in 0..400 {
        let x = r.min.x - 5.0 + k as f32 * 0.5;
        let p = g.hit(0, epaint::pos2(x, r.center().y)).unwrap();
        assert!(stops.contains(&p.offset), "x {x}: offset {} inside a cluster", p.offset);
        seen.insert(p.offset);
    }
    assert!(seen.len() >= 5, "hits cover several stops: {seen:?}");
}

#[test]
fn reveal_is_frozen_not_cleared_during_a_pointer_drag() {
    // Codex review PR #21 P2
    let mut t = T::new("plain **bold words** tail");
    t.set(9); // inside the strong span: revealed
    assert!(t.revealed());
    let before = t.ed.view.focus().map(|(i, f)| (i, f.clone()));
    t.ed.reveal = false; // pointer down: freeze
    t.ed.set_caret(DocPos::new(22), true); // drag to `tail` (outside the span)
    t.frame();
    assert_eq!(
        t.ed.view.focus().map(|(i, f)| (i, f.clone())),
        before,
        "markers stay as shown while dragging"
    );
    t.ed.reveal = true; // release
    t.frame();
    let p = t.ed.session.selection().primary();
    assert_eq!((p.anchor.offset, p.focus.offset), (9, 22));
    assert!(
        !t.revealed(),
        "after release the reveal follows the focus end (outside the span)"
    );
}

#[test]
fn generated_range_deletes_stay_inside_the_block_content() {
    // Codex review round 2 of PR #21 (P1): word / line deletes clamp to the caret's content
    let table = "| a | b |\n|---|---|\n";
    let cases: &[(&str, usize, &str, &str)] = &[
        // (source, caret, command, expected)
        (table, 6, "edit.deleteWordBackward", table), // before `b`
        (table, 3, "edit.deleteWordForward", table),  // after `a`
        (table, 6, "delete.toLineStart", table),
        (table, 7, "edit.deleteWordBackward", "| a |  |\n|---|---|\n"), // after `b`: the word only
        ("- one\n- two\n", 8, "edit.deleteWordBackward", "- one\n\n  two\n"), // falls back to Backspace (S-5.4)
        ("- one\n- two\n", 5, "edit.deleteWordForward", "- onetwo\n"),  // falls back to Delete (C48)
        ("- one\n- two\n", 8, "delete.toLineStart", "- one\n\n  two\n"),
        ("- one\n- two\n", 11, "delete.toLineStart", "- one\n- \n"),
        ("> one\n> two\n", 8, "delete.toLineStart", "> onetwo\n"), // line start: a plain join
        ("> one two\n", 9, "delete.toLineStart", "> \n"),
        ("```\nab cd\n```\n", 4, "edit.deleteWordBackward", "```\nab cd\n```\n"),
        ("```\nab cd\n```\n", 9, "edit.deleteWordForward", "```\nab cd\n```\n"),
        ("```\nab cd\n```\n", 9, "edit.deleteWordBackward", "```\nab \n```\n"),
        ("```\nab cd\n```\n", 9, "delete.toLineStart", "```\n\n```\n"),
    ];
    for (src, at, cmd, want) in cases {
        let mut t = T::new(src);
        t.ed.edit(|s| s.set_selection(Selection::caret(*at)));
        t.frame();
        t.run(cmd);
        assert_eq!(t.text(), *want, "{cmd} in {src:?} at {at}");
    }
    // a deliberate selection across blocks is still deleted as selected
    let mut t = T::new("- one\n- two\n");
    t.ed.edit(|s| s.set_selection(Selection::range(3, 9)));
    t.run("edit.deleteWordBackward");
    assert_eq!(t.text(), "- owo\n");
}

#[test]
fn inline_math_is_never_revealed() {
    let mut t = T::new("a $x^2$ b **c**");
    t.set(4);
    assert!(!t.revealed(), "a formula is one object, its `$` markers stay hidden");
    t.set(13);
    assert!(t.revealed());
}

#[test]
fn selection_moves_to_a_stop_when_its_stops_disappear() {
    // Codex review round 3 of PR #21: the caret must not stay in source that is no longer
    // shown as text (front matter collapsed here; rendered diagrams and formulas alike)
    let src = "---\ntitle: x\n---\n\nbody";
    let mut t = T::new(src);
    t.ed.view.toggle_expanded(0); // front matter shown as text: its lines are stops
    t.frame();
    t.ed.edit(|s| s.set_selection(Selection::caret(6)));
    t.frame();
    {
        let mut f = t.fonts.with_pixels_per_point(1.0);
        assert!(!t.ed.fix_selection(&mut f, &NoImages), "a stop while expanded");
    }
    t.ed.view.toggle_expanded(0); // collapsed: no stops in the block any more
    t.frame();
    let changed = {
        let mut f = t.fonts.with_pixels_per_point(1.0);
        t.ed.fix_selection(&mut f, &NoImages)
    };
    assert!(changed);
    assert_eq!(
        t.caret().offset,
        src.len() - 4,
        "the first stop after the block (`body`)"
    );
    let mut f = t.fonts.with_pixels_per_point(1.0);
    assert!(!t.ed.fix_selection(&mut f, &NoImages), "stable");
}

#[test]
fn backspace_and_delete_handle_leading_and_trailing_heading_syntax() {
    // Codex review round 5 of PR #21: setext underlines (and ATX closing sequences) are block
    // syntax after the text; Backspace/Delete treat them like the ATX marker
    let cases: &[(&str, usize, &str, &str)] = &[
        // Backspace at the start of a heading's text → paragraph (rule 4)
        ("Title\n=====\n", 0, "edit.deleteBackward", "Title\n"),
        ("Title\n-----\n", 0, "edit.deleteBackward", "Title\n"),
        ("# Title\n", 2, "edit.deleteBackward", "Title\n"),
        ("## Title ##\n", 3, "edit.deleteBackward", "Title\n"),
        ("para\n\nTitle\n=====\n", 6, "edit.deleteBackward", "para\n\nTitle\n"),
        ("para\n\nTitle\n-----\n", 6, "edit.deleteBackward", "para\n\nTitle\n"),
        ("para\n\n## Title\n", 9, "edit.deleteBackward", "para\n\nTitle\n"),
        // Backspace at a paragraph after a heading: merge, the heading keeps its syntax
        (
            "Title\n=====\n\nmore\n",
            13,
            "edit.deleteBackward",
            "Titlemore\n=====\n",
        ),
        (
            "Title\n-----\n\nmore\n",
            13,
            "edit.deleteBackward",
            "Titlemore\n-----\n",
        ),
        ("# Title\n\nmore\n", 9, "edit.deleteBackward", "# Titlemore\n"),
        // Delete at the end of a paragraph before a heading: the heading's syntax goes
        ("para\n\nTitle\n=====\n", 4, "edit.deleteForward", "paraTitle\n"),
        ("para\n\nTitle\n-----\n", 4, "edit.deleteForward", "paraTitle\n"),
        ("para\n\n# Title\n", 4, "edit.deleteForward", "paraTitle\n"),
        // Delete at the end of a heading's text: the next text joins, syntax kept
        ("Title\n=====\n\nmore\n", 5, "edit.deleteForward", "Titlemore\n=====\n"),
        ("# Title\n\nmore\n", 7, "edit.deleteForward", "# Titlemore\n"),
        // no merge with blocks whose syntax trails (code fence, table, definition)
        (
            "para\n\n```\ncode\n```\n",
            4,
            "edit.deleteForward",
            "para\n\n```\ncode\n```\n",
        ),
        (
            "para\n\n| a |\n|---|\n",
            4,
            "edit.deleteForward",
            "para\n\n| a |\n|---|\n",
        ),
        (
            "para\n\n[x]: http://e.com\n\nend\n",
            4,
            "edit.deleteForward",
            "para\n\n[x]: http://e.com\n\nend\n",
        ),
    ];
    for (src, at, cmd, want) in cases {
        let mut t = T::new(src);
        t.ed.edit(|s| s.set_selection(Selection::caret(*at)));
        t.frame();
        t.run(cmd);
        assert_eq!(t.text(), *want, "{cmd} in {src:?} at {at}");
        t.check_model();
        if want != src {
            t.run("edit.undo");
            assert_eq!(t.text(), *src, "undo of {cmd} in {src:?}");
            assert_eq!(t.caret().offset, *at, "caret restored");
            t.run("edit.redo");
            assert_eq!(t.text(), *want, "redo of {cmd} in {src:?}");
        }
    }
}

/// Runs `cmd` at `at` in `src`, expects `want`, then checks undo/redo and the model.
fn roundtrip(src: &str, at: usize, cmd: &str, want: &str) {
    let mut t = T::new(src);
    t.ed.edit(|s| s.set_selection(Selection::caret(at)));
    t.frame();
    t.run(cmd);
    assert_eq!(t.text(), want, "{cmd} in {src:?} at {at}");
    t.check_model();
    if want != src {
        t.run("edit.undo");
        assert_eq!(t.text(), src, "undo of {cmd} in {src:?}");
        assert_eq!(t.caret().offset, at, "caret restored ({src:?})");
        t.check_model();
        t.run("edit.redo");
        assert_eq!(t.text(), want, "redo of {cmd} in {src:?}");
        t.check_model();
    }
}

#[test]
fn enter_in_headings_with_containers_closing_sequences_and_crlf() {
    // Codex review round 6 of PR #21
    let n = "edit.newline";
    // setext heading inside a list item: heading + paragraph in the same item (C7)
    roundtrip("- ab cd\n  =====\n", 4, n, "- ab\n  =====\n\n  cd\n");
    roundtrip("- ab cd\n  -----\n", 4, n, "- ab\n  -----\n\n  cd\n");
    // ATX heading inside a list item and a quote
    roundtrip("- ## ab cd\n", 7, n, "- ## ab\n\n  cd\n");
    roundtrip("> ## ab cd\n", 7, n, "> ## ab\n>\n> cd\n");
    roundtrip("> ab cd\n> =====\n", 4, n, "> ab\n> =====\n>\n> cd\n");
    // ATX closing sequence stays on the heading part
    roundtrip("## ab cd ##\n", 5, n, "## ab ##\n\ncd\n");
    roundtrip("> ## ab cd ##\n", 7, n, "> ## ab ##\n>\n> cd\n");
    roundtrip("## ab cd ##\r\n", 5, n, "## ab ##\r\n\r\ncd\r\n");
    // CRLF setext and plain ATX
    roundtrip("ab cd\r\n=====\r\n", 2, n, "ab\r\n=====\r\n\r\ncd\r\n");
    roundtrip("# ab cd\r\nnext\r\n", 4, n, "# ab\r\n\r\ncd\r\nnext\r\n");
    // a list paragraph still makes a sibling item
    roundtrip("- ab cd\n", 4, n, "- ab\n- cd\n");
}

#[test]
fn empty_headings_and_merges_with_closing_sequences() {
    let n = "edit.newline";
    // C43: Enter at an empty heading turns it into an empty paragraph in place
    for (src, want) in [("#\n", "\n"), ("## \n", "\n"), ("- #\n", "- \n"), ("a\n\n## \n", "a\n")] {
        let mut t = T::new(src);
        t.ed.edit(|s| s.set_selection(Selection::caret(src.find('#').unwrap() + 1)));
        t.frame();
        t.run(n);
        assert_eq!(t.text(), want, "Enter in the empty heading {src:?}");
        t.check_model();
        t.typ("x");
        let typed = if want == "a\n" {
            "a\n\nx\n".to_string()
        } else {
            want.replace('\n', "x\n")
        };
        assert_eq!(t.text(), typed, "typing after Enter in {src:?}");
        t.check_model();
    }
    // Backspace at an empty heading: becomes an (empty) paragraph
    roundtrip("para\n\n##\n", 8, "edit.deleteBackward", "para\n\n\n");
    // merges with ATX closing sequences
    roundtrip("## Title ##\n\nmore\n", 13, "edit.deleteBackward", "## Titlemore ##\n");
    roundtrip("## Title ##\n\nmore\n", 8, "edit.deleteForward", "## Titlemore ##\n");
    roundtrip("para\n\n## Title ##\n", 4, "edit.deleteForward", "paraTitle\n");
    roundtrip("para\n\n## Title ##\n", 9, "edit.deleteBackward", "para\n\nTitle\n");
    // inside containers
    roundtrip(
        "> para\n>\n> ## Title ##\n",
        2 + 4,
        "edit.deleteForward",
        "> paraTitle\n",
    );
    roundtrip(
        "> Title\n> =====\n>\n> more\n",
        20,
        "edit.deleteBackward",
        "> Titlemore\n> =====\n",
    );
}

#[test]
fn enter_with_the_caret_inside_heading_syntax() {
    // a caret in the closing sequence / underline counts as the end of the text: the
    // virtual paragraph after the heading, no source change
    for (src, at) in [("## ab ##\n", 7), ("ab\n==\n", 4)] {
        let mut t = T::new(src);
        t.ed.edit(|s| s.set_selection(Selection::caret(at)));
        t.run("edit.newline");
        assert_eq!(t.text(), src);
        assert!(t.caret().virt > 0, "{src:?}: {:?}", t.caret());
    }
}

/// Spec 02 §5.2 rule 2 (a thematic break before the block is deleted) and rule 5 into the
/// last paragraph of a container ending right before the block (#20 c04, l07).
#[test]
fn backspace_deletes_a_rule_and_merges_into_a_preceding_container() {
    let b = "edit.deleteBackward";
    roundtrip("a\n\n---\n\nb\n", 8, b, "a\n\nb\n");
    roundtrip("---\n\n## T\n", 8, b, "## T\n");
    roundtrip("> a\n>\n> ***\n>\n> b\n", 16, b, "> a\n>\n> b\n");
    roundtrip("- a\n\nb\n", 5, b, "- ab\n");
    roundtrip("- a\n  - b\n\nc\n", 11, b, "- a\n  - bc\n");
    roundtrip("> q\n\nb\n", 5, b, "> qb\n");
}

/// End on a paragraph's last line goes past whitespace the file already had after the text,
/// so typing appends after it (#20 l01); not on a line ending in a hard break.
#[test]
fn line_end_includes_trailing_whitespace_from_the_file() {
    for (src, at, want) in [
        ("a \n\nnext\n", 0, 2),
        ("a\t \n", 0, 3),
        ("a  \nb\n", 0, 1),
        ("a b\n", 0, 3),
    ] {
        let mut t = T::new(src);
        t.ed.edit(|s| s.set_selection(Selection::caret(at)));
        t.frame();
        t.run("caret.lineEnd");
        assert_eq!(t.caret().offset, want, "End in {src:?}");
    }
}

/// C53: Down on the last visual row / Right at the very end of the last block moves to the
/// virtual paragraph after it (no source change), also from a table or a code block; Up
/// returns (#20 e05–e08, e15).
#[test]
fn c53_arrows_past_the_last_block_reach_the_virtual_paragraph() {
    for (src, at, key) in [
        ("a\n", 1, "caret.down"),
        ("a\n", 0, "caret.down"),
        ("a\n", 1, "caret.right"),
        ("a\n\nb\n", 3, "caret.down"),
        ("| a | b |\n|---|---|\n| x | y |\n", 22, "caret.down"),
        ("```\ncode\n```\n", 6, "caret.down"),
    ] {
        let mut t = T::new(src);
        t.ed.edit(|s| s.set_selection(Selection::caret(at)));
        t.frame();
        t.run(key);
        assert_eq!(t.caret().virt, 1, "{key} in {src:?} at {at}: {:?}", t.caret());
        assert_eq!(t.text(), src);
        t.run(key);
        assert_eq!(t.caret().virt, 1, "a second {key} stays");
    }
    let mut t = T::new("a\n");
    t.ed.edit(|s| s.set_selection(Selection::caret(1)));
    t.frame();
    t.run("caret.down");
    t.run("caret.up");
    assert_eq!((t.caret().offset, t.caret().virt), (1, 0));
}

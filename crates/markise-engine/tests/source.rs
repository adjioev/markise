//! M2d slice 1 (spec 02 §12.1 / §12.1.1, ADR 0006 §4.3, §5.5): the source editor on the
//! editing engine — verbatim typing and line breaks (99 C58), caret commands through the source
//! view, full Source mode's own history (fresh on entry, collapsed into "Source Edit" on exit,
//! split at saved states), Side by Side edits in the live history, and the incremental source
//! view / rendered model after every keystroke.

use std::sync::Arc;
use std::time::Duration;

use epaint::text::Fonts;
use markise_doc::{DocPos, DocumentFormat, Params, Selection};
use markise_engine::editor::{EditorSession, EditorView, HistoryLimits, ManualClock};
use markise_layout::source::{SourceParams, SourceView};
use markise_layout::{DocModel, View, testing};
use markise_md::MarkdownFormat;
use markise_theme::Theme;

struct S {
    ed: EditorView,
    fonts: Fonts,
    clock: ManualClock,
}

impl S {
    fn new(text: &str) -> S {
        let clock = ManualClock::default();
        let format: Arc<dyn DocumentFormat> = Arc::new(MarkdownFormat::default());
        let session = EditorSession::with(format, text, HistoryLimits::default(), Box::new(clock.clone()));
        let model = DocModel::build(session.doc(), None);
        let view = View::new(session.doc_arc().clone(), model, Theme::light(), 600.0, 1.0);
        let mut ed = EditorView::new(session, view, None);
        ed.open_source(SourceParams::default(), 500.0, 1.0);
        let mut s = S {
            ed,
            fonts: testing::fonts(),
            clock,
        };
        s.frame();
        s
    }

    fn frame(&mut self) {
        let mut f = self.fonts.with_pixels_per_point(1.0);
        self.ed.source_frame(600.0, 300.0, &mut f);
    }

    fn run(&mut self, id: &str) {
        self.run_p(id, Params::new());
    }

    fn run_p(&mut self, id: &str, p: Params) {
        {
            let mut f = self.fonts.with_pixels_per_point(1.0);
            let r = self.ed.source_execute(id, &p, &mut f, 600.0);
            if let Err(e) = r {
                assert!(
                    matches!(e, markise_engine::editor::command::CommandError::Disabled(_)),
                    "{id}: {e}"
                );
            }
        }
        self.frame();
        self.check();
    }

    fn typ(&mut self, s: &str) {
        for c in s.chars() {
            self.run_p("edit.insertText", Params::text("text", c.to_string()));
        }
    }

    fn text(&self) -> String {
        self.ed.session.text().to_string()
    }

    fn caret(&self) -> usize {
        self.ed.source.as_ref().unwrap().sel.primary().focus.offset
    }

    fn set(&mut self, off: usize) {
        self.ed.source_set_selection(Selection::caret(off));
    }

    /// The incremental source view and rendered model equal fresh builds.
    fn check(&self) {
        let text = self.ed.session.text();
        let v = &self.ed.source.as_ref().unwrap().view;
        v.check(text).unwrap();
        let full = SourceView::new(text, v.params(), &Theme::light(), v.width(), 1.0);
        assert_eq!(v.lines_debug(text), full.lines_debug(text));
        self.ed
            .view
            .model()
            .diff(&DocModel::build(self.ed.session.doc(), None))
            .expect("rendered model == full build");
    }
}

#[test]
fn source_typing_is_verbatim_and_grouped() {
    let mut s = S::new("para\n");
    s.set(5);
    // input rules and auto-pair of the rendered view do not apply in source mode
    s.typ("- *a* (b");
    assert_eq!(s.text(), "para\n- *a* (b");
    assert!(s.ed.session.hints().is_empty(), "no literal hints in source mode");
    s.run("edit.newline");
    s.typ("#x");
    assert_eq!(s.text(), "para\n- *a* (b\n#x");
    // undo: "#x", the line break, then the typing (word groups, C56)
    s.run("edit.undo");
    assert_eq!(s.text(), "para\n- *a* (b\n");
    s.run("edit.undo");
    assert_eq!(s.text(), "para\n- *a* (b");
    while s.ed.session.can_undo() {
        s.run("edit.undo");
    }
    assert_eq!(s.text(), "para\n");
    assert!(!s.ed.session.is_dirty(), "back at the opened state");
    s.run("edit.redo");
    assert_ne!(s.text(), "para\n");
}

#[test]
fn typing_pauses_split_groups() {
    let mut s = S::new("");
    s.typ("ab");
    s.clock.advance(Duration::from_secs(3));
    s.typ("cd");
    s.run("edit.undo");
    assert_eq!(s.text(), "ab");
}

#[test]
fn line_breaks_keep_the_line_ending_c58() {
    // uniformly CRLF: a typed break is CRLF
    let mut s = S::new("a\r\nb\r\n");
    s.set(1);
    s.run("edit.newline");
    assert_eq!(s.text(), "a\r\n\r\nb\r\n");
    assert_eq!(s.caret(), 3);
    // mixed endings: untouched terminators stay as they are, a break repeats its line's
    let mut s = S::new("a\r\nmiddle\nz");
    s.set(4);
    s.run("edit.newline");
    assert_eq!(s.text(), "a\r\nm\niddle\nz");
    s.set(s.text().len());
    s.typ("!");
    assert_eq!(s.text(), "a\r\nm\niddle\nz!");
    // Backspace after CRLF removes the whole terminator (one grapheme)
    let mut s = S::new("a\r\nb");
    s.set(3);
    s.run("edit.deleteBackward");
    assert_eq!(s.text(), "ab");
    // pasted text is inserted as given
    let mut s = S::new("x\n");
    s.set(0);
    s.ed.source_insert("p\r\nq\n", true).unwrap();
    assert_eq!(s.text(), "p\r\nq\nx\n");
}

#[test]
fn caret_commands_use_the_source_geometry() {
    let mut s = S::new("first line\nsecond line\n\nthird\n");
    s.set(3);
    s.run("caret.down");
    assert_eq!(s.caret(), 14, "same column on the next line");
    s.run("caret.down");
    assert_eq!(s.caret(), 23, "the empty line");
    s.run("caret.down");
    assert_eq!(s.caret(), 27, "goal column kept");
    s.run("caret.lineEnd");
    assert_eq!(s.caret(), 29);
    s.run("caret.wordLeft");
    assert_eq!(s.caret(), 24);
    s.run("nav.bottom");
    assert_eq!(s.caret(), s.text().len());
    s.run("select.all");
    let sel = s.ed.source.as_ref().unwrap().sel.primary();
    assert_eq!(sel.range(), 0..s.text().len());
    s.set(0);
    s.run("select.right");
    s.run("select.right");
    s.typ("X");
    assert_eq!(
        s.text(),
        "Xrst line\nsecond line\n\nthird\n",
        "typing replaces the selection"
    );
}

#[test]
fn tab_indents_lines_and_inserts_the_indent_unit() {
    let mut s = S::new("a\nb\nc\n");
    s.set(0);
    s.run("edit.indentOrNextCell");
    assert_eq!(s.text(), "    a\nb\nc\n");
    s.ed.source_set_selection(Selection::range(0, 8));
    s.run("edit.indentOrNextCell");
    assert_eq!(s.text(), "        a\n    b\nc\n");
    s.run("edit.outdentOrPrevCell");
    assert_eq!(s.text(), "    a\nb\nc\n");
    s.run("edit.undo");
    assert_eq!(s.text(), "        a\n    b\nc\n", "one undo step per command");
}

#[test]
fn format_commands_are_disabled_in_the_source_pane() {
    let mut s = S::new("word\n");
    s.ed.source_set_selection(Selection::range(0, 4));
    let mut f = s.fonts.with_pixels_per_point(1.0);
    let r = s.ed.source_execute("format.strong", &Params::new(), &mut f, 600.0);
    assert!(r.is_err());
    assert_eq!(s.ed.session.text(), "word\n");
}

/// ADR 0006 §4.3: fresh history on entry, collapsed into "Source Edit" on exit, split at saves;
/// the test the ADR names: edit, save, edit more, leave, undo once → clean.
#[test]
fn source_session_history_collapses_and_keeps_saved_boundaries() {
    let mut s = S::new("start\n");
    // a rendered-view edit before entering
    s.ed.session.set_selection(Selection::caret(5));
    let _ = s.ed.edit(|x| x.insert_text("!"));
    assert_eq!(s.text(), "start!\n");
    let before = s.ed.session.history().undo_len();
    s.ed.session.begin_source_session();
    s.set(0);
    s.typ("one");
    // the source session cannot undo the rendered edit made before it
    while s.ed.session.can_undo() {
        s.run("edit.undo");
    }
    assert_eq!(s.text(), "start!\n");
    s.run("edit.redo");
    assert_eq!(s.text(), "onestart!\n");
    let snap = s.ed.session.prepare_save().unwrap();
    s.ed.session.mark_saved(&snap);
    s.typ("two ");
    s.run("edit.newline");
    assert!(s.ed.session.is_dirty());
    s.ed.session.end_source_session();
    let hist: Vec<String> =
        s.ed.session
            .history()
            .undo_stack()
            .map(|t| t.label.to_string())
            .collect();
    assert_eq!(hist.len(), before + 2, "{hist:?}");
    assert_eq!(&hist[before..], &["Source Edit", "Source Edit"]);
    let _ = s.ed.edit(|x| x.undo());
    assert_eq!(s.ed.session.text(), "onestart!\n");
    assert!(
        !s.ed.session.is_dirty(),
        "undo reaches the saved state, which reads clean"
    );
    let _ = s.ed.edit(|x| x.undo());
    assert_eq!(s.ed.session.text(), "start!\n");
    let _ = s.ed.edit(|x| x.undo());
    assert_eq!(s.ed.session.text(), "start\n", "the rendered edit is undone separately");
}

/// A source session without edits leaves the history (and its redo stack) alone (S-12.60).
#[test]
fn source_session_without_edits_keeps_the_history() {
    let mut s = S::new("abc\n");
    s.ed.session.set_selection(Selection::caret(3));
    let _ = s.ed.edit(|x| x.insert_text("d"));
    let _ = s.ed.edit(|x| x.undo());
    assert!(s.ed.session.can_redo());
    s.ed.session.begin_source_session();
    assert!(!s.ed.session.can_redo(), "a fresh history in source mode");
    assert!(!s.ed.session.can_undo());
    s.run("caret.down");
    s.ed.session.end_source_session();
    assert!(s.ed.session.can_redo(), "the rendered redo stack is back");
    let _ = s.ed.edit(|x| x.redo());
    assert_eq!(s.text(), "abcd\n");
}

/// Side by Side: no source session — source edits are ordinary transactions of the live
/// history, undone from either pane.
#[test]
fn side_by_side_edits_are_live_history_transactions() {
    let mut s = S::new("x\n");
    s.set(1);
    s.typ("yz");
    assert_eq!(s.ed.session.history().undo_len(), 1);
    // undo from the rendered pane
    let _ = s.ed.edit(|x| x.undo());
    assert_eq!(s.text(), "x\n");
    // the source view followed (checked against a rebuild)
    s.check();
}

/// Random keystrokes on a document with containers, code, CRLF and long lines: after each one
/// the source view and the rendered model equal fresh builds (incremental == full).
#[test]
fn random_source_keystrokes_keep_both_views_incremental() {
    let mut src = String::from("# T\r\n\r\n- a\n- b\n\n> q\n\n```rs\nfn x() {}\n```\n\n");
    src.push_str(&"long ".repeat(1200));
    src.push_str("\n\n[r]: http://e.com\n");
    let mut s = S::new(&src);
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut rnd = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n.max(1) as u64) as usize
    };
    let keys = [
        "edit.newline",
        "edit.deleteBackward",
        "edit.deleteForward",
        "caret.down",
        "caret.up",
        "caret.right",
        "caret.wordLeft",
        "edit.undo",
        "edit.indentOrNextCell",
        "edit.deleteWordBackward",
    ];
    let typed = ["a", " ", "*", "`", "-", "é", "\t", "[r]", ">", "|"];
    for _ in 0..300 {
        let len = s.ed.session.text().len();
        if rnd(10) == 0 {
            let mut o = rnd(len + 1);
            while !s.ed.session.text().is_char_boundary(o) {
                o -= 1;
            }
            s.set(o);
        }
        if rnd(2) == 0 {
            s.run(keys[rnd(keys.len())]);
        } else {
            s.run_p(
                "edit.insertText",
                Params::text("text", typed[rnd(typed.len())].to_string()),
            );
        }
    }
    let _ = DocPos::new(0);
}

/// Review of PR #23 (P1): a change made outside the source pane (rendered pane, menu Undo)
/// maps the source selection through it — never beyond the text.
#[test]
fn rendered_edits_map_the_source_selection() {
    let mut s = S::new("abc\n");
    s.set(4);
    s.typ("xyz");
    assert_eq!(s.caret(), 7);
    // undo from the rendered pane (not through the source pane)
    let _ = s.ed.edit(|x| x.undo());
    assert_eq!(s.text(), "abc\n");
    assert!(
        s.caret() <= s.text().len(),
        "caret {} in {} bytes",
        s.caret(),
        s.text().len()
    );
    s.run("edit.deleteBackward");
    assert_eq!(s.text(), "abc");
    // an edit in front of the source caret shifts it
    s.set(2);
    let _ = s.ed.edit(|x| {
        x.set_selection(Selection::caret(0));
        x.insert_text("Q")
    });
    assert_eq!(s.text(), "Qabc");
    assert_eq!(s.caret(), 3, "the source caret moved with its text");
}

/// Review of PR #23 (P1): a replaced document (reload) never leaves the source caret inside a
/// UTF-8 character; every selection the pane takes is snapped to a caret position.
#[test]
fn replaced_documents_snap_the_source_selection() {
    let mut s = S::new("ab");
    s.set(1);
    let doc: Arc<dyn markise_doc::FormatDoc> = Arc::new(markise_md::parse("éb"));
    let _ =
        s.ed.edit(|x| x.replace_doc(doc, "Reload from Disk", markise_engine::editor::TxKind::Bulk, false));
    let c = s.caret();
    assert!(s.text().is_char_boundary(c), "caret {c} on a char boundary");
    s.run("edit.deleteBackward");
    s.check();
    // a selection handed to the pane inside a character or a CRLF is snapped
    let mut s = S::new("é\r\nx");
    s.ed.source_set_selection(Selection::range(1, 3));
    let r = s.ed.source.as_ref().unwrap().sel.primary().range();
    assert_eq!(r, 0..2);
    s.run("edit.deleteBackward");
    assert_eq!(s.text(), "\r\nx");
}

/// Review of PR #23 (P2): Backspace/Delete never split a grapheme cluster, however long.
#[test]
fn deletion_keeps_long_clusters_whole() {
    let cluster = format!("a{}", "\u{301}".repeat(40));
    let mut s = S::new(&format!("{cluster}b"));
    s.set(0);
    s.run("edit.deleteForward");
    assert_eq!(s.text(), "b");
    let mut s = S::new(&format!("x{cluster}"));
    s.set(s.text().len());
    s.run("edit.deleteBackward");
    assert_eq!(s.text(), "x");
    s.run("caret.left");
    assert_eq!(s.caret(), 0);
}

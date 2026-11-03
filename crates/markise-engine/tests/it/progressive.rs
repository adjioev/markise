//! Progressive open (issue #126): the editor starts on the first part of a file, the whole
//! document arrives later and is installed with [`EditorView::continue_with`]. The result must
//! equal a full open of the same text with the edits made meanwhile, keep the history, the
//! selection and the scroll position, and correct what the first part could only show
//! provisionally (reference definitions after the cut, disclosures closed after it, the
//! outline).

use crate::common;

use std::sync::Arc;

use common::T;
use markise_doc::{FormatDoc, Selection};
use markise_layout::block::NoImages;
use markise_layout::{DocModel, Paging};
use markise_md::{Document, ParseOptions, progressive_prefix};

const WANT: usize = 3000;

fn source(crlf: bool) -> String {
    let mut s = String::from("---\ntitle: Progressive\n---\n\n# Top\n\nSee [the ref] and [^n].\n\n");
    s.push_str("<details>\n\n<summary>Early</summary>\n\nInside.\n\n</details>\n\n");
    s.push_str("<details>\n\n<summary>Crossing</summary>\n\n");
    for i in 0..120 {
        s.push_str(&format!(
            "## Head {i}\n\nPara {i} with *words* and [the ref].\n\n- item {i}\n- second\n\n```\ncode {i}\n```\n\n"
        ));
    }
    s.push_str("</details>\n\n");
    for i in 120..160 {
        s.push_str(&format!("## Late {i}\n\nPara {i}.\n\n"));
    }
    s.push_str("[the ref]: https://example.com\n\n[^n]: A note.\n");
    if crlf { s.replace('\n', "\r\n") } else { s }
}

fn pagings() -> [Paging; 2] {
    [Paging::Paged { tops: 4 }, Paging::Full]
}

/// An editor on the first part of `text` (as the loader shows it) and the part's length.
fn open_first_part(text: &str, paging: Paging) -> (T, usize) {
    let (cut, part) = progressive_prefix(text, WANT, &ParseOptions::default()).expect("a cut");
    assert!(cut < text.len() / 2, "a real first part");
    assert_eq!(part.text(), &text[..cut]);
    let mut t = T::with_paging(&text[..cut], paging);
    t.fix_selection = true;
    (t, cut)
}

/// The whole document as the loader's rest delivers it.
fn whole(text: &str, paging: Paging) -> (Arc<dyn FormatDoc>, Arc<DocModel>) {
    let doc = Document::parse(text.to_string(), ParseOptions::default());
    let model = Arc::new(DocModel::build_with(&doc, None, paging));
    (Arc::new(doc), model)
}

/// The items on screen after a frame: (source start, y).
fn on_screen(t: &mut T) -> Vec<(usize, f32)> {
    t.ed.refresh_focus();
    let mut f = t.fonts.with_pixels_per_point(1.0);
    let vis = t.ed.view.frame(400.0, 0.0, &mut f, &NoImages);
    vis.iter().map(|v| (t.ed.view.model().item_start(v.idx), v.y)).collect()
}

#[test]
fn without_edits_the_result_equals_a_full_open() {
    for crlf in [false, true] {
        let text = source(crlf);
        for paging in pagings() {
            let (mut t, cut) = open_first_part(&text, paging);
            t.ed.view.scroll_by(700.0);
            let before = on_screen(&mut t);
            let anchor = t.ed.view.source_anchor();
            let (doc, model) = whole(&text, paging);
            let r = t.ed.continue_with(doc, model, cut).expect("continued");
            assert!(!r.edited);
            assert_eq!(t.text(), text);
            assert_eq!(
                t.ed.view.source_anchor(),
                anchor,
                "{paging:?} crlf={crlf}: scroll position"
            );
            let after = on_screen(&mut t);
            assert_eq!(
                after.first(),
                before.first(),
                "{paging:?} crlf={crlf}: content on screen"
            );
            t.check_model();
            let full = DocModel::build_with(t.ed.session.doc(), None, Paging::Full);
            let heads = |m: &DocModel| -> Vec<(u8, String, String)> {
                m.outline
                    .iter()
                    .map(|o| (o.level, o.text.clone(), o.anchor.clone()))
                    .collect()
            };
            assert_eq!(heads(t.ed.view.model()), heads(&full), "complete outline");
            assert!(!t.ed.session.is_dirty(), "the opened state is the saved one");
            assert_eq!(t.ed.session.history().undo_len(), 0, "no undo step");
        }
    }
}

#[test]
fn edits_made_meanwhile_keep_their_history_selection_and_scroll_position() {
    for crlf in [false, true] {
        let text = source(crlf);
        for paging in pagings() {
            let (mut t, cut) = open_first_part(&text, paging);
            // typing in a paragraph, a new block, a deletion near the end of the part
            let at = text.find("Para 3 ").unwrap() + 5;
            t.set(at);
            t.typ("NEW ");
            t.run("edit.newline");
            t.typ("block");
            let del = t.text().rfind("Para").unwrap();
            t.ed.set_selection(Selection::range(del, del + 4));
            t.frame();
            t.run("edit.deleteBackward");
            let edited = t.text().to_string();
            t.ed.set_selection(Selection::range(at, at + 3));
            t.frame();
            t.ed.view.scroll_by(500.0);
            let before = on_screen(&mut t);
            let anchor = t.ed.view.source_anchor();
            let sel = t.ed.session.selection().clone();
            let undo = t.ed.session.history().undo_len();
            assert!(undo >= 3);

            let (doc, model) = whole(&text, paging);
            let r = t.ed.continue_with(doc, model, cut).expect("continued");
            assert!(r.edited);
            assert_eq!(t.text(), format!("{edited}{}", &text[cut..]), "edits carried over");
            assert_eq!(t.ed.session.selection(), &sel, "{paging:?}: selection");
            assert_eq!(t.ed.view.source_anchor(), anchor, "{paging:?}: scroll position");
            assert_eq!(
                on_screen(&mut t).first(),
                before.first(),
                "{paging:?}: content on screen"
            );
            assert_eq!(t.ed.session.history().undo_len(), undo, "history kept, no new step");
            assert!(t.ed.session.is_dirty());
            t.check_model();

            // the history works on the whole document
            while t.ed.edit(|s| s.undo()) {
                t.frame();
            }
            assert_eq!(t.text(), text, "undo back to the file as read");
            assert!(!t.ed.session.is_dirty(), "back at the saved state");
            t.check_model();
            while t.ed.edit(|s| s.redo()) {
                t.frame();
            }
            assert_eq!(t.text(), format!("{edited}{}", &text[cut..]));
            t.check_model();
            // and keeps editing after the part
            let end = t.text().len();
            t.set(end);
            t.typ("Z");
            t.check_model();
        }
    }
}

#[test]
fn provisional_rendering_is_corrected() {
    let text = source(false);
    for paging in pagings() {
        let (mut t, cut) = open_first_part(&text, paging);
        // the part alone: the reference and the footnote are undefined, the crossing
        // disclosure has no end, few headings
        let part_model = t.ed.view.model().clone();
        let groups = part_model.cross.disclosures();
        assert_eq!(groups.len(), 1, "only the early disclosure pairs in the part");
        // the reader opens the early disclosure meanwhile
        let early = groups[0].start;
        let idx = t.ed.view.model().item_at_offset(early);
        assert!(!groups[0].open, "closed as written");
        t.ed.view.toggle_expanded(idx);
        assert!(t.ed.view.model().cross.disclosures()[0].open);
        let (doc, model) = whole(&text, paging);
        t.ed.continue_with(doc, model, cut).expect("continued");
        t.frame();
        t.check_model();
        let groups = t.ed.view.model().cross.disclosures();
        assert_eq!(groups.len(), 2, "the crossing disclosure pairs with its end");
        assert!(groups[0].open, "the reader's choice is kept");
        assert!(!groups[1].open, "closed as written: the view was above it");
        let outline = &t.ed.view.model().outline;
        assert!(
            outline.iter().any(|o| o.text == "Late 159"),
            "outline of the whole document"
        );
        // (the definitions after the cut: item keys and reference labels equal a full build's,
        // checked by check_model)
    }
}

/// The reader was inside a disclosure whose end is after the cut (the first part showed its
/// content): it stays open, so the content on screen does not disappear.
#[test]
fn a_disclosure_the_reader_is_inside_stays_open() {
    let text = source(false);
    for paging in pagings() {
        let (mut t, cut) = open_first_part(&text, paging);
        let inside = text.find("## Head 2\n").unwrap();
        t.ed.view.scroll_to_source(inside, 0, 0.0);
        let before = on_screen(&mut t);
        let (doc, model) = whole(&text, paging);
        t.ed.continue_with(doc, model, cut).expect("continued");
        assert_eq!(
            on_screen(&mut t).first(),
            before.first(),
            "{paging:?}: content on screen"
        );
        let groups = t.ed.view.model().cross.disclosures();
        assert!(groups[1].open, "kept open for the reader");
        t.check_model();
    }
    // the caret inside keeps it open too
    let (mut t, cut) = open_first_part(&text, Paging::Full);
    let inside = text.find("Para 2 ").unwrap();
    t.set(inside);
    let (doc, model) = whole(&text, Paging::Full);
    t.ed.continue_with(doc, model, cut).expect("continued");
    assert!(t.ed.view.model().cross.disclosures()[1].open);
}

#[test]
fn a_continuation_that_does_not_start_with_the_part_is_refused() {
    let text = source(false);
    let (mut t, cut) = open_first_part(&text, Paging::Full);
    let (doc, model) = whole(&text[..cut / 2], Paging::Full);
    assert!(t.ed.continue_with(doc, model, cut).is_err());
    assert_eq!(t.text(), &text[..cut], "nothing changed");
}

/// Full Source mode open while the rest loads: the source view shows the whole text after the
/// continuation, the source session's edits collapse and undo as usual.
#[test]
fn source_mode_during_loading() {
    let text = source(true);
    let (mut t, cut) = open_first_part(&text, Paging::Paged { tops: 4 });
    t.ed.open_source(markise_layout::source::SourceParams::default(), 600.0, 1.0);
    t.ed.session.begin_source_session();
    t.ed.source_set_selection(Selection::caret(10));
    t.ed.source_insert("SRC", false).unwrap();
    let lines_part = t.ed.source.as_ref().unwrap().view.line_count();
    let (doc, model) = whole(&text, Paging::Paged { tops: 4 });
    t.ed.continue_with(doc, model, cut).expect("continued");
    let mut want = text.clone();
    want.insert_str(10, "SRC");
    assert_eq!(t.text(), want);
    let p = t.ed.source.as_ref().unwrap();
    assert!(p.view.line_count() > lines_part, "the source view holds the whole text");
    let fresh = markise_layout::source::SourceView::new(
        &want,
        markise_layout::source::SourceParams::default(),
        &markise_theme::Theme::light(),
        600.0,
        1.0,
    );
    assert_eq!(p.view.line_count(), fresh.line_count());
    assert_eq!(p.sel, Selection::caret(13), "the source selection is kept");
    t.ed.session.end_source_session();
    t.frame();
    assert!(t.ed.edit(|s| s.undo()));
    assert_eq!(t.text(), text);
    t.check_model();
}

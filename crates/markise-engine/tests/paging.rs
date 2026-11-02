//! Editing over a paged view model (issue #125): typing, Enter, Backspace, Undo/Redo at page
//! edges, caret moves across pages, jumps into pages that are not materialized and selections
//! spanning them. Every scenario runs on a paged and a full model side by side: text and
//! selection agree after every step, and the paged model equals a full build where it holds
//! real items.

mod common;

use common::T;
use markise_doc::{DocPos, Params, Selection};
use markise_layout::Paging;

const TOPS: usize = 4;

fn source() -> String {
    let mut s = String::new();
    for i in 0..40 {
        s.push_str(&format!(
            "## Head {i}\n\nPara {i} has words here.\n\n- item {i}\n- second\n\n"
        ));
    }
    s
}

/// The same editor on a paged and on a full model.
struct Pair {
    paged: T,
    full: T,
}

impl Pair {
    fn new(src: &str) -> Pair {
        let mut p = Pair {
            paged: T::paged(src, TOPS),
            full: T::with_paging(src, Paging::Full),
        };
        // the viewer's frame: layout, then the selection fixed onto caret stops
        p.paged.fix_selection = true;
        p.full.fix_selection = true;
        assert!(p.paged.ed.view.model().page_count() > 10);
        p
    }

    fn same(&self, what: &str) {
        assert_eq!(self.paged.text(), self.full.text(), "{what}: text");
        assert_eq!(
            self.paged.ed.session.selection(),
            self.full.ed.session.selection(),
            "{what}: selection"
        );
        self.paged.check_model();
    }

    fn key(&mut self, id: &str) {
        self.paged.run(id);
        self.full.run(id);
        self.paged.reveal();
        self.full.reveal();
        self.same(id);
    }

    fn typ(&mut self, s: &str) {
        for c in s.chars() {
            let p = Params::text("text", c.to_string());
            self.paged.run_p("edit.insertText", p.clone());
            self.full.run_p("edit.insertText", p);
        }
        self.same(s);
    }

    fn set(&mut self, off: usize) {
        self.paged.set(off);
        self.full.set(off);
        self.same("set caret");
    }

    fn select(&mut self, a: usize, b: usize) {
        for t in [&mut self.paged, &mut self.full] {
            t.ed.set_selection(Selection::range(a, b));
            t.frame();
        }
        self.same("select");
    }

    fn extend(&mut self, from: usize, to: usize) {
        for t in [&mut self.paged, &mut self.full] {
            t.ed.set_caret(DocPos::new(from), false);
            t.ed.set_caret(DocPos::new(to), true);
            t.frame();
        }
        self.same("extend");
    }

    /// Start of top-level block `k`.
    fn top(&self, k: usize) -> usize {
        self.paged.ed.session.doc().top_span(k).unwrap().start
    }

    fn is_page_at(&self, off: usize) -> bool {
        let m = self.paged.ed.view.model();
        m.is_page(m.item_at_offset(off))
    }
}

#[test]
fn typing_enter_backspace_and_undo_at_page_edges() {
    let src = source();
    let mut p = Pair::new(&src);
    for page in [1, 5, 17, 28] {
        let k = page * TOPS;
        // the end of the last block of a page (a list item's text end)
        let end = p.top(k) - 2;
        p.set(end);
        assert!(!p.is_page_at(end));
        let before = p.paged.text().to_string();
        p.typ("xyz");
        assert_eq!(p.paged.text(), format!("{}xyz{}", &before[..end], &before[end..]));
        p.key("edit.newline");
        p.typ("new");
        // Backspace at the start of the first block of the next page merges across the edge
        let next = p.top(k + 1);
        p.set(next);
        p.key("edit.deleteBackward");
        p.key("edit.deleteBackward");
        for _ in 0..8 {
            p.key("edit.undo");
        }
        assert_eq!(p.paged.text(), before, "page {page}: undo restores the text");
        p.key("edit.redo");
        p.key("edit.undo");
        assert_eq!(p.paged.text(), before);
    }
}

#[test]
fn edits_far_from_the_viewport_after_eviction() {
    let src = source();
    let mut p = Pair::new(&src);
    // to the end: the pages at the top are evicted
    p.key("nav.bottom");
    let far = p.top(2 * TOPS);
    assert!(p.is_page_at(far), "evicted after the jump");
    // an edit there materializes its page
    p.set(far + 3);
    p.typ("Q");
    assert_eq!(&p.paged.text()[far + 3..far + 4], "Q");
    p.key("nav.top");
    p.key("nav.bottom");
    p.key("edit.undo");
    assert_eq!(p.paged.text(), src);
}

#[test]
fn caret_moves_cross_page_edges() {
    let src = source();
    let mut p = Pair::new(&src);
    p.set(0);
    for _ in 0..60 {
        p.key("caret.down");
        assert!(p.paged.caret_visible());
    }
    assert!(p.paged.caret().offset > p.top(6 * TOPS), "the caret crossed pages");
    for _ in 0..60 {
        p.key("caret.up");
    }
    p.key("caret.pageDown");
    p.key("caret.pageDown");
    p.key("caret.pageUp");
    assert!(p.paged.caret_visible());
}

#[test]
fn selections_spanning_unloaded_pages() {
    let src = source();
    let mut p = Pair::new(&src);
    let (a, b) = (p.top(2) + 2, p.top(25 * TOPS) - 3);
    assert!(
        (2..25 * TOPS).any(|k| p.is_page_at(p.top(k))),
        "placeholders in between"
    );
    p.select(a, b);
    p.key("edit.deleteBackward");
    assert_eq!(p.paged.text(), format!("{}{}", &src[..a], &src[b..]));
    p.key("edit.undo");
    assert_eq!(p.paged.text(), src);
    // select all and type over it
    p.key("select.all");
    p.typ("z");
    p.key("edit.undo");
    assert_eq!(p.paged.text(), src);
    // a caret set far away, then extended back across pages
    p.extend(p.top(28 * TOPS + 1), p.top(3));
    p.key("edit.deleteBackward");
    p.key("edit.undo");
    assert_eq!(p.paged.text(), src);
}

/// Review of #129 (P1): a long selection whose anchor's page would be evicted once the view
/// follows the focus far away keeps both ends: the selected text, copy and delete are those
/// of the full model.
#[test]
fn a_selection_keeps_its_far_anchor_when_the_view_moves_away() {
    let src = source();
    let mut p = Pair::new(&src);
    let anchor = p.top(10 * TOPS) + 5;
    p.set(anchor);
    // extend to the end of the document (the view follows the focus), many frames
    p.key("select.bottom");
    for _ in 0..5 {
        p.paged.frame();
        p.full.frame();
    }
    p.same("after the view moved away");
    let sel = p.paged.ed.session.selection().primary();
    assert_eq!(sel.anchor.offset, anchor, "the anchor stays");
    assert!(p.paged.ed.view.scroll_y() > 2000.0, "the view is far from the anchor");
    assert!(
        !p.is_page_at(anchor),
        "the anchor's page stays materialized while it is a selection end"
    );
    let copied = |t: &T| t.text()[t.ed.session.selection().primary().range()].to_string();
    assert_eq!(copied(&p.paged), copied(&p.full));
    let end = sel.focus.offset;
    p.key("edit.deleteBackward");
    assert_eq!(p.paged.text(), format!("{}{}", &src[..anchor], &src[end..]));
    p.key("edit.undo");
    assert_eq!(p.paged.text(), src);
}

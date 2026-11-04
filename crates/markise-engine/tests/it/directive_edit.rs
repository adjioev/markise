//! Editing around the marker lines of `:::name` blocks (ADR 0018): the keyboard never merges
//! text into a marker, never splits one and never leaves half a pair behind; everything inside
//! edits as usual; undo restores the source. Notation: `‸` caret, `⟦…⟧` selection.

use crate::common;
use common::T;
use markise_doc::{DocPos, Selection};
use proptest::prelude::*;

/// Opens `src` with `‸` marking the caret or `⟦…⟧` the selection.
fn open(src: &str) -> T {
    if src.contains('‸') {
        return T::at(src);
    }
    let a = src.find('⟦').expect("selection");
    let z = src.find('⟧').expect("selection") - '⟦'.len_utf8();
    let mut t = T::new(&src.replace(['⟦', '⟧'], ""));
    t.ed.session.set_selection(Selection::range(a, z));
    t
}

fn script(src: &str, steps: &[(&str, &str)]) {
    let mut t = open(src);
    for (k, want) in steps {
        if let Some(s) = k.strip_prefix("type:") {
            t.typ(s);
        } else {
            t.run(k);
        }
        assert_eq!(t.shown(), *want, "after {k} (from {src:?})");
    }
}

#[test]
fn backspace_at_the_first_line_inside_does_not_merge_into_the_opener() {
    script(
        ":::note\n\n‸text\n\n:::\n",
        &[("edit.deleteBackward", ":::note\n\n‸text\n\n:::\n")],
    );
    // without blank lines too
    script(
        ":::note\n‸text\n:::\n",
        &[("edit.deleteBackward", ":::note\n‸text\n:::\n")],
    );
}

#[test]
fn delete_at_the_last_line_inside_does_not_merge_the_closer() {
    script(
        ":::note\n\ntext‸\n\n:::\n",
        &[("edit.deleteForward", ":::note\n\ntext‸\n\n:::\n")],
    );
    script(
        ":::note\ntext‸\n:::\n",
        &[("edit.deleteForward", ":::note\ntext‸\n:::\n")],
    );
}

#[test]
fn backspace_and_delete_next_to_a_block_from_outside() {
    script(
        ":::note\nx\n:::\n\n‸after\n",
        &[("edit.deleteBackward", ":::note\nx\n:::\n\n‸after\n")],
    );
    script(
        "before‸\n\n:::note\nx\n:::\n",
        &[("edit.deleteForward", "before‸\n\n:::note\nx\n:::\n")],
    );
}

#[test]
fn typing_and_deleting_inside_edit_only_the_content() {
    script(
        ":::note\n\nte‸xt\n\n:::\n",
        &[
            ("type:!", ":::note\n\nte!‸xt\n\n:::\n"),
            ("edit.deleteBackward", ":::note\n\nte‸xt\n\n:::\n"),
            ("edit.undo", ":::note\n\nte!‸xt\n\n:::\n"),
        ],
    );
}

#[test]
fn enter_inside_splits_the_paragraph_and_stays_inside() {
    let mut t = T::at(":::note\n\nab‸cd\n\n:::\n");
    t.run("edit.newline");
    assert_eq!(t.text(), ":::note\n\nab\n\ncd\n\n:::\n");
    assert_eq!(t.ed.session.doc().named_containers_at(t.caret().offset).len(), 1);
}

#[test]
fn enter_at_the_end_of_the_last_line_then_typing_stays_inside() {
    let mut t = T::at(":::note\n\ntext‸\n\n:::\n");
    t.run("edit.newline");
    t.typ("x");
    assert_eq!(t.text(), ":::note\n\ntext\n\nx\n\n:::\n");
    t.run("edit.undo");
    assert_eq!(t.text(), ":::note\n\ntext\n\n:::\n");
    // no blank line before the closer: it stays closed
    let mut t = T::at(":::note\ntext‸\n:::\n");
    t.run("edit.newline");
    t.typ("x");
    assert_eq!(t.text(), ":::note\ntext\n\nx\n\n:::\n");
    assert_eq!(t.ed.session.doc().named_containers_at(t.caret().offset).len(), 1);
}

#[test]
fn an_empty_block_takes_typing_on_its_virtual_paragraph() {
    let src = ":::note\n:::\n";
    let mut t = T::new(src);
    let gap = ":::note".len();
    assert_eq!(t.ed.session.doc().gap_virt(gap), Some(1));
    t.ed.session
        .set_selection(Selection::caret_at(DocPos::with_virt(gap, 1)));
    t.typ("x");
    assert_eq!(t.text(), ":::note\n\nx\n\n:::\n");
    t.run("edit.undo");
    assert_eq!(t.text(), src);
}

#[test]
fn a_paragraph_typed_after_a_block_stands_outside() {
    let src = ":::note\nx\n:::\n\n```\ncode\n```\n";
    let mut t = T::new(src);
    let gap = ":::note\nx\n:::".len();
    assert_eq!(t.ed.session.doc().gap_virt(gap), Some(1));
    t.ed.session
        .set_selection(Selection::caret_at(DocPos::with_virt(gap, 1)));
    t.typ("y");
    assert_eq!(t.text(), ":::note\nx\n:::\n\ny\n\n```\ncode\n```\n");
    assert!(t.ed.session.doc().named_containers_at(t.caret().offset).is_empty());
}

#[test]
fn marker_lines_are_not_edited_by_the_keyboard() {
    for src in [":::no‸te\nx\n:::\n", ":::note\nx\n:‸::\n"] {
        let mut t = T::at(src);
        let plain = t.text().to_string();
        t.typ("z");
        assert_eq!(t.text(), plain, "typing in {src:?}");
        t.run("edit.deleteBackward");
        assert_eq!(t.text(), plain, "Backspace in {src:?}");
        t.run("edit.deleteForward");
        assert_eq!(t.text(), plain, "Delete in {src:?}");
    }
    // Enter does not split a marker: it goes to the virtual paragraph after the line
    let mut t = T::at(":::no‸te\nx\n:::\n");
    t.run("edit.newline");
    assert_eq!(t.text(), ":::note\nx\n:::\n");
    assert_eq!(t.caret(), DocPos::with_virt(":::note".len(), 1));
}

#[test]
fn block_commands_leave_marker_lines_alone() {
    for id in [
        "block.heading1",
        "block.paragraph",
        "block.quote",
        "block.bulletList",
        "block.orderedList",
        "block.taskList",
        "block.insertRule",
        "edit.moveBlockUp",
        "edit.moveBlockDown",
    ] {
        // with the caret on a marker, and with a selection running over one
        let t = T::at("a\n\n:::no‸te\nx\n:::\n\nb\n");
        assert!(t.ed.registry.enabled(&t.ed.session, id).is_err(), "{id} on a marker");
        let t = open("⟦a\n\n:::note\nx⟧\n:::\n\nb\n");
        assert!(t.ed.registry.enabled(&t.ed.session, id).is_err(), "{id} over a marker");
    }
    // a command on the content inside works as anywhere
    script(
        ":::note\n\nte‸xt\n\n:::\n",
        &[("block.heading1", ":::note\n\n# te‸xt\n\n:::\n")],
    );
    // a block moved past a marker leaves the pair intact
    let mut t = T::at(":::note\n\nte‸xt\n\n:::\n");
    t.run("edit.moveBlockDown");
    assert_eq!(markers(&t), (1, 0));
}

#[test]
fn a_selection_cutting_a_block_keeps_the_markers_and_deletes_the_rest() {
    // from before the block into it: the opener stays
    script(
        "one ⟦two\n\n:::note\n\nthree⟧ four\n\n:::\n",
        &[("edit.deleteBackward", "one ‸\n:::note\n four\n\n:::\n")],
    );
    // from inside the block past it: the closer stays
    script(
        ":::note\n\none ⟦two\n\n:::\n\nthree⟧ four\n",
        &[("edit.deleteForward", ":::note\n\none ‸\n:::\n four\n")],
    );
    // from one block into the next: both of the markers in between stay
    script(
        ":::a\nx⟦x\n:::\n\nmid\n\n:::b\ny⟧y\n:::\n",
        &[("edit.deleteBackward", ":::a\nx‸\n:::\n\n:::b\ny\n:::\n")],
    );
    // typing over it: the text goes where the selection started
    script(
        "one ⟦two\n\n:::note\n\nthree⟧ four\n\n:::\n",
        &[
            ("type:Z", "one Z‸\n:::note\n four\n\n:::\n"),
            ("edit.undo", "one ‸\n:::note\n four\n\n:::\n"),
        ],
    );
}

/// A selection reaching only the indentation of a marker line still touches that line.
#[test]
fn a_selection_ending_in_a_markers_indentation_keeps_its_line() {
    let src = "before\n  :::note\nx\n:::\n";
    let mut t = T::new(src);
    t.ed.session.set_selection(Selection::range(6, 8));
    t.run("edit.deleteBackward");
    assert_eq!(t.text(), src);
    assert_eq!(markers(&t), (1, 0));
    // typing there does not pull the marker into the line above either, nor does a caret
    // in the indentation edit the line
    let mut t = T::new(src);
    t.set(8);
    t.typ("z");
    assert_eq!(t.text(), src);
}

#[test]
fn a_selection_holding_a_whole_block_deletes_it() {
    script(
        "one⟦\n\n:::note\n\nx\n\n:::\n\n⟧two\n",
        &[("edit.deleteBackward", "one‸two\n")],
    );
    script("⟦:::note\nx\n:::⟧\n\ntwo\n", &[("edit.deleteBackward", "‸\n\ntwo\n")]);
}

#[test]
fn enter_after_a_typed_opener_writes_the_closer() {
    let mut t = T::new("");
    t.typ(":::note x");
    assert_eq!(t.text(), ":::note x", "as typed");
    assert!(t.ed.session.doc().named_containers_at(0).is_empty());
    t.run("edit.newline");
    assert_eq!(t.text(), ":::note x\n\n:::");
    t.typ("y");
    assert_eq!(t.text(), ":::note x\n\ny\n\n:::");
    let d = t.ed.session.doc();
    assert_eq!(d.named_containers_at(t.caret().offset).len(), 1);
}

#[test]
fn a_longer_fence_is_kept_by_its_closer() {
    let mut t = T::new("");
    t.typ("::::outer");
    t.run("edit.newline");
    assert_eq!(t.text(), "::::outer\n\n::::");
}

#[test]
fn leaving_a_typed_opener_without_enter_keeps_it_text() {
    let mut t = T::new("first\n\nsecond\n");
    let end = t.text().len();
    t.set(end);
    t.run("edit.newline");
    t.typ(":::note");
    let typed_at = t.text().find(":::note").unwrap();
    // the caret leaves the line
    t.set(2);
    t.ed.session.expire_hints_left_behind();
    assert!(t.text().contains("\\:::note"), "{:?}", t.text());
    assert!(t.ed.session.doc().named_containers_at(typed_at).is_empty());
}

#[test]
fn in_a_quote_the_same_text_is_plain_text_with_no_hint() {
    let mut t = T::at("> ‸\n");
    t.typ(":::a");
    t.set(0);
    t.ed.session.expire_hints_left_behind();
    assert!(t.text().contains(":::a"));
    assert!(!t.text().contains('\\'));
}

#[test]
fn inline_styles_over_a_block_skip_the_markers() {
    let mut t = open("⟦one\n\n:::note\n\ntwo⟧\n\n:::\n");
    t.run("format.strong");
    let d = t.text().to_string();
    assert!(d.contains("\n:::note\n"), "{d:?}");
    assert!(d.contains("\n:::\n"), "{d:?}");
    assert!(d.contains("**"), "{d:?}");
}

/// The same keys on a paged and a full model, with a block spanning many pages: the text and
/// the caret agree, and the paged model equals a full build where it holds real items.
#[test]
fn editing_across_pages_matches_a_full_model() {
    use markise_layout::Paging;
    let mut src = String::from(":::wide\n\n");
    for i in 0..60 {
        src.push_str(&format!("## Head {i}\n\nPara {i} has words.\n\n"));
    }
    src.push_str(":::\n\nAfter.\n");
    let mut p = T::paged(&src, 4);
    let mut f = T::with_paging(&src, Paging::Full);
    for t in [&mut p, &mut f] {
        t.fix_selection = true;
    }
    assert!(p.ed.view.model().page_count() > 5);
    let at = src.find("Para 30").unwrap() + 2;
    let steps: &[&str] = &[
        "edit.newline",
        "type:x",
        "edit.deleteBackward",
        "edit.undo",
        "edit.undo",
    ];
    for t in [&mut p, &mut f] {
        t.set(at);
    }
    for k in steps {
        for t in [&mut p, &mut f] {
            if let Some(s) = k.strip_prefix("type:") {
                t.typ(s);
            } else {
                t.run(k);
            }
        }
        assert_eq!(p.text(), f.text(), "{k}");
        assert_eq!(p.ed.session.selection(), f.ed.session.selection(), "{k}");
        p.ed.view.model().check_against_full(p.ed.session.doc()).unwrap();
    }
    // a closer removed through the source: the pages follow
    let end = p.text().rfind(":::").unwrap();
    for t in [&mut p, &mut f] {
        t.ed.session
            .apply_external(&markise_doc::PatchSet::single_verbatim(end..end + 3, ""), "Edit", false)
            .unwrap();
        t.ed.sync();
        t.frame();
    }
    p.ed.view.model().check_against_full(p.ed.session.doc()).unwrap();
    assert!(p.ed.session.doc().named_containers_at(at).is_empty());
}

/// Marker lines are no caret stops: the arrows go from outside to the first line inside.
#[test]
fn the_caret_skips_marker_lines() {
    let mut t = T::at("be‸fore\n\n:::note\n\ninside\n\n:::\n\nafter\n");
    t.fix_selection = true;
    t.run("caret.down");
    assert!(t.selection_on_stops());
    let le = t.text().find("inside").unwrap();
    assert!((le..le + 6).contains(&t.caret().offset), "{}", t.shown());
    t.run("caret.down");
    assert!(t.caret().offset >= t.text().find("after").unwrap(), "{}", t.shown());
    // an empty block: the caret stops on its virtual paragraph
    let mut t = T::at("be‸fore\n\n:::note\n:::\n\nafter\n");
    t.fix_selection = true;
    t.run("caret.down");
    assert_eq!(t.caret(), DocPos::with_virt("before\n\n:::note".len(), 1));
    t.typ("x");
    assert_eq!(t.text(), "before\n\n:::note\n\nx\n\n:::\n\nafter\n");
}

/// Content in a card starts further in and wraps earlier than the same text outside it.
#[test]
fn a_card_insets_its_content() {
    use markise_layout::block::NoImages;
    let mut t = T::at("outs‸ide\n\n:::note\n\ninside\n\n:::\n");
    let x = |t: &mut T| {
        let mut f = t.fonts.with_pixels_per_point(1.0);
        t.ed.caret_rect(&mut f, &NoImages).unwrap().min.x
    };
    let a = x(&mut t);
    let at = t.text().find("inside").unwrap() + 4;
    t.set(at);
    t.frame();
    let b = x(&mut t);
    assert!(b > a, "inside {b}, outside {a}");
}

/// Enter at the start of the first line inside: an empty paragraph above it, inside.
#[test]
fn enter_at_the_start_of_the_first_line_inside() {
    let mut t = T::at(":::note\n\n‸text\n\n:::\n");
    t.run("edit.newline");
    assert_eq!(t.caret(), DocPos::with_virt(":::note".len(), 1));
    t.typ("x");
    assert_eq!(t.text(), ":::note\n\nx\n\ntext\n\n:::\n");
}

/// Enter at the start of a block next to a marker: an empty paragraph above it, on the same
/// side of the marker, as next to any other block.
#[test]
fn enter_at_the_start_of_a_block_after_a_marker() {
    // the first block inside: the paragraph stands inside
    for (src, typed) in [
        (":::note\n\n‸text\n\n:::\n", ":::note\n\nnew\n\ntext\n\n:::\n"),
        (":::note\n‸text\n:::\n", ":::note\n\nnew\n\ntext\n:::\n"),
        (":::note\n\n# ‸title\n\n:::\n", ":::note\n\nnew\n\n# title\n\n:::\n"),
        // after a block: outside it
        (":::note\nx\n:::\n\n‸after\n", ":::note\nx\n:::\n\nnew\n\nafter\n"),
    ] {
        let mut t = T::at(src);
        let at = t.caret().offset;
        t.run("edit.newline");
        assert!(t.caret().virt > 0, "{src:?}: {}", t.shown());
        t.typ("new");
        assert_eq!(t.text(), typed, "{src:?}");
        let inside = t.ed.session.doc().named_containers_at(t.caret().offset).len();
        assert_eq!(inside, usize::from(src.find("after").is_none()), "{src:?}");
        while t.ed.session.undo() {}
        assert_eq!(t.text(), src.replace('‸', ""), "{at}");
    }
    // Shift+Enter there acts the same way or waits; it never writes into the marker
    let mut t = T::at(":::note\n\n‸text\n\n:::\n");
    t.run("edit.softBreak");
    t.typ("n");
    assert_eq!(markers(&t), (1, 0), "{}", t.shown());
}

/// An empty heading next to a marker: it goes, the marker stays, the caret lands off it.
#[test]
fn an_empty_heading_next_to_a_marker() {
    let mut t = T::at(":::note\n\n## ‸\n\n:::\n");
    t.run("edit.newline");
    assert_eq!(markers(&t), (1, 0), "{}", t.shown());
    assert_eq!(t.caret(), DocPos::with_virt(":::note".len(), 1), "{}", t.shown());
    let mut t = T::at(":::note\n\n## ‸\n\n:::\n");
    t.run("edit.deleteBackward");
    assert_eq!(markers(&t), (1, 0), "{}", t.shown());
    assert_eq!(t.caret(), DocPos::with_virt(":::note".len(), 1), "{}", t.shown());
    // Delete: the block after it is the closer, which takes no caret
    let mut t = T::at(":::note\n\n## ‸\n\n:::\n");
    t.run("edit.deleteForward");
    assert_eq!(markers(&t), (1, 0), "{}", t.shown());
    assert!(
        !t.ed.session.doc().leaf(t.caret().offset).is_some_and(|l| {
            matches!(l.kind, markise_doc::LeafKind::ContainerMarker { .. }) && t.caret().virt == 0
        }),
        "{}",
        t.shown()
    );
}

/// Commands that change no structure work over a selection holding markers.
#[test]
fn selecting_and_clearing_format_work_over_markers() {
    let mut t = open("⟦**one**\n\n:::note\n\n**two**⟧\n\n:::\n");
    t.run("format.clear");
    assert_eq!(t.text(), "one\n\n:::note\n\ntwo\n\n:::\n");
    // selecting further from a selection that reached a marker
    for id in ["select.line", "select.block", "select.word", "select.scope"] {
        let t = open("⟦one\n\n:::note\n\ntwo⟧\n\n:::\n");
        // as over the same selection with a paragraph in place of the marker
        let plain = open("⟦one\n\nnote\n\ntwo⟧\n\nend\n");
        assert_eq!(
            t.ed.registry.enabled(&t.ed.session, id).is_ok(),
            plain.ed.registry.enabled(&plain.ed.session, id).is_ok(),
            "{id}"
        );
    }
    let mut t = T::at("o‸ne\n\n:::note\n\ntwo\n\n:::\n");
    t.run("select.block");
    t.run("select.block");
    assert_eq!(markers(&t), (1, 0));
}

/// With the caret on a virtual paragraph, the frame goes on through that line: the item it
/// follows draws its side edges to its full height, at any depth; a closer's own card ends at
/// its bottom edge and the paragraph after it stands outside.
#[test]
fn card_frames_run_through_virtual_paragraphs() {
    use markise_layout::Prim;
    use markise_layout::block::NoImages;
    let sides = |t: &mut T| -> (usize, f32, Vec<(f32, f32)>, f32) {
        t.frame();
        let m = t.ed.view.model().clone();
        let at = t.caret().offset;
        let i = (0..m.items.len())
            .rfind(|&i| m.item_start(i) <= at)
            .expect("an item at the caret");
        let mut f = t.fonts.with_pixels_per_point(1.0);
        let l = t.ed.view.layout(i, &mut f, &NoImages);
        let vc = l.virtual_caret.expect("the virtual paragraph is laid out");
        let v: Vec<(f32, f32)> = l
            .prims
            .iter()
            .filter_map(|p| match p {
                Prim::Line { a, b, .. } if a.x == b.x => Some((a.x, b.y)),
                _ => None,
            })
            .collect();
        (i, l.height, v, vc.min.x)
    };
    let src = "::::outer\n\n:::inner\n\ntext\n\n```\ncode\n```\n\n:::\n\nend\n\n::::\n\nout\n";
    // Enter at the end of a paragraph in the inner card: both cards go through the new line
    let mut t = T::at(&src.replace("text\n", "text‸\n"));
    t.run("edit.newline");
    assert!(t.caret().virt > 0);
    let (_, h, v, _) = sides(&mut t);
    assert_eq!(v.len(), 4);
    assert!(v.iter().all(|&(_, b)| b == h), "{v:?} to {h}");
    // the slot after a code block, the last line inside
    let gap = src.find("```\n\n:::").unwrap() + 3;
    t.ed.session
        .set_selection(Selection::caret_at(DocPos::with_virt(gap, 1)));
    let (_, h, v, _) = sides(&mut t);
    assert_eq!(v.len(), 4);
    assert!(v.iter().all(|&(_, b)| b == h), "{v:?} to {h}");
    // an empty card: after its opener, the first line inside
    let mut t = T::new(":::note\n:::\n");
    t.ed.session.set_selection(Selection::caret_at(DocPos::with_virt(7, 1)));
    let (_, h, v, x) = sides(&mut t);
    assert_eq!(v.len(), 2);
    assert!(v.iter().all(|&(_, b)| b == h), "{v:?} to {h}");
    assert!(x > v[0].0 && x < v[1].0, "the caret inside the edges");
    // after an inner closer: the outer card goes on, the inner one has ended; the caret is
    // inside the outer card and outside the inner one
    let src = "::::outer\n\n:::inner\n\nx\n\n:::\n\n```\ncode\n```\n\n::::\n";
    let mut t = T::new(src);
    let gap = src.find(":::\n\n```").unwrap() + 3;
    t.ed.session
        .set_selection(Selection::caret_at(DocPos::with_virt(gap, 1)));
    let (_, h, v, x) = sides(&mut t);
    assert_eq!(v.len(), 4);
    let mut xs = v.clone();
    xs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (outer_l, inner_l) = (xs[0], xs[1]);
    assert_eq!(outer_l.1, h, "the outer card reaches the bottom");
    assert!(inner_l.1 < h, "the inner card ended above the paragraph");
    assert_eq!(
        x, inner_l.0,
        "the caret lines up with the inner card's edge, in the outer card"
    );
}

// ---- properties ----------------------------------------------------------------------------

/// `(pairs, marker lines that pair with nothing)`.
fn markers(t: &T) -> (usize, usize) {
    let d = markise_md::parse(t.text());
    let stray = d
        .tops()
        .filter(|x| matches!(x.block.kind, markise_md::BlockKind::Paragraph))
        .filter(|x| {
            let s = x.span().slice(t.text());
            s.starts_with(":::") && s.lines().count() == 1
        })
        .count();
    (d.directives().len(), stray)
}

fn doc() -> String {
    "a text\n\n:::::one x\n\nb text\n\n- item\n- two\n\n::::two\n\n:::three\n\nc text\n\n:::\n\nd text\n\n::::\n\n:::::\n\ne text\n\n:::four\n:::\n\nf text\n".into()
}

const CMDS: &[&str] = &["edit.deleteBackward", "edit.deleteForward", "edit.newline"];

proptest! {
    /// Random carets, selections and keys in the rendered editor: a caret key never changes
    /// the pairs, a selection removes only pairs it holds whole, no marker is left behind, and
    /// undo gets back to the source.
    #[test]
    fn the_keyboard_does_not_break_a_block(
        ops in prop::collection::vec((any::<prop::sample::Index>(), 0usize..30, 0usize..CMDS.len()), 1..12)
    ) {
        let src = doc();
        let mut t = T::new(&src);
        prop_assert_eq!(markers(&t), (4, 0));
        for (at, len, cmd) in ops {
            let n = t.text().len();
            let a = at.index(n + 1);
            let z = (a + len).min(n);
            if !t.text().is_char_boundary(a) || !t.text().is_char_boundary(z) {
                continue;
            }
            let caret = len % 3 == 0;
            t.ed.session.set_selection(if caret { Selection::caret(a) } else { Selection::range(a, z) });
            let before = t.text().to_string();
            let (pairs, _) = markers(&t);
            t.run(CMDS[cmd]);
            let (now, stray) = markers(&t);
            prop_assert_eq!(stray, 0, "{} at {}..{} on {:?}: {:?}", CMDS[cmd], a, z, before, t.text());
            if caret {
                prop_assert_eq!(now, pairs, "{} at {} on {:?}", CMDS[cmd], a, before);
            }
            t.check_model();
        }
        while t.ed.session.undo() {}
        prop_assert_eq!(t.text(), src.as_str());
    }
}

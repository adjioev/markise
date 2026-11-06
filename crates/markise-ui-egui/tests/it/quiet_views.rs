//! Calm native views on the real app (ADR 0021): the options of `editor/views.json`, block controls
//! that show when wanted, and a ground painted once.

use egui::{Key, Rect, Shape};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_ui_egui::MarkiseApp;

use crate::block_views::{Fixture, caret, shown, text};
use crate::common;

type H = Harness<'static, MarkiseApp>;

const NOTE: &str = r#"{"label": "Note label", "fields": [
    {"name": "title", "type": "heading", "level": 3, "min": 0},
    {"name": "body", "type": "text"}
]}"#;
const QUIET: &str = r#"{"label": "Quiet label", "fields": [
    {"name": "title", "type": "heading", "level": 3, "min": 0},
    {"name": "body", "type": "text"}
]}"#;
const VIEWS: &str = r#"{"note": {"kind": "callout", "accent": "green"},
    "quiet": {"kind": "callout", "accent": "blue", "fill": false, "label": false}}"#;
const MANIFEST: &str =
    r#"{"format":"markise-block-package/2","library":"sample","version":"1.0.0","blocks":["note","quiet"]}"#;
const PALETTE: &str = r#"{"groups": [], "blocks": {}, "presets": []}"#;

const DOC: &str = "Before.\n\n:::note\n\n### Noted\n\nThe first paragraph.\n\nThe second paragraph.\n\nThe third paragraph.\n\n:::\n\nBetween.\n\n:::quiet\n\n### Its heading\n\nIts body.\n\n:::\n\nAfter.\n";

fn fixture() -> Fixture {
    Fixture::with_package(
        DOC,
        &[
            ("manifest.json", MANIFEST),
            ("components/note/note.json", NOTE),
            ("components/quiet/quiet.json", QUIET),
            ("editor/palette.json", PALETTE),
            ("editor/views.json", VIEWS),
        ],
    )
}

fn shapes_in(shape: &Shape, within: Rect) -> bool {
    match shape {
        Shape::Vec(all) => all.iter().any(|s| shapes_in(s, within)),
        Shape::Noop => false,
        other => {
            let r = other.visual_bounding_rect();
            r.is_positive() && within.expand(1.0).contains_rect(r)
        }
    }
}

/// Something is painted in the place of this control in the last frame.
pub(crate) fn drawn(h: &mut H, label: &str) -> bool {
    let r = h.get_all_by_label(label).next().expect(label).rect();
    h.output().shapes.iter().any(|c| shapes_in(&c.shape, r))
}

fn item_rect(h: &H, needle: &str) -> Rect {
    let at = text(h).find(needle).unwrap();
    h.state().viewer_item_rect(at).unwrap()
}

const CONTROLS: [&str; 3] = ["Edit Block: note", "Show Markdown: note", "Delete Block: note"];

fn controls_drawn(h: &mut H) -> [bool; 3] {
    CONTROLS.map(|c| drawn(h, c))
}

#[test]
fn no_label_row_leaves_the_heading_and_the_name_for_a_screen_reader() {
    let f = fixture();
    let mut h = f.ready();
    let all = shown(&mut h);
    assert!(all.contains("Note label"), "{all}");
    assert!(!all.contains("Quiet label"), "{all}");
    assert!(all.contains("Its heading"), "{all}");
    assert!(
        h.query_by_label("Quiet label").is_some(),
        "the accessibility tree has it"
    );
    assert!(
        h.query_by_label("Note label").is_none(),
        "a label on screen says it itself"
    );
    // a press on the heading lands in it
    let r = item_rect(&h, "Its heading");
    h.hover_at(r.left_top() + egui::vec2(30.0, r.height() - 8.0));
    h.step();
    let p = r.left_top() + egui::vec2(30.0, r.height() - 8.0);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        h.step();
    }
    common::run(&mut h);
    let at = h
        .state_mut()
        .viewer()
        .unwrap()
        .ed
        .session
        .selection()
        .primary()
        .focus
        .offset;
    let heading = DOC.find("Its heading").unwrap();
    assert!((heading..heading + "Its heading".len()).contains(&at), "{at}");
    // the controls are on the row of the heading and in reach
    let edit = h.get_all_by_label("Edit Block: quiet").next().unwrap().rect();
    assert!(edit.intersects(r), "{edit:?} {r:?}");
}

#[test]
fn a_blocks_controls_show_on_hover_and_with_the_caret_and_nothing_moves() {
    let f = fixture();
    let mut h = f.ready();
    caret(&mut h, DOC.find("After").unwrap());
    h.hover_at(item_rect(&h, "Before").center());
    common::run(&mut h);
    assert_eq!(controls_drawn(&mut h), [false; 3], "quiet");
    let rects = CONTROLS.map(|c| h.get_all_by_label(c).next().unwrap().rect());
    let body = item_rect(&h, "The second");

    h.hover_at(body.center());
    common::run(&mut h);
    assert_eq!(controls_drawn(&mut h), [true; 3], "the pointer is on the block");
    assert_eq!(CONTROLS.map(|c| h.get_all_by_label(c).next().unwrap().rect()), rects);
    assert_eq!(item_rect(&h, "The second"), body, "no layout shift");
    assert!(!drawn(&mut h, "Edit Block: quiet"), "only that block's");

    h.hover_at(item_rect(&h, "Before").center());
    common::run(&mut h);
    assert_eq!(controls_drawn(&mut h), [false; 3], "the pointer left");

    caret(&mut h, DOC.find("The third").unwrap());
    common::run(&mut h);
    assert_eq!(controls_drawn(&mut h), [true; 3], "the caret is in the block");
    assert_eq!(item_rect(&h, "The second"), body, "no layout shift");

    caret(&mut h, DOC.find("After").unwrap());
    common::run(&mut h);
    assert_eq!(controls_drawn(&mut h), [false; 3], "the caret left");
}

#[test]
fn a_hidden_control_is_reached_by_the_keyboard_shows_and_works() {
    let f = fixture();
    let mut h = f.ready();
    caret(&mut h, DOC.find("After").unwrap());
    h.hover_at(item_rect(&h, "Before").center());
    common::run(&mut h);
    assert!(!drawn(&mut h, "Edit Block: note"));
    // out of the text, where Tab is the document's, and back from the end of what can have the keyboard
    h.key_press(Key::Escape);
    common::run(&mut h);
    let focused = |h: &H, label: &str| h.get_all_by_label(label).next().unwrap().is_focused();
    let mut shown_on_the_way = false;
    let mut reached = false;
    for _ in 0..40 {
        h.key_press_modifiers(egui::Modifiers::SHIFT, Key::Tab);
        common::run(&mut h);
        shown_on_the_way |= focused(&h, "Delete Block: note") && drawn(&mut h, "Edit Block: note");
        if focused(&h, "Edit Block: note") {
            reached = true;
            break;
        }
    }
    assert!(reached, "the keyboard comes to the control");
    assert!(shown_on_the_way, "all of the block's show while one of them has it");
    assert!(drawn(&mut h, "Edit Block: note"));
    h.key_press(Key::Enter);
    common::run(&mut h);
    assert!(h.query_by_label("Save").is_some(), "its form opened");
}

/// Translucent rectangles painted over the block's text, in the last frame.
fn grounds_over(h: &H, r: Rect) -> Vec<Rect> {
    fn walk(s: &Shape, r: Rect, out: &mut Vec<Rect>) {
        match s {
            Shape::Vec(all) => all.iter().for_each(|s| walk(s, r, out)),
            Shape::Rect(x) if x.fill.a() > 0 && x.fill.a() < 255 && x.rect.intersects(r) => out.push(x.rect),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &h.output().shapes {
        walk(&c.shape, r, &mut out);
    }
    out
}

#[test]
fn a_callout_is_one_ground_at_a_fractional_zoom() {
    for zoom in [1.0, 1.1, 1.25, 1.33] {
        let f = fixture();
        let mut h = f.ready();
        h.ctx.set_zoom_factor(zoom);
        common::run(&mut h);
        let (top, bottom) = (item_rect(&h, "Noted"), item_rect(&h, "The third"));
        let all = top.union(bottom);
        let grounds = grounds_over(&h, all);
        assert_eq!(grounds.len(), 1, "at {zoom}: {grounds:?}");
        assert!(grounds[0].top() < top.top() + 1.0 && grounds[0].bottom() > bottom.bottom() - 1.0);
    }
}

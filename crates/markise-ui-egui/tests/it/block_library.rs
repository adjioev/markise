//! The block library on the real app (ADR 0022), with a neutral package: when `/` opens it, what it
//! lists, how the keyboard and the mouse work it, and that the document is untouched until a choice.

use egui::{Event, Key};
use egui_kittest::kittest::Queryable;
use markise_doc::{DocPos, Selection};

use crate::block_views::{Fixture, caret, text};
use crate::common;

pub(crate) const PALETTE: &str = r#"{
  "format": 1,
  "groups": [
    {"id": "teach", "label": {"en": "Teaching", "ru": "Обучение"}, "blocks": ["tip"]},
    {"id": "ask", "label": {"en": "Asking", "ru": "Опрос"}, "blocks": ["poll"]}
  ],
  "blocks": {
    "tip": {"description": {"en": "A short hint", "ru": "Короткая подсказка"}, "icon": "bulb"},
    "poll": {"description": {"en": "Answers to choose from"}, "icon": "no-such-file"}
  },
  "presets": [
    {"id": "tip", "block": "tip", "label": {"en": "Tip", "ru": "Совет"}, "keywords": "заметка hint",
     "markdown": ":::tip\n\n### A title {#t1}\n\nHello\n\n:::\n"},
    {"id": "one", "block": "poll", "label": {"en": "One answer", "ru": "Один ответ"},
     "markdown": ":::poll\n\n## A question\n\n- [x] Yes {#a1}\n- [ ] No {#a2}\n\n:::\n"},
    {"id": "many", "block": "poll", "label": {"en": "Several answers", "ru": "Несколько ответов"},
     "markdown": ":::poll many\n\n## A question\n\n- [x] Yes {#a1}\n- [x] No {#a2}\n\n:::\n"}
  ]
}"#;

type H = egui_kittest::Harness<'static, markise_ui_egui::MarkiseApp>;

/// Frames until things are still; the open field's caret blinks, so a fixed number of them.
pub(crate) fn frames(h: &mut H) {
    common::settle(h);
    h.run_steps(4);
    common::settle(h);
}

fn free_line(h: &mut H, end: usize) {
    let v = h.state_mut().viewer().unwrap();
    v.ed.set_selection(Selection::caret_at(DocPos::with_virt(end, 1)));
    v.ed.refresh_focus();
    v.request_refocus();
    common::run(h);
}

fn type_text(h: &mut H, t: &str) {
    h.event(Event::Text(t.to_string()));
    frames(h);
}

fn press(h: &mut H, key: Key) {
    h.key_press(key);
    frames(h);
}

fn titles(h: &H) -> Vec<String> {
    let open = h.state().block_library().expect("the library is open");
    open.titles().into_iter().map(str::to_string).collect()
}

const DOC: &str = "Before.\n";

#[test]
fn a_slash_on_a_free_line_opens_it_and_writes_nothing() {
    let f = Fixture::with_palette(DOC, PALETTE);
    let mut h = f.ready();
    free_line(&mut h, 7);
    type_text(&mut h, "/");
    assert_eq!(
        titles(&h),
        ["Tip", "One answer", "Several answers"],
        "the package's order"
    );
    assert_eq!(text(&h), DOC, "nothing is written");
    assert!(!h.state().is_dirty());
    assert!(h.get_by_label("Tip").rect().width() > 0.0);
}

#[test]
fn an_empty_document_is_a_free_line_too() {
    let f = Fixture::with_palette("", PALETTE);
    let mut h = f.ready();
    caret(&mut h, 0);
    type_text(&mut h, "/");
    assert!(h.state().block_library().is_some());
    assert_eq!(text(&h), "");
}

#[test]
fn a_slash_in_text_or_a_field_is_a_slash() {
    let doc = "Before.\n\n:::tip\n\n### A title\n\nHello\n\n:::\n";
    let f = Fixture::with_palette(doc, PALETTE);
    let mut h = f.ready();
    caret(&mut h, 3);
    type_text(&mut h, "/");
    assert!(h.state().block_library().is_none());
    assert_eq!(text(&h), "Bef/ore.\n\n:::tip\n\n### A title\n\nHello\n\n:::\n");
    let hello = text(&h).find("Hello").unwrap();
    caret(&mut h, hello);
    type_text(&mut h, "/");
    assert!(h.state().block_library().is_none());
    assert!(text(&h).contains("/Hello"));
}

#[test]
fn with_no_package_in_use_a_slash_is_a_slash() {
    // bound and not accepted: the document works as it did
    let f = Fixture::with_palette(DOC, PALETTE);
    let mut h = f.opened();
    free_line(&mut h, 7);
    type_text(&mut h, "/");
    assert!(h.state().block_library().is_none());
    assert_eq!(text(&h), "Before.\n\n/\n");
    // accepted, with nothing to offer
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    free_line(&mut h, 7);
    type_text(&mut h, "/");
    assert!(h.state().block_library().is_none());
    assert_eq!(text(&h), "Before.\n\n/\n");
}

#[test]
fn the_query_filters_in_any_language_and_what_came_with_the_slash_counts() {
    let f = Fixture::with_palette(DOC, PALETTE);
    let mut h = f.ready();
    free_line(&mut h, 7);
    // a fast typist: characters right behind the slash
    h.event(Event::Text("/".to_string()));
    for c in ["п", "о", "д"] {
        h.event(Event::Text(c.to_string()));
    }
    frames(&mut h);
    assert_eq!(h.state().block_library().unwrap().query(), "под");
    assert_eq!(titles(&h), ["Tip"], "by a Russian description: подсказка");
    assert_eq!(text(&h), DOC);
    for (query, want) in [
        ("ответ", 2),
        ("Several", 1),
        ("заметка", 1),
        ("poll", 2),
        ("poLL  Sev", 1),
    ] {
        press(&mut h, Key::Escape);
        free_line(&mut h, 7);
        type_text(&mut h, "/");
        type_text(&mut h, query);
        assert_eq!(titles(&h).len(), want, "{query}");
    }
    assert_eq!(text(&h), DOC);
}

#[test]
fn arrows_move_return_chooses_and_esc_gives_the_keyboard_back() {
    let f = Fixture::with_palette(DOC, PALETTE);
    let mut h = f.ready();
    free_line(&mut h, 7);
    type_text(&mut h, "/");
    let selected = |h: &H| h.state().block_library().unwrap().selected_title().unwrap().to_string();
    assert_eq!(selected(&h), "Tip");
    press(&mut h, Key::ArrowDown);
    assert_eq!(selected(&h), "One answer");
    press(&mut h, Key::ArrowUp);
    press(&mut h, Key::ArrowUp);
    assert_eq!(selected(&h), "Several answers", "round");
    press(&mut h, Key::Enter);
    assert!(h.state().block_library().is_none());
    assert_eq!(h.state().block_library_chosen(), Some(2));
    assert_eq!(text(&h), DOC, "this phase chooses and writes nothing");

    free_line(&mut h, 7);
    type_text(&mut h, "/");
    press(&mut h, Key::Escape);
    assert!(h.state().block_library().is_none());
    assert_eq!(text(&h), DOC);
    type_text(&mut h, "x");
    assert_eq!(text(&h), "Before.\n\nx\n", "typing goes on in the document");
}

#[test]
fn what_turns_out_to_be_text_is_typed() {
    let f = Fixture::with_palette(DOC, PALETTE);
    let mut h = f.ready();
    // two slashes: one, as text
    free_line(&mut h, 7);
    type_text(&mut h, "/");
    type_text(&mut h, "/");
    assert!(h.state().block_library().is_none());
    assert_eq!(text(&h), "Before.\n\n/\n");
    h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
    frames(&mut h);
    assert_eq!(text(&h), DOC);
    // nothing matches: Return types what was typed
    free_line(&mut h, 7);
    type_text(&mut h, "/");
    type_text(&mut h, "usr/bin");
    assert!(titles(&h).is_empty());
    press(&mut h, Key::Enter);
    assert!(h.state().block_library().is_none());
    assert_eq!(text(&h), "Before.\n\n/usr/bin\n");
    h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
    frames(&mut h);
    // Backspace with nothing typed: as if the slash went
    free_line(&mut h, 7);
    type_text(&mut h, "/");
    press(&mut h, Key::Backspace);
    assert!(h.state().block_library().is_none());
    assert_eq!(text(&h), DOC);
}

#[test]
fn the_mouse_chooses_a_press_outside_closes_and_the_pointer_is_an_arrow() {
    let f = Fixture::with_palette(DOC, PALETTE);
    let mut h = f.ready();
    free_line(&mut h, 7);
    type_text(&mut h, "/");
    let row = h.get_by_label("One answer").rect();
    h.hover_at(row.center());
    frames(&mut h);
    assert_eq!(h.ctx.output(|o| o.cursor_icon), egui::CursorIcon::Default);
    assert_eq!(
        h.state().block_library().unwrap().selected_title(),
        Some("One answer"),
        "the pointer takes the selection when it moves"
    );
    h.get_by_label("One answer").click();
    frames(&mut h);
    assert!(h.state().block_library().is_none());
    assert_eq!(h.state().block_library_chosen(), Some(1));

    free_line(&mut h, 7);
    type_text(&mut h, "/");
    let outside = egui::pos2(1050.0, 1300.0);
    let (button, modifiers) = (egui::PointerButton::Primary, egui::Modifiers::NONE);
    h.hover_at(outside);
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos: outside,
            button,
            pressed,
            modifiers,
        });
    }
    frames(&mut h);
    assert!(h.state().block_library().is_none());
    assert_eq!(text(&h), DOC);
}

#[test]
fn another_change_of_the_document_closes_it() {
    let f = Fixture::with_palette(DOC, PALETTE);
    let mut h = f.ready();
    free_line(&mut h, 7);
    type_text(&mut h, "/");
    h.state_mut().viewer().unwrap().ed.edit(|s| s.insert_text("x")).unwrap();
    frames(&mut h);
    assert!(h.state().block_library().is_none());
}

#[test]
fn icons_come_by_name_through_the_worker() {
    let f = Fixture::with_palette(DOC, PALETTE);
    let mut h = f.ready();
    free_line(&mut h, 7);
    type_text(&mut h, "/");
    // a name with no file is no icon and no error; the rows still show
    assert!(h.get_by_label("Tip").rect().height() > 0.0);
    assert!(h.get_by_label("One answer").rect().height() > 0.0);
    frames(&mut h);
    assert!(h.state().background_settled());
    // the name with a file gets its picture
    assert_eq!(h.state().block_library().unwrap().pictures(), 1);
}

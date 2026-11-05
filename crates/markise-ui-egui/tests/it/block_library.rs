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
    assert!(text(&h).contains(":::poll many"), "{}", text(&h));
    h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
    frames(&mut h);
    assert_eq!(text(&h), DOC);

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
    h.step();
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Default);
    // over the text beside it the pointer is what it is there: this reads a real answer
    let beyond = h.get_by_label("One answer").rect().right_center() + egui::vec2(60.0, 0.0);
    h.hover_at(beyond);
    h.step();
    h.step();
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Text);
    h.hover_at(row.center());
    frames(&mut h);
    assert_eq!(
        h.state().block_library().unwrap().selected_title(),
        Some("One answer"),
        "the pointer takes the selection when it moves"
    );
    h.get_by_label("One answer").click();
    frames(&mut h);
    assert!(h.state().block_library().is_none());
    assert!(text(&h).contains(":::poll\n"), "{}", text(&h));
    h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
    frames(&mut h);

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

mod insert {
    use markise_ui_egui::ViewMode;
    use markise_ui_egui::menu::MenuAction;

    use super::*;

    const WITH_A_BAD_ONE: &str = r#"{"format": 1, "presets": [
        {"id": "bad", "block": "tip", "label": "Broken", "markdown": ":::tip\n\nnever closed\n"},
        {"id": "two", "block": "tip", "label": "Two blocks", "markdown": ":::tip\n\nx\n\n:::\n\nmore\n"}
    ]}"#;

    #[test]
    fn return_inserts_at_the_free_line_one_undo_step_with_the_caret_in_the_field() {
        let f = Fixture::with_palette(DOC, PALETTE);
        let mut h = f.ready();
        free_line(&mut h, 7);
        type_text(&mut h, "/");
        press(&mut h, Key::Enter);
        assert_eq!(text(&h), "Before.\n\n:::tip\n\n### A title {#t1}\n\nHello\n\n:::\n");
        // the keyboard is back in the document, in the first field
        type_text(&mut h, "!");
        assert!(text(&h).contains("### A title! {#t1}"), "{}", text(&h));
        let v = h.state_mut().viewer().unwrap();
        let at = v.ed.session.text().find("Hello").unwrap();
        assert!(v.ed.view.native_block(at).is_some(), "a native view, not a card");
        v.ed.edit(|s| s.undo());
        v.ed.edit(|s| s.undo());
        frames(&mut h);
        assert_eq!(text(&h), DOC);
    }

    #[test]
    fn the_command_inserts_after_the_block_holding_the_caret_and_new_ids_come_with_it() {
        let doc = "Before.\n\n:::poll\n\n## Q\n\n- [x] Yes {#a1}\n- [ ] No {#a2}\n\n:::\n\nAfter.\n";
        let f = Fixture::with_palette(doc, PALETTE);
        let mut h = f.ready();
        caret(&mut h, doc.find("Yes").unwrap());
        let ctx = h.ctx.clone();
        assert!(
            h.state_mut()
                .menu_action(&ctx, &MenuAction::Command("block.insertFromLibrary"))
        );
        frames(&mut h);
        assert!(h.state().block_library().is_some(), "the same library");
        type_text(&mut h, "one");
        press(&mut h, Key::Enter);
        assert_eq!(
            text(&h),
            "Before.\n\n:::poll\n\n## Q\n\n- [x] Yes {#a1}\n- [ ] No {#a2}\n\n:::\n\n\
             :::poll\n\n## A question\n\n- [x] Yes {#a3}\n- [ ] No {#a4}\n\n:::\n\nAfter.\n"
        );
    }

    #[test]
    fn the_command_works_in_source_and_side_by_side() {
        for mode in [ViewMode::Source, ViewMode::SideBySide] {
            let f = Fixture::with_palette(DOC, PALETTE);
            let mut h = f.ready();
            h.state_mut().set_view_mode(mode);
            frames(&mut h);
            let ctx = h.ctx.clone();
            if mode == ViewMode::Source {
                assert!(h.state().source_caret().is_some());
            }
            assert!(
                h.state_mut()
                    .menu_action(&ctx, &MenuAction::Command("block.insertFromLibrary"))
            );
            frames(&mut h);
            press(&mut h, Key::Enter);
            assert_eq!(
                text(&h),
                "Before.\n\n:::tip\n\n### A title {#t1}\n\nHello\n\n:::\n",
                "{mode:?}"
            );
            if mode == ViewMode::Source {
                let want = text(&h).find(" {#t1}").unwrap();
                assert_eq!(h.state().source_caret(), Some(want), "the pane's caret in the field");
            }
        }
    }

    #[test]
    fn a_preset_that_is_not_its_block_is_refused_with_a_message() {
        let f = Fixture::with_palette(DOC, WITH_A_BAD_ONE);
        let mut h = f.ready();
        for query in ["Broken", "Two"] {
            free_line(&mut h, 7);
            type_text(&mut h, "/");
            type_text(&mut h, query);
            press(&mut h, Key::Enter);
            assert_eq!(text(&h), DOC);
            assert!(!h.state().is_dirty());
            let banner = h.state().banner_text().unwrap_or_default();
            assert!(banner.contains("was not inserted"), "{banner}");
        }
    }

    #[test]
    fn insert_block_is_in_the_menu_only_with_a_package_and_its_chord_works() {
        let f = Fixture::with_palette(DOC, PALETTE);
        let h = f.opened();
        assert!(h.state().menu_model().item("block.insertFromLibrary").is_none());
        let f = Fixture::with_palette(DOC, PALETTE);
        let mut h = f.ready();
        assert!(
            h.state()
                .menu_model()
                .item("block.insertFromLibrary")
                .is_some_and(|i| i.enabled)
        );
        caret(&mut h, 2);
        let chord = if cfg!(target_os = "macos") {
            egui::Modifiers::ALT | egui::Modifiers::MAC_CMD
        } else {
            egui::Modifiers::CTRL | egui::Modifiers::ALT
        };
        h.key_press_modifiers(chord, Key::B);
        frames(&mut h);
        assert!(h.state().block_library().is_some());
        press(&mut h, Key::Escape);
        assert_eq!(text(&h), DOC);
    }
}

mod moves {
    use egui::{Modifiers, PointerButton, Pos2, pos2};
    use markise_engine::editor::command::Registry;
    use markise_engine::editor::keymap::{Keymap, Platform, reserved_chords};
    use markise_md::MarkdownFormat;
    use markise_ui_egui::menu::{APP_COMMANDS, MenuAction, descriptor_chord};

    use super::*;

    const TIP: &str = ":::tip\n\n### A title\n\nHello\n\n:::";
    const POLL: &str = ":::poll\n\n## Which?\n\n- [x] One {#a1}\n- [ ] Two {#a2}\n\n:::";

    fn doc() -> String {
        format!("Before.\n\n{TIP}\n\nMiddle.\n\n{POLL}\n\nAfter.\n")
    }

    fn chord(up: bool) -> (Modifiers, Key) {
        let m = if cfg!(target_os = "macos") {
            Modifiers::ALT | Modifiers::MAC_CMD
        } else {
            Modifiers::CTRL | Modifiers::SHIFT
        };
        (m, if up { Key::ArrowUp } else { Key::ArrowDown })
    }

    fn opener_rect(h: &H, needle: &str) -> egui::Rect {
        let at = text(h).find(needle).unwrap();
        h.state().viewer_item_rect(at).unwrap()
    }

    fn pointer_to(h: &mut H, p: Pos2) {
        h.hover_at(p);
        frames(h);
    }

    fn button(h: &mut H, p: Pos2, pressed: bool) {
        h.event(Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        frames(h);
    }

    /// The cursor the last frame asked for, from a frame that has run.
    fn cursor(h: &mut H) -> egui::CursorIcon {
        h.step();
        h.output().platform_output.cursor_icon
    }

    /// As a hand moves: a frame per position, on the way from one place to another.
    fn walk(h: &mut H, from: Pos2, to: Pos2, steps: usize) {
        for i in 1..=steps {
            h.hover_at(from + (to - from) * (i as f32 / steps as f32));
            h.step();
        }
    }

    /// As a person does it: from inside the block to the handle over frames in which the pointer
    /// hovers, and a press there. The handle is there all the way. The caller goes on from the point.
    fn pick_up(h: &mut H, inside: &str) -> Pos2 {
        let body = opener_rect(h, inside);
        let from = pos2(body.left() + 40.0, body.center().y);
        h.hover_at(from);
        h.step();
        h.step();
        let handle = h.get_by_label("Move Block").rect();
        let at = handle.center();
        for i in 1..=10 {
            h.hover_at(from + (at - from) * (i as f32 / 10.0));
            h.step();
            assert_eq!(h.get_by_label("Move Block").rect(), handle, "step {i}");
        }
        h.event(Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        h.step();
        at
    }

    fn release(h: &mut H, at: Pos2) {
        h.event(Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        h.step();
        h.step();
        frames(h);
    }

    fn shown_drag(h: &mut H) -> markise_ui_egui::viewer::BlockDragShown {
        let v = h.state_mut().viewer().unwrap();
        v.block_drag_shown().expect("a drag shows").clone()
    }

    fn nothing_to_undo(h: &mut H) -> bool {
        !h.state_mut().viewer().unwrap().ed.session.can_undo()
    }

    /// `:::plain` has no view: a card.
    const PLAIN: &str = ":::plain\n\nPlain text.\n\n:::";

    #[test]
    fn a_press_shows_at_once_that_the_block_is_picked_up() {
        for (block, inside, label) in [
            (TIP, "Hello", "Tip stays here"),
            (PLAIN, "Plain text.", "Plain stays here"),
        ] {
            let d = format!("Before.\n\n{block}\n\nAfter.\n");
            let f = Fixture::with_palette(&d, PALETTE);
            let mut h = f.ready();
            let body = opener_rect(&h, inside);
            pick_up(&mut h, inside);
            // no movement yet
            let s = shown_drag(&mut h);
            let outline = s.outline.expect("the block is outlined");
            assert!(
                outline.top() < body.top() && outline.bottom() > body.bottom(),
                "{outline:?}"
            );
            assert!(
                outline.left() <= body.left() && outline.right() >= body.right(),
                "{outline:?}"
            );
            assert!(!s.moves, "it stays until it goes somewhere");
            assert_eq!(s.label, label);
            assert_eq!(cursor(&mut h), egui::CursorIcon::Grabbing);
        }
    }

    #[test]
    fn a_short_pull_shows_a_place_also_from_a_tall_block() {
        let tall = format!(":::plain\n\n{}:::", "A paragraph.\n\n".repeat(12));
        let d = format!("{tall}\n\nMiddle.\n\nAfter.\n");
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        let at = pick_up(&mut h, "A paragraph.");
        let row = opener_rect(&h, "A paragraph.").height();
        let to = at + egui::vec2(0.0, 3.0 * row);
        walk(&mut h, at, to, 6);
        let s = shown_drag(&mut h);
        assert!(s.moves, "the line is there, with the block's end still far: {s:?}");
        assert_eq!(s.label, "Move Plain here");
        release(&mut h, to);
        assert_eq!(text(&h), format!("Middle.\n\n{tall}\n\nAfter.\n"));
    }

    #[test]
    fn just_out_of_a_block_lands_past_what_is_next() {
        let d = doc();
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        let middle = opener_rect(&h, "Middle.");
        let at = pick_up(&mut h, "Hello");
        let to = pos2(at.x, middle.top() + 2.0);
        walk(&mut h, at, to, 8);
        let s = shown_drag(&mut h);
        assert!(s.moves && s.line_y > middle.top(), "{s:?}");
        release(&mut h, to);
        assert_eq!(text(&h), format!("Before.\n\nMiddle.\n\n{TIP}\n\n{POLL}\n\nAfter.\n"));
    }

    #[test]
    fn going_back_shows_it_stays_and_a_release_or_esc_leaves_no_undo_step() {
        let d = doc();
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        let top = opener_rect(&h, "Before.").center();
        let at = pick_up(&mut h, "Hello");
        walk(&mut h, at, top, 6);
        assert!(shown_drag(&mut h).moves);
        walk(&mut h, top, at, 6);
        let s = shown_drag(&mut h);
        assert!(!s.moves && s.label.ends_with("stays here"), "{s:?}");
        release(&mut h, at);
        assert_eq!(text(&h), d);
        assert!(nothing_to_undo(&mut h) && !h.state().is_dirty());

        let at = pick_up(&mut h, "Hello");
        walk(&mut h, at, top, 6);
        h.key_press(Key::Escape);
        frames(&mut h);
        assert!(!h.state_mut().viewer().unwrap().is_dragging_block());
        release(&mut h, top);
        assert_eq!(text(&h), d);
        assert!(nothing_to_undo(&mut h) && !h.state().is_dirty());
    }

    #[test]
    fn the_handle_has_more_to_hit_than_to_see_and_the_margin_keeps_it() {
        let d = doc();
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        let body = opener_rect(&h, "Hello");
        h.hover_at(body.center());
        h.step();
        h.step();
        let hit = h.get_by_label("Move Block").rect();
        assert!(hit.width() >= 26.0 && hit.height() >= 24.0, "{hit:?}");
        // past it in the margin, it still shows
        let margin = pos2(hit.left() - 10.0, body.center().y);
        h.hover_at(margin);
        h.step();
        assert_eq!(h.get_by_label("Move Block").rect(), hit);
        // the corner of what takes a press, up to the block's edge
        let corner = hit.right_bottom() - egui::vec2(1.0, 1.0);
        walk(&mut h, margin, corner, 4);
        h.event(Event::PointerButton {
            pos: corner,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        h.step();
        assert!(h.state_mut().viewer().unwrap().is_dragging_block());
        release(&mut h, corner);
        assert_eq!(text(&h), d);
    }

    /// Presses the handle of the block with this opener line.
    fn grab(h: &mut H, needle: &str) -> Pos2 {
        let r = opener_rect(h, needle);
        pointer_to(h, r.center());
        let p = h.get_by_label("Move Block").rect().center();
        pointer_to(h, p);
        button(h, p, true);
        p
    }

    #[test]
    fn the_shortcuts_clash_with_nothing_in_the_registry_or_the_app() {
        for platform in [Platform::Mac, Platform::Other] {
            let keymap = Keymap::new(&Registry::for_format(&MarkdownFormat::default()), platform);
            let reserved = reserved_chords(platform);
            for id in [
                "block.insertFromLibrary",
                "block.moveUp",
                "block.moveDown",
                "block.delete",
            ] {
                let d = markise_ui_egui::menu::app_command(id).unwrap();
                let c = descriptor_chord(d, platform).expect("a chord");
                assert!(
                    keymap.lookup(&c).is_empty(),
                    "{platform:?} {id}: {:?}",
                    keymap.lookup(&c)
                );
                assert!(!reserved.contains(&c), "{platform:?} {id}");
                for other in APP_COMMANDS.iter().filter(|o| o.id != id) {
                    assert_ne!(
                        descriptor_chord(other, platform).as_ref(),
                        Some(&c),
                        "{platform:?} {id}, {}",
                        other.id
                    );
                }
            }
        }
    }

    #[test]
    fn the_keys_move_the_block_at_the_caret_one_undo_step_at_a_time() {
        let d = doc();
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        caret(&mut h, d.find("Hello").unwrap() + 1);
        let (m, k) = chord(false);
        h.key_press_modifiers(m, k);
        frames(&mut h);
        assert_eq!(text(&h), format!("Before.\n\nMiddle.\n\n{TIP}\n\n{POLL}\n\nAfter.\n"));
        h.key_press_modifiers(m, k);
        frames(&mut h);
        assert_eq!(
            text(&h),
            format!("Before.\n\nMiddle.\n\n{POLL}\n\n{TIP}\n\nAfter.\n"),
            "past a block, whole"
        );
        // typing goes on in the field
        type_text(&mut h, "x");
        assert!(text(&h).contains("Hxello"), "{}", text(&h));
        let v = h.state_mut().viewer().unwrap();
        for _ in 0..3 {
            v.ed.edit(|s| s.undo());
        }
        frames(&mut h);
        assert_eq!(text(&h), d);
    }

    #[test]
    fn the_menu_says_when_it_can_move() {
        let d = doc();
        let f = Fixture::with_palette(&d, PALETTE);
        let h = f.opened();
        assert!(
            h.state().menu_model().item("block.moveUp").is_none(),
            "no package, no entry"
        );
        let mut h = f.ready();
        let enabled = |h: &H, id: &str| h.state().menu_model().item(id).map(|i| i.enabled);
        caret(&mut h, 2);
        assert_eq!(enabled(&h, "block.moveUp"), Some(false), "not in a block");
        caret(&mut h, d.find("Hello").unwrap());
        assert_eq!(enabled(&h, "block.moveUp"), Some(true));
        assert_eq!(enabled(&h, "block.moveDown"), Some(true));
    }

    #[test]
    fn a_changed_draft_comes_first() {
        let d = doc();
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        h.get_all_by_label("Edit Block: tip").next().unwrap().click();
        frames(&mut h);
        h.get_by_label("Body").click();
        frames(&mut h);
        h.get_by_label("Body").type_text("!");
        frames(&mut h);
        assert!(h.state().block_form().is_some_and(|f| f.is_dirty()));
        let ctx = h.ctx.clone();
        caret(&mut h, d.find("Hello").unwrap());
        assert!(!h.state_mut().menu_action(&ctx, &MenuAction::Command("block.moveDown")));
        assert_eq!(
            h.state().menu_model().item("block.moveDown").map(|i| i.enabled),
            Some(false)
        );
        assert_eq!(text(&h), d);
        assert!(
            h.state().block_form().is_some_and(|f| f.is_dirty()),
            "the draft is still there"
        );
    }

    #[test]
    fn the_handle_shows_only_over_a_block_and_the_pointer_is_a_hand_on_it() {
        let d = doc();
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        assert!(h.query_by_label("Move Block").is_none(), "nothing floats by itself");
        // no package in use, no handle
        let g = Fixture::with_palette(&d, PALETTE);
        let mut plain = g.opened();
        let hello = opener_rect(&plain, "Hello").center();
        pointer_to(&mut plain, hello);
        assert!(plain.query_by_label("Move Block").is_none());
        let over_text = opener_rect(&h, "Before.").center();
        pointer_to(&mut h, over_text);
        assert!(h.query_by_label("Move Block").is_none(), "nothing beside plain text");
        let hello = opener_rect(&h, "Hello").center();
        pointer_to(&mut h, hello);
        let handle = h.get_by_label("Move Block").rect();
        pointer_to(&mut h, handle.center());
        assert!(
            h.query_by_label("Move Block").is_some(),
            "it stays when the pointer goes to it"
        );
        assert_eq!(cursor(&mut h), egui::CursorIcon::Grab);
        assert!(h.state_mut().viewer().unwrap().on_block_handle(), "it stands out");
        pointer_to(&mut h, hello);
        assert!(!h.state_mut().viewer().unwrap().on_block_handle());
    }

    #[test]
    fn dragging_shows_a_place_and_dropping_moves_it_in_one_step() {
        let d = doc();
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        grab(&mut h, ":::tip");
        assert_eq!(
            h.state_mut().viewer().unwrap().block_drag_target(),
            None,
            "it is where it is"
        );
        let to = opener_rect(&h, "After.").left_top();
        let to = pos2(to.x + 20.0, to.y);
        pointer_to(&mut h, to);
        let viewer = h.state_mut().viewer().unwrap();
        assert!(viewer.is_dragging_block());
        assert_eq!(viewer.block_drag_target(), Some(d.find("After.").unwrap()));
        assert_eq!(text(&h), d, "nothing changes before the drop");
        assert_eq!(cursor(&mut h), egui::CursorIcon::Grabbing);
        button(&mut h, to, false);
        assert_eq!(text(&h), format!("Before.\n\nMiddle.\n\n{POLL}\n\n{TIP}\n\nAfter.\n"));
        let v = h.state_mut().viewer().unwrap();
        assert!(!v.is_dragging_block());
        v.ed.edit(|s| s.undo());
        frames(&mut h);
        assert_eq!(text(&h), d, "one undo step");
    }

    #[test]
    fn a_press_on_the_handle_with_no_move_moves_nothing() {
        let d = doc();
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        let p = grab(&mut h, ":::poll");
        button(&mut h, p, false);
        assert_eq!(text(&h), d);
        assert!(!h.state().is_dirty());
    }

    #[test]
    fn esc_cancels_a_drag_and_it_starts_no_selection() {
        let d = doc();
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        caret(&mut h, 2);
        grab(&mut h, ":::poll");
        let top = opener_rect(&h, "Before.").left_top();
        pointer_to(&mut h, pos2(top.x + 20.0, top.y));
        assert!(h.state_mut().viewer().unwrap().block_drag_target().is_some());
        let sel = h.state_mut().viewer().unwrap().ed.session.selection().clone();
        assert!(
            sel.primary().is_caret(),
            "the press was the handle's, not a selection's"
        );
        h.key_press(Key::Escape);
        frames(&mut h);
        assert!(!h.state_mut().viewer().unwrap().is_dragging_block());
        button(&mut h, pos2(top.x + 20.0, top.y), false);
        assert_eq!(text(&h), d);
    }

    #[test]
    fn a_drop_for_a_document_that_changed_since_goes_nowhere() {
        let d = doc();
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        grab(&mut h, ":::tip");
        let to = opener_rect(&h, "After.").left_top();
        let to = pos2(to.x + 20.0, to.y);
        pointer_to(&mut h, to);
        // the frame of the release asks; before the next one the text changes
        h.event(Event::PointerButton {
            pos: to,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        h.step();
        let v = h.state_mut().viewer().unwrap();
        // after the block and the place: both still stand where they did
        let end = d.len() - 1;
        v.ed.set_selection(Selection::caret(end));
        v.ed.edit(|s| s.insert_text("x")).unwrap();
        let now = text(&h);
        frames(&mut h);
        assert_eq!(text(&h), now, "the request named another revision");
    }

    #[test]
    fn a_nested_block_lands_only_in_its_parent() {
        let d = "Before.\n\n::::plain\n\n:::tip\n\nOne\n\n:::\n\nBetween\n\n:::tip\n\nTwo\n\n:::\n\n::::\n\nAfter.\n";
        let f = Fixture::with_palette(d, PALETTE);
        let mut h = f.ready();
        let r = h.state().viewer_item_rect(d.find("Two").unwrap()).unwrap();
        pointer_to(&mut h, r.center());
        let p = h.get_by_label("Move Block").rect().center();
        pointer_to(&mut h, p);
        button(&mut h, p, true);
        // over what is outside the parent: the nearest place is still inside it
        let outside = opener_rect(&h, "Before.").left_top();
        pointer_to(&mut h, pos2(outside.x + 20.0, outside.y));
        let target = h.state_mut().viewer().unwrap().block_drag_target().unwrap();
        assert!(target > d.find("::::plain").unwrap() && target < d.rfind("::::").unwrap());
        button(&mut h, pos2(outside.x + 20.0, outside.y), false);
        assert!(text(&h).starts_with("Before.\n\n::::plain\n\n"), "{}", text(&h));
        assert!(text(&h).ends_with("::::\n\nAfter.\n"), "{}", text(&h));
    }

    #[test]
    fn a_drag_at_the_edge_scrolls_a_long_document_and_lands_far_away() {
        let mut d = format!("{TIP}\n\n");
        for i in 0..1500 {
            d.push_str(&format!("Paragraph {i}\n\n"));
        }
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        grab(&mut h, ":::tip");
        let bottom = pos2(600.0, 1360.0);
        let area = h.state().viewer_item_rect(d.find("Paragraph 0").unwrap()).unwrap();
        let near = pos2(area.left() + 20.0, bottom.y);
        pointer_to(&mut h, near);
        let before = h.state_mut().viewer().unwrap().ed.view.scroll_y();
        for _ in 0..3 {
            frames(&mut h);
        }
        let after = h.state_mut().viewer().unwrap().ed.view.scroll_y();
        assert!(after > before, "the view went on: {before} to {after}");
        let target = h
            .state_mut()
            .viewer()
            .unwrap()
            .block_drag_target()
            .expect("a place on screen");
        assert!(target > 0);
        button(&mut h, near, false);
        let t = text(&h);
        assert!(t.starts_with("Paragraph 0\n"), "{}", &t[..40]);
        assert!(
            t.contains(&format!("\n\n{TIP}\n\nParagraph ")),
            "the block is whole among the paragraphs"
        );
        assert_eq!(t.len(), d.len());
    }
}

mod deletes {
    use egui::{Modifiers, PointerButton, Pos2};
    use markise_doc::{DocPos, Selection};
    use markise_ui_egui::confirm::{Choice, Kind};
    use markise_ui_egui::menu::MenuAction;

    use super::*;

    const TIP: &str = ":::tip\n\n### A title\n\nHello\n\n:::";
    const POLL: &str = ":::poll\n\n## Which?\n\n- [x] One {#a1}\n- [ ] Two {#a2}\n\n:::";
    const PLAIN: &str = ":::plain\n\nPlain text.\n\n:::";

    fn doc() -> String {
        format!("Before.\n\n{TIP}\n\nMiddle.\n\n{POLL}\n\n{PLAIN}\n\nAfter.\n")
    }

    fn rect_of(h: &H, needle: &str) -> egui::Rect {
        let at = text(h).find(needle).unwrap();
        h.state().viewer_item_rect(at).unwrap()
    }

    fn hover(h: &mut H, p: Pos2) {
        h.hover_at(p);
        frames(h);
    }

    /// Goes to the block's text, then its button, and clicks.
    fn click_delete(h: &mut H, inside: &str, label: &str) {
        let r = rect_of(h, inside);
        hover(h, r.center());
        let p = h.get_by_label(label).rect().center();
        hover(h, p);
        for pressed in [true, false] {
            h.event(Event::PointerButton {
                pos: p,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
            h.step();
        }
        frames(h);
    }

    fn chord() -> (Modifiers, Key) {
        if cfg!(target_os = "macos") {
            (Modifiers::ALT | Modifiers::MAC_CMD, Key::Backspace)
        } else {
            (Modifiers::CTRL | Modifiers::SHIFT, Key::Delete)
        }
    }

    fn undo(h: &mut H) {
        h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
        frames(h);
    }

    #[test]
    fn the_button_shows_by_the_others_with_a_label_and_an_arrow_and_one_undo_brings_it_back() {
        let d = doc();
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        assert!(
            h.query_by_label("Delete Block: tip").is_none(),
            "quiet until the block is under the pointer"
        );
        let r = rect_of(&h, "Hello");
        hover(&mut h, r.center());
        let bin = h.get_by_label("Delete Block: tip").rect();
        let pencil = h.get_by_label("Edit Block: tip").rect();
        assert_eq!(bin.top(), pencil.top());
        assert!(bin.right() < pencil.left(), "{bin:?} {pencil:?}");
        assert!(
            bin.right() >= pencil.left() - 2.0 * pencil.width() - 12.0,
            "next to the others"
        );
        hover(&mut h, bin.center());
        h.step();
        assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Default);

        click_delete(&mut h, "Hello", "Delete Block: tip");
        assert_eq!(text(&h), format!("Before.\n\nMiddle.\n\n{POLL}\n\n{PLAIN}\n\nAfter.\n"));
        undo(&mut h);
        assert_eq!(text(&h), d, "one step, exact");
    }

    #[test]
    fn a_card_and_a_nested_block_go_by_the_button_too() {
        let d = doc();
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        click_delete(&mut h, "Plain text.", "Delete Block: plain");
        assert_eq!(text(&h), format!("Before.\n\n{TIP}\n\nMiddle.\n\n{POLL}\n\nAfter.\n"));

        let d = format!("Before.\n\n::::tip\n\nOuter.\n\n{PLAIN}\n\n::::\n\nAfter.\n");
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        click_delete(&mut h, "Plain text.", "Delete Block: plain");
        assert_eq!(text(&h), "Before.\n\n::::tip\n\nOuter.\n\n::::\n\nAfter.\n");
    }

    #[test]
    fn with_no_package_the_button_and_the_command_work_and_the_caret_lands() {
        let d = doc();
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.opened();
        assert!(h.state().block_package().is_none());
        click_delete(&mut h, "Hello", "Delete Block: tip");
        assert_eq!(text(&h), format!("Before.\n\nMiddle.\n\n{POLL}\n\n{PLAIN}\n\nAfter.\n"));

        let t = text(&h);
        caret(&mut h, t.find("Plain text.").unwrap() + 1);
        let (m, k) = chord();
        h.key_press_modifiers(m, k);
        frames(&mut h);
        assert_eq!(text(&h), format!("Before.\n\nMiddle.\n\n{POLL}\n\nAfter.\n"));
        let t = text(&h);
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
        let after = t.find("After.").unwrap();
        assert!(
            (after - 2..=after).contains(&at),
            "between the block before and what follows: {at}"
        );
        undo(&mut h);
        undo(&mut h);
        assert_eq!(text(&h), d);
    }

    #[test]
    fn the_menu_says_when_it_can_and_the_command_goes_by_it_in_source_as_well() {
        let d = doc();
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        let enabled = |h: &H| h.state().menu_model().item("block.delete").map(|i| i.enabled);
        caret(&mut h, 2);
        assert_eq!(enabled(&h), Some(false), "not in a block");
        caret(&mut h, d.find("Hello").unwrap() + 1);
        assert_eq!(enabled(&h), Some(true));
        let ctx = h.ctx.clone();
        assert!(h.state_mut().menu_action(&ctx, &MenuAction::Command("block.delete")));
        frames(&mut h);
        assert!(!text(&h).contains(":::tip"));
        let v = h.state_mut().viewer().unwrap();
        let sel = v.ed.session.selection().primary();
        assert_eq!(sel.focus.offset, "Before.".len(), "the end of the text before");
        // no package, the entry is there
        let g = Fixture::with_palette(&d, PALETTE);
        let plain = g.opened();
        assert!(plain.state().menu_model().item("block.delete").is_some());
    }

    #[test]
    fn a_changed_draft_of_it_asks_first_and_a_clean_panel_closes() {
        let d = doc();
        // nothing changed: the panel closes with the block
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        assert!(h.state_mut().open_block_form(d.find("Hello").unwrap()));
        frames(&mut h);
        click_delete(&mut h, "Hello", "Delete Block: tip");
        assert!(!text(&h).contains(":::tip"));
        assert!(h.state().block_form().is_none());

        // a change: the question, and Cancel keeps both
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        assert!(h.state_mut().open_block_form(d.find("Hello").unwrap()));
        frames(&mut h);
        h.get_by_label("Body").click();
        frames(&mut h);
        h.get_by_label("Body").type_text("!");
        frames(&mut h);
        assert!(h.state().block_form().unwrap().is_dirty());
        click_delete(&mut h, "Hello", "Delete Block: tip");
        assert_eq!(text(&h), d, "not yet");
        assert_eq!(
            h.state().question().expect("the draft's question").kind,
            Kind::BlockDraft
        );
        h.state_mut().answer_question(Choice::Cancel);
        frames(&mut h);
        assert_eq!(text(&h), d);
        assert!(h.state().block_form().unwrap().is_dirty());
        // Discard goes on
        click_delete(&mut h, "Hello", "Delete Block: tip");
        h.state_mut().answer_question(Choice::DontSave);
        frames(&mut h);
        assert!(h.state().block_form().is_none());
        let without = format!("Before.\n\nMiddle.\n\n{POLL}\n\n{PLAIN}\n\nAfter.\n");
        assert_eq!(text(&h), without);
        undo(&mut h);
        assert_eq!(text(&h), d, "the draft is gone, the source is back");

        // Save writes it, and the block goes all the same
        assert!(h.state_mut().open_block_form(d.find("Hello").unwrap()));
        frames(&mut h);
        h.get_by_label("Body").click();
        frames(&mut h);
        h.get_by_label("Body").type_text("!");
        frames(&mut h);
        click_delete(&mut h, "Hello", "Delete Block: tip");
        h.state_mut().answer_question(Choice::Save);
        frames(&mut h);
        assert!(h.state().block_form().is_none());
        assert_eq!(text(&h), without);
        undo(&mut h);
        assert!(text(&h).contains("Hello!"), "one step back: as saved");
    }

    #[test]
    fn a_draft_of_another_block_stays_and_a_selection_elsewhere_stays_too() {
        let d = doc();
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        let (a, b) = (d.find("Before").unwrap(), "Before.".len());
        h.state_mut()
            .viewer()
            .unwrap()
            .ed
            .set_selection(Selection::single(DocPos::new(a), DocPos::new(b)));
        frames(&mut h);
        assert!(h.state_mut().open_block_form(d.find("Hello").unwrap()));
        frames(&mut h);
        h.get_by_label("Body").click();
        frames(&mut h);
        h.get_by_label("Body").type_text("!");
        frames(&mut h);
        click_delete(&mut h, "Plain text.", "Delete Block: plain");
        assert!(!text(&h).contains(":::plain"));
        assert!(
            h.state().block_form().unwrap().is_dirty(),
            "the draft is not this block's"
        );
    }
}

mod review {
    use markise_ui_egui::menu::MenuAction;

    use super::*;

    #[test]
    fn an_ime_enter_with_its_commit_in_one_frame_chooses_nothing_and_writes_nothing() {
        let f = Fixture::with_palette(DOC, PALETTE);
        let mut h = f.ready();
        free_line(&mut h, 7);
        type_text(&mut h, "/");
        h.event(Event::Ime(egui::ImeEvent::Preedit {
            text: "a".to_string(),
            active_range_chars: Some(0..1),
        }));
        frames(&mut h);
        // one frame: the Return that ends the composition, and what it commits
        h.input_mut().events.push(Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.input_mut()
            .events
            .push(Event::Ime(egui::ImeEvent::Commit("ab".to_string())));
        frames(&mut h);
        assert_eq!(text(&h), DOC, "neither a block nor the commit in the document");
        let open = h.state().block_library().expect("Return was the composition's");
        assert_eq!(open.query(), "ab");
    }

    #[test]
    fn closing_takes_a_commit_of_the_same_frame_with_it() {
        let f = Fixture::with_palette(DOC, PALETTE);
        let mut h = f.ready();
        free_line(&mut h, 7);
        type_text(&mut h, "/");
        h.input_mut().events.push(Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.input_mut()
            .events
            .push(Event::Ime(egui::ImeEvent::Commit("ab".to_string())));
        frames(&mut h);
        assert!(h.state().block_library().is_none());
        assert_eq!(text(&h), DOC);
    }

    #[test]
    fn document_commands_wait_while_it_is_open() {
        let f = Fixture::with_palette(DOC, PALETTE);
        let mut h = f.ready();
        free_line(&mut h, 7);
        type_text(&mut h, "/");
        // what is behind the library could take a heading
        h.state_mut().viewer().unwrap().ed.set_selection(Selection::caret(2));
        frames(&mut h);
        assert!(h.state().block_library().is_some());
        let ctx = h.ctx.clone();
        assert_eq!(
            h.state().menu_model().item("block.heading1").map(|i| i.enabled),
            Some(false)
        );
        assert!(!h.state_mut().menu_action(&ctx, &MenuAction::Command("block.heading1")));
        frames(&mut h);
        assert_eq!(text(&h), DOC, "the document behind the library is not its target");
        assert!(h.state().block_library().is_some());
    }

    #[test]
    fn text_and_return_of_one_frame_read_in_that_order() {
        let f = Fixture::with_palette(DOC, PALETTE);
        let mut h = f.ready();
        free_line(&mut h, 7);
        type_text(&mut h, "/");
        let enter = |h: &mut H| {
            h.input_mut().events.push(Event::Text("usr/bin".to_string()));
            h.input_mut().events.push(Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
        };
        enter(&mut h);
        frames(&mut h);
        assert_eq!(text(&h), "Before.\n\n/usr/bin\n");
        assert!(h.state().block_library().is_none());
        // what was typed narrows what Return takes
        h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
        frames(&mut h);
        free_line(&mut h, 7);
        type_text(&mut h, "/");
        h.input_mut().events.push(Event::Text("Several".to_string()));
        h.input_mut().events.push(Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        frames(&mut h);
        assert!(h.state().block_library().is_none());
        assert!(text(&h).contains(":::poll many"), "{}", text(&h));
    }
}

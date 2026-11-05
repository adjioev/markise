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
    assert_eq!(h.ctx.output(|o| o.cursor_icon), egui::CursorIcon::Default);
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
            for id in ["block.insertFromLibrary", "block.moveUp", "block.moveDown"] {
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
    fn the_handle_shows_only_over_a_block_and_the_pointer_is_an_arrow_on_it() {
        let d = doc();
        let f = Fixture::with_palette(&d, PALETTE);
        let mut h = f.ready();
        assert!(h.query_by_label("Move Block").is_none(), "nothing floats by itself");
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
        assert_eq!(h.ctx.output(|o| o.cursor_icon), egui::CursorIcon::Default);
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
        assert_eq!(h.ctx.output(|o| o.cursor_icon), egui::CursorIcon::Default);
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

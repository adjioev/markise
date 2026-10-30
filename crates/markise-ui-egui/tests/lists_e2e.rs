//! M2c list editing end to end (egui_kittest): the real app, mouse clicks into list items,
//! keys routed through the engine keymap (Enter, Backspace, Tab, Shift+Tab, undo), save;
//! asserts the saved bytes and the caret.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use markise_ui_egui::{AppOptions, MarkiseApp};

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-lists-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

type H = Harness<'static, MarkiseApp>;

fn open(name: &str, text: &str) -> (H, PathBuf) {
    let dir = tmpdir(name);
    let p = dir.join("l.md");
    std::fs::write(&p, text).unwrap();
    let app = MarkiseApp::new(AppOptions {
        open: vec![p.clone()],
        ..Default::default()
    });
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    (h, p)
}

fn wait_until(h: &mut H, what: &str, f: impl Fn(&MarkiseApp) -> bool) {
    let t = Instant::now();
    while !f(h.state()) {
        assert!(t.elapsed() < common::HANG_GUARD, "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    h.step();
}

fn cmd() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers {
            mac_cmd: true,
            command: true,
            ..Default::default()
        }
    } else {
        Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        }
    }
}

fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..Default::default()
    }
}

/// Types like the platform does (egui-winit): a key press, the text it produced, the
/// release (the helper of `editing.rs`, issue #31).
fn typ(h: &mut H, s: &str) {
    for c in s.chars() {
        let key = if c == ' ' {
            Some(Key::Space)
        } else {
            Key::from_name(&c.to_string())
        };
        let ev = |k: Key, pressed: bool| Event::Key {
            key: k,
            physical_key: Some(k),
            pressed,
            repeat: false,
            modifiers: Modifiers::default(),
        };
        if let Some(k) = key {
            h.event(ev(k, true));
        }
        h.event(Event::Text(c.to_string()));
        if let Some(k) = key {
            h.event(ev(k, false));
        }
        h.step();
    }
}

fn key(h: &mut H, k: Key) {
    h.key_press(k);
    h.step();
}

fn text(h: &H) -> String {
    h.state().document_text().unwrap()
}

fn caret(h: &mut H) -> (usize, u16) {
    let f = h.state_mut().viewer().unwrap().ed.session.selection().primary().focus;
    (f.offset, f.virt)
}

/// Clicks at the end of the first visual line of the item holding `offset`.
fn click_line_end(h: &mut H, offset: usize) {
    let r = h
        .state_mut()
        .viewer()
        .unwrap()
        .item_screen_rect(offset)
        .expect("item visible");
    let p = egui::pos2(r.right() - 4.0, r.top() + 8.0);
    h.hover_at(p);
    h.drag_at(p);
    h.step();
    h.drop_at(p);
    h.step();
}

/// Clicks just right of the item's left edge (the start of its text).
fn click_line_start(h: &mut H, offset: usize) {
    let r = h
        .state_mut()
        .viewer()
        .unwrap()
        .item_screen_rect(offset)
        .expect("item visible");
    // the item's text starts after the bullet indentation; click slightly into the text
    let p = egui::pos2(r.left() + 1.0, r.top() + 8.0);
    h.hover_at(p);
    h.drag_at(p);
    h.step();
    h.drop_at(p);
    h.step();
    // the click may land before the first character: Home makes sure
    if cfg!(target_os = "macos") {
        h.key_press_modifiers(cmd(), Key::ArrowLeft);
    } else {
        h.key_press(Key::Home);
    }
    h.step();
}

fn save_and_read(h: &mut H, p: &Path) -> String {
    h.key_press_modifiers(cmd(), Key::S);
    wait_until(h, "save", |a| !a.is_busy() && !a.is_dirty());
    std::fs::read_to_string(p).unwrap()
}

#[test]
fn bullet_list_enter_exit_and_undo() {
    let (mut h, p) = open("bullets", "- milk\n- eggs\n\nafter\n");
    click_line_end(&mut h, 2);
    assert_eq!(caret(&mut h), (6, 0), "caret at the end of `milk`");
    key(&mut h, Key::Enter);
    assert_eq!(text(&h), "- milk\n- \n- eggs\n\nafter\n");
    assert_eq!(caret(&mut h), (9, 0), "caret in the new empty item");
    typ(&mut h, "bread");
    assert_eq!(text(&h), "- milk\n- bread\n- eggs\n\nafter\n");
    // to the end of `eggs`, Enter, Enter on the empty item leaves the list (S-2.2, S-5.1)
    click_line_end(&mut h, 17);
    key(&mut h, Key::Enter);
    key(&mut h, Key::Enter);
    assert_eq!(text(&h), "- milk\n- bread\n- eggs\n\nafter\n");
    assert_eq!(caret(&mut h), (21, 3), "virtual paragraph after the list");
    typ(&mut h, "x");
    assert_eq!(text(&h), "- milk\n- bread\n- eggs\n\nx\n\nafter\n");
    assert_eq!(save_and_read(&mut h, &p), "- milk\n- bread\n- eggs\n\nx\n\nafter\n");
    // undo back to the opened bytes
    for _ in 0..10 {
        h.key_press_modifiers(cmd(), Key::Z);
        h.step();
    }
    assert_eq!(text(&h), "- milk\n- eggs\n\nafter\n");
}

#[test]
fn ordered_list_renumbers_and_nests() {
    let (mut h, p) = open("ordered", "1. one\n2. two\n3. three\n");
    click_line_end(&mut h, 3);
    key(&mut h, Key::Enter);
    assert_eq!(text(&h), "1. one\n2. \n3. two\n4. three\n");
    typ(&mut h, "half");
    // Tab nests the new item under `one`, renumbering the rest (C11)
    key(&mut h, Key::Tab);
    assert_eq!(text(&h), "1. one\n   1. half\n2. two\n3. three\n");
    h.key_press_modifiers(shift(), Key::Tab);
    h.step();
    assert_eq!(text(&h), "1. one\n2. half\n3. two\n4. three\n");
    assert_eq!(save_and_read(&mut h, &p), "1. one\n2. half\n3. two\n4. three\n");
}

#[test]
fn backspace_at_item_start_joins_and_unwraps() {
    let (mut h, _p) = open("backspace", "- a\n- b\n- c\n");
    // S-5.4: `b` becomes a second paragraph of `a`
    click_line_start(&mut h, 6);
    assert_eq!(caret(&mut h), (6, 0), "caret before `b`, not on its marker");
    key(&mut h, Key::Backspace);
    assert_eq!(text(&h), "- a\n\n  b\n- c\n");
    h.key_press_modifiers(cmd(), Key::Z);
    h.step();
    assert_eq!(text(&h), "- a\n- b\n- c\n");
    // C46: Backspace at the start of the first item unwraps it
    click_line_start(&mut h, 2);
    assert_eq!(caret(&mut h), (2, 0));
    key(&mut h, Key::Backspace);
    assert_eq!(text(&h), "a\n\n- b\n- c\n");
    assert_eq!(caret(&mut h), (0, 0));
}

#[test]
fn lists_in_quotes_and_crlf() {
    let (mut h, p) = open("quote", "> - a\r\n> - b\r\n");
    click_line_end(&mut h, 4);
    key(&mut h, Key::Enter);
    typ(&mut h, "x");
    assert_eq!(text(&h), "> - a\r\n> - x\r\n> - b\r\n");
    key(&mut h, Key::Tab);
    assert_eq!(text(&h), "> - a\r\n>   - x\r\n> - b\r\n");
    assert_eq!(save_and_read(&mut h, &p), "> - a\r\n>   - x\r\n> - b\r\n");
}

#[test]
fn typing_a_list_from_scratch() {
    let (mut h, p) = open("scratch", "");
    // an empty document has no items: click anywhere on the page
    let at = egui::pos2(600.0, 200.0);
    h.hover_at(at);
    h.drag_at(at);
    h.step();
    h.drop_at(at);
    h.step();
    typ(&mut h, "- a");
    key(&mut h, Key::Enter);
    typ(&mut h, "b");
    key(&mut h, Key::Enter);
    key(&mut h, Key::Tab);
    assert_eq!(text(&h), "- a\n- b\n  - ");
    typ(&mut h, "c");
    key(&mut h, Key::Enter);
    key(&mut h, Key::Enter); // outdent (S-5.2)
    typ(&mut h, "d");
    assert_eq!(save_and_read(&mut h, &p), "- a\n- b\n  - c\n- d");
}

#[test]
fn rules_format_commands_and_auto_pair_in_the_app() {
    let (mut h, p) = open("rules", "hello world\n");
    click_line_end(&mut h, 2);
    // Select Word (⌘D / Ctrl+D) then Strong (⌘B / Ctrl+B), twice = toggle off
    for _ in 0..2 {
        if cfg!(target_os = "macos") {
            h.key_press_modifiers(cmd(), Key::ArrowLeft);
        } else {
            h.key_press(Key::Home);
        }
        h.step();
        h.key_press_modifiers(cmd(), Key::D);
        h.step();
        h.key_press_modifiers(cmd(), Key::B);
        h.step();
    }
    assert_eq!(text(&h), "hello world\n");
    h.key_press_modifiers(cmd(), Key::D);
    h.step();
    h.key_press_modifiers(cmd(), Key::B);
    h.step();
    assert_eq!(text(&h), "**hello** world\n");
    // the selection stays on the word: ⌘B again removes the style, ⌘B once more restores it
    h.key_press_modifiers(cmd(), Key::B);
    h.step();
    assert_eq!(text(&h), "hello world\n");
    h.key_press_modifiers(cmd(), Key::B);
    h.step();
    assert_eq!(text(&h), "**hello** world\n");
    // a heading typed at the end of the document: a paragraph until Enter
    if cfg!(target_os = "macos") {
        h.key_press_modifiers(cmd(), Key::ArrowDown);
    } else {
        h.key_press_modifiers(cmd(), Key::End);
    }
    h.step();
    key(&mut h, Key::Enter);
    typ(&mut h, "## Plan");
    assert_eq!(text(&h), "**hello** world\n\n## Plan\n");
    key(&mut h, Key::Enter);
    typ(&mut h, "> (x");
    assert_eq!(text(&h), "**hello** world\n\n## Plan\n\n> (x)\n");
    assert_eq!(save_and_read(&mut h, &p), "**hello** world\n\n## Plan\n\n> (x)\n");
}

/// Codex review of PR #35, items 1–3, 5, 6: the fixes through the real app's input path.
#[test]
fn review_fixes_through_the_app() {
    // 1: Clear Format keeps literal code content
    let (mut h, p) = open("rev-clear", "a `<u>x</u>` b\n");
    click_line_end(&mut h, 0);
    h.key_press_modifiers(cmd(), Key::A);
    h.step();
    h.key_press_modifiers(cmd(), Key::Backslash);
    h.step();
    assert_eq!(save_and_read(&mut h, &p), "a <u>x</u> b\n");
    // 2: renumbering 9 → 10 keeps the child inside its item
    let (mut h, p) = open("rev-width", "8. a\n9. b\n   - child\n");
    click_line_end(&mut h, 3);
    key(&mut h, Key::Enter);
    typ(&mut h, "z");
    assert_eq!(save_and_read(&mut h, &p), "8. a\n9. z\n10. b\n    - child\n");
    // 3: leaving the outer list from after a nested item gives a top-level paragraph
    let (mut h, p) = open("rev-exit", "- a\n  - b\n- \n");
    click_line_end(&mut h, 10);
    assert_eq!(caret(&mut h), (12, 0), "caret in the empty item");
    key(&mut h, Key::Enter);
    typ(&mut h, "x");
    assert_eq!(save_and_read(&mut h, &p), "- a\n  - b\n\nx\n");
    // 5: leaving the middle of a quote splits it at once
    let (mut h, p) = open("rev-quote", "> a\n>\n> c\n");
    click_line_end(&mut h, 2);
    key(&mut h, Key::Enter);
    key(&mut h, Key::Enter);
    assert_eq!(text(&h), "> a\n\n> c\n");
    typ(&mut h, "x");
    assert_eq!(save_and_read(&mut h, &p), "> a\n\nx\n\n> c\n");
    // 6: Delete at the end of a paragraph before a rule / a quote
    let (mut h, p) = open("rev-delete", "a\n\n---\n\nb\n\n> c\n> d\n");
    click_line_end(&mut h, 0);
    key(&mut h, Key::Delete);
    assert_eq!(text(&h), "a\n\nb\n\n> c\n> d\n");
    let at = text(&h).find('b').unwrap();
    click_line_end(&mut h, at);
    key(&mut h, Key::Delete);
    assert_eq!(save_and_read(&mut h, &p), "a\n\nbc\nd\n");
}

/// Codex review of PR #35, item 7: host shortcuts follow `keyBinding`.
#[test]
fn key_bindings_rebind_and_unbind_host_shortcuts() {
    let (mut h, p) = open("rev-keys", "hello\n");
    let alt_cmd = Modifiers { alt: true, ..cmd() };
    // Save rebound to ⌥⌘S / Ctrl+Alt+S
    h.state_mut()
        .set_key_bindings_override(Some(vec![("Save".into(), "Cmd+Alt+S".into())]));
    h.step();
    click_line_end(&mut h, 0);
    typ(&mut h, "!");
    h.key_press_modifiers(cmd(), Key::S);
    h.step();
    h.step();
    assert!(h.state().is_dirty(), "the old chord no longer saves");
    h.key_press_modifiers(alt_cmd, Key::S);
    wait_until(&mut h, "save", |a| !a.is_busy() && !a.is_dirty());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello!\n");
    // unbound: nothing saves; another command can take ⌘S
    h.state_mut().set_key_bindings_override(Some(vec![
        ("file.save".into(), "".into()),
        ("Strong".into(), "Cmd+S".into()),
    ]));
    h.step();
    h.key_press_modifiers(cmd(), Key::A);
    h.step();
    h.key_press_modifiers(cmd(), Key::S);
    h.step();
    assert_eq!(text(&h), "**hello!**\n", "⌘S runs Strong now");
    assert!(h.state().is_dirty(), "and did not save");
    // the menus show the chords in effect (one keymap for shortcuts and menus)
    let chord_of = |h: &H, id: &str| -> Option<markise_engine::editor::keymap::Chord> {
        h.state().menu_model().menus.iter().flat_map(|m| m.entries.iter()).find_map(|e| match e {
            markise_ui_egui::menu::Entry::Item(it) if matches!(&it.action, markise_ui_egui::menu::MenuAction::Command(x) if *x == id) => {
                Some(it.chord.clone())
            }
            _ => None,
        })?
    };
    let plat = markise_engine::editor::keymap::Platform::current();
    assert_eq!(
        chord_of(&h, "format.strong"),
        markise_engine::editor::keymap::Chord::parse("Cmd+S", plat)
    );
    assert_eq!(chord_of(&h, "file.save"), None, "Save is unbound");

    // Codex round 5: a conflicting override (Save keeps its default) — the keymap's winner
    // alone shows and installs ⌘S; no chord appears on two menu items (native key equivalents)
    let chords = |h: &H| -> Vec<markise_engine::editor::keymap::Chord> {
        fn walk(es: &[markise_ui_egui::menu::Entry], out: &mut Vec<markise_engine::editor::keymap::Chord>) {
            for e in es {
                match e {
                    markise_ui_egui::menu::Entry::Item(it) => out.extend(it.chord.clone()),
                    markise_ui_egui::menu::Entry::Submenu(m) => walk(&m.entries, out),
                    _ => {}
                }
            }
        }
        let mut v = Vec::new();
        for m in &h.state().menu_model().menus {
            walk(&m.entries, &mut v);
        }
        v
    };
    let unique = |v: &[markise_engine::editor::keymap::Chord]| {
        let mut seen = std::collections::HashSet::new();
        v.iter().all(|c| seen.insert(c.clone()))
    };
    assert!(unique(&chords(&h)), "no duplicate chords with the default-ish bindings");
    h.state_mut()
        .set_key_bindings_override(Some(vec![("Strong".into(), "Cmd+S".into())]));
    h.step();
    assert_eq!(
        chord_of(&h, "format.strong"),
        markise_engine::editor::keymap::Chord::parse("Cmd+S", plat)
    );
    assert_eq!(chord_of(&h, "file.save"), None, "Save is shadowed by Strong");
    assert!(unique(&chords(&h)), "each chord on exactly one menu item");
    // an application command's chord yields to a registry binding too
    h.state_mut()
        .set_key_bindings_override(Some(vec![("Strong".into(), "Cmd+W".into())]));
    h.step();
    assert_eq!(chord_of(&h, "file.close"), None, "Close yields ⌘W to Strong");
    assert!(unique(&chords(&h)));
}

/// Codex review round 3: Delete between list items with headings, in the real app.
#[test]
fn delete_between_list_items_with_headings() {
    let (mut h, p) = open("rev3-a", "- a\n- ## B\n");
    click_line_end(&mut h, 2);
    key(&mut h, Key::Delete);
    assert_eq!(save_and_read(&mut h, &p), "- aB\n");
    let (mut h, p) = open("rev3-b", "- A\n  ===\n- b\n");
    click_line_end(&mut h, 2);
    key(&mut h, Key::Delete);
    assert_eq!(save_and_read(&mut h, &p), "- Ab\n  ===\n");
}

/// Codex review round 6: an application shortcut matches exact modifiers (a binding with
/// an extra Shift is another chord), and system-reserved chords cannot be bound.
#[test]
fn app_shortcuts_match_exact_chords_and_reserved_chords_are_refused() {
    let (mut h, _p) = open("rev6", "hello\n");
    let shift_cmd = Modifiers { shift: true, ..cmd() };
    h.state_mut()
        .set_key_bindings_override(Some(vec![("Strong".into(), "Shift+Cmd+W".into())]));
    h.step();
    click_line_end(&mut h, 0);
    h.key_press_modifiers(cmd(), Key::A);
    h.step();
    h.key_press_modifiers(shift_cmd, Key::W);
    h.step();
    assert_eq!(text(&h), "**hello**\n", "⇧⌘W runs Strong, not Close (⌘W)");
    // a reserved chord is refused: Strong keeps its default
    let reserved = if cfg!(target_os = "macos") { "Cmd+H" } else { "Alt+F4" };
    h.state_mut()
        .set_key_bindings_override(Some(vec![("Strong".into(), reserved.into())]));
    h.step();
    h.key_press_modifiers(cmd(), Key::B);
    h.step();
    assert_eq!(text(&h), "hello\n", "⌘B still toggles Strong");
}

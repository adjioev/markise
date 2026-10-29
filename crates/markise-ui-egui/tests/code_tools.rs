//! Fenced code block tools in the real app (spec 02 §7, issue #76): the language field at the
//! block's bottom-right (click or Esc to edit, autocomplete, Enter / Esc / arrows, one undo
//! step, diagrams) and the copy button at its top-right.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers, PointerButton};
use egui_kittest::Harness;
use markise_doc::Selection;
use markise_ui_egui::{AppOptions, MarkiseApp};

type H = Harness<'static, MarkiseApp>;

fn tmpfile(name: &str, text: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-codetools-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join("doc.md");
    std::fs::write(&p, text).unwrap();
    p
}

fn harness(text: &str, name: &str) -> H {
    let app = MarkiseApp::new(AppOptions {
        open: vec![tmpfile(name, text)],
        ..Default::default()
    });
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app);
    let t = Instant::now();
    while !(h.state().model().is_some() && !h.state().is_loading()) {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out loading");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(3);
    h
}

fn text(h: &H) -> String {
    h.state().document_text().unwrap()
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

fn key(k: Key) -> Event {
    Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

fn press(h: &mut H, k: Key) {
    h.event(key(k));
    h.step();
    h.step();
}

fn typ(h: &mut H, s: &str) {
    for c in s.chars() {
        h.event(Event::Text(c.to_string()));
        h.step();
    }
    h.step();
}

fn click(h: &mut H, pos: egui::Pos2) {
    h.hover_at(pos);
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    h.step();
    h.step();
}

/// Hovers the code block holding `offset` and returns the drawn tools (copy, language).
fn hover_block(h: &mut H, offset: usize) -> markise_ui_egui::viewer::ShownTools {
    let r = h.state().viewer_item_rect(offset).expect("block visible");
    h.hover_at(r.center());
    h.run_steps(2);
    let v = h.state_mut().viewer().unwrap();
    *v.code_tools_shown().first().expect("tools shown on hover")
}

fn caret(h: &mut H) -> usize {
    h.state_mut()
        .viewer()
        .unwrap()
        .ed
        .session
        .selection()
        .primary()
        .focus
        .offset
}

/// Puts the caret at `at` and gives the document the keyboard focus.
fn caret_at(h: &mut H, at: usize) {
    let v = h.state_mut().viewer().unwrap();
    v.ed.edit(|s| s.set_selection(Selection::caret(at)));
    v.request_refocus();
    h.run_steps(2);
}

fn editing(h: &mut H) -> Option<String> {
    h.state_mut().viewer().unwrap().lang_edit().map(|e| e.text.clone())
}

#[test]
fn click_the_field_and_pick_sh_with_autocomplete() {
    let src = "# T\n\n```rust\nfn main() {}\n```\n\nafter\n";
    let mut h = harness(src, "pick");
    let tools = hover_block(&mut h, src.find("fn").unwrap());
    click(&mut h, tools.lang.center());
    assert_eq!(
        editing(&mut h).as_deref(),
        Some("rust"),
        "field opened with the language"
    );
    // the text is selected: typing replaces it and lists matches
    typ(&mut h, "sh");
    {
        let v = h.state_mut().viewer().unwrap();
        let e = v.lang_edit().unwrap();
        assert_eq!(e.text, "sh");
        assert_eq!(e.items[0].word, "sh");
        assert!(e.items.iter().any(|i| i.word == "bash"), "{:?}", e.items);
    }
    assert_eq!(text(&h), src, "nothing written while typing");
    press(&mut h, Key::Enter);
    assert_eq!(text(&h), "# T\n\n```sh\nfn main() {}\n```\n\nafter\n");
    assert!(editing(&mut h).is_none());
    // the document has the focus back
    let v = h.state_mut().viewer().unwrap();
    let id = v.surface_id().unwrap();
    assert!(h.ctx.memory(|m| m.has_focus(id)), "focus back in the document");
}

#[test]
fn arrows_choose_a_row_and_alias_finds_the_language() {
    let src = "```\nlet x = 1;\n```\n";
    let mut h = harness(src, "alias");
    let tools = hover_block(&mut h, src.find("let").unwrap());
    click(&mut h, tools.lang.center());
    assert_eq!(editing(&mut h).as_deref(), Some(""), "empty language");
    typ(&mut h, "js");
    let rows: Vec<String> = h
        .state_mut()
        .viewer()
        .unwrap()
        .lang_edit()
        .unwrap()
        .items
        .iter()
        .map(|i| i.word.clone())
        .collect();
    let jsi = rows
        .iter()
        .position(|w| w == "javascript")
        .expect("alias js → javascript");
    for _ in 0..jsi {
        press(&mut h, Key::ArrowDown);
    }
    press(&mut h, Key::Tab);
    assert_eq!(text(&h), "```javascript\nlet x = 1;\n```\n", "added a language");
}

#[test]
fn tilde_and_long_fences_keep_their_fence() {
    let src = "~~~rust {.numbers}\nx\n~~~\n\n````py\ny\n````\n";
    let mut h = harness(src, "fences");
    caret_at(&mut h, src.find("x\n").unwrap());
    press(&mut h, Key::Escape);
    assert_eq!(
        editing(&mut h).as_deref(),
        Some("rust"),
        "Esc in the block opens the field"
    );
    typ(&mut h, "go");
    press(&mut h, Key::Enter);
    let t = text(&h);
    assert_eq!(t, "~~~go {.numbers}\nx\n~~~\n\n````py\ny\n````\n");
    caret_at(&mut h, t.find("y\n").unwrap());
    press(&mut h, Key::Escape);
    typ(&mut h, "zig");
    press(&mut h, Key::Enter);
    assert_eq!(text(&h), "~~~go {.numbers}\nx\n~~~\n\n````zig\ny\n````\n");
}

#[test]
fn clearing_removes_the_info_string() {
    let src = "``` rust\nx\n```\n";
    let mut h = harness(src, "clear");
    let tools = hover_block(&mut h, src.find("x\n").unwrap());
    click(&mut h, tools.lang.center());
    press(&mut h, Key::Backspace);
    assert_eq!(editing(&mut h).as_deref(), Some(""));
    press(&mut h, Key::Enter);
    assert_eq!(text(&h), "```\nx\n```\n");
}

#[test]
fn esc_reverts_and_undo_redo_restore_the_bytes() {
    let src = "para\n\n```rust\nfn a() {}\n```\n";
    let mut h = harness(src, "undo");
    let at = src.find("fn").unwrap() + 3;
    caret_at(&mut h, at);
    press(&mut h, Key::Escape);
    typ(&mut h, "python");
    press(&mut h, Key::Escape);
    assert!(editing(&mut h).is_none(), "Esc closes the field");
    assert_eq!(text(&h), src, "Esc reverts");
    assert_eq!(caret(&mut h), at, "caret unchanged");

    press(&mut h, Key::Escape);
    typ(&mut h, "toml");
    press(&mut h, Key::Enter);
    let changed = "para\n\n```toml\nfn a() {}\n```\n";
    assert_eq!(text(&h), changed);
    h.key_press_modifiers(cmd(), Key::Z);
    h.run_steps(2);
    assert_eq!(text(&h), src, "one undo step restores the exact bytes");
    h.key_press_modifiers(Modifiers { shift: true, ..cmd() }, Key::Z);
    h.run_steps(2);
    assert_eq!(text(&h), changed, "redo");
}

#[test]
fn left_at_start_and_right_at_end_leave_the_field() {
    let src = "```rust\nab\n```\n\nnext\n";
    let mut h = harness(src, "arrows");
    caret_at(&mut h, src.find("ab").unwrap());
    press(&mut h, Key::Escape);
    // all text selected: Left collapses the selection first, then Home-like moves are typing
    press(&mut h, Key::End);
    press(&mut h, Key::ArrowRight);
    assert!(editing(&mut h).is_none(), "Right at the end leaves");
    assert_eq!(text(&h), src);
    assert_eq!(caret(&mut h), src.find("next").unwrap(), "after the block");

    caret_at(&mut h, src.find("ab").unwrap());
    press(&mut h, Key::Escape);
    typ(&mut h, "c");
    press(&mut h, Key::Home);
    press(&mut h, Key::ArrowLeft);
    assert!(editing(&mut h).is_none(), "Left at the start leaves");
    let t = text(&h);
    assert_eq!(t, "```c\nab\n```\n\nnext\n", "leaving commits");
    assert_eq!(caret(&mut h), t.find("ab").unwrap() + 2, "at the end of the code");
}

#[test]
fn mermaid_switches_to_a_diagram() {
    let src = "```text\ngraph TD\n  A --> B\n```\n";
    let mut h = harness(src, "mermaid");
    h.state_mut().set_diagrams(true);
    h.run_steps(2);
    let tools = hover_block(&mut h, src.find("graph").unwrap());
    click(&mut h, tools.lang.center());
    typ(&mut h, "mermaid");
    press(&mut h, Key::Enter);
    assert_eq!(text(&h), "```mermaid\ngraph TD\n  A --> B\n```\n");
    let m = h.state().model().unwrap().clone();
    assert!(
        m.items
            .iter()
            .any(|i| matches!(&i.kind, markise_layout::ItemKind::Diagram { lang } if lang == "mermaid")),
        "a diagram item"
    );
}

#[test]
fn ime_composition_does_not_commit() {
    let src = "```rust\nx\n```\n";
    let mut h = harness(src, "ime");
    caret_at(&mut h, src.find("x\n").unwrap());
    press(&mut h, Key::Escape);
    assert!(editing(&mut h).is_some());
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit {
        text: "ご".into(),
        active_range_chars: None,
    }));
    h.input_mut().events.push(key(Key::Enter));
    h.input_mut().events.push(key(Key::Escape));
    h.run_steps(2);
    assert_eq!(text(&h), src, "Enter and Esc belong to the composition");
    assert!(editing(&mut h).is_some(), "field still open");
    h.input_mut()
        .events
        .push(Event::Ime(egui::ImeEvent::Commit("go".into())));
    h.run_steps(2);
    assert_eq!(editing(&mut h).as_deref(), Some("go"));
    press(&mut h, Key::Enter);
    assert_eq!(text(&h), "```go\nx\n```\n");
}

/// The copied text (clipboard seam: the frame's copy command).
fn copied(h: &H) -> Option<String> {
    h.output().platform_output.commands.iter().find_map(|c| match c {
        egui::OutputCommand::CopyText(t) => Some(t.clone()),
        _ => None,
    })
}

#[test]
fn copy_button_copies_the_code_only() {
    let src = "intro\n\n~~~py\n  def f():\r\n      return 1\n~~~\n\n````sh\necho `x`\n````\n";
    let mut h = harness(src, "copy");
    let at = 2;
    caret_at(&mut h, at);
    // the button appears on hover
    let tools = hover_block(&mut h, src.find("def").unwrap());
    h.hover_at(tools.copy.center());
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos: tools.copy.center(),
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    h.step();
    assert_eq!(copied(&h).as_deref(), Some("  def f():\r\n      return 1"));
    h.step();
    assert_eq!(caret(&mut h), at, "caret unchanged");
    assert!(
        h.state_mut()
            .viewer()
            .unwrap()
            .ed
            .session
            .selection()
            .primary()
            .is_caret(),
        "nothing selected"
    );
    assert_eq!(text(&h), src);

    let tools = hover_block(&mut h, src.find("echo").unwrap());
    h.hover_at(tools.copy.center());
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos: tools.copy.center(),
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    h.step();
    assert_eq!(copied(&h).as_deref(), Some("echo `x`"));
}

#[test]
fn tools_show_with_the_caret_and_hide_otherwise() {
    let src = "para one\n\n```rust\nx\n```\n";
    let mut h = harness(src, "caret");
    h.hover_at(egui::pos2(990.0, 690.0));
    caret_at(&mut h, 2);
    assert!(h.state_mut().viewer().unwrap().code_tools_shown().is_empty());
    caret_at(&mut h, src.find("x\n").unwrap());
    assert_eq!(h.state_mut().viewer().unwrap().code_tools_shown().len(), 1);
}

#[test]
fn works_in_dark_and_zoomed() {
    let src = "```rust\nfn main() {}\n```\n";
    let mut h = harness(src, "darkzoom");
    h.state_mut().set_theme_choice(markise_ui_egui::app::ThemeChoice::Dark);
    h.state_mut().set_zoom(1.5);
    h.run_steps(4);
    let tools = hover_block(&mut h, src.find("fn").unwrap());
    assert!(tools.lang.height() > 10.0 && tools.copy.width() >= 18.0, "{tools:?}");
    click(&mut h, tools.lang.center());
    assert_eq!(editing(&mut h).as_deref(), Some("rust"));
    typ(&mut h, "c");
    press(&mut h, Key::Enter);
    assert_eq!(text(&h), "```c\nfn main() {}\n```\n");
}

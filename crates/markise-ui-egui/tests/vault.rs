//! Vault quick wins in the real app (spec 06 §2–§3): clicking a wikilink opens the note and
//! selects it in the file tree, Back / Forward restore the position, an unresolved link
//! offers to create the note, `[[` autocomplete inserts a link, Markdown links with encoded
//! paths open, and View → Wikilinks off shows `[[…]]` as plain text.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers, PointerButton};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use markise_doc::Selection;
use markise_ui_egui::confirm::{Choice, Confirm, Kind, Scripted};
use markise_ui_egui::{AppOptions, MarkiseApp};

type H = Harness<'static, MarkiseApp>;

fn vault(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-vault-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("Sub")).unwrap();
    std::fs::create_dir_all(d.join("Folder A")).unwrap();
    let d = std::fs::canonicalize(&d).unwrap();
    let mut home =
        String::from("# Home\n\nGo to [[Other]] or [[Sub/Deep#Part Two|deep part]] or [[Missing note]].\n\n");
    home.push_str("Spaced [p](Folder%20A/Spaced%20Note.md) and [q](<Folder A/Spaced Note.md>).\n\n");
    for i in 0..60 {
        home.push_str(&format!("Paragraph {i} of the home note, long enough to scroll.\n\n"));
    }
    std::fs::write(d.join("Home.md"), home).unwrap();
    std::fs::write(d.join("Other.md"), "# Other\n\nBack to [[Home]].\n").unwrap();
    let mut deep = String::from("# Deep\n\n");
    for i in 0..80 {
        deep.push_str(&format!("Filler {i}.\n\n"));
    }
    deep.push_str("## Part Two\n\nHere.\n");
    for i in 0..80 {
        deep.push_str(&format!("\nAfter {i}.\n"));
    }
    std::fs::write(d.join("Sub/Deep.md"), deep).unwrap();
    std::fs::write(d.join("Folder A/Spaced Note.md"), "# Spaced\n").unwrap();
    d
}

fn harness(dir: &Path, s: Option<&Arc<Scripted>>) -> H {
    let mut opts = AppOptions {
        open: vec![dir.to_path_buf(), dir.join("Home.md")],
        ..Default::default()
    };
    if let Some(s) = s {
        opts.confirm = s.clone() as Arc<dyn Confirm>;
    }
    Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), MarkiseApp::new(opts))
}

fn wait_until(h: &mut H, what: &str, f: impl Fn(&MarkiseApp) -> bool) {
    let t = Instant::now();
    while !f(h.state()) {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(2));
    }
    h.step();
}

fn ready(h: &mut H, file: &Path) {
    let f = file.to_path_buf();
    wait_until(h, "document and vault index", move |a| {
        a.document_path() == Some(f.as_path()) && !a.is_busy() && a.vault_index().is_some_and(|ix| ix.len() >= 4)
    });
    h.run_steps(2);
}

fn command() -> Modifiers {
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

fn click(h: &mut H, pos: egui::Pos2, modifiers: Modifiers) {
    h.hover_at(pos);
    h.event(Event::ModifiersChanged(modifiers));
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers,
        });
    }
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
}

/// Cmd/Ctrl+click on the link reported as `target`.
fn cmd_click_link(h: &mut H, target: &str) {
    let p = h
        .state()
        .link_center(target)
        .unwrap_or_else(|| panic!("link {target} not visible"));
    click(h, p, command());
}

/// Back (⌃⌘← on macOS, Alt+← elsewhere).
fn back_forward(h: &mut H, back: bool) {
    let m = if cfg!(target_os = "macos") {
        Modifiers {
            ctrl: true,
            mac_cmd: true,
            command: true,
            ..Default::default()
        }
    } else {
        Modifiers::ALT
    };
    h.key_press_modifiers(m, if back { Key::ArrowLeft } else { Key::ArrowRight });
    h.step();
}

/// The tree row `name` is shown as the selected (current) file.
fn selected_in_tree(h: &H, name: &str) -> bool {
    h.query_all_by_label(name)
        .any(|n| n.accesskit_node().toggled() == Some(egui::accesskit::Toggled::True))
}

#[test]
fn clicking_a_wikilink_opens_the_note_and_selects_it_in_the_tree() {
    let d = vault("click");
    let mut h = harness(&d, None);
    ready(&mut h, &d.join("Home.md"));
    cmd_click_link(&mut h, "[[Other]]");
    ready(&mut h, &d.join("Other.md"));
    assert!(
        selected_in_tree(&h, "Other.md"),
        "the opened note is the selected tree row"
    );

    // a link with a folder and a heading: the folder expands, the row is selected, and the
    // view starts at the heading
    h.state_mut().follow_link("[[Sub/Deep#Part Two]]");
    ready(&mut h, &d.join("Sub/Deep.md"));
    wait_until(&mut h, "tree row", |a| {
        a.file_tree_rows().contains(&"Deep.md".to_string())
    });
    assert!(h.state().file_tree().unwrap().is_expanded(&d.join("Sub")));
    assert!(selected_in_tree(&h, "Deep.md"));
    let m = h.state().model().unwrap().clone();
    let part = m.outline.iter().find(|o| o.text == "Part Two").unwrap().item;
    let anchor = h.state_mut().viewer().unwrap().ed.view.anchor().0;
    assert_eq!(anchor, part, "scrolled to the heading");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn back_and_forward_restore_the_position() {
    let d = vault("history");
    let mut h = harness(&d, None);
    ready(&mut h, &d.join("Home.md"));
    // scroll and put the caret somewhere in Home
    {
        let v = h.state_mut().viewer().unwrap();
        v.ed.view.scroll_to_item(20);
        v.ed.view.scroll_by(7.0);
        v.ed.edit(|s| s.set_selection(Selection::caret(300)));
    }
    h.step();
    let anchor = h.state_mut().viewer().unwrap().ed.view.anchor();
    h.state_mut().follow_link("[[Other]]");
    ready(&mut h, &d.join("Other.md"));
    assert!(h.state().can_go_back() && !h.state().can_go_forward());

    back_forward(&mut h, true);
    ready(&mut h, &d.join("Home.md"));
    let v = h.state_mut().viewer().unwrap();
    assert_eq!(v.ed.view.anchor().0, anchor.0, "scroll position restored");
    assert_eq!(v.ed.session.selection().primary().focus.offset, 300, "caret restored");
    assert!(h.state().can_go_forward());
    assert!(selected_in_tree(&h, "Home.md"));

    back_forward(&mut h, false);
    ready(&mut h, &d.join("Other.md"));
    assert!(!h.state().can_go_forward());
    // the Go menu has both items
    let bar = h.state().menu_model();
    assert!(bar.item("go.back").is_some_and(|i| i.enabled));
    assert!(bar.item("go.forward").is_some_and(|i| !i.enabled));
    let (paths, at) = h.state().nav_history().paths();
    assert_eq!(paths.len(), 2);
    assert_eq!(at, 1);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_unresolved_link_offers_to_create_the_note() {
    let d = vault("create");
    let s = Scripted::new();
    let mut h = harness(&d, Some(&s));
    ready(&mut h, &d.join("Home.md"));
    s.answer(Choice::Create);
    cmd_click_link(&mut h, "[[Missing note]]");
    let q = s.last().expect("asked");
    assert_eq!(q.kind, Kind::CreateNote);
    assert_eq!(q.default, Choice::Create);
    let created = d.join("Missing note.md");
    ready(&mut h, &created);
    assert_eq!(std::fs::read(&created).unwrap(), b"", "an empty note in the vault root");

    // a target with folders: created below the root, folders included (spec 06 E4)
    s.answer(Choice::Create);
    h.state_mut().follow_link("[[Projects/New/Deep]]");
    h.step();
    ready(&mut h, &d.join("Projects/New/Deep.md"));
    // Cancel creates nothing; attachments are never created (E5)
    s.answer(Choice::Cancel);
    h.state_mut().follow_link("[[Nope]]");
    h.run_steps(3);
    assert!(!d.join("Nope.md").exists());
    let asked = s.asked().len();
    h.state_mut().follow_link("[[Missing.png]]");
    h.run_steps(2);
    assert_eq!(s.asked().len(), asked, "no question for an attachment");
    assert!(h.state().banner_text().is_some_and(|b| b.contains("Only notes")));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn typing_double_bracket_offers_notes_and_enter_inserts_the_link() {
    let d = vault("complete");
    let mut h = harness(&d, None);
    ready(&mut h, &d.join("Home.md"));
    // caret at the end of the first paragraph, surface focused
    let end = h.state().document_text().unwrap().find("[[Missing note]].").unwrap() + "[[Missing note]].".len();
    {
        let v = h.state_mut().viewer().unwrap();
        v.ed.edit(|s| s.set_selection(Selection::caret(end)));
        v.request_refocus();
    }
    h.run_steps(2);
    for t in [" ", "[", "["] {
        h.event(Event::Text(t.into()));
        h.step();
    }
    assert!(h.state().completion().is_some(), "`[[` opens the popup");
    for t in ["O", "t", "h"] {
        h.event(Event::Text(t.into()));
        h.step();
    }
    let c = h.state().completion().expect("still open").clone();
    assert_eq!(c.query, "Oth");
    assert_eq!(c.items[0].label, "Other");
    h.key_press(Key::Enter);
    h.run_steps(2);
    let text = h.state().document_text().unwrap();
    assert!(text.contains("[[Missing note]]. [[Other]]\n"), "{text:?}");
    assert!(!text.contains("[[Other]]]]"));
    let caret = h
        .state_mut()
        .viewer()
        .unwrap()
        .ed
        .session
        .selection()
        .primary()
        .focus
        .offset;
    assert_eq!(
        &text[..caret],
        &text[..end + " [[Other]]".len()],
        "caret after the link"
    );
    assert!(h.state().completion().is_none());

    // Esc closes it and leaves the typed text
    for t in [" ", "[", "[", "Su"] {
        h.event(Event::Text(t.into()));
        h.step();
    }
    assert!(h.state().completion().is_some_and(|c| !c.items.is_empty()));
    h.key_press(Key::Escape);
    h.step();
    assert!(h.state().completion().is_none());
    assert!(h.state().document_text().unwrap().contains("[[Su"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn markdown_links_with_encoded_paths_open() {
    let d = vault("mdlinks");
    let mut h = harness(&d, None);
    ready(&mut h, &d.join("Home.md"));
    let spaced = d.join("Folder A/Spaced Note.md");
    cmd_click_link(&mut h, "Folder%20A/Spaced%20Note.md");
    ready(&mut h, &spaced);
    back_forward(&mut h, true);
    ready(&mut h, &d.join("Home.md"));
    cmd_click_link(&mut h, "Folder A/Spaced Note.md");
    ready(&mut h, &spaced);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn wikilinks_off_shows_plain_text_and_keeps_the_bytes() {
    let d = vault("toggle");
    let mut h = harness(&d, None);
    ready(&mut h, &d.join("Home.md"));
    let before = h.state().document_text().unwrap();
    assert!(h.state().wikilinks());
    assert!(h.state().link_center("[[Other]]").is_some());
    let bar = h.state().menu_model();
    assert!(bar.item("view.wikilinks").is_some());

    h.state_mut().set_wikilinks(false);
    wait_until(&mut h, "re-parse", |a| !a.is_reparsing());
    h.run_steps(2);
    assert!(!h.state().wikilinks());
    assert!(h.state().link_center("[[Other]]").is_none(), "no link any more");
    assert_eq!(h.state().document_text().unwrap(), before, "byte-identical");
    assert!(!h.state().is_dirty());
    // following a typed `[[` offers nothing while off
    h.state_mut().set_wikilinks(true);
    wait_until(&mut h, "re-parse", |a| !a.is_reparsing());
    h.run_steps(2);
    assert!(h.state().link_center("[[Other]]").is_some());
    assert!(!h.state().is_dirty());
    let _ = std::fs::remove_dir_all(&d);
}

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

/// Makes `d` an Obsidian vault (an empty `.obsidian/app.json`: Obsidian's defaults).
fn make_obsidian(d: &Path) {
    std::fs::create_dir_all(d.join(".obsidian")).unwrap();
    std::fs::write(d.join(".obsidian/app.json"), "{}").unwrap();
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

/// Back (⌥⌘← on macOS, Alt+← elsewhere).
fn back_forward(h: &mut H, back: bool) {
    let m = if cfg!(target_os = "macos") {
        Modifiers {
            alt: true,
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
    // protected and non-portable names are refused before asking (review P1-2, P2-8)
    for bad in ["[[.obsidian/New]]", "[[CON]]", "[[a:b]]"] {
        h.state_mut().follow_link(bad);
        h.run_steps(2);
        assert_eq!(s.asked().len(), asked, "{bad}: no question");
        assert!(
            h.state().banner_text().is_some_and(|b| b.contains("Cannot create")),
            "{bad}"
        );
    }
    assert!(!d.join(".obsidian").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn typing_double_bracket_offers_notes_and_enter_inserts_the_link() {
    let d = vault("complete");
    make_obsidian(&d);
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
    // an Obsidian vault writes wikilinks (spec 06 §14), shortest form
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
    assert_eq!(caret, end + " [[Other".len(), "V3.4: caret before `]]`");
    // move past the link before typing on
    {
        let v = h.state_mut().viewer().unwrap();
        v.ed.edit(|s| s.set_selection(Selection::caret(caret + 2)));
    }
    h.step();
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

/// Spec 06 Q-V15: in a folder without `.obsidian/` the list inserts a Markdown link with a
/// relative, encoded path (caret after it); Q-V18: new notes go next to the current note.
#[test]
fn plain_folders_write_markdown_links_and_create_next_to_the_note() {
    let d = vault("plain");
    std::fs::write(d.join("Sub/Inner.md"), "# Inner\n\nTo [[Fresh]].\n").unwrap();
    let s = Scripted::new();
    let mut h = harness(&d, Some(&s));
    ready(&mut h, &d.join("Home.md"));
    let end = h.state().document_text().unwrap().find("[[Missing note]].").unwrap() + "[[Missing note]].".len();
    {
        let v = h.state_mut().viewer().unwrap();
        v.ed.edit(|s| s.set_selection(Selection::caret(end)));
        v.request_refocus();
    }
    h.run_steps(2);
    for t in [" ", "[", "[", "Spac"] {
        h.event(Event::Text(t.into()));
        h.step();
    }
    assert_eq!(h.state().completion().unwrap().items[0].label, "Spaced Note");
    h.key_press(Key::Enter);
    h.run_steps(2);
    let text = h.state().document_text().unwrap();
    let link = "[Spaced Note](Folder%20A/Spaced%20Note.md)";
    assert!(text.contains(&format!("[[Missing note]]. {link}\n")), "{text:?}");
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
    assert_eq!(caret, end + 1 + link.len(), "caret after a Markdown link");

    // a note created from a link in Sub/ lands in Sub/
    h.state_mut().open(d.join("Sub/Inner.md"));
    h.step();
    if h.state().is_asking() {
        s.answer(Choice::DontSave);
        h.step();
    }
    ready(&mut h, &d.join("Sub/Inner.md"));
    s.answer(Choice::Create);
    h.state_mut().follow_link("[[Fresh]]");
    h.step();
    ready(&mut h, &d.join("Sub/Fresh.md"));
    assert!(!d.join("Fresh.md").exists());
    let _ = std::fs::remove_dir_all(&d);
}

/// Q-V18: in an Obsidian vault, notes created from a link in a subfolder go to the root.
#[test]
fn obsidian_vaults_create_new_notes_in_the_root() {
    let d = vault("obsidian-create");
    make_obsidian(&d);
    std::fs::write(d.join("Sub/Inner.md"), "To [[Fresh]].\n").unwrap();
    let s = Scripted::new();
    let mut h = harness(&d, Some(&s));
    ready(&mut h, &d.join("Home.md"));
    h.state_mut().open(d.join("Sub/Inner.md"));
    ready(&mut h, &d.join("Sub/Inner.md"));
    s.answer(Choice::Create);
    h.state_mut().follow_link("[[Fresh]]");
    h.step();
    ready(&mut h, &d.join("Fresh.md"));
    let _ = std::fs::remove_dir_all(&d);
}

/// Review P2-7: while an IME composition is active the popup leaves Enter to it; a commit
/// followed by Enter in the same frame accepts.
#[test]
fn completion_keys_wait_for_ime_composition() {
    let d = vault("ime");
    make_obsidian(&d);
    let mut h = harness(&d, None);
    ready(&mut h, &d.join("Home.md"));
    let end = h.state().document_text().unwrap().find("[[Missing note]].").unwrap() + "[[Missing note]].".len();
    {
        let v = h.state_mut().viewer().unwrap();
        v.ed.edit(|s| s.set_selection(Selection::caret(end)));
        v.request_refocus();
    }
    h.run_steps(2);
    for t in [" ", "[", "[", "Oth"] {
        h.event(Event::Text(t.into()));
        h.step();
    }
    assert_eq!(h.state().completion().unwrap().items[0].label, "Other");
    let enter = Event::Key {
        key: Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    // composing: Enter is not the popup's
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit {
        text: "e".into(),
        active_range_chars: None,
    }));
    h.input_mut().events.push(enter.clone());
    h.step();
    assert!(
        !h.state()
            .document_text()
            .unwrap()
            .contains("[[Missing note]]. [[Other]]")
    );
    // commit, then Enter (same frame, in this order): accepted
    let mut h2 = harness(&d, None);
    ready(&mut h2, &d.join("Home.md"));
    {
        let v = h2.state_mut().viewer().unwrap();
        v.ed.edit(|s| s.set_selection(Selection::caret(end)));
        v.request_refocus();
    }
    h2.run_steps(2);
    for t in [" ", "[", "[", "Oth"] {
        h2.event(Event::Text(t.into()));
        h2.step();
    }
    h2.input_mut().events.push(Event::Ime(egui::ImeEvent::Preedit {
        text: "e".into(),
        active_range_chars: None,
    }));
    h2.step();
    h2.input_mut()
        .events
        .push(Event::Ime(egui::ImeEvent::Commit("e".into())));
    h2.input_mut().events.push(enter);
    h2.run_steps(2);
    let text = h2.state().document_text().unwrap();
    assert!(text.contains("[[Missing note]]. [[Other]]"), "{text:?}");
    let _ = std::fs::remove_dir_all(&d);
}

/// Review P2-5: exports parse with the current wikilinks setting, repeats included.
#[test]
fn exports_follow_the_wikilinks_switch() {
    let d = vault("export");
    let mut h = harness(&d, None);
    ready(&mut h, &d.join("Home.md"));
    let run = |h: &mut H, out: &Path| {
        let req = markise_engine::export::ExportRequest::new(
            markise_engine::export::ExportKind::HtmlPlain,
            h.state().document_text().unwrap(),
            Some(d.join("Home.md")),
            out.to_path_buf(),
            markise_theme::Theme::light(),
        );
        h.state_mut().start_export(req);
        let o = out.to_path_buf();
        wait_until(h, "export", move |a| !a.export_state().is_running() && o.exists());
        std::fs::read_to_string(out).unwrap()
    };
    let on = run(&mut h, &d.join("on.html"));
    assert!(on.contains("<a href=\"Other.md\">Other</a>"), "{on}");
    h.state_mut().set_wikilinks(false);
    wait_until(&mut h, "re-parse", |a| !a.is_reparsing());
    let off = run(&mut h, &d.join("off.html"));
    assert!(off.contains("[[Other]]"), "{off}");
    let _ = std::fs::remove_dir_all(&d);
}

/// Spec 06 E17 (review round 2): `.obsidian/app.json` edited in place by another app switches
/// the inserted link syntax without a restart — through the real file watcher.
#[test]
fn obsidian_settings_edited_in_place_take_effect() {
    let d = vault("appjson");
    make_obsidian(&d);
    let mut h = harness(&d, None);
    ready(&mut h, &d.join("Home.md"));
    assert!(h.state().vault_index().unwrap().settings().wikilinks);
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(d.join(".obsidian/app.json"))
            .unwrap();
        f.write_all(br#"{"useMarkdownLinks": true}"#).unwrap();
    }
    wait_until(&mut h, "settings re-read", |a| {
        a.vault_index().is_some_and(|ix| !ix.settings().wikilinks)
    });
    let _ = std::fs::remove_dir_all(&d);
}

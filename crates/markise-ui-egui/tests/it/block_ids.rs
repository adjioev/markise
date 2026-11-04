//! A `{#id}` on screen (ADR 0018): not shown on a heading or on a list item in a card, there in the
//! file, and back in sight with the caret on it, as other marks are.

use std::path::PathBuf;

use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use markise_doc::Selection;
use markise_ui_egui::{MarkiseApp, ViewMode};

use crate::common::{self, wait_until};

type H = Harness<'static, MarkiseApp>;

const DOC: &str = "## Out {#out}\n\n- Listed {#l}\n\n:::q\n\n## Asked {#q}\n\n- [ ] One {#a1}\n- [x] Two {#a2}\n\nPlain {#p}\n\n:::\n";

fn open(text: &str) -> (tempfile::TempDir, PathBuf, H) {
    let dir = tempfile::tempdir().unwrap();
    let doc = std::fs::canonicalize(dir.path()).unwrap().join("d.md");
    std::fs::write(&doc, text).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1000.0, 800.0)).build_ui_state(
        |ui, app: &mut MarkiseApp| app.ui(ui),
        MarkiseApp::new(common::no_watch()),
    );
    h.state_mut().open(doc.clone());
    let d = doc.clone();
    wait_until(&mut h, "load", move |a| {
        a.document_path() == Some(d.as_path()) && !a.is_loading()
    });
    common::run(&mut h);
    (dir, doc, h)
}

fn shown(h: &mut H) -> String {
    h.state_mut().viewer().unwrap().visible_text()
}

fn caret(h: &mut H, at: usize) {
    let v = h.state_mut().viewer().unwrap();
    v.ed.set_selection(Selection::caret(at));
    v.ed.refresh_focus();
    v.request_refocus();
    common::run(h);
}

#[test]
fn a_card_does_not_show_the_id_and_the_file_keeps_it() {
    let (_d, _p, mut h) = open(DOC);
    caret(&mut h, DOC.find("Plain").unwrap());
    let s = shown(&mut h);
    assert!(s.contains("Asked") && s.contains("One") && s.contains("Two"), "{s}");
    for id in ["{#out}", "{#q}", "{#a1}", "{#a2}"] {
        assert!(!s.contains(id), "{id} in {s}");
    }
    assert!(s.contains("{#l}"), "a list item outside a card keeps it: {s}");
    assert!(s.contains("{#p}"), "and a plain paragraph: {s}");
    assert_eq!(h.state().document_text().unwrap(), DOC);
    assert!(!h.state().is_dirty());
}

#[test]
fn the_caret_on_the_line_brings_it_back_and_leaving_hides_it() {
    let (_d, _p, mut h) = open(DOC);
    caret(&mut h, DOC.find("One").unwrap() + 3);
    assert!(shown(&mut h).contains("{#a1}"), "at the end of the text");
    assert!(!shown(&mut h).contains("{#a2}"), "only there");
    caret(&mut h, DOC.find("Plain").unwrap());
    assert!(!shown(&mut h).contains("{#a1}"));
}

#[test]
fn typing_at_the_end_of_the_text_stays_before_the_id() {
    let (_d, _p, mut h) = open(DOC);
    caret(&mut h, DOC.find("One").unwrap() + 3);
    h.event(Event::Text("!".into()));
    common::run(&mut h);
    assert!(h.state().document_text().unwrap().contains("- [ ] One! {#a1}\n"));
}

#[test]
fn the_end_of_the_line_stops_at_the_id_after_the_text() {
    let (_d, _p, mut h) = open(DOC);
    caret(&mut h, DOC.find("Asked").unwrap());
    h.key_press_modifiers(Modifiers::NONE, Key::End);
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
    let line_end = DOC.find(" {#q}").unwrap() + " {#q}".len();
    assert!(
        (DOC.find(" {#q}").unwrap()..=line_end).contains(&at),
        "the caret is on the line: {at}"
    );
    assert!(shown(&mut h).contains("{#q}"), "and shows it");
}

#[test]
fn the_outline_has_the_headings_without_their_ids() {
    let (_d, _p, mut h) = open(DOC);
    let v = h.state_mut().viewer().unwrap();
    let titles: Vec<String> = v.model().outline.iter().map(|e| e.text.to_string()).collect();
    assert_eq!(titles, ["Out", "Asked"]);
    let anchors: Vec<String> = v.model().outline.iter().map(|e| e.anchor.clone()).collect();
    assert_eq!(anchors, ["out", "asked"], "anchors from what shows");
}

#[test]
fn search_finds_it_in_the_source_and_does_not_light_it_up() {
    let (_d, _p, mut h) = open(DOC);
    caret(&mut h, DOC.find("Plain").unwrap());
    h.state_mut().open_find(false);
    h.state_mut().set_find_query(markise_engine::search::Query {
        text: "{#q}".into(),
        case_sensitive: false,
        whole_word: false,
        regex: false,
    });
    wait_until(&mut h, "the search", |a| {
        a.find_results().is_some_and(|r| r.matches.len() == 1)
    });
    common::run(&mut h);
    // the caret is elsewhere: a hit in the source, and nothing to light up on screen
    h.state_mut()
        .viewer()
        .unwrap()
        .ed
        .set_selection(Selection::caret(DOC.find("Plain").unwrap()));
    common::run(&mut h);
    assert!(h.state_mut().viewer().unwrap().search_highlights().is_empty());
}

#[test]
fn a_selection_copies_the_source_as_everywhere() {
    let (_d, _p, mut h) = open(DOC);
    caret(&mut h, DOC.find("One").unwrap());
    h.key_press_modifiers(Modifiers::SHIFT, Key::End);
    common::run(&mut h);
    h.event(Event::Copy);
    h.step();
    let copied = h.output().platform_output.commands.iter().find_map(|c| match c {
        egui::OutputCommand::CopyText(t) => Some(t.clone()),
        _ => None,
    });
    // source bytes, whatever part of the line the selection reached; never the next line
    let copied = copied.expect("something was selected");
    assert!(copied == "One" || copied == "One {#a1}", "{copied:?}");
}

#[test]
fn source_mode_shows_it_and_nothing_changes() {
    let (_d, _p, mut h) = open(DOC);
    h.state_mut().set_view_mode(ViewMode::Source);
    common::run(&mut h);
    assert_eq!(h.state().document_text().unwrap(), DOC);
    h.state_mut().set_view_mode(ViewMode::Rendered);
    common::run(&mut h);
    assert!(!h.state().is_dirty());
}

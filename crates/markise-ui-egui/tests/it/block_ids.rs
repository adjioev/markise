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

fn search_for(h: &mut H, what: &str) {
    h.state_mut().open_find(true);
    h.state_mut().set_find_query(markise_engine::search::Query {
        text: what.into(),
        case_sensitive: false,
        whole_word: false,
        regex: false,
    });
    wait_until(h, "the search", |a| a.find_results().is_some());
    common::run(h);
}

#[test]
fn search_finds_a_hidden_id_in_source_mode_only() {
    let (_d, _p, mut h) = open(DOC);
    caret(&mut h, DOC.find("Plain").unwrap());
    // a query of an id, of its blank, and of the part of one that shows
    search_for(&mut h, "{#q}");
    assert_eq!(
        h.state().find_results().unwrap().matches.len(),
        0,
        "not in the rendered view"
    );
    assert!(h.state_mut().viewer().unwrap().search_highlights().is_empty());
    search_for(&mut h, "Asked {#q");
    assert_eq!(
        h.state().find_results().unwrap().matches.len(),
        1,
        "what also shows is a hit"
    );
    // an id on a plain paragraph or an ordinary list item is text
    search_for(&mut h, "{#p}");
    assert_eq!(h.state().find_results().unwrap().matches.len(), 1);
    search_for(&mut h, "{#l}");
    assert_eq!(h.state().find_results().unwrap().matches.len(), 1);

    search_for(&mut h, "{#q}");
    h.state_mut().set_view_mode(ViewMode::Source);
    wait_until(&mut h, "the search again", |a| {
        a.find_results().is_some_and(|r| r.matches.len() == 1)
    });
    h.state_mut().set_view_mode(ViewMode::Rendered);
    wait_until(&mut h, "and back", |a| {
        a.find_results().is_some_and(|r| r.matches.is_empty())
    });
}

#[test]
fn replacing_in_the_rendered_view_does_not_touch_a_hidden_id() {
    let text = "## a1 {#a1}\n\nsome a1\n\n:::q\n\n- [ ] a1 {#a1}\n\n:::\n";
    let (_d, _p, mut h) = open(text);
    search_for(&mut h, "a1");
    assert_eq!(h.state().find_results().unwrap().matches.len(), 3);
    // the last one shown, past both ids before it in the search's own numbering
    h.state_mut().find_next(true);
    h.state_mut().replace_find("b".into(), false);
    wait_until(&mut h, "the replacement", |a| {
        a.document_text().unwrap().contains("- [ ] b {#a1}")
    });
    h.state_mut().replace_find("c".into(), true);
    wait_until(&mut h, "all of them", |a| a.document_text().unwrap().contains("some c"));
    assert_eq!(
        h.state().document_text().unwrap(),
        "## c {#a1}\n\nsome c\n\n:::q\n\n- [ ] b {#a1}\n\n:::\n"
    );
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

const ID_ONLY: &str = "Top.\n\n:::q\n\n- {#a}\n- Next\n\n:::\n\nEnd.\n";

#[test]
fn an_item_of_only_an_id_keeps_its_place_for_the_caret() {
    let (_d, _p, mut h) = open(ID_ONLY);
    let next = ID_ONLY.find("Next").unwrap();
    caret(&mut h, next);
    h.key_press(Key::ArrowUp);
    common::run(&mut h);
    let v = h.state_mut().viewer().unwrap();
    let at = v.ed.session.selection().primary().focus.offset;
    let id = ID_ONLY.find("{#a}").unwrap();
    assert!(
        (id..id + 4).contains(&at) || at == id + 4,
        "the arrow lands on the item, not past it: {at}"
    );
    assert!(shown(&mut h).contains("{#a}"), "and with the caret on it, it shows");
}

#[test]
fn a_click_on_an_item_of_only_an_id_lands_on_it() {
    let (_d, _p, mut h) = open(ID_ONLY);
    let id = ID_ONLY.find("{#a}").unwrap();
    let rect = h.state().viewer_item_rect(id).expect("the item is on screen");
    let p = rect.center();
    h.hover_at(p);
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
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
    assert!((id..=id + 4).contains(&at), "{at}");
}

#[test]
fn a_heading_of_only_an_id_keeps_its_place_for_the_caret() {
    let doc = "Top.\n\n## {#h}\n\nNext.\n";
    let (_d, _p, mut h) = open(doc);
    caret(&mut h, doc.find("Next").unwrap());
    h.key_press(Key::ArrowUp);
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
    let id = doc.find("{#h}").unwrap();
    assert!((id..=id + 4).contains(&at), "{at}");
    assert!(shown(&mut h).contains("{#h}"));
}

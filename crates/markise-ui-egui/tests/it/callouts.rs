//! Folding extended callouts in the real app (ADR 0025): the click is view state only; a caret or
//! a search hit going into a closed callout opens it; source mode shows the text.
use crate::common;
use egui_kittest::Harness;
use markise_doc::Selection;
use markise_engine::search::Query;
use markise_ui_egui::{AppOptions, MarkiseApp, ViewMode};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const DOC: &str = "Intro line\n\n> [!warning]- Folded title\n> Secret body.\n>\n> More secret.\n\nBetween\n\n> [!info]+ Open title\n> Visible body.\n\n> [!tip] Not foldable\n> Plain body.\n\nAfter\n";

fn open() -> (Harness<'static, MarkiseApp>, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "markise-callouts-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.md");
    std::fs::write(&path, DOC).unwrap();
    let app = MarkiseApp::new(AppOptions {
        open: vec![path.clone()],
        ..common::no_watch()
    });
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 800.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app);
    let t = Instant::now();
    while h.state().model().is_none() || h.state().is_loading() {
        assert!(t.elapsed() < common::HANG_GUARD, "timed out waiting for the load");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    common::run(&mut h);
    (h, path)
}

fn shown(h: &mut Harness<'static, MarkiseApp>) -> String {
    h.state_mut().viewer().unwrap().visible_text()
}

/// Clicks the header of the callout whose tag starts at `tag` in the document.
fn click_header(h: &mut Harness<'static, MarkiseApp>, tag: &str) {
    let offset = DOC.find(tag).unwrap();
    let v = h.state_mut().viewer().unwrap();
    let r = v.item_screen_rect(offset).unwrap();
    let i = v.ed.view.model().item_at_offset(offset);
    let l = v.ed.view.cache_peek(i).unwrap();
    let p = r.min + l.toggle.expect("a foldable header").center().to_vec2();
    h.hover_at(p);
    h.step();
    h.drag_at(p);
    h.step();
    h.drop_at(p);
    common::run(h);
}

fn caret_to(h: &mut Harness<'static, MarkiseApp>, at: usize) {
    let v = h.state_mut().viewer().unwrap();
    v.ed.set_selection(Selection::caret(at));
    v.ed.refresh_focus();
    common::run(h);
}

fn type_x(h: &mut Harness<'static, MarkiseApp>) {
    let ed = &mut h.state_mut().viewer().unwrap().ed;
    ed.session.insert_text("x").unwrap();
    ed.sync();
    ed.refresh_focus();
    h.run_steps(3);
}

#[test]
fn folds_as_written_and_toggling_never_changes_bytes_dirty_state_or_undo() {
    let (mut h, path) = open();
    let text = shown(&mut h);
    assert!(text.contains("Folded title"), "{text}");
    assert!(!text.contains("Secret body"), "closed as written: {text}");
    assert!(text.contains("Visible body"), "{text}");
    assert!(text.contains("Plain body"), "{text}");
    assert!(!text.contains("[!"), "the tag never shows: {text}");
    let undo = h.state_mut().viewer().unwrap().ed.session.history().undo_len();
    for (tag, body, opens) in [("[!warning]", "Secret body", true), ("[!info]", "Visible body", false)] {
        click_header(&mut h, tag);
        assert_eq!(shown(&mut h).contains(body), opens, "{tag}");
        click_header(&mut h, tag);
        assert_eq!(shown(&mut h).contains(body), !opens, "{tag}");
        click_header(&mut h, tag);
        assert_eq!(h.state().viewed_text(), Some(DOC));
        assert!(!h.state().is_dirty(), "a toggle does not dirty the document");
        assert_eq!(h.state_mut().viewer().unwrap().ed.session.history().undo_len(), undo);
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), DOC);
    // the state stays through a trip to source mode, which shows the text
    h.state_mut().set_view_mode(ViewMode::Source);
    common::run(&mut h);
    assert_eq!(h.state().viewed_text(), Some(DOC));
    h.state_mut().set_view_mode(ViewMode::Rendered);
    common::run(&mut h);
    let text = shown(&mut h);
    assert!(text.contains("Secret body"), "opened above: {text}");
    assert!(!text.contains("Visible body"), "closed above: {text}");
}

#[test]
fn a_header_without_a_fold_mark_is_no_control() {
    let (mut h, _path) = open();
    let offset = DOC.find("[!tip]").unwrap();
    let v = h.state_mut().viewer().unwrap();
    let i = v.ed.view.model().item_at_offset(offset);
    assert!(v.ed.view.cache_peek(i).unwrap().toggle.is_none());
}

#[test]
fn the_caret_entering_a_closed_callout_opens_it() {
    let (mut h, _path) = open();
    caret_to(&mut h, DOC.find("More secret").unwrap());
    assert!(shown(&mut h).contains("More secret"));
    assert!(!h.state().is_dirty());
    assert_eq!(h.state().viewed_text(), Some(DOC));
    // closed again with the caret inside, it stays closed (no reopening every frame) …
    click_header(&mut h, "[!warning]");
    h.run_steps(3);
    assert!(!shown(&mut h).contains("More secret"));
    assert!(!h.state().is_dirty());
    // … and the caret has left it: what is typed next is never out of sight
    type_x(&mut h);
    let text = h.state().viewed_text().unwrap().to_string();
    assert!(text.contains("> More secret.\n"), "{text}");
    assert!(shown(&mut h).contains("xBetween"), "{text:?} shown {}", shown(&mut h));
}

/// An undo can put the caret back into a closed callout, and the change there shows.
#[test]
fn an_edit_landing_in_a_closed_callout_opens_it() {
    let (mut h, _path) = open();
    caret_to(&mut h, DOC.find("More secret").unwrap());
    type_x(&mut h);
    click_header(&mut h, "[!warning]");
    assert!(!shown(&mut h).contains("More secret"));
    let ed = &mut h.state_mut().viewer().unwrap().ed;
    ed.session.undo();
    ed.sync();
    ed.refresh_focus();
    h.run_steps(3);
    assert_eq!(h.state().viewed_text(), Some(DOC));
    assert!(shown(&mut h).contains("More secret"), "{}", shown(&mut h));
}

#[test]
fn the_caret_on_the_title_of_a_closed_callout_opens_it() {
    let (mut h, _path) = open();
    caret_to(&mut h, DOC.find("Folded title").unwrap() + 3);
    assert!(shown(&mut h).contains("Secret body"));
}

#[test]
fn a_search_hit_in_a_closed_callout_opens_it() {
    let (mut h, _path) = open();
    assert!(!shown(&mut h).contains("Secret body"));
    h.state_mut().open_find(true);
    h.state_mut().set_find_query(Query {
        text: "Secret body".into(),
        ..Default::default()
    });
    common::wait_until(&mut h, "find results", |a| a.find_results().is_some());
    h.state_mut().find_next(false);
    common::run(&mut h);
    assert!(shown(&mut h).contains("Secret body"));
    assert_eq!(h.state().viewed_text(), Some(DOC));
    assert!(!h.state().is_dirty());
}

/// The other way round too: closed by the reader with the caret on its first leaf, a hit in that
/// leaf's hidden lines still opens it.
#[test]
fn a_search_hit_on_the_item_the_caret_is_on_opens_it() {
    let (mut h, _path) = open();
    caret_to(&mut h, DOC.find("Folded title").unwrap() + 3);
    click_header(&mut h, "[!warning]");
    assert!(!shown(&mut h).contains("Secret body"));
    h.state_mut().open_find(true);
    h.state_mut().set_find_query(Query {
        text: "Secret body".into(),
        ..Default::default()
    });
    common::wait_until(&mut h, "find results", |a| a.find_results().is_some());
    h.state_mut().find_next(false);
    common::run(&mut h);
    assert!(shown(&mut h).contains("Secret body"));
}

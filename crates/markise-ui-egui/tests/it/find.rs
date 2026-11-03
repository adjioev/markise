//! In-document Find across the common session; spec 04 Edit Find / spec 02 §11.
use crate::common;
use egui_kittest::Harness;
use markise_engine::search::Query;
use markise_ui_egui::menu::MenuAction;
use markise_ui_egui::{MarkiseApp, ViewMode};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn harness() -> Harness<'static, MarkiseApp> {
    let app = MarkiseApp::new(common::no_watch());
    let mut h = Harness::builder()
        .with_size(egui::vec2(1100.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app);
    h.step();
    let ctx = h.ctx.clone();
    h.state_mut().menu_action(&ctx, &MenuAction::Command("file.new"));
    h.step();
    h.state_mut().set_view_mode(ViewMode::Source);
    h.state_mut()
        .edit_source_text(|s| *s = "# Alpha\n\nalpha **beta** alpha\n".into());
    h.state_mut().set_view_mode(ViewMode::Rendered);
    h
}
fn until(h: &mut Harness<'static, MarkiseApp>, condition: impl Fn(&MarkiseApp) -> bool) {
    let start = Instant::now();
    while !condition(h.state()) {
        assert!(start.elapsed() < common::HANG_GUARD, "find worker timed out");
        h.step();
        std::thread::sleep(Duration::from_millis(2));
    }
    h.step();
}
fn search(h: &mut Harness<'static, MarkiseApp>, text: &str) {
    h.state_mut().open_find(true);
    h.state_mut().set_find_query(Query {
        text: text.into(),
        ..Default::default()
    });
    until(h, |a| a.find_results().is_some());
}
#[test]
fn modes_navigation_replace_all_and_guarded_undo() {
    let mut h = harness();
    search(&mut h, "alpha");
    assert_eq!(h.state().find_results().unwrap().matches.len(), 3);
    for mode in [ViewMode::Rendered, ViewMode::Source, ViewMode::SideBySide] {
        h.state_mut().set_view_mode(mode);
        h.step();
        // Switching modes may change the document revision through literal hints; await refresh.
        until(&mut h, |a| a.find_results().is_some());
        let before = h.state().viewed_text().unwrap().to_owned();
        h.state_mut().find_next(false);
        h.step();
        assert_eq!(h.state().viewed_text(), Some(before.as_str()));
        assert!(h.state().find_active().is_some());
        if mode != ViewMode::Rendered {
            let i = h.state().find_active().unwrap();
            let range = h.state().find_results().unwrap().matches[i].clone();
            assert_eq!(h.state().source_selection(), Some(range));
        }
        assert!(h.state().menu_model().item("find.open").unwrap().enabled);
    }
    h.state_mut().replace_find("X".into(), true);
    until(&mut h, |a| a.viewed_text() == Some("# X\n\nX **beta** X\n"));
    assert!(h.state_mut().undo_replace_all());
    h.step();
    assert_eq!(h.state().viewed_text(), Some("# Alpha\n\nalpha **beta** alpha\n"));
    search(&mut h, "alpha");
    h.state_mut().replace_find("Y".into(), true);
    until(&mut h, |a| a.viewed_text() == Some("# Y\n\nY **beta** Y\n"));
    h.state_mut().set_view_mode(ViewMode::Source);
    h.state_mut().edit_source_text(|s| s.push('!'));
    assert!(!h.state_mut().undo_replace_all());
    assert!(h.state().viewed_text().unwrap().ends_with('!'));
}
#[test]
fn no_match_replace_is_noop_and_edits_reject_old_results() {
    let mut h = harness();
    search(&mut h, "missing");
    h.state_mut().replace_find("z".into(), false); // Regression: no recursive navigation on no matches.
    assert_eq!(h.state().viewed_text(), Some("# Alpha\n\nalpha **beta** alpha\n"));
    search(&mut h, "alpha");
    let old = h.state().find_results().unwrap().key;
    h.state_mut().set_view_mode(ViewMode::Source);
    h.state_mut().edit_source_text(|s| *s = "new text".into());
    assert!(h.state().find_results().is_none());
    h.state_mut().replace_find("bad".into(), true);
    until(&mut h, |a| a.find_results().is_some());
    let results = h.state().find_results().unwrap();
    assert_ne!(results.key.revision, old.revision);
    assert!(results.matches.is_empty());
    assert_eq!(h.state().viewed_text(), Some("new text"));
    search(&mut h, "new");
    let shared = Arc::clone(&h.state().find_results().unwrap().matches);
    h.step();
    h.step();
    assert!(Arc::ptr_eq(&shared, &h.state().find_results().unwrap().matches));
    h.state_mut().close_find();
    h.state_mut().find_next(false);
    h.step();
    assert_eq!(h.state().find_active(), Some(0));
}

#[test]
fn keyboard_bar_focus_highlights_and_no_snapshot_cow() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness();
    h.run_steps(3);
    let id = h.state_mut().viewer().unwrap().surface_id().unwrap();
    h.ctx.memory_mut(|m| m.request_focus(id));
    let copies = h.state_mut().viewer().unwrap().ed.session.copies;
    h.key_press_modifiers(
        egui::Modifiers {
            command: true,
            mac_cmd: cfg!(target_os = "macos"),
            ctrl: !cfg!(target_os = "macos"),
            ..Default::default()
        },
        egui::Key::F,
    );
    h.run_steps(2);
    h.get_by_label("Find").type_text("alpha");
    until(&mut h, |a| a.find_results().is_some_and(|r| r.matches.len() == 3));
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().find_active(), Some(0));
    {
        let viewer = h.state_mut().viewer().unwrap();
        assert_eq!(viewer.ed.session.selection().primary().range(), 2..7);
        assert!(!viewer.search_highlights().is_empty());
        assert!(viewer.search_highlights().iter().all(|r| r.is_positive()));
        assert_eq!(
            viewer.ed.session.copies, copies,
            "search snapshot must not add document copies"
        );
    }
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().find_active(), Some(2));
    h.get_by_label("Replace…").click();
    h.run_steps(2);
    h.get_by_label("Replace with").focus();
    h.run_steps(1);
    h.get_by_label("Replace with").type_text("Z");
    h.run_steps(2);
    h.get_by_label("Replace").click();
    until(&mut h, |a| a.viewed_text() == Some("# Alpha\n\nalpha **beta** Z\n"));
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.ctx.memory(|m| m.has_focus(id)), "Escape restores the editor focus");
    h.event(egui::Event::Text("!".into()));
    h.run_steps(2);
    assert!(h.state().viewed_text().unwrap().contains('!'));
}

#[test]
fn initial_navigation_uses_independent_source_caret() {
    let mut h = harness();
    h.state_mut().set_view_mode(ViewMode::Source);
    h.step();
    {
        let ed = &mut h.state_mut().viewer().unwrap().ed;
        ed.source.as_mut().unwrap().sel = markise_doc::Selection::caret(8);
        ed.source_store_selection();
        ed.session.set_selection(markise_doc::Selection::caret(0));
    }
    search(&mut h, "alpha");
    h.state_mut().find_next(false);
    h.step();
    assert_eq!(h.state().find_active(), Some(1));
    assert_eq!(h.state().source_selection(), Some(9..14));
}

#[test]
fn hidden_syntax_navigation_preserves_raw_range_and_source_fallback() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness();
    search(&mut h, "**");
    h.get_by_label("Next").click();
    h.run_steps(2);
    assert_eq!(
        h.state_mut().viewer().unwrap().ed.session.selection().primary().range(),
        15..17
    );
    h.get_by_label("Show in Source").click();
    h.run_steps(2);
    assert_eq!(h.state().source_selection(), Some(15..17));
    assert_eq!(h.state().viewed_text(), Some("# Alpha\n\nalpha **beta** alpha\n"));
}

#[test]
fn replace_next_reopens_closed_bar_and_repeated_find_selects_query() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness();
    search(&mut h, "alpha");
    h.get_by_label("Replace with").focus();
    h.run_steps(1);
    h.get_by_label("Replace with").type_text("Z");
    h.run_steps(2);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    let ctx = h.ctx.clone();
    h.state_mut()
        .menu_action(&ctx, &MenuAction::Command("find.replaceNext"));
    until(&mut h, |a| a.viewed_text() == Some("# Z\n\nalpha **beta** alpha\n"));
    h.get_by_label("Find");
    h.state_mut().menu_action(&ctx, &MenuAction::Command("find.open"));
    h.run_steps(2);
    h.event(egui::Event::Text("beta".into()));
    until(&mut h, |a| {
        a.find_results()
            .is_some_and(|r| r.matches.len() == 1 && r.matches[0] == (13..17))
    });
}

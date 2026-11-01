//! Issue #103: clicks and source reveal use the real editor, not a second HTML editor.
use egui_kittest::Harness;
use markise_ui_egui::{AppOptions, MarkiseApp, ViewMode, menu::MenuAction};
const DOC: &str = "Intro\n\n<details>\n<summary>A useful hint</summary>\n\nSecret **answer**.\n\n</details>\n\nAfter <kbd>Enter</kbd>.\n\n<!-- hidden comment -->\n";
fn harness() -> Harness<'static, MarkiseApp> {
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 800.0))
        .allow_missing_glyphs()
        .build_ui_state(
            |ui, app: &mut MarkiseApp| app.ui(ui),
            MarkiseApp::new(AppOptions::default()),
        );
    h.step();
    let ctx = h.ctx.clone();
    h.state_mut().menu_action(&ctx, &MenuAction::Command("file.new"));
    h.step();
    h.state_mut().set_view_mode(ViewMode::Source);
    h.state_mut().edit_source_text(|s| *s = DOC.into());
    h.state_mut()
        .viewer()
        .unwrap()
        .ed
        .set_selection(markise_doc::Selection::caret(0));
    h.state_mut().set_view_mode(ViewMode::Rendered);
    h.run_steps(4);
    assert_eq!(h.state().viewed_text(), Some(DOC));
    h
}
fn click_header(h: &mut Harness<'static, MarkiseApp>) {
    let offset = DOC.find("<details>").unwrap();
    let v = h.state_mut().viewer().unwrap();
    let r = v.item_screen_rect(offset).unwrap();
    let i = v.ed.view.model().item_at_offset(offset);
    let l = v.ed.view.cache_peek(i).unwrap();
    let p = r.min + l.toggle.expect("disclosure toggle").center().to_vec2();
    h.hover_at(p);
    h.step();
    h.drag_at(p);
    h.step();
    h.drop_at(p);
    h.run_steps(3);
}
#[test]
fn click_toggles_do_not_edit_or_add_undo_and_source_mode_keeps_all_markup() {
    let mut h = harness();
    let before = h.state_mut().viewer().unwrap().ed.session.history().undo_len();
    assert!(!h.state_mut().viewer().unwrap().visible_text().contains("Secret"));
    click_header(&mut h);
    assert!(h.state_mut().viewer().unwrap().visible_text().contains("Secret"));
    // The caret inside a child must not reopen it every frame after the user collapses it.
    h.state_mut()
        .viewer()
        .unwrap()
        .ed
        .set_selection(markise_doc::Selection::caret(DOC.find("answer").unwrap()));
    h.state_mut().viewer().unwrap().ed.refresh_focus();
    h.run_steps(2);
    click_header(&mut h);
    assert!(!h.state_mut().viewer().unwrap().visible_text().contains("Secret"));
    assert_eq!(h.state().viewed_text(), Some(DOC));
    assert_eq!(h.state_mut().viewer().unwrap().ed.session.history().undo_len(), before);
    h.state_mut().set_view_mode(ViewMode::Source);
    h.run_steps(2);
    assert_eq!(h.state().viewed_text(), Some(DOC));
}
#[test]
fn caret_on_tag_or_comment_reveals_source_for_editing() {
    let mut h = harness();
    for token in ["summary>", "hidden comment"] {
        h.state_mut()
            .viewer()
            .unwrap()
            .ed
            .set_selection(markise_doc::Selection::caret(DOC.find(token).unwrap() + 2));
        h.state_mut().viewer().unwrap().ed.refresh_focus();
        h.run_steps(3);
        let text = h.state_mut().viewer().unwrap().visible_text();
        assert!(text.contains(token), "{token}: {text:?}");
    }
    assert_eq!(h.state().viewed_text(), Some(DOC));
}

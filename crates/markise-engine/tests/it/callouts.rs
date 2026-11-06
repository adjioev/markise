//! Editing an extended callout in the rendered view (ADR 0025): the title is text; the tag is
//! syntax a Backspace removes as a whole; other bytes stay.

use crate::common::T;

#[test]
fn typing_in_the_title_and_the_body_changes_only_those_bytes() {
    let mut t = T::at("> [!warning]- Ti‸tle\n> Body.\n");
    t.typ("x");
    assert_eq!(t.shown(), "> [!warning]- Tix‸tle\n> Body.\n");
    assert!(t.caret_visible());
    assert!(t.selection_on_stops());
    t.check_model();
    let mut t = T::at("> [!warning]- Title\n> Bo‸dy.\n");
    t.typ("x");
    assert_eq!(t.shown(), "> [!warning]- Title\n> Box‸dy.\n");
    assert!(t.selection_on_stops());
    t.check_model();
}

#[test]
fn the_caret_walks_from_the_body_up_to_the_title_and_never_into_the_tag() {
    let mut t = T::at("> [!info] Title\n> ‸Body\n");
    t.run("caret.up");
    assert!(t.shown().contains("[!info] ") && t.shown().contains('‸'));
    let at = t.shown().find('‸').unwrap();
    assert!(at >= "> [!info] ".len(), "{}", t.shown());
    assert!(t.selection_on_stops());
    t.run("caret.lineStart");
    assert_eq!(t.shown(), "> [!info] ‸Title\n> Body\n");
    assert!(t.selection_on_stops());
}

#[test]
fn backspace_at_the_start_of_a_title_removes_the_tag() {
    let mut t = T::at("> [!warning]- ‸Title\n> Body.\n");
    t.run("edit.deleteBackward");
    assert_eq!(t.shown(), "> ‸Title\n> Body.\n");
    assert!(t.ed.session.doc().debug_tree().starts_with("quote\n"));
    t.check_model();
    t.run("edit.undo");
    assert_eq!(t.text(), "> [!warning]- Title\n> Body.\n");
}

#[test]
fn backspace_at_the_start_of_the_body_removes_a_tag_without_a_title() {
    let mut t = T::at("> [!info]-\n> ‸Body\n");
    t.run("edit.deleteBackward");
    assert_eq!(t.shown(), "> ‸Body\n");
    t.check_model();
    // with a title, the two lines join like any others and the title stays
    let mut t = T::at("> [!info] Title\n> ‸Body\n");
    t.run("edit.deleteBackward");
    assert!(t.text().starts_with("> [!info] Title"), "{}", t.shown());
    assert!(t.text().contains("Body"));
    t.check_model();
}

#[test]
fn enter_in_a_title_splits_it_into_the_body() {
    let mut t = T::at("> [!info] Ti‸tle\n");
    t.run("edit.newline");
    assert!(t.text().starts_with("> [!info] Ti\n"), "{}", t.shown());
    assert!(t.ed.session.doc().debug_tree().starts_with("callout-info"));
    assert!(t.selection_on_stops());
    t.check_model();
}

/// With the option off the line is plain text: Backspace removes one space, not the tag.
#[test]
fn backspace_leaves_a_tag_alone_when_it_is_not_a_callout() {
    use markise_doc::{DocumentFormat, Selection};
    use markise_engine::editor::{EditorSession, EditorView, HistoryLimits, ManualClock};
    use markise_layout::{DocModel, Paging, View, testing};
    use markise_md::{MarkdownFormat, ParseOptions};
    use std::sync::Arc;

    let plain = |text: &str, at: usize| {
        let clock = ManualClock::default();
        let format: Arc<dyn DocumentFormat> = Arc::new(MarkdownFormat {
            opts: ParseOptions {
                alerts: false,
                callouts: false,
                ..Default::default()
            },
        });
        let session = EditorSession::with(format, text, HistoryLimits::default(), Box::new(clock.clone()));
        let model = Arc::new(DocModel::build_with(session.doc(), None, Paging::Full));
        let view = View::new(
            session.doc_arc().clone(),
            model,
            markise_theme::Theme::light(),
            600.0,
            1.0,
        );
        let mut t = T {
            ed: EditorView::new(session, view, None),
            fonts: testing::fonts(),
            clock,
            fix_selection: false,
        };
        t.ed.set_selection(Selection::caret(at));
        t.frame();
        t.run("edit.deleteBackward");
        t.text().to_string()
    };
    let src = "> [!info] Title\n";
    assert_eq!(plain(src, src.find("Title").unwrap()), "> [!info]Title\n");
    // on the second line: the lines join, the tag stays
    let src = "> [!info]\n> Body\n";
    let got = plain(src, src.find("Body").unwrap());
    assert!(got.contains("[!info]"), "{got:?}");
}

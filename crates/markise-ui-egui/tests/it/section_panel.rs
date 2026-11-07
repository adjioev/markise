//! A section's row in the outline, on the real app (ADR 0026): its menu, by the pointer and the keyboard, what the
//! engine refuses, the notice with Undo, and that Read has none of it. No window.

use egui::{Event, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use markise_ui_egui::{AppOptions, ConfigDir, MarkiseApp};

use crate::block_library::frames;
use crate::block_views::{Fixture, text};
use crate::common;

type H = Harness<'static, MarkiseApp>;

const HEAD: &str = "---\nauthoring: blocks-v3\n---\n\n";
const DOC: &str =
    "---\nauthoring: blocks-v3\n---\n\nOne.\n\n---\n\n## Two {#two}\n\nText.\n\n---\n\n## Three\n\nMore.\n";

pub(crate) struct Plain {
    _dir: tempfile::TempDir,
    config: std::path::PathBuf,
    doc: std::path::PathBuf,
}

pub(crate) fn plain(text: &str) -> Plain {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let (config, doc) = (root.join("config"), root.join("doc.md"));
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(&doc, text).unwrap();
    Plain { _dir: dir, config, doc }
}

pub(crate) fn opened(p: &Plain) -> H {
    let opts = AppOptions {
        config: ConfigDir::Dir(p.config.clone()),
        ..common::no_watch()
    };
    let mut h = Harness::builder()
        .with_size(egui::vec2(1100.0, 800.0))
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), MarkiseApp::new(opts));
    h.state_mut().open(p.doc.clone());
    let want = p.doc.clone();
    common::wait_until(&mut h, "document load", move |a| {
        a.document_path() == Some(want.as_path()) && !a.is_loading()
    });
    common::run(&mut h);
    h
}

pub(crate) fn row(h: &H, name: &str) -> Pos2 {
    // at the left, a heading of the same name standing in the document too; the first, its heading's row
    // standing under it
    h.query_all_by_label(name)
        .map(|n| n.rect())
        .min_by(|a, b| (a.left(), a.top()).partial_cmp(&(b.left(), b.top())).unwrap())
        .unwrap_or_else(|| panic!("no row {name}"))
        .center()
}

fn secondary_click(h: &mut H, p: Pos2) {
    h.hover_at(p);
    frames(h);
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos: p,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        h.step();
    }
    frames(h);
}

/// A few frames only: every frame here counts as a quarter of a second, and a notice goes after a while.
fn press_item(h: &mut H, label: &str) {
    h.get_by_label(label).click();
    for _ in 0..4 {
        h.step();
    }
}

fn menu_on(h: &mut H, name: &str) {
    let p = row(h, name);
    secondary_click(h, p);
}

#[test]
fn a_right_click_has_the_menu_and_its_moves_are_one_undo_step() {
    let p = plain(DOC);
    let mut h = opened(&p);
    menu_on(&mut h, "Two");
    press_item(&mut h, "Move Down");
    assert_eq!(
        text(&h),
        format!("{HEAD}One.\n\n---\n\n## Three\n\nMore.\n\n---\n\n## Two {{#two}}\n\nText.\n")
    );
    h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
    frames(&mut h);
    assert_eq!(text(&h), DOC);
    assert!(h.state().banner_text().is_none(), "no title here, so nothing to say");
}

#[test]
fn the_dots_show_on_hover_and_have_the_same_menu() {
    let p = plain(DOC);
    let mut h = opened(&p);
    assert_eq!(h.query_all_by_label("Section actions").count(), 0, "quiet");
    let at = row(&h, "Two");
    h.hover_at(at);
    frames(&mut h);
    assert_eq!(h.query_all_by_label("Section actions").count(), 1);
    let dots = h.get_by_label("Section actions").rect().center();
    h.hover_at(dots);
    frames(&mut h);
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos: dots,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        h.step();
    }
    frames(&mut h);
    press_item(&mut h, "Move Up");
    assert_eq!(
        text(&h),
        format!("{HEAD}## Two {{#two}}\n\nText.\n\n---\n\nOne.\n\n---\n\n## Three\n\nMore.\n")
    );
}

#[test]
fn only_what_can_be_done_is_offered() {
    let p = plain(DOC);
    let mut h = opened(&p);
    menu_on(&mut h, "Section 1");
    let enabled = |h: &H, l: &str| !h.get_by_label(l).accesskit_node().is_disabled();
    assert!(!enabled(&h, "Move Up"));
    assert!(enabled(&h, "Move Down"));
    h.key_press(Key::Escape);
    frames(&mut h);
    menu_on(&mut h, "Three");
    assert!(!enabled(&h, "Move Down"));
    assert!(enabled(&h, "Move Up"));
    assert!(enabled(&h, "Delete"));
}

#[test]
fn a_refusal_says_the_engine_s_message_and_changes_nothing() {
    let doc = format!("{HEAD}One.\n\n---\n\n:::section-metadata\n\nid: m\n\n:::\n\n---\n\nThree.\n");
    let p = plain(&doc);
    let mut h = opened(&p);
    menu_on(&mut h, "m");
    press_item(&mut h, "Move Down");
    assert_eq!(text(&h), doc);
    let said = h.state().banner_text().unwrap();
    assert!(said.contains("Section not moved"), "{said}");
    assert!(said.contains("fill it in or delete it first"), "{said}");
}

#[test]
fn delete_asks_nothing_and_the_notice_takes_it_back_exactly() {
    let p = plain(DOC);
    let mut h = opened(&p);
    menu_on(&mut h, "Two");
    press_item(&mut h, "Delete");
    let gone = format!("{HEAD}One.\n\n---\n\n## Three\n\nMore.\n");
    assert_eq!(text(&h), gone);
    assert!(!h.state().is_asking());
    assert_eq!(h.state().banner_text().unwrap(), "Section deleted: Two");
    h.get_by_label("Undo").click();
    frames(&mut h);
    frames(&mut h);
    assert_eq!(text(&h), DOC);
    assert!(h.state().banner_text().is_none());
}

#[test]
fn the_notice_goes_with_the_next_edit_and_after_a_while() {
    let p = plain(DOC);
    let mut h = opened(&p);
    menu_on(&mut h, "Two");
    press_item(&mut h, "Delete");
    assert!(h.state().banner_text().is_some());
    // a key the document takes: its Undo would no longer be this delete's
    let v = h.state_mut().viewer().unwrap();
    v.ed.set_selection(markise_doc::Selection::caret_at(markise_doc::DocPos::new(HEAD.len())));
    let _ = v.ed.edit(|s| s.insert_text("x"));
    frames(&mut h);
    assert!(h.state().banner_text().is_none());

    let p = plain(DOC);
    let mut h = opened(&p);
    menu_on(&mut h, "Two");
    press_item(&mut h, "Delete");
    let from = h.ctx.input(|i| i.time);
    while h.ctx.input(|i| i.time) < from + 10.0 {
        h.step();
    }
    h.step();
    assert!(h.state().banner_text().is_none(), "short");
}

#[test]
fn duplicate_gives_new_ids_and_is_one_undo_step() {
    let p = plain(DOC);
    let mut h = opened(&p);
    menu_on(&mut h, "Two");
    press_item(&mut h, "Duplicate");
    assert_eq!(
        text(&h),
        format!(
            "{HEAD}One.\n\n---\n\n## Two {{#two}}\n\nText.\n\n---\n\n## Two {{#two-2}}\n\nText.\n\n---\n\n## Three\n\nMore.\n"
        )
    );
    h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
    frames(&mut h);
    assert_eq!(text(&h), DOC);
}

#[test]
fn insert_after_opens_the_form_and_goes_after_that_one_wherever_the_caret_is() {
    let p = plain(DOC);
    let mut h = opened(&p);
    menu_on(&mut h, "Two");
    press_item(&mut h, "Insert After…");
    assert_eq!(text(&h), DOC, "nothing before Save");
    assert_eq!(h.state().section_form().unwrap().heading(), "Section");
    h.state_mut().set_section_form_title("New");
    let ctx = h.ctx.clone();
    assert!(h.state_mut().save_section_form(&ctx));
    frames(&mut h);
    assert_eq!(
        text(&h),
        format!(
            "{HEAD}One.\n\n---\n\n## Two {{#two}}\n\nText.\n\n---\n\n## New {{#new}}\n\n---\n\n## Three\n\nMore.\n"
        )
    );
    h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
    frames(&mut h);
    assert_eq!(text(&h), DOC);
}

#[test]
fn the_title_going_along_is_said_in_one_line_and_not_refused() {
    let title = format!("{HEAD}# Title\n\nIntro.\n\n---\n\nTwo.\n\n---\n\nThree.\n");
    // with its section
    let p = plain(&title);
    let mut h = opened(&p);
    menu_on(&mut h, "Section 1");
    press_item(&mut h, "Move Down");
    assert_eq!(
        text(&h),
        format!("{HEAD}Two.\n\n---\n\n# Title\n\nIntro.\n\n---\n\nThree.\n")
    );
    let said = h.state().banner_text().unwrap();
    assert!(said.contains("title with it"), "{said}");
    assert!(!said.contains('\n'));
    // Undo takes it back
    h.get_by_label("Undo").click();
    frames(&mut h);
    frames(&mut h);
    assert_eq!(text(&h), title);
    // another one going above it
    menu_on(&mut h, "Section 2");
    press_item(&mut h, "Move Up");
    assert_eq!(
        text(&h),
        format!("{HEAD}Two.\n\n---\n\n# Title\n\nIntro.\n\n---\n\nThree.\n")
    );
    assert!(h.state().banner_text().unwrap().contains("title moves too"));
    // far from it: nothing to say
    h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
    frames(&mut h);
    assert_eq!(text(&h), title);
    menu_on(&mut h, "Section 3");
    press_item(&mut h, "Move Up");
    assert!(h.state().banner_text().is_none());
    // a second `#`: no one title to speak of
    let two = format!("{HEAD}# Title\n\nIntro.\n\n---\n\n# Other\n\nTwo.\n");
    let p = plain(&two);
    let mut h = opened(&p);
    menu_on(&mut h, "Section 1");
    press_item(&mut h, "Move Down");
    assert_ne!(text(&h), two);
    assert!(h.state().banner_text().is_none());
}

#[test]
fn a_section_s_row_is_not_a_heading_s_and_only_it_has_controls() {
    let doc = format!("{HEAD}# Title\n\n## Under\n\n---\n\n## Named\n\n### Below\n\n---\n\nNo heading.\n");
    let p = plain(&doc);
    let mut h = opened(&p);
    // the title is a heading under a section; a heading that names its section has no row of its own
    for name in ["Section 1", "Title", "Under", "Named", "Below", "Section 3"] {
        assert_eq!(h.query_all_by_label(name).count(), 1, "{name}");
    }
    let (section, title, under) = (row(&h, "Section 1"), row(&h, "Title"), row(&h, "Under"));
    assert!(section.y < title.y && title.y < under.y);
    for heading in ["Title", "Under", "Below"] {
        h.hover_at(row(&h, heading));
        frames(&mut h);
        assert_eq!(h.query_all_by_label("Section actions").count(), 0, "{heading}");
        let at = row(&h, heading);
        secondary_click(&mut h, at);
        assert_eq!(h.query_all_by_label("Delete").count(), 0, "{heading}");
        h.key_press(Key::Escape);
        frames(&mut h);
    }
    // its controls are at the row's start, on it
    for name in ["Section 1", "Named", "Section 3"] {
        let at = row(&h, name);
        h.hover_at(at);
        frames(&mut h);
        let dots = h.get_by_label("Section actions").rect();
        assert!((dots.center().y - at.y).abs() < 2.0, "{name}");
        assert!(dots.right() < at.x, "at its start: {name}");
    }
    // in Read it reads the same and has none
    h.state_mut().viewer().unwrap().ed.set_read_mode(true);
    frames(&mut h);
    for name in ["Section 1", "Title", "Named"] {
        assert_eq!(h.query_all_by_label(name).count(), 1, "{name}");
        h.hover_at(row(&h, name));
        frames(&mut h);
        assert_eq!(h.query_all_by_label("Section actions").count(), 0, "{name}");
    }
    assert_eq!(text(&h), doc);
}

/// What a reload does: the document goes on with another text, with no edit of the surface's.
pub(crate) fn reload(h: &mut H, text: &str) {
    let doc = std::sync::Arc::new(markise_md::parse(text));
    let model = markise_layout::DocModel::build(doc.as_ref(), None);
    let v = h.state_mut().viewer().unwrap();
    v.install(doc, model, "Reload from Disk", false).unwrap();
}

#[test]
fn a_reload_ends_an_open_menu_and_nothing_is_done_with_what_was_at_its_place() {
    const ABC: &str = "---\nauthoring: blocks-v3\n---\n\n## A\n\n---\n\n## B\n\n---\n\n## C\n";
    const XBC: &str = "---\nauthoring: blocks-v3\n---\n\n## A\n\n---\n\n## X\n\n---\n\n## B\n\n---\n\n## C\n";
    let p = plain(ABC);
    let mut h = opened(&p);
    menu_on(&mut h, "B");
    let delete = h.get_by_label("Delete").rect().center();
    reload(&mut h, XBC);
    h.step();
    h.step();
    assert_eq!(h.query_all_by_label("Delete").count(), 0, "the menu closed");
    // a press where it stood: on the row or the document that lies there, not on Delete
    h.hover_at(delete);
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos: delete,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        h.step();
    }
    frames(&mut h);
    assert_eq!(text(&h), XBC, "X is still there");
    // and a new menu is for what is there now
    menu_on(&mut h, "X");
    press_item(&mut h, "Delete");
    assert_eq!(
        text(&h),
        "---\nauthoring: blocks-v3\n---\n\n## A\n\n---\n\n## B\n\n---\n\n## C\n"
    );
}

#[test]
fn a_duplicate_keeps_what_a_block_means_by_its_ids() {
    let doc = format!("{HEAD}A\n\n---\n\n## B {{#b}}\n\n:::q\n\n- [ ] One {{#a1}}\n- [ ] None {{#none}}\n\n:::\n");
    let p = plain(&doc);
    let mut h = opened(&p);
    menu_on(&mut h, "B");
    press_item(&mut h, "Duplicate");
    let t = text(&h);
    assert_eq!(t.matches("- [ ] None {#none}\n").count(), 2, "{t}");
    assert_eq!(t.matches("- [ ] One {#a1}\n").count(), 2, "{t}");
    assert!(t.contains("## B {#b-2}\n"), "{t}");
}

#[test]
fn read_has_none_of_it_and_neither_has_a_document_without_sections() {
    let p = plain(DOC);
    let mut h = opened(&p);
    h.state_mut().viewer().unwrap().ed.set_read_mode(true);
    frames(&mut h);
    let at = row(&h, "Two");
    h.hover_at(at);
    frames(&mut h);
    assert_eq!(h.query_all_by_label("Section actions").count(), 0);
    secondary_click(&mut h, at);
    assert_eq!(h.query_all_by_label("Delete").count(), 0);
    assert_eq!(text(&h), DOC);
    // back in Edit it is there
    h.state_mut().viewer().unwrap().ed.set_read_mode(false);
    frames(&mut h);
    h.hover_at(at);
    frames(&mut h);
    assert_eq!(h.query_all_by_label("Section actions").count(), 1);

    let p = plain("# One\n\n---\n\nTwo.\n");
    let mut h = opened(&p);
    assert_eq!(h.query_all_by_label("Section 1").count(), 0, "a plain outline");
    let at = row(&h, "One");
    h.hover_at(at);
    frames(&mut h);
    assert_eq!(h.query_all_by_label("Section actions").count(), 0);
}

const FILES: &[(&str, &str)] = &[
    (
        "manifest.json",
        r#"{"format":"markise-block-package/2","library":"sample","version":"1.0.0","blocks":["tip"],"authoring":"blocks-v3"}"#,
    ),
    (
        "components/tip/tip.json",
        r#"{"label":"Tip","fields":[{"name":"body","type":"text"}]}"#,
    ),
    ("editor/palette.json", r#"{"format":1}"#),
    (
        "editor/sections.json",
        r#"{"format":1,"label":{"en":"Chapter","ru":"Глава"}}"#,
    ),
];

#[test]
fn the_word_is_the_package_s() {
    let doc = "One.\n\n---\n\nTwo.\n\n---\n\nThree.\n";
    let f = Fixture::with_package(doc, FILES);
    let mut h = f.ready();
    let at = row(&h, "Chapter 2");
    h.hover_at(at);
    frames(&mut h);
    assert_eq!(h.query_all_by_label("Chapter actions").count(), 1);
    menu_on(&mut h, "Chapter 2");
    press_item(&mut h, "Delete");
    assert_eq!(h.state().banner_text().unwrap(), "Chapter deleted: Chapter 2");
}

/// From the keyboard alone: off the document, Tab along the panel to a row, Shift+F10, the arrows and Return.
#[test]
fn the_menu_can_be_reached_and_used_by_the_keyboard() {
    let p = plain(DOC);
    let mut h = opened(&p);
    let mut guard = 0;
    let want = row(&h, "Two");
    // Esc lets go of the document
    h.key_press(Key::Escape);
    frames(&mut h);
    loop {
        h.key_press(Key::Tab);
        frames(&mut h);
        let on = h.ctx.memory(|m| m.focused());
        let r = on.and_then(|id| h.ctx.read_response(id)).map(|r| r.rect);
        if r.is_some_and(|r| r.center() == want) {
            break;
        }
        guard += 1;
        assert!(guard < 60, "no way to the row by Tab: {r:?}");
    }
    // its dots show now, and the next Tab is on them
    assert_eq!(h.query_all_by_label("Section actions").count(), 1);
    h.key_press_modifiers(Modifiers::SHIFT, Key::F10);
    frames(&mut h);
    assert_eq!(h.query_all_by_label("Move Up").count(), 1);
    // on the first item: down to the next and Return
    h.key_press(Key::ArrowDown);
    frames(&mut h);
    h.key_press(Key::Enter);
    frames(&mut h);
    frames(&mut h);
    assert_eq!(
        text(&h),
        format!("{HEAD}One.\n\n---\n\n## Three\n\nMore.\n\n---\n\n## Two {{#two}}\n\nText.\n")
    );
}

#[test]
fn esc_closes_the_keyboard_s_menu_and_the_keyboard_goes_back_to_the_dots() {
    let p = plain(DOC);
    let mut h = opened(&p);
    // the keyboard on the dots
    h.hover_at(row(&h, "Two"));
    frames(&mut h);
    h.get_by_label("Section actions").focus();
    frames(&mut h);
    h.key_press(Key::Enter);
    frames(&mut h);
    assert_eq!(h.query_all_by_label("Duplicate").count(), 1);
    h.key_press(Key::Escape);
    frames(&mut h);
    assert_eq!(h.query_all_by_label("Duplicate").count(), 0);
    assert_eq!(text(&h), DOC);
    let dots = h.get_by_label("Section actions").rect();
    let on = h.ctx.memory(|m| m.focused()).and_then(|id| h.ctx.read_response(id));
    assert_eq!(on.map(|r| r.rect), Some(dots));
}

/// Dragging a row (ADR 0026).
mod dragging {
    use egui::vec2;
    use markise_ui_egui::section_panel::SectionDragShown;

    use super::*;

    const FOUR: &str = "---\nauthoring: blocks-v3\n---\n\n## One\n\n### Under one\n\n---\n\n## Two\n\n### Under two\n\n---\n\n## Three\n\n---\n\n## Four\n";
    const LAST_FIRST: &str = "---\nauthoring: blocks-v3\n---\n\n## Four\n\n---\n\n## One\n\n### Under one\n\n---\n\n## Two\n\n### Under two\n\n---\n\n## Three\n";

    fn button(h: &mut H, p: Pos2, pressed: bool) {
        h.event(Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        h.step();
        h.step();
    }

    /// The pointer to the row, then to its grip, and a press.
    fn pick_up(h: &mut H, name: &str) -> Pos2 {
        let at = row(h, name);
        h.hover_at(at);
        frames(h);
        let grip = h.get_by_label("Drag Section to move").rect().center();
        h.hover_at(grip);
        h.step();
        h.step();
        button(h, grip, true);
        grip
    }

    fn walk(h: &mut H, to: Pos2) {
        h.hover_at(to);
        h.step();
        h.step();
    }

    fn shown(h: &H) -> SectionDragShown {
        h.state().section_drag_shown().expect("a drag shows").clone()
    }

    fn cursor(h: &mut H) -> egui::CursorIcon {
        h.step();
        h.output().platform_output.cursor_icon
    }

    #[test]
    fn a_grip_shows_on_hover_with_grab_and_read_has_none() {
        let p = plain(FOUR);
        let mut h = opened(&p);
        assert_eq!(h.query_all_by_label("Drag Section to move").count(), 0, "quiet");
        h.hover_at(row(&h, "Two"));
        frames(&mut h);
        let grip = h.get_by_label("Drag Section to move").rect().center();
        h.hover_at(grip);
        frames(&mut h);
        assert_eq!(cursor(&mut h), egui::CursorIcon::Grab);
        h.state_mut().viewer().unwrap().ed.set_read_mode(true);
        frames(&mut h);
        assert_eq!(h.query_all_by_label("Drag Section to move").count(), 0);
    }

    #[test]
    fn only_a_section_s_row_has_a_grip_at_its_start_and_read_keeps_it_out() {
        let doc = format!("{HEAD}# Title\n\n## Under\n\n---\n\n## Named\n\n### Below\n\n---\n\nText.\n");
        let p = plain(&doc);
        let mut h = opened(&p);
        for heading in ["Title", "Under", "Below"] {
            h.hover_at(row(&h, heading));
            frames(&mut h);
            assert_eq!(h.query_all_by_label("Drag Section to move").count(), 0, "{heading}");
        }
        for name in ["Section 1", "Named", "Section 3"] {
            let at = row(&h, name);
            h.hover_at(at);
            frames(&mut h);
            let grip = h.get_by_label("Drag Section to move").rect();
            assert!((grip.center().y - at.y).abs() < 2.0, "{name}");
            let dots = h.get_by_label("Section actions").rect();
            assert!(grip.right() <= dots.left() + 1.0 && dots.right() < at.x, "{name}");
        }
        // on the title's section: what goes is the section, with the warning
        let grip = pick_up(&mut h, "Section 1");
        let to = grip + vec2(0.0, 26.0 * 5.0);
        walk(&mut h, to);
        walk(&mut h, to);
        assert!(shown(&h).moves);
        button(&mut h, to, false);
        h.step();
        assert!(h.state().banner_text().unwrap().contains("title with it"));
        h.state_mut().viewer().unwrap().ed.set_read_mode(true);
        frames(&mut h);
        h.hover_at(row(&h, "Named"));
        frames(&mut h);
        assert_eq!(h.query_all_by_label("Drag Section to move").count(), 0);
    }

    #[test]
    fn from_the_press_a_line_shows_headings_collapse_and_the_cursor_is_grabbing() {
        let p = plain(FOUR);
        let mut h = opened(&p);
        let grip = pick_up(&mut h, "Two");
        let s = shown(&h);
        assert_eq!(s.section, 1);
        assert_eq!(s.rows, 4, "only sections while it lasts");
        assert!(!s.moves, "it stays until it goes somewhere");
        assert!((s.line_y - grip.y).abs() < 20.0, "by the row: {} {}", s.line_y, grip.y);
        assert_eq!(cursor(&mut h), egui::CursorIcon::Grabbing);
        assert_eq!(h.query_all_by_label("Under two").count(), 0, "its row is folded away");
        // and back
        button(&mut h, grip, false);
        frames(&mut h);
        assert!(h.state().section_drag_shown().is_none());
        assert_eq!(h.query_all_by_label("Under two").count(), 1, "the headings are back");
    }

    #[test]
    fn a_drop_on_its_own_place_does_nothing_and_makes_no_undo_step() {
        let p = plain(FOUR);
        let mut h = opened(&p);
        let steps = h.state_mut().viewer().unwrap().ed.session.history().undo_len();
        let grip = pick_up(&mut h, "Two");
        // a little either way is still its own place
        walk(&mut h, grip + vec2(0.0, 6.0));
        assert!(!shown(&h).moves);
        button(&mut h, grip + vec2(0.0, 6.0), false);
        frames(&mut h);
        assert_eq!(text(&h), FOUR);
        assert_eq!(h.state_mut().viewer().unwrap().ed.session.history().undo_len(), steps);
    }

    #[test]
    fn a_drop_elsewhere_moves_it_exactly_as_one_undo_step() {
        let p = plain(FOUR);
        let mut h = opened(&p);
        let grip = pick_up(&mut h, "Four");
        let row_h = 26.0;
        let to = grip - vec2(0.0, row_h * 3.2);
        walk(&mut h, to);
        walk(&mut h, to);
        let s = shown(&h);
        assert!(s.moves);
        assert_eq!(s.to, 0);
        button(&mut h, to, false);
        frames(&mut h);
        assert_eq!(text(&h), LAST_FIRST);
        h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
        frames(&mut h);
        assert_eq!(text(&h), FOUR);
    }

    #[test]
    fn esc_cancels_and_the_release_does_nothing() {
        let p = plain(FOUR);
        let mut h = opened(&p);
        let grip = pick_up(&mut h, "Four");
        let to = grip - vec2(0.0, 80.0);
        walk(&mut h, to);
        assert!(shown(&h).moves);
        h.key_press(Key::Escape);
        h.step();
        h.step();
        assert!(h.state().section_drag_shown().is_none());
        button(&mut h, to, false);
        frames(&mut h);
        assert_eq!(text(&h), FOUR);
    }

    #[test]
    fn an_edit_or_another_document_during_it_makes_the_drop_do_nothing() {
        let p = plain(FOUR);
        let mut h = opened(&p);
        let grip = pick_up(&mut h, "Four");
        let to = grip - vec2(0.0, 80.0);
        walk(&mut h, to);
        assert!(shown(&h).moves);
        let v = h.state_mut().viewer().unwrap();
        v.ed.set_selection(markise_doc::Selection::caret_at(markise_doc::DocPos::new(FOUR.len())));
        let _ = v.ed.edit(|s| s.insert_text("x"));
        h.step();
        h.step();
        assert!(h.state().section_drag_shown().is_none());
        let edited = text(&h);
        button(&mut h, to, false);
        frames(&mut h);
        assert_eq!(text(&h), edited);

        let p = plain(FOUR);
        let other = plain("---\nauthoring: blocks-v3\n---\n\nA\n\n---\n\nB\n\n---\n\nC\n\n---\n\nD\n");
        let mut h = opened(&p);
        let grip = pick_up(&mut h, "Four");
        let to = grip - vec2(0.0, 80.0);
        walk(&mut h, to);
        h.state_mut().open(other.doc.clone());
        let want = other.doc.clone();
        common::wait_until(&mut h, "the other document", move |a| {
            a.document_path() == Some(want.as_path()) && !a.is_loading()
        });
        h.step();
        h.step();
        assert!(h.state().section_drag_shown().is_none());
        let before = text(&h);
        button(&mut h, to, false);
        frames(&mut h);
        assert_eq!(text(&h), before);
    }

    #[test]
    fn a_reload_during_it_ends_it_and_the_drop_does_nothing() {
        const FIVE: &str = "---\nauthoring: blocks-v3\n---\n\n## One\n\n---\n\n## New\n\n---\n\n## Two\n\n---\n\n## Three\n\n---\n\n## Four\n";
        let p = plain(FOUR);
        let mut h = opened(&p);
        let grip = pick_up(&mut h, "Four");
        let to = grip - vec2(0.0, 80.0);
        walk(&mut h, to);
        assert!(shown(&h).moves);
        reload(&mut h, FIVE);
        h.step();
        h.step();
        assert!(h.state().section_drag_shown().is_none(), "it is over");
        button(&mut h, to, false);
        frames(&mut h);
        assert_eq!(text(&h), FIVE);
    }

    #[test]
    fn keys_edit_nothing_while_it_lasts_and_go_on_after_it() {
        let p = plain(FOUR);
        let mut h = opened(&p);
        // the caret in the document, which keeps the focus
        let at = FOUR.find("Three").unwrap() + 2;
        let v = h.state_mut().viewer().unwrap();
        v.ed.set_selection(markise_doc::Selection::caret_at(markise_doc::DocPos::new(at)));
        v.ed.refresh_focus();
        v.request_refocus();
        frames(&mut h);
        let grip = pick_up(&mut h, "Two");
        h.key_press(Key::Backspace);
        h.step();
        h.event(Event::Text("x".to_string()));
        h.step();
        h.step();
        assert_eq!(text(&h), FOUR);
        assert!(h.state().section_drag_shown().is_some(), "it goes on");
        button(&mut h, grip, false);
        frames(&mut h);
        assert_eq!(text(&h), FOUR);
    }

    #[test]
    fn esc_and_the_release_in_one_frame_is_a_cancelled_drag() {
        let p = plain(FOUR);
        let mut h = opened(&p);
        let grip = pick_up(&mut h, "Four");
        let to = grip - vec2(0.0, 80.0);
        walk(&mut h, to);
        assert!(shown(&h).moves);
        // in one frame: a queued event has a frame of its own
        h.input_mut().events.extend([
            Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
            Event::PointerButton {
                pos: to,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ]);
        h.step();
        frames(&mut h);
        assert_eq!(text(&h), FOUR);
    }

    #[test]
    fn the_list_scrolls_at_its_edges_and_a_section_gets_past_what_was_off_screen() {
        let mut doc = String::from(HEAD);
        for i in 0..80 {
            doc.push_str(&format!("## Part {i}\n\n---\n\n"));
        }
        doc.push_str("## End\n");
        let p = plain(&doc);
        let mut h = opened(&p);
        let grip = pick_up(&mut h, "Part 1");
        let start = shown(&h).offset;
        // to the panel's bottom edge, and held there
        let bottom = Pos2::new(grip.x, 800.0 - 40.0);
        let mut at_edge = bottom;
        for _ in 0..8 {
            walk(&mut h, at_edge);
            at_edge.y -= 0.0;
        }
        let s = shown(&h);
        assert!(s.offset > start + 50.0, "{start} {}", s.offset);
        assert!(s.to > 20, "{}", s.to);
        let to = s.to;
        button(&mut h, at_edge, false);
        frames(&mut h);
        let after = h.state_mut().viewer().unwrap().ed.session.doc().section_count();
        assert_eq!(after, 81);
        // the row went where the line was
        let t = text(&h);
        let at = t.find("## Part 1\n").unwrap();
        let v = h.state_mut().viewer().unwrap();
        let index = v.ed.session.doc().section_at(at).index;
        assert!(index + 2 >= to && index < to + 8, "{index} {to}");
    }

    #[test]
    fn a_refusal_on_the_way_says_so_and_a_title_going_along_is_said() {
        let doc = format!("{HEAD}One.\n\n---\n\n:::section-metadata\n\nid: m\n\n:::\n\n---\n\nThree.\n");
        let p = plain(&doc);
        let mut h = opened(&p);
        let grip = pick_up(&mut h, "Section 3");
        let to = grip - vec2(0.0, 26.0 * 2.2);
        walk(&mut h, to);
        walk(&mut h, to);
        button(&mut h, to, false);
        h.step();
        assert_eq!(text(&h), doc);
        assert!(
            h.state()
                .banner_text()
                .unwrap()
                .contains("fill it in or delete it first")
        );

        let doc = format!("{HEAD}# Title\n\n---\n\nTwo.\n\n---\n\nThree.\n");
        let p = plain(&doc);
        let mut h = opened(&p);
        let grip = pick_up(&mut h, "Section 3");
        let to = grip - vec2(0.0, 26.0 * 2.2);
        walk(&mut h, to);
        walk(&mut h, to);
        assert_eq!(shown(&h).to, 0);
        button(&mut h, to, false);
        h.step();
        assert_eq!(text(&h), format!("{HEAD}Three.\n\n---\n\n# Title\n\n---\n\nTwo.\n"));
        assert!(h.state().banner_text().unwrap().contains("title moves too"));
    }
}

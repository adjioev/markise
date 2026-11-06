//! Sections on the real app (ADR 0026): what turns them on, what shows, the switch between a plaque
//! and a card, and that the file stays as it was. No window.

use std::path::{Path, PathBuf};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_layout::BlockViewMode;
use markise_ui_egui::{AppOptions, ConfigDir, MarkiseApp, ViewMode};

use crate::block_views::{Fixture, caret, shown, text};
use crate::common::{self, wait_until};

type H = Harness<'static, MarkiseApp>;

const DOC: &str = "---\nauthoring: blocks-v3\n---\n\n:::section-metadata\n\nid: top\nkind: opening\nnot a pair\n\n:::\n\n# First\n\nOne.\n\n---\n\nTwo.\n\n***\n\nStill two.\n\n---\n\n:::section-metadata\n\nstyle: wide\n\n:::\n\nThree.\n";

/// A boundary's number stands on a row of its own in what shows.
fn badge(shown: &str, number: &str) -> bool {
    shown.lines().any(|l| l.trim() == number)
}

struct Plain {
    _dir: tempfile::TempDir,
    config: PathBuf,
    doc: PathBuf,
}

/// A document in a folder with no package.
fn plain(text: &str) -> Plain {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let (config, doc) = (root.join("config"), root.join("doc.md"));
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(&doc, text).unwrap();
    Plain { _dir: dir, config, doc }
}

fn opened(p: &Plain) -> H {
    open_at(&p.config, &p.doc)
}

fn open_at(config: &Path, doc: &Path) -> H {
    let opts = AppOptions {
        config: ConfigDir::Dir(config.to_path_buf()),
        ..common::no_watch()
    };
    let mut h = Harness::builder()
        .with_size(egui::vec2(1100.0, 1000.0))
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), MarkiseApp::new(opts));
    h.state_mut().open(doc.to_path_buf());
    let want = doc.to_path_buf();
    wait_until(&mut h, "document load", move |a| {
        a.document_path() == Some(want.as_path()) && !a.is_loading()
    });
    common::run(&mut h);
    h
}

fn at(needle: &str) -> usize {
    DOC.find(needle).unwrap()
}

fn focus(h: &mut H) -> usize {
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

#[test]
fn a_document_in_the_dialect_shows_dividers_and_plaques_and_stays_as_it_was() {
    let p = plain(DOC);
    let mut h = opened(&p);
    let s = shown(&mut h);
    assert!(badge(&s, "2"), "{s}");
    assert!(s.contains("wide"), "{s}");
    assert!(s.contains("id: top   kind: opening"), "{s}");
    assert!(s.contains("⚠"), "{s}");
    let v = h.state_mut().viewer().unwrap();
    assert_eq!(v.section_dialect().unwrap().origin(), "the document's front matter");
    assert!(v.ed.view.sections_on());
    assert_eq!(text(&h), DOC);
    assert_eq!(std::fs::read_to_string(&p.doc).unwrap(), DOC);
}

#[test]
fn no_dialect_or_another_one_leaves_the_document_as_ever() {
    for head in ["", "---\nauthoring: blocks-v2\n---\n\n"] {
        let body = DOC.split_once("---\n\n").unwrap().1;
        let doc = format!("{head}{body}");
        let p = plain(&doc);
        let mut h = opened(&p);
        let s = shown(&mut h);
        assert!(s.contains("section-metadata"), "a card, {s}");
        assert!(!badge(&s, "2"), "{s}");
        assert!(!h.state_mut().viewer().unwrap().ed.view.sections_on());
    }
}

const FILES_ON: &[(&str, &str)] = &[
    (
        "manifest.json",
        r#"{"format":"markise-block-package/2","library":"sample","version":"1.0.0","blocks":["tip"],"authoring":"blocks-v3"}"#,
    ),
    (
        "components/tip/tip.json",
        r#"{"label":"Tip","fields":[{"name":"body","type":"text"}]}"#,
    ),
    ("editor/palette.json", r#"{"format":1}"#),
];

#[test]
fn an_accepted_package_turns_them_on_and_front_matter_wins_over_it() {
    let body = "One.\n\n---\n\nTwo.\n";
    let f = Fixture::with_package(body, FILES_ON);
    let mut h = f.opened();
    assert!(
        !h.state_mut().viewer().unwrap().ed.view.sections_on(),
        "not before it is accepted"
    );
    h.get_by_label("Use Package").click();
    h.step();
    wait_until(&mut h, "the install", |a| a.block_package().is_some());
    common::run(&mut h);
    let v = h.state_mut().viewer().unwrap();
    assert!(v.ed.view.sections_on());
    assert_eq!(v.section_dialect().unwrap().origin(), "package sample 1.0.0");
    assert!(badge(&shown(&mut h), "2"));

    let doc = format!("---\nauthoring: blocks-v2\n---\n\n{body}");
    let f = Fixture::with_package(&doc, FILES_ON);
    let mut h = f.opened();
    h.get_by_label("Use Package").click();
    h.step();
    wait_until(&mut h, "the install", |a| a.block_package().is_some());
    common::run(&mut h);
    assert!(!h.state_mut().viewer().unwrap().ed.view.sections_on());
    assert!(!badge(&shown(&mut h), "2"));
}

#[test]
fn the_switch_opens_a_plaque_as_a_card_and_back() {
    let p = plain(DOC);
    let mut h = opened(&p);
    assert!(!shown(&mut h).contains("section-metadata"));
    h.get_all_by_label("Show Markdown: section metadata")
        .next()
        .unwrap()
        .click();
    common::run(&mut h);
    let s = shown(&mut h);
    assert!(s.contains("section-metadata"), "the first one opened, {s}");
    assert!(s.contains("wide"), "the other stays a plaque, {s}");
    assert_eq!(text(&h), DOC);
    // its text can be edited there
    let x = at("opening");
    caret(&mut h, x);
    h.event(egui::Event::Text("re".to_string()));
    common::run(&mut h);
    assert!(text(&h).contains("kind: reopening"), "{}", text(&h));
    h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
    common::run(&mut h);
    assert_eq!(text(&h), DOC);
    h.get_by_label("Show Native View: section metadata").click();
    common::run(&mut h);
    assert!(!shown(&mut h).contains("section-metadata"), "a plaque again");
    assert_eq!(text(&h), DOC, "the switch, not the delete button beside it");
    let now = focus(&mut h);
    assert!(
        !(at(":::section-metadata") + 1..at("\n\n# First")).contains(&now),
        "{now}"
    );
}

#[test]
fn the_switch_works_with_the_document_in_another_state() {
    let p = plain(DOC);
    let mut h = opened(&p);
    let v = h.state_mut().viewer().unwrap();
    v.ed.restore_block_view_state(BlockViewMode::Site, &[]);
    common::run(&mut h);
    assert!(
        !shown(&mut h).contains("section-metadata"),
        "a plaque in that state too"
    );
    h.get_all_by_label("Show Markdown: section metadata")
        .next()
        .unwrap()
        .click();
    common::run(&mut h);
    assert!(shown(&mut h).contains("section-metadata"), "it opens");
    h.get_by_label("Show Native View: section metadata").click();
    common::run(&mut h);
    assert!(!shown(&mut h).contains("section-metadata"), "and closes");
    assert_eq!(text(&h), DOC);
}

#[test]
fn a_plaque_says_what_is_wrong_when_its_lines_are_tight_or_more_than_one_paragraph() {
    for doc in [
        "---\nauthoring: blocks-v3\n---\n\n:::section-metadata\nid: a\n:::\n\nText.\n",
        "---\nauthoring: blocks-v3\n---\n\n:::section-metadata\n\nid: a\n\nkind: b\n\n:::\n\nText.\n",
        "---\nauthoring: blocks-v3\n---\n\n:::section-metadata\n\n- id: a\n\n:::\n\nText.\n",
    ] {
        let p = plain(doc);
        let mut h = opened(&p);
        let s = shown(&mut h);
        assert!(s.contains("⚠ Malformed"), "{s}");
        assert_eq!(text(&h), doc);
    }
}

#[test]
fn horizontal_line_writes_stars_on_a_page_with_sections_and_dashes_elsewhere() {
    for (head, rule) in [("---\nauthoring: blocks-v3\n---\n\n", "***"), ("", "---")] {
        let doc = format!("{head}One.\n");
        let p = plain(&doc);
        let mut h = opened(&p);
        caret(&mut h, doc.find("One").unwrap());
        let ctx = h.ctx.clone();
        (h.state_mut().viewer().unwrap())
            .run(&ctx, None, "block.insertRule", &markise_doc::Params::new())
            .unwrap();
        common::run(&mut h);
        assert!(
            text(&h).ends_with(&format!("One.\n\n{rule}\n")) || text(&h).contains(&format!("\n\n{rule}")),
            "{}",
            text(&h)
        );
        if rule == "***" {
            let count = h.state_mut().viewer().unwrap().ed.session.doc().section_count();
            assert_eq!(count, 1, "no section started");
        }
    }
}

#[test]
fn typed_dashes_and_enter_make_a_boundary_and_backspace_after_it_takes_it() {
    let doc = "---\nauthoring: blocks-v3\n---\n\nOne.\n\nTwo.\n";
    let p = plain(doc);
    let mut h = opened(&p);
    // a new line between them, then the three dashes and Enter
    caret(&mut h, doc.find("One.").unwrap() + 4);
    h.key_press(egui::Key::Enter);
    common::run(&mut h);
    h.key_press(egui::Key::Enter);
    common::run(&mut h);
    h.event(egui::Event::Text("---".to_string()));
    common::run(&mut h);
    let count = |h: &mut H| h.state_mut().viewer().unwrap().ed.session.doc().section_count();
    assert_eq!(count(&mut h), 1, "literal until Enter");
    h.key_press(egui::Key::Enter);
    common::run(&mut h);
    assert_eq!(count(&mut h), 2, "{:?}", text(&h));
    assert!(text(&h).contains("\n---\n"), "{:?}", text(&h));
    // backspace at the start of what follows takes the rule, and undo brings it back
    let two = text(&h).find("Two.").unwrap();
    caret(&mut h, two);
    h.key_press(egui::Key::Backspace);
    common::run(&mut h);
    assert_eq!(text(&h), doc, "the sections are one again");
    assert_eq!(count(&mut h), 1);
    h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
    common::run(&mut h);
    assert_eq!(count(&mut h), 2, "undo brings the boundary back");
}

#[test]
fn keys_go_past_a_plaque_and_a_divider() {
    let p = plain(DOC);
    let mut h = opened(&p);
    let inside = at("id: top")..at("# First");
    caret(&mut h, at("First"));
    for _ in 0..4 {
        h.key_press(egui::Key::ArrowUp);
        common::run(&mut h);
        let now = focus(&mut h);
        assert!(!inside.contains(&now), "{now}");
    }
    // down over the divider, from the end of one section to the next
    let one = at("One.");
    caret(&mut h, one);
    h.key_press(egui::Key::ArrowDown);
    common::run(&mut h);
    let now = focus(&mut h);
    assert!(now >= at("Two.") || now <= one + 4, "{now}");
    assert_eq!(text(&h), DOC);
}

#[test]
fn a_selection_in_a_plaque_shows_the_card_and_leaving_shows_the_plaque() {
    let p = plain(DOC);
    let mut h = opened(&p);
    caret(&mut h, at("kind: opening"));
    assert!(shown(&mut h).contains("section-metadata"), "what is found can be seen");
    caret(&mut h, at("One."));
    assert!(!shown(&mut h).contains("section-metadata"));
    assert_eq!(text(&h), DOC);
}

#[test]
fn source_mode_shows_the_text() {
    let p = plain(DOC);
    let mut h = opened(&p);
    h.state_mut().set_view_mode(ViewMode::Source);
    common::run(&mut h);
    let source = h.state_mut().viewer().unwrap().ed.source.is_some();
    assert!(source);
    assert_eq!(text(&h), DOC);
    h.state_mut().set_view_mode(ViewMode::Rendered);
    common::run(&mut h);
    assert!(badge(&shown(&mut h), "2"));
}

#[test]
fn the_example_document_opens_as_written() {
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sections/sample.md");
    let bytes = std::fs::read_to_string(&sample).unwrap();
    let p = plain(&bytes);
    let mut h = opened(&p);
    let s = shown(&mut h);
    assert!(s.contains("zones"), "{s}");
    assert_eq!(text(&h), bytes);
    assert_eq!(std::fs::read_to_string(&p.doc).unwrap(), bytes);
}

#[test]
fn an_empty_or_non_string_authoring_is_said_quietly_and_never_goes_to_the_package() {
    for value in ["", " 3", " [blocks-v3]"] {
        let doc = format!("---\nauthoring:{value}\n---\n\nOne.\n\n---\n\nTwo.\n");
        let f = Fixture::with_package(&doc, FILES_ON);
        let mut h = f.opened();
        h.get_by_label("Use Package").click();
        h.step();
        wait_until(&mut h, "the install", |a| a.block_package().is_some());
        common::run(&mut h);
        let v = h.state_mut().viewer().unwrap();
        assert!(!v.ed.view.sections_on(), "{value:?}");
        let (name, about) = v.section_status(false).unwrap();
        assert_eq!(name, "§ off");
        assert!(about.contains("not used"), "{about}");
    }
}

#[test]
fn a_section_goes_by_its_first_heading_in_the_outline_and_status() {
    let doc =
        "---\nauthoring: blocks-v3\n---\n\n# Top\n\nOne.\n\n---\n\n## Second part {#x}\n\nTwo.\n\n---\n\nThree.\n";
    let p = plain(doc);
    let mut h = opened(&p);
    assert!(h.query_all_by_label("Second part").count() >= 1);
    assert!(h.query_by_label("Section 3").is_some());
    let (name, _) = h.state_mut().viewer().unwrap().section_status(false).unwrap();
    assert_eq!(name, "§ Top");
}

const READ_DOC: &str = "---\nauthoring: blocks-v3\n---\n\n:::section-metadata\n\nid: top\nnot a pair\n\n:::\n\n# First\n\nOne.\n\n---\n\n:::question\n\n## Hidden prompt\n\n- [x] Yes\n- [ ] No\n\n:::\n\nTwo.\n\n---\n\n## Third\n\nThree.\n";

#[test]
fn in_read_dividers_and_plaques_stay_and_no_control_does() {
    let f = Fixture::with_package(READ_DOC, crate::practice::FILES);
    let mut h = f.ready();
    // in Edit the switch is there for a block under the pointer or the caret
    caret(&mut h, READ_DOC.find("id: top").unwrap());
    assert!(h.query_by_label("Show Markdown: section metadata").is_some());
    caret(&mut h, READ_DOC.find("One.").unwrap());
    h.state_mut().viewer().unwrap().ed.set_read_mode(true);
    common::run(&mut h);
    let s = shown(&mut h);
    assert!(s.contains("id: top"), "the plaque reads, {s}");
    assert!(s.contains("⚠"), "with what is wrong, {s}");
    assert!(badge(&s, "2"), "the divider reads, {s}");
    for gone in [
        "Show Markdown: section metadata",
        "Show Native View: section metadata",
        "Delete Block: section-metadata",
    ] {
        assert!(h.query_by_label(gone).is_none(), "{gone}");
    }
    // the outline keeps sections, leaves out headings in blocks, and no name tells one
    assert!(h.query_by_label("top").is_some());
    assert!(
        h.query_by_label("Section 2").is_some(),
        "no name from a heading in a block"
    );
    assert_eq!(
        h.query_all_by_label("Third").count(),
        2,
        "a section's name and its heading"
    );
    assert!(h.query_by_label("Hidden prompt").is_none());
    let v = h.state_mut().viewer().unwrap();
    assert!(!v.section_status(false).unwrap().0.contains("Hidden"));
    assert_eq!(text(&h), READ_DOC);
    // back in Edit it lists them
    h.state_mut().viewer().unwrap().ed.set_read_mode(false);
    common::run(&mut h);
    assert!(h.query_by_label("Hidden prompt").is_some());
}

#[test]
fn a_block_opened_as_a_card_is_a_plaque_in_read_and_a_card_again_after() {
    let p = plain(DOC);
    let mut h = opened(&p);
    h.get_all_by_label("Show Markdown: section metadata")
        .next()
        .unwrap()
        .click();
    common::run(&mut h);
    assert!(shown(&mut h).contains("section-metadata"));
    h.state_mut().viewer().unwrap().ed.set_read_mode(true);
    common::run(&mut h);
    assert!(!shown(&mut h).contains("section-metadata"), "Read has no card for it");
    h.state_mut().viewer().unwrap().ed.set_read_mode(false);
    common::run(&mut h);
    assert!(shown(&mut h).contains("section-metadata"));
    assert_eq!(text(&h), DOC);
}

#[test]
fn the_outline_has_sections_above_headings_and_a_click_goes_there() {
    let mut doc = String::from("---\nauthoring: blocks-v3\n---\n\n# Top\n\n");
    for i in 0..40 {
        doc.push_str(&format!(
            "---\n\n:::section-metadata\n\nid: s{i}\n\n:::\n\n## Part {i}\n\nText.\n\nMore text.\n\n"
        ));
    }
    let p = plain(&doc);
    let mut h = opened(&p);
    h.get_by_label("s10").click();
    common::run(&mut h);
    let s = shown(&mut h);
    assert!(s.contains("Part 10"), "{s}");
    assert_eq!(text(&h), doc);
    // the status bar follows, by what is on screen
    let v = h.state_mut().viewer().unwrap();
    let (name, about) = v.section_status(false).unwrap();
    assert!(name.starts_with("§ s"), "{name}");
    assert!(about.contains("front matter"), "{about}");
}

#[test]
fn a_first_part_says_more_is_to_come_and_the_whole_document_completes_it() {
    use std::sync::{Arc, Condvar, Mutex};

    use markise_engine::load::{ProgressiveOpts, Stage};

    let mut doc = String::from("---\nauthoring: blocks-v3\n---\n\n# Top\n\n");
    for i in 0..600 {
        doc.push_str(&format!(
            "---\n\n:::section-metadata\n\nid: s{i}\n\n:::\n\n## Part {i}\n\nSome text.\n\n"
        ));
    }
    let p = plain(&doc);
    let gate = Arc::new((Mutex::new(true), Condvar::new()));
    let held = gate.clone();
    let mut h: H = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_ui_state(
        |ui, app: &mut MarkiseApp| app.ui(ui),
        MarkiseApp::new(AppOptions {
            open: vec![p.doc.clone()],
            config: ConfigDir::Dir(p.config.clone()),
            progressive: Some(ProgressiveOpts {
                min_bytes: 16 << 10,
                prefix_bytes: 4 << 10,
            }),
            load_hook: Some(Arc::new(move |st, _| {
                if st == Stage::Parse && std::thread::current().name() == Some("markise-load-rest") {
                    let (m, cv) = &*held;
                    let mut on = m.lock().unwrap();
                    while *on {
                        on = cv.wait(on).unwrap();
                    }
                }
            })),
            ..common::no_watch()
        }),
    );
    wait_until(&mut h, "the first part", |a| a.is_partial());
    h.step();
    let v = h.state_mut().viewer().unwrap();
    assert!(v.ed.view.sections_on(), "the front matter is in the part");
    let part = v.ed.session.doc().section_count();
    assert!(part < 601);
    // on to the end of what there is: the count is not known yet
    let item = v.ed.view.ensure_section(part - 1).unwrap();
    v.ed.view.scroll_to_item(item);
    let at = v.ed.session.doc().text().len();
    v.ed.set_caret(markise_doc::DocPos::new(at), false);
    h.step();
    h.step();
    let (_, about) = h.state_mut().viewer().unwrap().section_status(true).unwrap();
    assert!(about.contains(&format!("of {part}+")), "{about}");
    {
        let (m, cv) = &*gate;
        *m.lock().unwrap() = false;
        cv.notify_all();
    }
    wait_until(&mut h, "the whole document", |a| !a.is_partial());
    common::run(&mut h);
    let v = h.state_mut().viewer().unwrap();
    assert_eq!(v.ed.session.doc().section_count(), 601);
    let (name, _) = v.section_status(false).unwrap();
    assert!(!name.ends_with('…'), "{name}");
    assert_eq!(text(&h), doc);
}

/// A new section from the library (ADR 0026).
mod inserting {
    use egui::{Event, Key};
    use markise_doc::{DocPos, Selection};
    use markise_ui_egui::menu::MenuAction;

    use super::*;
    use crate::block_library::frames;

    const WORDS: &str = r#"{"format":1,"label":{"en":"Part","fr":"Partie"},"description":"A part of the page","keywords":"chapter",
        "fields":[{"key":"style","label":"Style","values":[{"value":"wide","label":"Wide"},{"value":"tall"}]},{"key":"kind"}]}"#;

    fn type_text(h: &mut H, t: &str) {
        h.event(Event::Text(t.to_string()));
        frames(h);
    }

    fn press(h: &mut H, key: Key) {
        h.key_press(key);
        frames(h);
    }

    /// The caret on the empty line after what ends at `end`.
    fn free_line(h: &mut H, end: usize) {
        let v = h.state_mut().viewer().unwrap();
        v.ed.set_selection(Selection::caret_at(DocPos::with_virt(end, 1)));
        v.ed.refresh_focus();
        v.request_refocus();
        common::run(h);
    }

    fn with_words(body: &str) -> Fixture {
        let mut files = FILES_ON.to_vec();
        files.push(("editor/sections.json", WORDS));
        Fixture::with_package(body, &files)
    }

    #[test]
    fn with_no_package_the_item_says_section_and_save_inserts_one_undo_step() {
        let doc = "---\nauthoring: blocks-v3\n---\n\nOne.\n\nTwo.\n";
        let p = plain(doc);
        let mut h = opened(&p);
        free_line(&mut h, doc.find("\n\nTwo").unwrap());
        type_text(&mut h, "/");
        assert_eq!(h.state().block_library().unwrap().titles(), ["Section"]);
        press(&mut h, Key::Enter);
        assert!(h.state().block_library().is_none());
        assert_eq!(h.state().section_form().unwrap().heading(), "Section");
        assert!(h.state().section_form().unwrap().keys().is_empty());
        assert_eq!(text(&h), doc, "nothing is written before Save");
        type_text(&mut h, "New one");
        assert!(h.state().section_form().unwrap().can_save());
        press(&mut h, Key::Enter);
        assert!(h.state().section_form().is_none());
        let t = text(&h);
        assert!(t.contains("One.\n\n---\n\n## New one {#"), "{t}");
        assert!(t.ends_with("}\n\nTwo.\n"), "{t}");
        assert!(shown(&mut h).contains("New one"));
        h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
        common::run(&mut h);
        assert_eq!(text(&h), doc);
    }

    #[test]
    fn cancel_inserts_nothing_and_empty_cannot_be_saved() {
        let doc = "---\nauthoring: blocks-v3\n---\n\nOne.\n";
        let p = plain(doc);
        let mut h = opened(&p);
        free_line(&mut h, doc.len() - 1);
        assert!(
            h.state()
                .menu_model()
                .item("block.insertFromLibrary")
                .is_some_and(|i| i.enabled)
        );
        type_text(&mut h, "/");
        press(&mut h, Key::Enter);
        press(&mut h, Key::Enter);
        assert!(h.state().section_form().is_some(), "no title: Return saves nothing");
        press(&mut h, Key::Escape);
        assert!(h.state().section_form().is_none());
        assert_eq!(text(&h), doc);
        assert!(!h.state().is_dirty());
    }

    #[test]
    fn the_package_gives_its_word_and_fields_and_metadata_goes_first() {
        let doc = "One.\n\nTwo.\n";
        let f = with_words(doc);
        let mut h = f.ready();
        free_line(&mut h, doc.len() - 1);
        type_text(&mut h, "/");
        let open = h.state().block_library().unwrap();
        assert_eq!(open.titles(), ["Part"], "first, in the package's word");
        type_text(&mut h, "chapter");
        assert_eq!(h.state().block_library().unwrap().titles(), ["Part"], "by its keywords");
        press(&mut h, Key::Enter);
        let form = h.state().section_form().unwrap();
        assert_eq!(form.heading(), "Part");
        assert_eq!(form.keys(), ["style", "kind"]);
        h.state_mut().set_section_form_title("Title");
        h.state_mut().set_section_form_value("style", "wide");
        common::run(&mut h);
        let ctx = h.ctx.clone();
        assert!(h.state_mut().save_section_form(&ctx));
        common::run(&mut h);
        let t = text(&h);
        assert!(
            t.starts_with("One.\n\nTwo.\n\n---\n\n:::section-metadata\n\nstyle: wide\n\n:::\n\n## Title {#"),
            "{t}"
        );
    }

    #[test]
    fn the_command_in_source_mode_goes_by_the_pane_s_caret() {
        let doc = "---\nauthoring: blocks-v3\n---\n\nOne.\n\nTwo.\n";
        let p = plain(doc);
        let mut h = opened(&p);
        free_line(&mut h, doc.find("\n\nTwo").unwrap());
        h.state_mut().set_view_mode(ViewMode::Source);
        frames(&mut h);
        assert!(h.state().source_caret().is_some());
        let ctx = h.ctx.clone();
        assert!(
            h.state_mut()
                .menu_action(&ctx, &MenuAction::Command("block.insertFromLibrary"))
        );
        frames(&mut h);
        assert_eq!(h.state().block_library().unwrap().titles(), ["Section"]);
        press(&mut h, Key::Enter);
        type_text(&mut h, "Mid");
        press(&mut h, Key::Enter);
        let t = text(&h);
        assert!(t.contains("One.\n\n---\n\n## Mid {#"), "{t}");
        assert!(t.ends_with("}\n\nTwo.\n"), "{t}");
    }

    #[test]
    fn the_id_follows_the_title_is_free_on_the_page_and_can_be_edited() {
        let doc =
            "---\nauthoring: blocks-v3\n---\n\n:::section-metadata\n\nid: taken-too\n\n:::\n\n## Taken {#taken}\n";
        let p = plain(doc);
        let mut h = opened(&p);
        free_line(&mut h, doc.len() - 1);
        type_text(&mut h, "/");
        press(&mut h, Key::Enter);
        assert!(!h.state().section_form().unwrap().can_save());
        // Latin, by the table, and free
        h.state_mut().set_section_form_title("Как действовать?");
        assert_eq!(h.state().section_form().unwrap().id(), "kak-deystvovat");
        h.state_mut().set_section_form_title("Taken");
        assert_eq!(h.state().section_form().unwrap().id(), "taken-2");
        h.state_mut().set_section_form_title("Taken too");
        assert_eq!(h.state().section_form().unwrap().id(), "taken-too-2");
        h.state_mut().set_section_form_title("123");
        assert_eq!(h.state().section_form().unwrap().id(), "section-1");
        frames(&mut h);
        // edited, it stops following; what is no id or is taken cannot be saved
        for bad in ["1a", "a b", "я", "taken", "taken-too"] {
            h.state_mut().set_section_form_id(bad);
            assert!(!h.state().section_form().unwrap().can_save(), "{bad}");
            frames(&mut h);
        }
        h.state_mut().set_section_form_id("my_Id-1");
        h.state_mut().set_section_form_title("Later");
        assert_eq!(h.state().section_form().unwrap().id(), "my_Id-1");
        let ctx = h.ctx.clone();
        assert!(h.state_mut().save_section_form(&ctx));
        frames(&mut h);
        assert!(text(&h).ends_with("\n\n---\n\n## Later {#my_Id-1}\n"), "{}", text(&h));
    }

    #[test]
    fn metadata_values_stay_literal_and_free_text_keeps_to_the_limits() {
        const SPACED: &str = r#"{"fields":[{"key":"style","values":[{"value":"a  b"}]},{"key":"kind"}]}"#;
        let mut files = FILES_ON.to_vec();
        files.push(("editor/sections.json", SPACED));
        let doc = "One.\n\nTwo.\n";
        let f = Fixture::with_package(doc, &files);
        let mut h = f.ready();
        free_line(&mut h, doc.len() - 1);
        type_text(&mut h, "/");
        press(&mut h, Key::Enter);
        h.state_mut().set_section_form_title("Title");
        h.state_mut().set_section_form_value("style", "a  b");
        h.state_mut().set_section_form_value("kind", &"x".repeat(201));
        assert!(!h.state().section_form().unwrap().can_save(), "201 characters");
        frames(&mut h);
        let ctx = h.ctx.clone();
        assert!(!h.state_mut().save_section_form(&ctx));
        assert!(h.state().section_form().is_some());
        h.state_mut().set_section_form_value("kind", &"я".repeat(200));
        assert!(h.state().section_form().unwrap().can_save(), "characters, not bytes");
        h.state_mut().set_section_form_value("kind", "  two  words ");
        assert!(h.state_mut().save_section_form(&ctx));
        frames(&mut h);
        let t = text(&h);
        assert!(t.contains("style: a  b\nkind: two  words\n"), "{t}");
    }

    #[test]
    fn nineteen_fields_stay_on_a_small_window_with_save_in_reach() {
        let fields: Vec<String> = (0..19)
            .map(|i| format!(r#"{{"key":"k{i}","label":"Key {i}"}}"#))
            .collect();
        let many = format!(r#"{{"fields":[{}]}}"#, fields.join(","));
        let mut files = FILES_ON.to_vec();
        files.push(("editor/sections.json", many.as_str()));
        let doc = "One.\n\nTwo.\n";
        let f = Fixture::with_package(doc, &files);
        let mut h = f.ready();
        h.set_size(egui::vec2(800.0, 600.0));
        frames(&mut h);
        free_line(&mut h, doc.len() - 1);
        type_text(&mut h, "/");
        press(&mut h, Key::Enter);
        for name in ["Save", "Cancel"] {
            let r = h.get_by_label(name).rect();
            assert!(r.top() >= 0.0 && r.bottom() <= 600.0, "{name}: {r:?}");
        }
    }

    #[test]
    fn only_a_document_with_sections_shows_it() {
        for doc in ["One.\n", "---\nauthoring: blocks-v2\n---\n\nOne.\n"] {
            let p = plain(doc);
            let mut h = opened(&p);
            free_line(&mut h, doc.len() - 1);
            type_text(&mut h, "/");
            assert!(h.state().block_library().is_none(), "{doc:?}");
            assert!(
                text(&h).contains("One.\n\n/"),
                "a slash as ever: {doc:?}: {:?}",
                text(&h)
            );
            assert!(
                h.state().menu_model().item("block.insertFromLibrary").is_none(),
                "{doc:?}"
            );
        }
    }

    #[test]
    fn read_has_none_of_it() {
        let doc = "---\nauthoring: blocks-v3\n---\n\nOne.\n";
        let p = plain(doc);
        let mut h = opened(&p);
        h.state_mut().viewer().unwrap().ed.set_read_mode(true);
        common::run(&mut h);
        free_line(&mut h, doc.len() - 1);
        type_text(&mut h, "/");
        assert!(h.state().block_library().is_none());
        assert!(
            !h.state()
                .menu_model()
                .item("block.insertFromLibrary")
                .is_some_and(|i| i.enabled)
        );
        assert_eq!(text(&h), doc);
    }

    #[test]
    fn a_broken_declaration_refuses_the_package_as_any_broken_file_does() {
        let mut files = FILES_ON.to_vec();
        files.push(("editor/sections.json", r#"{"fields":[{"key":"id"}]}"#));
        let f = Fixture::with_package("One.\n", &files);
        let h = f.opened();
        assert!(h.state().block_package().is_none());
    }
}

/// Moving and deleting a section (ADR 0026).
mod moving {
    use egui::Pos2;
    use markise_doc::{DocPos, Selection};
    use markise_ui_egui::menu::MenuAction;

    use super::*;
    use crate::block_library::frames;

    const DOC: &str = "---\nauthoring: blocks-v3\n---\n\nOne.\n\n---\n\nTwo.\n\n---\n\nThree.\n";
    const UP: &str = "---\nauthoring: blocks-v3\n---\n\nTwo.\n\n---\n\nOne.\n\n---\n\nThree.\n";

    fn caret_at(h: &mut H, needle: &str) {
        let at = text(h).find(needle).unwrap();
        let v = h.state_mut().viewer().unwrap();
        v.ed.set_selection(Selection::caret_at(DocPos::new(at)));
        v.ed.refresh_focus();
        v.request_refocus();
        common::run(h);
    }

    fn command(h: &mut H, id: &'static str) -> bool {
        let ctx = h.ctx.clone();
        let done = h.state_mut().menu_action(&ctx, &MenuAction::Command(id));
        frames(h);
        done
    }

    fn undo(h: &mut H) {
        h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
        frames(h);
    }

    fn pointer_to(h: &mut H, p: Pos2) {
        h.hover_at(p);
        frames(h);
    }

    #[test]
    fn the_commands_act_on_the_section_of_the_caret_one_undo_step_each() {
        let p = plain(DOC);
        let mut h = opened(&p);
        caret_at(&mut h, "Two");
        let menu = h.state().menu_model();
        for id in ["section.moveUp", "section.moveDown", "section.delete"] {
            assert!(menu.item(id).is_some_and(|i| i.enabled), "{id}");
        }
        assert!(command(&mut h, "section.moveUp"));
        assert_eq!(text(&h), UP);
        assert_eq!(h.state_mut().viewer().unwrap().ed.section_at_caret(), 0);
        undo(&mut h);
        assert_eq!(text(&h), DOC);
        caret_at(&mut h, "Three");
        assert!(command(&mut h, "section.delete"));
        assert_eq!(text(&h), "---\nauthoring: blocks-v3\n---\n\nOne.\n\n---\n\nTwo.\n");
        undo(&mut h);
        assert_eq!(text(&h), DOC);
    }

    #[test]
    fn the_keys_are_on_the_commands_and_at_the_ends_they_are_off() {
        let p = plain(DOC);
        let mut h = opened(&p);
        caret_at(&mut h, "One");
        let menu = h.state().menu_model();
        assert!(!menu.item("section.moveUp").unwrap().enabled);
        assert!(menu.item("section.moveDown").unwrap().enabled);
        let (m, k) = if cfg!(target_os = "macos") {
            (
                egui::Modifiers::MAC_CMD | egui::Modifiers::ALT | egui::Modifiers::SHIFT,
                egui::Key::ArrowDown,
            )
        } else {
            (
                egui::Modifiers::CTRL | egui::Modifiers::ALT | egui::Modifiers::SHIFT,
                egui::Key::ArrowDown,
            )
        };
        h.key_press_modifiers(m, k);
        frames(&mut h);
        assert_eq!(text(&h), UP);
    }

    #[test]
    fn no_dialect_has_no_commands_and_read_has_them_off() {
        let p = plain("One.\n\n---\n\nTwo.\n");
        let h = opened(&p);
        assert!(h.state().menu_model().item("section.moveUp").is_none());
        let p = plain(DOC);
        let mut h = opened(&p);
        caret_at(&mut h, "Two");
        h.state_mut().viewer().unwrap().ed.set_read_mode(true);
        common::run(&mut h);
        let menu = h.state().menu_model();
        for id in ["section.moveUp", "section.moveDown", "section.delete"] {
            assert!(!menu.item(id).is_some_and(|i| i.enabled), "{id}");
        }
        command(&mut h, "section.delete");
        assert_eq!(text(&h), DOC);
    }

    fn row_of_divider(h: &mut H, nth: usize) -> Pos2 {
        let doc = text(h);
        // past the front matter's two
        let at = doc.match_indices("---\n").map(|(i, _)| i).nth(nth + 2).unwrap();
        h.state().viewer_item_rect(at).expect("on screen").center()
    }

    fn click_at(h: &mut H, p: Pos2) {
        pointer_to(h, p);
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            // quickly: a long press is no click
            h.step();
        }
        frames(h);
    }

    #[test]
    fn a_divider_has_quiet_controls_under_the_pointer_and_none_in_read() {
        let p = plain(DOC);
        let mut h = opened(&p);
        caret_at(&mut h, "One");
        // the second divider opens the last section: it cannot go down
        let p2 = row_of_divider(&mut h, 1);
        pointer_to(&mut h, p2);
        let up = h.query_all_by_label("Move Section Up").count();
        assert_eq!(up, 2, "one on each divider");
        assert_eq!(h.query_all_by_label("Move Section Down").count(), 1);
        assert_eq!(h.query_all_by_label("Delete Section").count(), 2);
        h.state_mut().viewer().unwrap().ed.set_read_mode(true);
        frames(&mut h);
        assert_eq!(h.query_all_by_label("Move Section Up").count(), 0);
        assert_eq!(h.query_all_by_label("Delete Section").count(), 0);
    }

    #[test]
    fn pressing_a_control_moves_or_deletes_the_section_it_stands_on() {
        let p = plain(DOC);
        let mut h = opened(&p);
        caret_at(&mut h, "One");
        let row = row_of_divider(&mut h, 0);
        pointer_to(&mut h, row);
        // the first of the two dividers: "Two."
        let up: Vec<_> = h.query_all_by_label("Move Section Up").map(|n| n.rect()).collect();
        let first = up.iter().min_by(|a, b| a.top().total_cmp(&b.top())).copied().unwrap();
        click_at(&mut h, first.center());
        frames(&mut h);
        assert_eq!(text(&h), UP);
        undo(&mut h);
        let row = row_of_divider(&mut h, 0);
        pointer_to(&mut h, row);
        let bins: Vec<_> = h.query_all_by_label("Delete Section").map(|n| n.rect()).collect();
        let first = bins.iter().min_by(|a, b| a.top().total_cmp(&b.top())).copied().unwrap();
        click_at(&mut h, first.center());
        frames(&mut h);
        assert_eq!(text(&h), "---\nauthoring: blocks-v3\n---\n\nOne.\n\n---\n\nThree.\n");
    }
}

/// How a boundary looks, and a package's say in it (ADR 0026).
mod looks {
    use super::*;

    fn with_view(body: &str, view: &str) -> Fixture {
        let declared = format!(r#"{{"format":1,"view":{view}}}"#);
        let mut files: Vec<(&str, &str)> = FILES_ON.to_vec();
        files.push(("editor/sections.json", &declared));
        Fixture::with_package(body, &files)
    }

    #[test]
    fn a_frame_changes_what_shows_and_no_byte_and_read_has_no_control() {
        let doc = "One.\n\n---\n\n## Two\n\nText.\n\n---\n\nThree.\n";
        let f = with_view(doc, r#"{"frame":true,"accent":"blue"}"#);
        let mut h = f.ready();
        common::run(&mut h);
        let v = h.state_mut().viewer().unwrap();
        let inset = v.ed.view.section_frame_inset(&markise_theme::Theme::light());
        assert!(inset > 0.0);
        let s = shown(&mut h);
        assert!(badge(&s, "2") && badge(&s, "3"), "{s}");
        assert_eq!(text(&h), doc);
        assert_eq!(std::fs::read_to_string(&f.doc).unwrap(), doc);
        // the controls are as they are with no frame, and Read has none
        let at = doc.find("---").unwrap();
        let row = h.state().viewer_item_rect(at).unwrap().center();
        h.hover_at(row);
        common::run(&mut h);
        assert_eq!(h.query_all_by_label("Delete Section").count(), 2);
        h.state_mut().viewer().unwrap().ed.set_read_mode(true);
        common::run(&mut h);
        assert_eq!(h.query_all_by_label("Delete Section").count(), 0);
        assert!(badge(&shown(&mut h), "2"), "the same look");
        assert_eq!(text(&h), doc);
    }

    #[test]
    fn a_number_can_be_left_out() {
        let f = with_view("One.\n\n---\n\nTwo.\n", r#"{"numbered":false}"#);
        let mut h = f.ready();
        common::run(&mut h);
        assert!(!badge(&shown(&mut h), "2"));
    }

    #[test]
    fn a_boundary_says_the_first_heading_below_it_only_once() {
        let doc = "---\nauthoring: blocks-v3\n---\n\nOne.\n\n---\n\n## The heading\n\n---\n\n:::section-metadata\n\nid: named\n\n:::\n\n## Other\n";
        let p = plain(doc);
        let mut h = opened(&p);
        let s = shown(&mut h);
        assert_eq!(s.matches("The heading").count(), 1, "{s}");
        assert!(s.contains("named"), "an id is not what the heading says: {s}");
    }
}

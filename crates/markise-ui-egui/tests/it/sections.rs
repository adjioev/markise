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
    assert!(s.contains("Section 2"), "{s}");
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
        assert!(!s.contains("Section 2"), "{s}");
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
    assert!(shown(&mut h).contains("Section 2"));

    let doc = format!("---\nauthoring: blocks-v2\n---\n\n{body}");
    let f = Fixture::with_package(&doc, FILES_ON);
    let mut h = f.opened();
    h.get_by_label("Use Package").click();
    h.step();
    wait_until(&mut h, "the install", |a| a.block_package().is_some());
    common::run(&mut h);
    assert!(!h.state_mut().viewer().unwrap().ed.view.sections_on());
    assert!(!shown(&mut h).contains("Section 2"));
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
    assert!(shown(&mut h).contains("Section 2"));
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

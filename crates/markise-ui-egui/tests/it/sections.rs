//! Sections on the real app (ADR 0026): what turns them on, what shows, the switch between a plaque
//! and a card, and that the file stays as it was. No window.

use std::path::{Path, PathBuf};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
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
    assert!(s.contains("zones · reading"), "{s}");
    assert_eq!(text(&h), bytes);
    assert_eq!(std::fs::read_to_string(&p.doc).unwrap(), bytes);
}

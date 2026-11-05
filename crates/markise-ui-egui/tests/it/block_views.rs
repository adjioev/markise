//! Native views of blocks on the real app (ADR 0021), on a small neutral package: what shows, what
//! stays a card, and that the form and the file are as they were.

use std::fs;
use std::path::{Path, PathBuf};

use egui::Event;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_doc::Selection;
use markise_package::Sha256Hex;
use markise_ui_egui::{AppOptions, ConfigDir, MarkiseApp};

use crate::common::{self, settle, wait_until};

type H = Harness<'static, MarkiseApp>;

const TIP: &str = r#"{"label": "Tip", "fields": [
    {"name": "title", "type": "heading", "level": 3, "min": 0},
    {"name": "body", "type": "text", "min": 1}
]}"#;
const POLL: &str = r#"{"label": "Poll", "variants": {"": "one answer", "many": "several"},
    "fields": [
        {"name": "question", "type": "heading", "level": 2},
        {"name": "answers", "type": "choices", "ids": "required", "minItems": 2,
         "correct": {"": "exactly-one", "many": "at-least-one"}},
        {"name": "why", "type": "quote", "min": 0}
    ]}"#;
const STEPS: &str = r#"{"label": "Steps", "fields": [{"name": "items", "type": "list", "minItems": 1}]}"#;
const PLAIN: &str = r#"{"label": "Plain", "fields": [{"name": "body", "type": "text"}]}"#;
const VIEWS: &str = r#"{"tip": {"kind": "callout", "accent": "green", "icon": "bulb"},
    "poll": {"kind": "choices", "accent": "purple"},
    "steps": {"kind": "card", "accent": "blue", "icon": "no-such-file"}}"#;
const ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor"><path d="M19 12H5"/></svg>"#;

pub(crate) const DOC: &str = "# Doc\n\nBefore.\n\n:::tip\n\n### A title\n\nHello\n\n:::\n\n:::poll\n\n## Which?\n\n- [ ] One {#a1}\n- [x] Two {#a2}\n\n> Because.\n\n:::\n\n:::steps\n\n- First\n- Second\n\n:::\n\n:::plain\n\nNo view.\n\n:::\n\n:::poll\n\n## Outside\n\n- [x] Marked {#b1}\n- [x] Marked too {#b2}\n\n:::\n\n:::unknown\n\nNot in the package.\n\n:::\n";

pub(crate) struct Fixture {
    _dir: tempfile::TempDir,
    pub config: PathBuf,
    pub doc: PathBuf,
}

fn archive() -> Vec<u8> {
    let manifest = r#"{"format":"markise-block-package/2","library":"sample","version":"1.0.0","blocks":["tip","poll","steps","plain"]}"#;
    let files = [
        ("manifest.json", manifest),
        ("components/tip/tip.json", TIP),
        ("components/poll/poll.json", POLL),
        ("components/steps/steps.json", STEPS),
        ("components/plain/plain.json", PLAIN),
        ("editor/palette.json", r#"{"format": 1}"#),
        ("editor/views.json", VIEWS),
        ("assets/bulb.svg", ICON),
    ];
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    for (path, content) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, path, content.as_bytes()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

impl Fixture {
    pub(crate) fn new(text: &str) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let folder = root.join("site");
        let config = root.join("config");
        fs::create_dir_all(&folder).unwrap();
        fs::create_dir_all(&config).unwrap();
        let doc = folder.join("doc.md");
        fs::write(&doc, text).unwrap();
        let bytes = archive();
        let sha = Sha256Hex::of(&bytes);
        fs::write(folder.join("p.tar.gz"), &bytes).unwrap();
        fs::write(
            folder.join(markise_package::BINDING_FILE),
            format!(r#"{{"library":"sample","version":"1.0.0","source":"p.tar.gz","sha256":"{sha}"}}"#),
        )
        .unwrap();
        Fixture { _dir: dir, config, doc }
    }

    pub(crate) fn opened(&self) -> H {
        let opts = AppOptions {
            config: ConfigDir::Dir(self.config.clone()),
            ..common::no_watch()
        };
        let mut h = Harness::builder()
            .with_size(egui::vec2(1100.0, 1400.0))
            .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), MarkiseApp::new(opts));
        open(&mut h, &self.doc);
        h
    }

    /// Opened, and the package accepted.
    pub(crate) fn ready(&self) -> H {
        let mut h = self.opened();
        h.get_by_label("Use Package").click();
        h.step();
        wait_until(&mut h, "the install", |a| a.block_package().is_some());
        settle(&mut h);
        common::run(&mut h);
        h
    }
}

fn open(h: &mut H, doc: &Path) {
    h.state_mut().open(doc.to_path_buf());
    wait_until(h, "document load", |a| {
        a.document_path() == Some(doc) && !a.is_loading()
    });
    settle(h);
    common::run(h);
}

pub(crate) fn shown(h: &mut H) -> String {
    h.state_mut().viewer().unwrap().visible_text()
}

pub(crate) fn text(h: &H) -> String {
    h.state().document_text().unwrap()
}

pub(crate) fn caret(h: &mut H, at: usize) {
    let v = h.state_mut().viewer().unwrap();
    v.ed.set_selection(Selection::caret(at));
    v.ed.refresh_focus();
    v.request_refocus();
    common::run(h);
}

#[test]
fn a_package_not_yet_accepted_leaves_cards() {
    let f = Fixture::new(DOC);
    let mut h = f.opened();
    let s = shown(&mut h);
    assert!(s.contains("tip") && s.contains("poll"), "{s}");
    assert!(!s.contains("Tip"), "{s}");
}

#[test]
fn blocks_with_a_view_show_it_and_the_rest_stay_cards() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    let s = shown(&mut h);
    for label in ["Tip", "Poll", "Steps"] {
        assert!(s.contains(label), "{label} in {s}");
    }
    assert!(s.contains("plain"), "a schema and no view: a card: {s}");
    assert!(s.contains("unknown"), "not in the package: a card: {s}");
    let at = |needle: &str| DOC.find(needle).unwrap();
    let v = &mut h.state_mut().viewer().unwrap().ed.view;
    assert!(v.native_block(at("Hello")).is_some());
    assert!(v.native_block(at("One")).is_some());
    assert!(v.native_block(at("First")).is_some());
    assert!(v.native_block(at("No view")).is_none());
    assert!(v.native_block(at("Outside")).is_none(), "outside its schema: a card");
    assert!(v.native_block(at("Not in the package")).is_none());
    assert_eq!(text(&h), DOC);
    assert!(!h.state().is_dirty());
}

#[test]
fn the_caret_in_a_native_block_does_not_bring_the_id_back() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    caret(&mut h, DOC.find("One").unwrap() + 3);
    assert!(!shown(&mut h).contains("{#a1}"));
    // in the generic card next to it, as ever
    caret(&mut h, DOC.find("Marked").unwrap() + 6);
    assert!(shown(&mut h).contains("{#b1}"));
}

#[test]
fn the_form_works_on_a_native_block_and_undo_is_the_documents() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    h.get_all_by_label("Edit Block: tip").next().unwrap().click();
    common::run(&mut h);
    assert!(h.state().block_form().is_some());
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("!");
    common::run(&mut h);
    let ctx = h.ctx.clone();
    h.state_mut().block_form_save(&ctx);
    common::run(&mut h);
    assert!(text(&h).contains("Hello!"));
    let v = h.state_mut().viewer().unwrap();
    v.ed.edit(|s| s.undo());
    common::run(&mut h);
    assert_eq!(text(&h), DOC);
}

#[test]
fn typing_in_a_native_block_edits_its_field() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    caret(&mut h, DOC.find("Hello").unwrap() + 5);
    h.event(Event::Text("!".into()));
    common::run(&mut h);
    assert!(text(&h).contains("Hello!\n"), "{}", text(&h));
    // an IME's text is typed text, and a paste is one step
    h.event(Event::Ime(egui::ImeEvent::Commit("ю".into())));
    common::run(&mut h);
    assert!(text(&h).contains("Hello!ю\n"));
    h.event(Event::Paste("two\nlines".into()));
    common::run(&mut h);
    assert!(text(&h).contains("Hello!юtwo\nlines\n"), "{}", text(&h));
    let v = h.state_mut().viewer().unwrap();
    assert!(v.ed.edit(|s| s.undo()));
    common::run(&mut h);
    assert!(text(&h).contains("Hello!ю\n"));
    // an answer keeps its id, and one line: a line break in a paste is a blank there
    let one = text(&h).find("One").unwrap();
    caret(&mut h, one + 3);
    h.event(Event::Paste("a\nb".into()));
    common::run(&mut h);
    assert!(text(&h).contains("- [ ] Onea b {#a1}\n"), "{}", text(&h));
}

#[test]
fn a_click_on_a_native_answer_lands_in_its_text() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    let at = DOC.find("Two").unwrap();
    let r = h.state_mut().viewer().unwrap().item_screen_rect(at).expect("on screen");
    // well past the text: the end of the answer, before its id
    h.hover_at(egui::pos2(r.right() - 40.0, r.center().y));
    h.drag_at(egui::pos2(r.right() - 40.0, r.center().y));
    h.drop_at(egui::pos2(r.right() - 40.0, r.center().y));
    common::run(&mut h);
    h.event(Event::Text("!".into()));
    common::run(&mut h);
    assert!(text(&h).contains("- [x] Two! {#a2}\n"), "{}", text(&h));
}

#[test]
fn a_block_outside_its_schema_is_a_card_with_a_warning_and_can_be_saved() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    let s = shown(&mut h);
    assert!(s.contains("⚠ Answers: Mark exactly one"), "{s}");
    assert!(!s.contains("⚠ Tip"), "only that one: {s}");
    // for a screen reader, the reasons in full
    let label = "Outside its schema: Answers: Mark exactly one";
    assert!(h.query_by_label(label).is_some());
    assert_eq!(text(&h), DOC);
    assert!(!h.state().is_dirty());
    // and it edits as any card: the marks of ADR 0018 still work there
    caret(&mut h, DOC.find("Marked too").unwrap());
    h.event(Event::Text("x".into()));
    common::run(&mut h);
    assert!(text(&h).contains("xMarked too"));
}

fn icons_of(h: &mut H, needle: &str) -> Vec<String> {
    let at = DOC.find(needle).unwrap();
    let v = &mut h.state_mut().viewer().unwrap();
    let opener = v.ed.view.native_block(at).unwrap().range.start;
    let i = v.ed.view.model().item_at_offset(opener);
    let layout = v.ed.view.cache_peek(i).expect("on screen").clone();
    (layout.prims.iter())
        .filter_map(|p| match p {
            markise_layout::block::Prim::Icon { name, .. } => Some(name.to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn an_icon_comes_from_its_file_through_the_worker_and_no_file_means_no_icon() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    settle(&mut h);
    common::run(&mut h);
    assert_eq!(icons_of(&mut h, "Hello"), ["assets/bulb.svg"]);
    assert!(icons_of(&mut h, "First").is_empty(), "the file is not in the package");
    assert!(h.state().banner_text().is_none(), "and that is no error");
}

#[test]
fn typing_on_the_place_after_a_native_block_is_outside_it() {
    let doc = ":::tip\n\nBody.\n\n:::\n";
    let f = Fixture::new(doc);
    let mut h = f.ready();
    let v = h.state_mut().viewer().unwrap();
    let end = doc.rfind(":::").unwrap() + 3;
    assert!(v.ed.view.native_block(2).is_some());
    v.ed.set_selection(Selection::caret_at(markise_doc::DocPos::with_virt(end, 1)));
    v.ed.refresh_focus();
    v.request_refocus();
    common::run(&mut h);
    h.event(Event::Text("After".into()));
    common::run(&mut h);
    assert!(text(&h).starts_with(":::tip\n\nBody.\n\n:::\n"), "{}", text(&h));
    assert!(text(&h).contains("After"), "{}", text(&h));
}

#[test]
fn a_link_completed_in_a_field_is_an_edit_of_that_field() {
    let f = Fixture::new(DOC);
    let folder = f.doc.parent().unwrap().to_path_buf();
    fs::create_dir_all(folder.join(".obsidian")).unwrap();
    fs::write(folder.join(".obsidian/app.json"), "{}").unwrap();
    fs::write(folder.join("Other.md"), "# Other\n").unwrap();
    let mut h = f.opened();
    h.state_mut().open(folder.clone());
    h.get_by_label("Use Package").click();
    h.step();
    wait_until(&mut h, "the install", |a| a.block_package().is_some());
    settle(&mut h);
    common::run(&mut h);
    let end = DOC.find("Hello").unwrap() + 5;
    caret(&mut h, end);
    for t in [" ", "[", "[", "O", "t", "h"] {
        h.event(Event::Text(t.into()));
        h.step();
    }
    assert!(h.state().completion().is_some(), "{}", text(&h));
    h.state_mut().accept_completion(0);
    common::run(&mut h);
    let t = text(&h);
    assert!(t.contains("Hello [[Other]]\n"), "{t}");
    let v = h.state_mut().viewer().unwrap();
    assert_eq!(&t[v.ed.session.selection().primary().focus.offset..][..2], "]]");
    assert!(v.ed.view.native_block(end).is_some());
    v.ed.edit(|s| s.undo());
    common::run(&mut h);
    assert!(text(&h).contains("Hello [[Oth\n"), "one step: {}", text(&h));
}

#[test]
fn a_withdrawn_package_brings_the_cards_back() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    assert!(shown(&mut h).contains("Tip"));
    h.state_mut()
        .block_package_action(markise_ui_egui::package_ui::PackageAction::Forget);
    settle(&mut h);
    common::run(&mut h);
    let s = shown(&mut h);
    assert!(!s.contains("Tip") && s.contains("tip"), "{s}");
}

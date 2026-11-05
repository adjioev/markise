//! The form of a block in a side panel (ADR 0020), on a small neutral package: opening, draft,
//! Save and Cancel, undo, and what becomes of the draft when the document changes under it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use markise_doc::{Patch, PatchSet};
use markise_engine::jobs::JobHooks;
use markise_package::Sha256Hex;
use markise_ui_egui::confirm::{Choice, Kind};
use markise_ui_egui::menu::MenuAction;
use markise_ui_egui::{AppOptions, ConfigDir, MarkiseApp, ViewMode};
use serde_json::{Value, json};

use crate::common::{self, settle, wait_until};

type H = Harness<'static, MarkiseApp>;

const TIP: &str = r#"{"label": "Tip", "fields": [
    {"name": "title", "type": "heading", "level": 3, "min": 0},
    {"name": "body", "type": "text", "min": 1}
]}"#;
const POLL: &str = r#"{"label": "Poll",
    "variants": {"": "one answer", "many": "several", "ways": "ways"},
    "fields": [
        {"name": "question", "type": "heading", "level": 2},
        {"name": "answers", "type": "choices", "ids": "required", "minItems": 2,
         "correct": {"": "exactly-one", "many": "at-least-one", "ways": "at-least-one"}}
    ]}"#;
const GALLERY: &str = r#"{"label": "Gallery", "fields": [{"name": "pictures", "type": "image", "min": 0}]}"#;
const LIMITED: &str = r#"{"label": "Limited", "fields": [{"name": "body", "type": "text", "max": 3}]}"#;
const PALETTE: &str = r#"{"format": 1, "presets": [{
    "id": "ways", "block": "poll", "variant": "ways", "widget": "icon-choices", "field": "answers",
    "columns": 2, "field_label": "Ways", "markdown": ":::poll ways\n:::\n",
    "answers": [
        {"label": "Left", "icon": "assets/left.svg"},
        {"label": "Right", "icon": "assets/right.svg"},
        {"label": "No way", "exclusive": true}
    ]}]}"#;
const ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor"><path d="M19 12H5"/></svg>"#;

const DOC: &str = "# Doc\n\nBefore.\n\n:::tip\n\n### A title\n\nHello\n\n:::\n\nBetween.\n\n:::poll\n\n## Which?\n\n- [ ] One {#a1}\n- [x] Two {#a2}\n\n:::\n\n:::poll ways\n\n## Where?\n\n- [ ] Left {#a1}\n- [x] Right {#a2}\n- [ ] No way {#a3}\n\n:::\n\n:::gallery\n\n![Alt](images/one.png)\n\n:::\n\n:::other\n\nNo schema.\n\n:::\n";

struct Fixture {
    _dir: tempfile::TempDir,
    config: PathBuf,
    folder: PathBuf,
    doc: PathBuf,
    picks: Arc<Mutex<Option<PathBuf>>>,
}

fn archive() -> Vec<u8> {
    let manifest = r#"{"format":"markise-block-package/2","library":"sample","version":"1.0.0","blocks":["tip","poll","gallery","limited"]}"#;
    let files = [
        ("manifest.json", manifest),
        ("components/tip/tip.json", TIP),
        ("components/poll/poll.json", POLL),
        ("components/gallery/gallery.json", GALLERY),
        ("components/limited/limited.json", LIMITED),
        ("editor/palette.json", PALETTE),
        ("assets/left.svg", ICON),
        ("assets/right.svg", ICON),
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

/// A small PNG for the thumbnail: one pixel.
fn one_pixel_png() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(1, 1)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

impl Fixture {
    fn new(text: &str) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let config = root.join("config");
        let folder = root.join("content");
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(folder.join("images")).unwrap();
        fs::write(folder.join("images/one.png"), one_pixel_png()).unwrap();
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
        Fixture {
            config,
            folder,
            doc,
            picks: Arc::new(Mutex::new(None)),
            _dir: dir,
        }
    }

    fn harness(&self) -> H {
        self.harness_with(Default::default())
    }

    fn harness_with(&self, hooks: JobHooks) -> H {
        let picks = self.picks.clone();
        let opts = AppOptions {
            config: ConfigDir::Dir(self.config.clone()),
            block_image_picker: Some(Arc::new(move |_: &Path| picks.lock().unwrap().clone())),
            hooks: std::sync::Arc::new(hooks),
            ..common::no_watch()
        };
        Harness::builder()
            .with_size(egui::vec2(1100.0, 1400.0))
            .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), MarkiseApp::new(opts))
    }

    /// Opened, and the package accepted.
    fn ready(&self) -> H {
        let mut h = self.harness();
        open(&mut h, &self.doc);
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

fn has(h: &H, text: &str) -> bool {
    h.query_by_label_contains(text).is_some()
}

fn text(h: &H) -> String {
    h.state().document_text().unwrap()
}

fn draft(h: &H) -> Value {
    h.state().block_form().unwrap().values().unwrap().clone()
}

fn open_form(h: &mut H, name: &str) {
    h.get_all_by_label(&format!("Edit Block: {name}"))
        .next()
        .unwrap()
        .click();
    common::run(h);
    assert!(h.state().block_form().is_some(), "{name} opens");
}

/// With a picture or an icon on its way, the worker wakes the UI when it comes, and a spinner
/// keeps asking for frames: wait for the background instead of a quiet UI.
fn wait(h: &mut H) {
    h.step();
    h.step();
    common::run(h);
}

fn offset(text: &str, needle: &str) -> usize {
    text.find(needle).unwrap()
}

/// An edit from elsewhere in the app, as typing in the document or Side by Side would do it.
fn edit_document(h: &mut H, at: std::ops::Range<usize>, insert: &str) {
    let patches = PatchSet::new(vec![Patch::verbatim(at, insert.to_string())]).unwrap();
    let v = h.state_mut().viewer().unwrap();
    v.ed.edit(|s| s.apply_external(&patches, "Edit", false)).unwrap();
    common::run(h);
}

#[test]
fn no_package_no_button() {
    // not accepted yet
    let f = Fixture::new(DOC);
    let mut h = f.harness();
    open(&mut h, &f.doc);
    assert!(!has(&h, "Edit Block"));
    let ctx = h.ctx.clone();
    assert!(!h.state_mut().menu_action(&ctx, &MenuAction::Command("block.editForm")));
    assert!(h.state().block_form().is_none());
    assert!(!h.state_mut().open_block_form(offset(DOC, ":::tip")));

    // no binding at all
    fs::remove_file(f.folder.join(markise_package::BINDING_FILE)).unwrap();
    let mut h = f.harness();
    open(&mut h, &f.doc);
    assert!(!has(&h, "Edit Block"));
    assert!(h.state().block_package().is_none());
}

#[test]
fn a_block_with_no_schema_has_no_form() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    assert!(has(&h, "Edit Block: tip"));
    assert!(!has(&h, "Edit Block: other"));
    assert!(!h.state_mut().open_block_form(offset(DOC, ":::other")));
    assert!(h.state().block_form().is_none());
}

#[test]
fn edit_and_save_is_one_undo_step() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    let undo_before = h.state_mut().viewer().unwrap().ed.session.history().undo_len();
    open_form(&mut h, "tip");
    assert_eq!(draft(&h), json!({"title": "A title", "body": "Hello"}));
    assert_eq!(text(&h), DOC, "opening changes nothing");

    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("!");
    common::run(&mut h);
    assert_eq!(draft(&h)["body"], json!("Hello!"));
    assert_eq!(text(&h), DOC, "a draft stays out of the document");

    h.get_by_label("Save").click();
    common::run(&mut h);
    assert!(h.state().block_form().is_none(), "Save closes");
    let saved = text(&h);
    assert_eq!(saved, DOC.replacen("Hello\n", "Hello!\n", 1));
    let v = h.state_mut().viewer().unwrap();
    assert_eq!(v.ed.session.history().undo_len(), undo_before + 1);
    assert!(v.ed.edit(|s| s.undo()));
    common::run(&mut h);
    assert_eq!(text(&h), DOC);
}

#[test]
fn an_unchanged_save_writes_nothing() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    let before = h.state_mut().viewer().unwrap().ed.changes();
    open_form(&mut h, "poll");
    h.get_by_label("Save").click();
    common::run(&mut h);
    assert!(h.state().block_form().is_none());
    assert_eq!(text(&h), DOC);
    assert_eq!(h.state_mut().viewer().unwrap().ed.changes(), before);
    assert!(!h.state().is_dirty());
}

#[test]
fn cancel_and_its_keys_leave_the_document_alone() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    open_form(&mut h, "tip");
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("!");
    common::run(&mut h);
    h.get_by_label("Cancel").click();
    common::run(&mut h);
    assert!(h.state().block_form().is_none());
    assert_eq!(text(&h), DOC);

    open_form(&mut h, "tip");
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.key_press(Key::Escape);
    common::run(&mut h);
    assert!(h.state().block_form().is_none(), "Esc cancels");
    assert_eq!(text(&h), DOC);
}

#[test]
fn the_command_shortcuts_save_and_close_only_the_form() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    open_form(&mut h, "tip");
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("!");
    common::run(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    common::run(&mut h);
    assert!(h.state().block_form().is_none(), "Cmd+Enter saves");
    assert!(text(&h).contains("Hello!\n"));

    // the file itself is not written by Save of a form, Cmd+S goes to the form first
    open_form(&mut h, "tip");
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("?");
    common::run(&mut h);
    let ctx = h.ctx.clone();
    assert!(h.state_mut().menu_action(&ctx, &MenuAction::Command("file.save")));
    common::run(&mut h);
    assert!(h.state().block_form().is_none());
    assert!(text(&h).contains("Hello!?\n"));
    assert_eq!(
        fs::read_to_string(&f.doc).unwrap(),
        DOC,
        "the file waits for the next Save"
    );

    open_form(&mut h, "tip");
    h.get_by_label("Body").click();
    common::run(&mut h);
    let ctx = h.ctx.clone();
    assert!(h.state_mut().menu_action(&ctx, &MenuAction::Command("file.close")));
    common::run(&mut h);
    assert!(h.state().block_form().is_none(), "Cmd+W cancels the form");
    assert_eq!(h.state().document_path(), Some(f.doc.as_path()), "the document stays");
}

#[test]
fn the_footers_note_going_away_does_not_move_the_focus_to_save() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    open_form(&mut h, "tip");
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("!");
    common::run(&mut h);
    h.get_by_label("Cancel").focus();
    common::run(&mut h);
    assert!(h.query_by_label_contains("Esc cancels").is_some(), "the note is there");
    // narrow enough that the note in front of the buttons goes away
    h.set_size(egui::vec2(520.0, 1400.0));
    common::run(&mut h);
    assert!(h.query_by_label_contains("Esc cancels").is_none(), "the note is gone");
    h.key_press(Key::Space);
    common::run(&mut h);
    assert!(h.state().block_form().is_none(), "Cancel took it");
    assert_eq!(text(&h), DOC, "and it was not Save");
}

#[test]
fn the_keyboard_opens_the_form_at_the_caret_and_lands_in_it() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    let inside = offset(DOC, "Hello");
    let v = h.state_mut().viewer().unwrap();
    v.ed.edit(|s| s.set_selection(markise_doc::Selection::caret(inside)));
    let ctx = h.ctx.clone();
    assert!(h.state_mut().menu_action(&ctx, &MenuAction::Command("block.editForm")));
    common::run(&mut h);
    assert!(h.state().block_form().is_some());
    assert!(
        h.get_by_label("Title").accesskit_node().is_focused(),
        "the first field has the keyboard"
    );
    assert_eq!(
        h.state().menu_model().item("block.editForm").map(|i| i.enabled),
        Some(true)
    );
}

#[test]
fn undo_in_a_field_stays_in_the_field() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    edit_document(&mut h, 0..0, "x");
    let doc = text(&h);
    open_form(&mut h, "tip");
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("abc");
    common::run(&mut h);
    assert_eq!(draft(&h)["body"], json!("Helloabc"));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    common::run(&mut h);
    assert_eq!(draft(&h)["body"], json!("Hello"), "the field's own undo");
    assert_eq!(text(&h), doc, "the document's undo did not run");
    assert!(h.state().block_form().is_some());
}

#[test]
fn document_commands_wait_while_the_keyboard_is_in_the_form() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    open_form(&mut h, "poll");
    h.get_by_label("Add").click();
    common::run(&mut h);
    // the focus on a button, not in a text field
    h.get_by_label("Move up: Answers 3").focus();
    common::run(&mut h);
    let ctx = h.ctx.clone();
    let before = text(&h);
    assert!(!h.state_mut().menu_action(&ctx, &MenuAction::Command("format.strong")));
    common::run(&mut h);
    assert_eq!(text(&h), before);
}

#[test]
fn a_block_that_changed_refuses_a_draft_clearly() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    open_form(&mut h, "tip");
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("!");
    common::run(&mut h);
    let at = offset(DOC, "Hello");
    edit_document(&mut h, at..at, "Changed ");
    assert!(has(&h, "The block changed in the document"));
    let after_edit = text(&h);
    h.get_by_label("Save").click();
    common::run(&mut h);
    assert_eq!(text(&h), after_edit, "refused");
    assert!(h.state().block_form().is_some(), "the draft is still there");
    assert_eq!(draft(&h)["body"], json!("Hello!"));

    h.get_by_label("Read the Block Again").click();
    common::run(&mut h);
    assert_eq!(draft(&h)["body"], json!("Changed Hello"));
    h.get_by_label("Save").click();
    common::run(&mut h);
    assert!(h.state().block_form().is_none());
    assert_eq!(text(&h), after_edit);
}

#[test]
fn a_clean_panel_follows_the_document() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    open_form(&mut h, "tip");
    let at = offset(DOC, "Hello");
    edit_document(&mut h, at..at, "New ");
    assert!(h.state().block_form().is_some());
    assert_eq!(draft(&h)["body"], json!("New Hello"), "refreshed, nothing to lose");
    assert!(!has(&h, "The block changed"));

    // a change above only moves the block; a draft survives it and saves in the right place
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("!");
    common::run(&mut h);
    edit_document(&mut h, 0..0, "Above\n\n");
    assert!(!has(&h, "The block changed"));
    assert!(h.state().block_form().unwrap().key() > offset(DOC, ":::tip"));
    h.get_by_label("Save").click();
    common::run(&mut h);
    assert!(text(&h).starts_with("Above\n\n# Doc"));
    assert!(text(&h).contains("New Hello!\n"));
}

const TWINS: &str = "Top.\n\n:::tip\n\nSame\n\n:::\n\nMiddle.\n\n:::tip\n\nSame\n\n:::\n";

fn twin_offsets() -> (usize, usize) {
    let first = TWINS.find(":::tip").unwrap();
    (first, TWINS.rfind(":::tip").unwrap())
}

#[test]
fn a_changed_block_goes_stale_and_never_moves_to_a_look_alike() {
    let f = Fixture::new(TWINS);
    let mut h = f.ready();
    let (first, _) = twin_offsets();
    assert!(h.state_mut().open_block_form(first));
    common::run(&mut h);
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("!");
    common::run(&mut h);
    let at = first + TWINS[first..].find("Same").unwrap();
    edit_document(&mut h, at..at, "Changed ");
    assert!(has(&h, "The block changed in the document"));
    assert_eq!(h.state().block_form().unwrap().key(), first);
    let doc = text(&h);
    h.get_by_label("Save").click();
    common::run(&mut h);
    assert_eq!(text(&h), doc, "nothing goes anywhere, the twin least of all");
}

#[test]
fn a_look_alike_above_does_not_take_the_draft() {
    let f = Fixture::new(TWINS);
    let mut h = f.ready();
    let (_, second) = twin_offsets();
    assert!(h.state_mut().open_block_form(second));
    common::run(&mut h);
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("!");
    common::run(&mut h);
    // the same block's text again, at the top; the nearest copy of it may be any
    let copy = ":::tip\n\nSame\n\n:::\n\n";
    edit_document(&mut h, second..second, copy);
    assert!(!has(&h, "The block changed"));
    h.get_by_label("Save").click();
    common::run(&mut h);
    let saved = text(&h);
    let expected = format!("{}{copy}:::tip\n\nSame!\n\n:::\n", &TWINS[..second]);
    assert_eq!(saved, expected);
}

#[test]
fn a_deleted_block_does_not_become_the_twin_right_after_it() {
    let doc = ":::tip\n\nSame\n\n:::\n:::tip\n\nSame\n\n:::\n";
    let f = Fixture::new(doc);
    let mut h = f.ready();
    assert!(h.state_mut().open_block_form(0));
    common::run(&mut h);
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("!");
    common::run(&mut h);
    let end = doc.rfind(":::tip").unwrap();
    edit_document(&mut h, 0..end, "");
    assert!(has(&h, "no longer in the document"));
    let left = text(&h);
    h.get_by_label("Save").click();
    common::run(&mut h);
    assert_eq!(text(&h), left);
}

#[test]
fn save_finds_the_block_by_its_marks_even_before_a_frame() {
    let doc = ":::tip\n\nSame\n\n:::\n:::tip\n\nSame\n\n:::\n";
    let f = Fixture::new(doc);
    let mut h = f.ready();
    assert!(h.state_mut().open_block_form(0));
    common::run(&mut h);
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("!");
    common::run(&mut h);
    // the twin moves into the place the form last read, and Save comes before the panel looks
    let end = doc.rfind(":::tip").unwrap();
    let patches = PatchSet::new(vec![Patch::verbatim(0..end, String::new())]).unwrap();
    let v = h.state_mut().viewer().unwrap();
    v.ed.edit(|s| s.apply_external(&patches, "Edit", false)).unwrap();
    let left = text(&h);
    let ctx = h.ctx.clone();
    h.state_mut().block_form_save(&ctx);
    assert_eq!(text(&h), left, "the twin keeps its text");
    assert!(
        h.state()
            .block_form()
            .unwrap()
            .message()
            .is_some_and(|m| m.contains("no longer"))
    );
}

#[test]
fn a_reload_landing_before_save_is_seen_by_save() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    open_form(&mut h, "tip");
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("!");
    common::run(&mut h);
    // moved: saved where it stands now
    let moved = format!("New top.\n\n{DOC}");
    reload_no_frame(&mut h, &f, &moved);
    let ctx = h.ctx.clone();
    h.state_mut().block_form_save(&ctx);
    assert_eq!(text(&h), moved.replacen("Hello\n", "Hello!\n", 1));
    assert!(h.state().block_form().is_none());

    // changed under the draft: refused
    common::run(&mut h);
    open_form(&mut h, "tip");
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("?");
    common::run(&mut h);
    let other = text(&h).replacen("Hello!", "Third", 1);
    reload_no_frame(&mut h, &f, &other);
    h.state_mut().block_form_save(&ctx);
    assert_eq!(text(&h), other);
    assert!(
        h.state()
            .block_form()
            .unwrap()
            .message()
            .is_some_and(|m| m.contains("changed"))
    );
}

#[test]
fn a_draft_does_not_go_into_the_document_that_took_its_place() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    open_form(&mut h, "tip");
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("!");
    common::run(&mut h);
    // another document with the same bytes, with no frame in between and no question: what
    // happens when a load is polled and Save comes on that same frame
    h.state_mut().replace_document_for_test();
    let patches = PatchSet::new(vec![Patch::verbatim(0..0, DOC.to_string())]).unwrap();
    let v = h.state_mut().viewer().unwrap();
    v.ed.edit(|s| s.apply_external(&patches, "Edit", false)).unwrap();
    let ctx = h.ctx.clone();
    h.state_mut().block_form_save(&ctx);
    assert_eq!(text(&h), DOC, "the draft stays out of it");
    assert_eq!(draft(&h)["body"], json!("Hello!"), "and can still be read");
    common::run(&mut h);
    assert!(has(&h, "copy the text"), "and says so");
    h.state_mut().block_form_cancel();
    assert!(h.state().block_form().is_none());
}

#[test]
fn the_keys_of_the_form_wait_for_a_composition() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    open_form(&mut h, "tip");
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.event(egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "n".into(),
        active_range_chars: Some(0..1),
    }));
    common::run(&mut h);
    let ctx = h.ctx.clone();
    assert!(h.state_mut().menu_action(&ctx, &MenuAction::Command("file.close")));
    common::run(&mut h);
    assert!(h.state().block_form().is_some(), "Cmd+W waits, as Esc does");
    assert_eq!(text(&h), DOC);
    assert!(h.state_mut().menu_action(&ctx, &MenuAction::Command("file.save")));
    common::run(&mut h);
    assert!(h.state().block_form().is_some(), "and so does Cmd+S");
    assert_eq!(text(&h), DOC);
}

#[test]
fn another_variant_in_the_document_gets_its_own_form() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    open_form(&mut h, "poll");
    assert!(has(&h, "Poll") && has(&h, "one answer"));
    let at = offset(DOC, ":::poll\n") + ":::poll".len();
    edit_document(&mut h, at..at, " many");
    common::run(&mut h);
    assert!(has(&h, "several"), "opened anew");
    assert_eq!(h.state().block_form().unwrap().args(), "many");
}

#[test]
fn a_deleted_block_closes_a_clean_panel_and_warns_a_draft() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    let start = offset(DOC, ":::tip");
    let end = offset(DOC, "Between.");
    open_form(&mut h, "tip");
    edit_document(&mut h, start..end, "");
    assert!(h.state().block_form().is_none(), "nothing to lose");

    let v = h.state_mut().viewer().unwrap();
    assert!(v.ed.edit(|s| s.undo()));
    common::run(&mut h);
    open_form(&mut h, "tip");
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("!");
    common::run(&mut h);
    edit_document(&mut h, start..end, "");
    assert!(has(&h, "no longer in the document"));
    assert_eq!(draft(&h)["body"], json!("Hello!"), "still readable");
    let doc = text(&h);
    h.get_by_label("Save").click();
    common::run(&mut h);
    assert_eq!(text(&h), doc);
    h.get_by_label("Cancel").click();
    common::run(&mut h);
    assert!(h.state().block_form().is_none());
}

#[test]
fn an_edit_over_the_whole_text_leaves_no_block_to_follow() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    open_form(&mut h, "tip");
    // nothing says the block is still the block, even if a copy is there now
    edit_document(&mut h, 0..DOC.len(), &format!("New top.\n\n{DOC}"));
    assert!(h.state().block_form().is_none());
}

/// Replaces the document the way a reload from disk does: it is not an edit of the session.
fn reload(h: &mut H, f: &Fixture, new: &str) {
    reload_no_frame(h, f, new);
    common::run(h);
}

fn reload_no_frame(h: &mut H, f: &Fixture, new: &str) {
    let doc = markise_md::Document::parse(new.to_string(), markise_md::ParseOptions::default());
    let model = markise_layout::DocModel::build(&doc, Some(&f.doc));
    let doc: Arc<dyn markise_doc::FormatDoc> = Arc::new(doc);
    let v = h.state_mut().viewer().unwrap();
    v.install(doc, model, "Reload from Disk", false).unwrap();
}

#[test]
fn a_reload_from_disk_is_followed_like_any_change() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    open_form(&mut h, "tip");

    // changed inside, no draft: the panel shows what is on disk now
    let changed = DOC.replacen("Hello", "From disk", 1);
    reload(&mut h, &f, &changed);
    assert_eq!(draft(&h)["body"], json!("From disk"));

    // moved: a draft goes along and saves where the block is now
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("!");
    common::run(&mut h);
    let moved = format!("New top.\n\n{changed}");
    reload(&mut h, &f, &moved);
    assert_eq!(draft(&h)["body"], json!("From disk!"));
    assert!(!has(&h, "The block changed"));
    h.get_by_label("Save").click();
    common::run(&mut h);
    assert_eq!(text(&h), moved.replacen("From disk", "From disk!", 1));

    // changed under a draft: refused, clearly
    open_form(&mut h, "tip");
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("?");
    common::run(&mut h);
    let other = text(&h).replacen("From disk!", "Third", 1);
    reload(&mut h, &f, &other);
    assert!(has(&h, "The block changed in the document"));
    h.get_by_label("Save").click();
    common::run(&mut h);
    assert_eq!(text(&h), other);

    // gone, no draft: closed
    h.get_by_label("Read the Block Again").click();
    common::run(&mut h);
    reload(&mut h, &f, "Nothing here.\n");
    assert!(h.state().block_form().is_none());
}

#[test]
fn a_document_switch_and_a_withdrawn_package_close_a_clean_panel() {
    let f = Fixture::new(DOC);
    let other = f.folder.join("other.md");
    fs::write(&other, "No blocks.\n").unwrap();
    let mut h = f.ready();
    open_form(&mut h, "tip");
    open(&mut h, &other);
    assert!(h.state().block_form().is_none());
    assert!(!has(&h, "Edit Block"));

    open(&mut h, &f.doc);
    open_form(&mut h, "tip");
    h.state_mut()
        .block_package_action(markise_ui_egui::package_ui::PackageAction::Forget);
    wait(&mut h);
    assert!(h.state().block_form().is_none());
    assert!(!has(&h, "Edit Block"));
}

fn changed_draft(h: &mut H) {
    open_form(h, "tip");
    h.get_by_label("Body").click();
    common::run(h);
    h.get_by_label("Body").type_text("!");
    common::run(h);
}

fn asked(h: &H) -> Kind {
    h.state().question().expect("a question").kind
}

#[test]
fn a_document_switch_asks_about_a_changed_draft() {
    let f = Fixture::new(DOC);
    let other = f.folder.join("other.md");
    fs::write(&other, "No blocks.\n").unwrap();
    let mut h = f.ready();
    changed_draft(&mut h);

    // Cancel: nothing happened
    h.state_mut().open(other.clone());
    assert_eq!(asked(&h), Kind::BlockDraft);
    h.state_mut().answer_question(Choice::Cancel);
    common::run(&mut h);
    assert_eq!(h.state().document_path(), Some(f.doc.as_path()));
    assert_eq!(draft(&h)["body"], json!("Hello!"));

    // Discard: on to the other document; the document itself never changed
    h.state_mut().open(other.clone());
    h.state_mut().answer_question(Choice::DontSave);
    wait_until(&mut h, "the other document", |a| {
        a.document_path() == Some(other.as_path()) && !a.is_loading()
    });
    assert!(h.state().block_form().is_none());

    // Save: into the document, which then has an unsaved edit of its own to ask about
    open(&mut h, &f.doc);
    changed_draft(&mut h);
    h.state_mut().open(other.clone());
    h.state_mut().answer_question(Choice::Save);
    assert!(h.state().block_form().is_none());
    assert!(text(&h).contains("Hello!"));
    assert_eq!(asked(&h), Kind::UnsavedChanges);
    h.state_mut().answer_question(Choice::Cancel);
    common::run(&mut h);
    assert_eq!(h.state().document_path(), Some(f.doc.as_path()));
    assert!(h.state().is_dirty());
}

#[test]
fn a_draft_with_nowhere_to_go_is_not_overwritten_by_another_form() {
    let f = Fixture::new(DOC);
    let other = f.folder.join("other.md");
    fs::write(&other, DOC).unwrap();
    let mut h = f.ready();
    changed_draft(&mut h);
    // another document takes its place with no question, and it has blocks too
    h.state_mut().open_answered_for_test(other.clone());
    wait_until(&mut h, "the other document", |a| {
        a.document_path() == Some(other.as_path()) && !a.is_loading() && a.block_package().is_some()
    });
    common::run(&mut h);
    assert_eq!(draft(&h)["body"], json!("Hello!"));
    let tip = text(&h).find("Hello").unwrap();
    assert!(!h.state_mut().open_block_form(tip), "refused");
    assert_eq!(draft(&h)["body"], json!("Hello!"), "still there");
    common::run(&mut h);
    assert!(has(&h, "Cancel this draft"), "and it says why");
}

#[test]
fn a_draft_changed_during_the_save_that_closes_the_window_is_asked_about() {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let rx = Mutex::new(rx);
    let f = Fixture::new(DOC);
    let mut h = f.harness_with(JobHooks {
        before_save: Some(Box::new(move |_| {
            let _ = rx.lock().unwrap().recv();
        })),
        ..Default::default()
    });
    open(&mut h, &f.doc);
    h.get_by_label("Use Package").click();
    h.step();
    wait_until(&mut h, "the install", |a| a.block_package().is_some());
    settle(&mut h);
    common::run(&mut h);
    open_form(&mut h, "tip");
    // the document has an edit of its own; the draft has none yet
    let patches = PatchSet::new(vec![Patch::verbatim(0..0, "x".to_string())]).unwrap();
    h.state_mut()
        .viewer()
        .unwrap()
        .ed
        .edit(|s| s.apply_external(&patches, "Edit", false))
        .unwrap();
    h.step();
    assert!(!h.state_mut().request_close());
    assert_eq!(asked(&h), Kind::UnsavedChanges);
    h.state_mut().answer_question(Choice::Save);
    // while it saves, the draft changes
    h.get_by_label("Body").click();
    for _ in 0..4 {
        h.step();
    }
    h.get_by_label("Body").type_text("!");
    for _ in 0..4 {
        h.step();
    }
    assert_eq!(draft(&h)["body"], json!("Hello!"));
    tx.send(()).unwrap();
    wait_until(&mut h, "the question", |a| a.is_asking());
    assert_eq!(asked(&h), Kind::BlockDraft, "not closed");
}

#[test]
fn closing_the_window_asks_about_a_changed_draft() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    changed_draft(&mut h);
    assert!(!h.state_mut().request_close());
    assert_eq!(asked(&h), Kind::BlockDraft);
    h.state_mut().answer_question(Choice::Cancel);
    assert_eq!(draft(&h)["body"], json!("Hello!"));
}

#[test]
fn stopping_the_package_asks_about_a_changed_draft() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    changed_draft(&mut h);
    let stop = markise_ui_egui::package_ui::PackageAction::Forget;
    h.state_mut().block_package_action(stop);
    assert_eq!(asked(&h), Kind::BlockDraft);
    h.state_mut().answer_question(Choice::Cancel);
    wait(&mut h);
    assert!(h.state().block_package().is_some(), "still in use");
    assert_eq!(draft(&h)["body"], json!("Hello!"));

    h.state_mut().block_package_action(stop);
    h.state_mut().answer_question(Choice::Save);
    wait(&mut h);
    assert!(h.state().block_package().is_none(), "stopped after the save");
    assert!(text(&h).contains("Hello!"));
}

#[test]
fn a_package_lost_without_a_question_keeps_the_draft_and_its_save() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    changed_draft(&mut h);
    // what a Save As into a folder without the package does: the lookup finds nothing there
    let elsewhere = tempfile::tempdir().unwrap();
    let moved = fs::canonicalize(elsewhere.path()).unwrap().join("moved.md");
    h.state_mut().move_document_for_test(moved);
    wait(&mut h);
    assert!(h.state().block_package().is_none());
    assert_eq!(draft(&h)["body"], json!("Hello!"), "kept");
    let ctx = h.ctx.clone();
    h.state_mut().block_form_save(&ctx);
    assert!(h.state().block_form().is_none());
    assert!(text(&h).contains("Hello!"), "saved");
}

#[test]
fn a_draft_is_not_thrown_away_for_another_block() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    open_form(&mut h, "tip");
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("!");
    common::run(&mut h);
    assert!(!h.state_mut().open_block_form(offset(DOC, ":::gallery")));
    common::run(&mut h);
    assert_eq!(h.state().block_form().unwrap().name(), "tip");
    assert!(has(&h, "Save or cancel this draft"));
}

#[test]
fn a_block_outside_its_schema_shows_reasons_and_can_be_saved() {
    let doc = ":::poll\n\n- [ ] One {#a1}\n- [ ] One more\n\nStray text.\n\n:::\n";
    let f = Fixture::new(doc);
    let mut h = f.ready();
    open_form(&mut h, "poll");
    assert!(has(&h, "does not fit its schema"));
    assert!(has(&h, "Mark exactly one"));
    assert!(has(&h, "Has no id"));
    assert!(has(&h, "In no field, kept as it is: Stray text."));
    h.get_by_label("Question").click();
    common::run(&mut h);
    h.get_by_label("Question").type_text("Asked?");
    common::run(&mut h);
    h.get_by_label("Save").click();
    common::run(&mut h);
    assert!(h.state().block_form().is_none(), "saved all the same");
    let saved = text(&h);
    assert!(saved.contains("Asked?") && saved.contains("Stray text."), "{saved}");
}

#[test]
fn an_unknown_variant_is_a_reason_too() {
    let doc = ":::poll nope\n\n## Q\n\n- [x] One {#a1}\n- [ ] Two {#a2}\n\n:::\n";
    let f = Fixture::new(doc);
    let mut h = f.ready();
    open_form(&mut h, "poll");
    assert!(has(&h, "The block has no variant `nope`"));
}

#[test]
fn a_form_the_package_makes_impossible_says_why() {
    let doc = ":::limited\n\nHello\n\n:::\n";
    let f = Fixture::new(doc);
    let mut h = f.ready();
    open_form(&mut h, "limited");
    assert!(has(&h, "This block has no form"));
    assert!(has(&h, "`max` 3"));
    assert!(h.state().block_form().unwrap().values().is_none());
    h.get_by_label("Cancel").click();
    common::run(&mut h);
    assert!(h.state().block_form().is_none());
    assert_eq!(text(&h), doc);
}

#[test]
fn icon_choices_write_their_marks_and_exclusive_clears() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    let at = offset(DOC, ":::poll ways");
    assert!(h.state_mut().open_block_form(at));
    // the worker's answers wake the UI whenever they come: a wait for them, not for a quiet UI
    wait(&mut h);
    assert!(has(&h, "Ways"), "the label comes from the package");
    h.get_by_label("Left").click();
    common::run(&mut h);
    let marks = |h: &H| {
        (draft(h)["answers"].as_array().unwrap().iter())
            .map(|a| a["correct"].as_bool().unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(marks(&h), [true, true, false]);
    h.get_by_label("No way").click();
    common::run(&mut h);
    assert_eq!(marks(&h), [false, false, true]);
    h.get_by_label("Save").click();
    common::run(&mut h);
    let saved = text(&h);
    assert!(
        saved.contains("- [ ] Left {#a1}\n- [ ] Right {#a2}\n- [x] No way {#a3}"),
        "{saved}"
    );
}

#[test]
fn a_new_answer_gets_its_id_on_save() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    open_form(&mut h, "poll");
    h.get_by_label("Add").click();
    common::run(&mut h);
    assert!(!has(&h, "Has no id"), "a new row is not missing one");
    h.get_by_label("Answers 3").type_text("Three");
    common::run(&mut h);
    h.get_by_label("Save").click();
    common::run(&mut h);
    assert!(text(&h).contains("- [ ] Three {#a3}"), "{}", text(&h));
}

#[test]
fn a_picked_image_goes_in_relative_and_one_outside_is_refused() {
    let f = Fixture::new(DOC);
    let inside = f.folder.join("images/two pics.png");
    fs::write(&inside, one_pixel_png()).unwrap();
    let mut h = f.ready();
    h.get_by_label("Edit Block: gallery").click();
    wait(&mut h);
    *f.picks.lock().unwrap() = Some(inside);
    h.get_by_label("Choose file: Pictures 1").click();
    wait(&mut h);
    assert_eq!(draft(&h)["pictures"][0]["path"], json!("images/two pics.png"));

    let outside = f.config.join("out.png");
    fs::write(&outside, one_pixel_png()).unwrap();
    *f.picks.lock().unwrap() = Some(outside);
    h.get_by_label("Choose file: Pictures 1").click();
    wait(&mut h);
    assert_eq!(draft(&h)["pictures"][0]["path"], json!("images/two pics.png"), "kept");
    assert!(has(&h, "not inside the document's folder"));

    h.get_by_label("Save").click();
    wait(&mut h);
    let saved = text(&h);
    assert!(saved.contains("two") && saved.contains("pics.png"), "{saved}");
    // the path reads back the same
    let inside_again = saved.find(":::gallery").unwrap();
    assert!(h.state_mut().open_block_form(inside_again));
    wait(&mut h);
    assert_eq!(draft(&h)["pictures"][0]["path"], json!("images/two pics.png"));
}

#[test]
fn the_form_works_from_source_mode_and_side_by_side() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    common::run(&mut h);
    open_form(&mut h, "tip");
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("!");
    common::run(&mut h);
    h.get_by_label("Save").click();
    common::run(&mut h);
    assert!(text(&h).contains("Hello!\n"));

    h.state_mut().set_view_mode(ViewMode::Source);
    common::run(&mut h);
    assert!(!has(&h, "Edit Block"), "no cards to put a button on");
    let at = offset(&text(&h), ":::tip");
    assert!(h.state_mut().open_block_form(at));
    common::run(&mut h);
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("?");
    common::run(&mut h);
    h.get_by_label("Save").click();
    common::run(&mut h);
    assert!(text(&h).contains("Hello!?\n"), "{}", text(&h));
    assert!(h.state().block_form().is_none());
}

#[test]
fn a_nested_block_opens_its_own_form() {
    let doc = "::::gallery\n\n:::tip\n\nInner\n\n:::\n\n::::\n";
    let f = Fixture::new(doc);
    let mut h = f.ready();
    assert!(h.state_mut().open_block_form(offset(doc, ":::tip")));
    common::run(&mut h);
    assert_eq!(h.state().block_form().unwrap().name(), "tip");
    assert_eq!(draft(&h)["body"], json!("Inner"));
}

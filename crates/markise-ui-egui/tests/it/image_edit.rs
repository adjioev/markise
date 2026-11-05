//! An image's Replace… and Edit… buttons on the real app, with a chosen file played by a stub.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use egui::{Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_engine::settings::CommentView;
use markise_ui_egui::{AppOptions, ConfigDir, MarkiseApp};

use crate::common;

type H = Harness<'static, MarkiseApp>;

struct Fx {
    dir: tempfile::TempDir,
    pick: Arc<Mutex<Option<PathBuf>>>,
}

fn png(path: &Path) {
    image::RgbaImage::from_pixel(300, 200, image::Rgba([20, 90, 200, 255]))
        .save(path)
        .unwrap();
}

fn fixture(doc: &str) -> (Fx, H) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir_all(root.join("note")).unwrap();
    std::fs::create_dir_all(root.join("away")).unwrap();
    std::fs::create_dir_all(root.join("config")).unwrap();
    png(&root.join("note/old.png"));
    let path = root.join("note/doc.md");
    std::fs::write(&path, doc).unwrap();
    let pick: Arc<Mutex<Option<PathBuf>>> = Default::default();
    let answer = pick.clone();
    let opts = AppOptions {
        open: vec![path.clone()],
        config: ConfigDir::Dir(root.join("config")),
        block_image_picker: Some(Arc::new(move |_: &Path| answer.lock().unwrap().clone())),
        ..common::no_watch()
    };
    let mut h = Harness::builder()
        .with_size(egui::vec2(1100.0, 900.0))
        .with_step_dt(0.02)
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), MarkiseApp::new(opts));
    common::wait_until(&mut h, "load", |a| {
        a.document_path() == Some(path.as_path()) && !a.is_loading()
    });
    common::settle(&mut h);
    common::run(&mut h);
    (Fx { dir, pick }, h)
}

impl Fx {
    fn root(&self) -> PathBuf {
        std::fs::canonicalize(self.dir.path()).unwrap()
    }
}

fn text(h: &H) -> String {
    h.state().document_text().unwrap()
}

fn image_rect(h: &H, needle: &str) -> egui::Rect {
    let at = text(h).find(needle).unwrap();
    h.state().viewer_item_rect(at).unwrap()
}

fn hover(h: &mut H, p: Pos2) {
    h.hover_at(p);
    h.step();
    h.step();
}

fn click(h: &mut H, p: Pos2) {
    hover(h, p);
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        h.step();
    }
    h.step();
    h.step();
    common::settle(h);
    common::run(h);
}

fn press_tool(h: &mut H, needle: &str, label: &str) {
    // (an image failing to load has another height from the frame it shows that)
    let r = image_rect(h, needle);
    hover(h, r.center());
    common::settle(h);
    common::run(h);
    let r = image_rect(h, needle);
    hover(h, r.center());
    let p = h.get_by_label(label).rect().center();
    click(h, p);
}

/// Replace…, then Browse… in the picker, where the stub answers.
fn replace_by_browsing(h: &mut H) {
    press_tool(h, "![", "Replace Image…");
    assert!(h.state().image_picker().is_some(), "the picker");
    h.get_by_label("Browse…").click();
    common::settle(h);
    common::run(h);
}

#[test]
fn replace_copies_by_the_rules_changes_only_the_path_and_one_undo_brings_it_back() {
    for (doc, want) in [
        (
            "a\n\n![Alt text](old.png \"Title\")\n\nb\n",
            "a\n\n![Alt text](assets/my%20pic%20%281%29.png \"Title\")\n\nb\n",
        ),
        (
            "a\r\n\r\n![Alt](old.png)\r\n\r\nb\r\n",
            "a\r\n\r\n![Alt](assets/my%20pic%20%281%29.png)\r\n\r\nb\r\n",
        ),
        (
            ":::x\n\n![Alt](old.png)\n\n:::\n",
            ":::x\n\n![Alt](assets/my%20pic%20%281%29.png)\n\n:::\n",
        ),
    ] {
        let (f, mut h) = fixture(doc);
        let away = f.root().join("away/my pic (1).png");
        png(&away);
        *f.pick.lock().unwrap() = Some(away);
        replace_by_browsing(&mut h);
        assert_eq!(text(&h), want);
        let copied = f.root().join("note/assets/my pic (1).png");
        assert!(copied.is_file(), "copied as a drop is");
        h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
        common::run(&mut h);
        assert_eq!(text(&h), doc, "exact");
        assert!(copied.is_file(), "undo does not delete the file");
    }
}

#[test]
fn a_remote_image_and_a_file_in_the_folder_too_and_no_choice_changes_nothing() {
    let doc = "![a](https://127.0.0.1:9/x.png)\n";
    let (f, mut h) = fixture(doc);
    *f.pick.lock().unwrap() = None;
    replace_by_browsing(&mut h);
    assert_eq!(text(&h), doc, "the dialog was cancelled");
    assert!(h.state().image_picker().is_some(), "and the picker is still there");
    *f.pick.lock().unwrap() = Some(f.root().join("note/old.png"));
    h.get_by_label("Browse…").click();
    common::settle(&mut h);
    common::run(&mut h);
    assert_eq!(
        text(&h),
        "![a](old.png)\n",
        "what is in the folder is linked where it is"
    );
}

#[test]
fn a_reference_image_offers_no_change_and_a_read_only_view_refuses() {
    let doc = "![a][r]\n\n[r]: old.png\n";
    let (_f, mut h) = fixture(doc);
    let r = image_rect(&h, "![");
    hover(&mut h, r.center());
    assert!(h.query_by_label("Replace Image…").is_none());

    let doc = "![a](old.png)\n\nb {>>note<<}\n";
    let (f, mut h) = fixture(doc);
    *f.pick.lock().unwrap() = Some(f.root().join("note/old.png"));
    h.state_mut().set_comment_view(CommentView::Final);
    common::run(&mut h);
    let r = image_rect(&h, "![");
    hover(&mut h, r.center());
    assert!(h.query_by_label("Replace Image…").is_none(), "no button");
    let at = 0..doc.find('\n').unwrap();
    h.state_mut().replace_image(at.clone());
    h.state_mut().open_image_edit(at);
    common::run(&mut h);
    assert!(!h.state().image_edit_is_open());
    assert_eq!(text(&h), doc);
}

#[test]
fn the_tools_have_labels_and_an_arrow_and_edit_changes_the_description_and_the_path() {
    let doc = "a\n\n![Old](old.png \"T\")\n\nb\n";
    let (_f, mut h) = fixture(doc);
    let r = image_rect(&h, "![");
    hover(&mut h, r.center());
    let replace = h.get_by_label("Replace Image…").rect();
    let expand = h.get_by_label("Open in full window").rect();
    assert!(replace.right() <= expand.left(), "next to the expand button");
    hover(&mut h, replace.center());
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Default);

    press_tool(&mut h, "![", "Edit Image…");
    assert!(h.state().image_edit_is_open());
    let ids = ["Description", "Path"];
    for (label, typed) in ids.into_iter().zip(["!", "x"]) {
        let field = h.get_by_role_and_label(egui::accesskit::Role::TextInput, label);
        field.click();
        h.step();
        h.step();
        h.get_by_role_and_label(egui::accesskit::Role::TextInput, label)
            .type_text(typed);
        h.step();
        h.step();
    }
    h.get_by_label("Save").click();
    common::run(&mut h);
    assert!(!h.state().image_edit_is_open());
    let got = text(&h);
    assert!(got.contains(" \"T\")"), "{got}");
    assert_ne!(got, doc);
    h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
    common::run(&mut h);
    assert_eq!(text(&h), doc, "one step");
}

#[test]
fn a_review_mode_after_the_window_opened_refuses_the_save_and_closes_it() {
    let doc = "![Old](old.png)\n\nb {>>note<<}\n";
    let (_f, mut h) = fixture(doc);
    h.state_mut().set_view_mode(markise_ui_egui::ViewMode::SideBySide);
    common::settle(&mut h);
    common::run(&mut h);
    press_tool(&mut h, "![", "Edit Image…");
    assert!(h.state().image_edit_is_open());
    h.state_mut().set_comment_view(CommentView::Final);
    common::run(&mut h);
    assert!(!h.state().image_edit_is_open());
    assert_eq!(text(&h), doc);
}

#[test]
fn a_copy_finishing_in_a_review_mode_replaces_nothing() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let doc = "![Old](old.png)\n\nb {>>note<<}\n";
    let (f, mut h) = fixture(doc);
    // in Side by Side a review mode leaves the session writable, by the source pane
    h.state_mut().set_view_mode(markise_ui_egui::ViewMode::SideBySide);
    common::settle(&mut h);
    common::run(&mut h);
    let away = f.root().join("away/x.png");
    png(&away);
    *f.pick.lock().unwrap() = Some(away);
    let hold = Arc::new(AtomicBool::new(true));
    let held = hold.clone();
    let job: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        while held.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });
    h.state_mut()
        .set_attach_job_test(std::time::Duration::from_secs(30), Some(job));
    let r = image_rect(&h, "![");
    hover(&mut h, r.center());
    let p = h.get_by_label("Replace Image…").rect().center();
    hover(&mut h, p);
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        h.step();
    }
    h.step();
    h.step();
    common::run(&mut h);
    h.get_by_label("Browse…").click();
    h.step();
    h.step();
    assert!(!h.state().background_settled(), "the copy is under way");
    h.state_mut().set_comment_view(CommentView::Final);
    h.step();
    hold.store(false, Ordering::SeqCst);
    common::settle(&mut h);
    common::run(&mut h);
    assert_eq!(text(&h), doc);
    assert!(f.root().join("note/assets/x.png").is_file(), "it had got that far");
}

#[test]
fn esc_closes_the_edit_window_and_changes_nothing() {
    let doc = "![Old](old.png)\n";
    let (_f, mut h) = fixture(doc);
    press_tool(&mut h, "![", "Edit Image…");
    assert!(h.state().image_edit_is_open());
    h.key_press(egui::Key::Escape);
    common::run(&mut h);
    assert!(!h.state().image_edit_is_open());
    assert_eq!(text(&h), doc);
}

mod picker {
    use super::*;
    use egui::Key;

    fn stocked(doc: &str, names: &[&str]) -> (Fx, H) {
        let (f, h) = fixture(doc);
        for n in names {
            let p = f.root().join("note").join(n);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            png(&p);
        }
        (f, h)
    }

    fn open(h: &mut H) {
        press_tool(h, "![", "Replace Image…");
        common::settle(h);
        common::run(h);
        assert!(h.state().image_picker().is_some());
    }

    fn names(h: &H) -> Vec<String> {
        let p = h.state().image_picker().unwrap();
        p.names().into_iter().map(str::to_string).collect()
    }

    fn key(h: &mut H, k: Key) {
        h.key_press(k);
        common::settle(h);
        common::run(h);
    }

    #[test]
    fn nearest_first_the_current_one_selected_and_the_keyboard_chooses_with_one_undo() {
        let doc = "a\n\n![Alt](old.png \"T\")\n\nb\n";
        let (f, mut h) = stocked(doc, &["z.png", "sub/a.png", "sub/deep/b.png"]);
        std::fs::write(f.root().join("note/note.txt"), b"x").unwrap();
        open(&mut h);
        assert_eq!(names(&h), ["old.png", "z.png", "sub/a.png", "sub/deep/b.png"]);
        assert_eq!(h.state().image_picker().unwrap().selected_name(), Some("old.png"));
        assert!(h.query_by_label("Image: old.png, the current one").is_some());
        assert!(h.query_by_label("Image: z.png").is_some());
        // a thumbnail on screen is drawn off the UI thread, and at rest it asks for no frame
        common::settle(&mut h);
        h.step();
        assert!(!h.state().image_picker().unwrap().is_busy());
        h.step();
        assert!(!h.ctx.has_requested_repaint(), "no frames at rest");
        key(&mut h, Key::ArrowRight);
        assert_eq!(h.state().image_picker().unwrap().selected_name(), Some("z.png"));
        key(&mut h, Key::Enter);
        assert!(h.state().image_picker().is_none());
        assert_eq!(text(&h), "a\n\n![Alt](z.png \"T\")\n\nb\n");
        h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
        common::run(&mut h);
        assert_eq!(text(&h), doc, "exact");
    }

    #[test]
    fn search_narrows_esc_closes_and_nothing_changes() {
        let doc = "![Alt](old.png)\n";
        let (_f, mut h) = stocked(doc, &["Red Car.png", "blue-car.png", "tree.png"]);
        open(&mut h);
        h.get_by_label("Search Images").type_text("CAR");
        common::settle(&mut h);
        common::run(&mut h);
        assert!(names(&h).iter().all(|n| n.to_lowercase().contains("car")));
        assert_eq!(names(&h).len(), 2);
        key(&mut h, Key::Escape);
        assert!(h.state().image_picker().is_none());
        assert_eq!(text(&h), doc);
    }

    #[test]
    fn a_broken_file_shows_as_such_and_another_can_be_chosen() {
        let doc = "![Alt](old.png)\n";
        let (f, mut h) = stocked(doc, &["good.png"]);
        std::fs::write(f.root().join("note/bad.png"), b"not an image").unwrap();
        open(&mut h);
        assert!(
            names(&h).contains(&"bad.png".to_string()),
            "listed, and it says so when shown"
        );
        common::settle(&mut h);
        h.step();
        h.get_by_label("Image: good.png").click();
        h.step();
        h.get_by_label("Use Image").click();
        common::settle(&mut h);
        common::run(&mut h);
        assert_eq!(text(&h), "![Alt](good.png)\n");
    }

    #[test]
    fn a_large_folder_shows_a_note_and_stays_bounded() {
        let doc = "![Alt](old.png)\n";
        let (f, mut h) = stocked(doc, &[]);
        // empty files: nothing has to be an image until it is on screen
        let many = f.root().join("note/many");
        std::fs::create_dir_all(&many).unwrap();
        for i in 0..2100 {
            std::fs::write(many.join(format!("i{i:04}.png")), b"").unwrap();
        }
        open(&mut h);
        assert_eq!(names(&h).len(), markise_engine::image_list::MAX_IMAGES);
        assert_eq!(names(&h)[0], "old.png", "the nearest first");
        let note = h.state().image_picker().unwrap().note();
        assert!(note.contains("Showing the nearest 2000"), "{note}");
        key(&mut h, Key::Escape);
    }

    #[test]
    fn a_file_dropped_on_the_window_is_copied_and_comes_first() {
        let doc = "![Alt](old.png)\n";
        let (f, mut h) = stocked(doc, &[]);
        let away = f.root().join("away/dropped pic.png");
        png(&away);
        open(&mut h);
        h.input_mut()
            .dropped_files
            .push(Arc::new(crate::images_paste::Dropped(away)));
        common::settle(&mut h);
        common::run(&mut h);
        assert_eq!(text(&h), doc, "the drop is the window's, not the document's");
        assert!(f.root().join("note/assets/dropped pic.png").is_file());
        assert_eq!(names(&h)[0], "assets/dropped pic.png");
        key(&mut h, Key::Enter);
        assert_eq!(text(&h), "![Alt](assets/dropped%20pic.png)\n");
    }

    #[test]
    fn an_unsaved_document_goes_straight_to_the_dialog() {
        let dir = tempfile::tempdir().unwrap();
        let away = dir.path().join("x.png");
        png(&away);
        let picked = away.clone();
        let opts = AppOptions {
            config: ConfigDir::Dir(dir.path().to_path_buf()),
            block_image_picker: Some(Arc::new(move |_: &Path| Some(picked.clone()))),
            ..common::no_watch()
        };
        let mut h: H = Harness::builder()
            .with_size(egui::vec2(1100.0, 900.0))
            .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), MarkiseApp::new(opts));
        h.state_mut().new_document();
        common::settle(&mut h);
        common::run(&mut h);
        common::settle(&mut h);
        common::run(&mut h);
        let v = h.state_mut().viewer().unwrap();
        v.ed.edit(|s| s.insert_text("![a](old.png)")).unwrap();
        common::run(&mut h);
        h.state_mut().replace_image(0.."![a](old.png)".len());
        assert!(h.state().image_picker().is_none());
        common::settle(&mut h);
        common::run(&mut h);
        let t = text(&h);
        assert!(t.starts_with("![a](") && t.contains("x.png"), "{t}");
    }
}

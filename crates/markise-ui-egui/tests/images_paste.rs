//! Pasting and dropping images and files (issue #51; spec 04 §7.1–§7.3, spec 05 §4.4, spec 06
//! §14 V14.2 / V14.4) in the real app: clipboard image data becomes a PNG in the image folder
//! and an image line at the caret (one undo step; undo keeps the file), every folder setting
//! including Obsidian's `attachmentFolderPath`, collision names, a never-saved document,
//! dropped image and non-image files at the drop point, a file manager copy pasted through the
//! clipboard seam, image URLs, the size limit and the settings in Preferences.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::accesskit::Toggled;
use egui::{Color32, ColorImage, Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use markise_engine::settings::{ImageLocation, ImageSettings};
use markise_ui_egui::clipboard::{ClipboardFiles, Scripted};
use markise_ui_egui::menu::MenuAction;
use markise_ui_egui::prefs::{Change, Section};
use markise_ui_egui::{AppOptions, ConfigDir, MarkiseApp};

type H = Harness<'static, MarkiseApp>;

const STAMP: &str = "20261029101500";
const PASTED: &str = "Pasted image 20261029101500.png";
const PASTED_ENC: &str = "Pasted%20image%2020261029101500.png";

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-images-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn cmd() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers {
            mac_cmd: true,
            command: true,
            ..Default::default()
        }
    } else {
        Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        }
    }
}

fn harness_with(opts: AppOptions) -> H {
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), MarkiseApp::new(opts));
    h.state_mut().set_paste_stamp(Some(STAMP.into()));
    h
}

fn wait_until(h: &mut H, what: &str, f: impl Fn(&MarkiseApp) -> bool) {
    let t = Instant::now();
    while !f(h.state()) {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(2));
    }
    h.step();
}

/// Opens `open` (files / folders) and waits until `doc` is shown; with a folder, also for its
/// vault index.
fn open(open: Vec<PathBuf>, doc: &Path, opts: AppOptions) -> H {
    let folder = open.iter().any(|p| p.is_dir());
    let mut h = harness_with(AppOptions { open, ..opts });
    let doc = doc.to_path_buf();
    wait_until(&mut h, "document", move |a| {
        a.document_path() == Some(doc.as_path()) && !a.is_busy() && (!folder || a.vault_index().is_some())
    });
    h
}

/// Clicks into the document (focus) and puts the caret at the document end.
fn focus_end(h: &mut H) {
    let p = egui::pos2(600.0, 200.0);
    h.hover_at(p);
    h.drag_at(p);
    h.step();
    h.drop_at(p);
    h.step();
    if cfg!(target_os = "macos") {
        h.key_press_modifiers(cmd(), Key::ArrowDown);
    } else {
        h.key_press_modifiers(cmd(), Key::End);
    }
    h.step();
}

fn text(h: &H) -> String {
    h.state().document_text().unwrap()
}

fn image() -> Arc<ColorImage> {
    Arc::new(ColorImage::new([3, 2], vec![Color32::from_rgb(200, 30, 40); 6]))
}

/// ⌘V with image data on the clipboard: egui's backend delivers it as `PasteImage`.
fn paste_image(h: &mut H) {
    h.event(Event::PasteImage(image()));
    h.step();
    wait_until(h, "attachment written", |a| !a.is_attaching());
}

fn is_png(p: &Path) -> bool {
    std::fs::read(p).is_ok_and(|b| b.starts_with(b"\x89PNG\r\n\x1a\n"))
}

/// A file dropped on the window (what egui-winit delivers).
#[derive(Debug)]
struct Dropped(PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

fn drop_files(h: &mut H, at: egui::Pos2, files: &[PathBuf]) {
    h.hover_at(at);
    h.step();
    for f in files {
        h.input_mut().dropped_files.push(Arc::new(Dropped(f.clone())));
    }
    h.step();
    wait_until(h, "drop handled", |a| !a.is_attaching());
}

fn set_images(h: &mut H, s: ImageSettings) {
    let ctx = h.ctx.clone();
    h.state_mut().apply_preference(&ctx, Change::Images(s));
    h.step();
}

const NOTE: &str = "# Note\n\nFirst paragraph.\n\nSecond paragraph.\n";

#[test]
fn pasted_screenshot_goes_to_assets_and_undo_keeps_the_file() {
    let d = tmpdir("paste");
    let note = d.join("note.md");
    std::fs::write(&note, NOTE).unwrap();
    let mut h = open(vec![note.clone()], &note, AppOptions::default());
    focus_end(&mut h);
    paste_image(&mut h);
    let png = d.join("assets").join(PASTED);
    assert!(is_png(&png), "{} written as PNG", png.display());
    let t = text(&h);
    assert!(t.contains(&format!("![](assets/{PASTED_ENC})")), "{t}");
    assert!(h.state().is_dirty());
    // the same second again: a collision suffix (V14.4)
    paste_image(&mut h);
    assert!(is_png(&d.join("assets/Pasted image 20261029101500 1.png")));
    assert!(text(&h).contains("assets/Pasted%20image%2020261029101500%201.png"));
    // each paste is one undo step; undo removes the text but not the file
    h.key_press_modifiers(cmd(), Key::Z);
    h.step();
    h.key_press_modifiers(cmd(), Key::Z);
    h.step();
    assert_eq!(text(&h), NOTE);
    assert!(png.is_file(), "undo does not delete the written file");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn folder_settings_root_same_folder_and_absolute_paths() {
    let d = tmpdir("folders");
    std::fs::create_dir_all(d.join("Notes")).unwrap();
    let note = d.join("Notes/n.md");
    std::fs::write(&note, NOTE).unwrap();
    let mut h = open(vec![d.clone(), note.clone()], &note, AppOptions::default());
    focus_end(&mut h);
    // a fixed folder below the opened folder
    set_images(
        &mut h,
        ImageSettings {
            location: ImageLocation::RootFolder,
            folder: "media/pics".into(),
            ..Default::default()
        },
    );
    paste_image(&mut h);
    assert!(is_png(&d.join("media/pics").join(PASTED)));
    assert!(
        text(&h).contains(&format!("![](../media/pics/{PASTED_ENC})")),
        "{}",
        text(&h)
    );
    // the note's own folder
    set_images(
        &mut h,
        ImageSettings {
            location: ImageLocation::SameFolder,
            ..Default::default()
        },
    );
    paste_image(&mut h);
    assert!(is_png(&d.join("Notes").join(PASTED)));
    assert!(text(&h).contains(&format!("![]({PASTED_ENC})")));
    // a configurable folder name next to the note, absolute paths
    set_images(
        &mut h,
        ImageSettings {
            folder: "img".into(),
            relative_paths: false,
            ..Default::default()
        },
    );
    paste_image(&mut h);
    let abs = d.join("Notes/img").join(PASTED);
    assert!(is_png(&abs));
    let want = markise_engine::attach::destination(None, None, &abs, false);
    assert!(
        want.ends_with("/Notes/img/Pasted%20image%2020261029101500.png"),
        "{want}"
    );
    assert!(text(&h).contains(&format!("![]({want})")), "{}", text(&h));
    let _ = std::fs::remove_dir_all(&d);
}

/// Spec 06 V14.2: an Obsidian vault's `attachmentFolderPath` wins while the setting is on
/// (read only: `.obsidian/` is never written).
#[test]
fn obsidian_attachment_folder_is_followed() {
    let d = tmpdir("obsidian");
    std::fs::create_dir_all(d.join(".obsidian")).unwrap();
    std::fs::write(
        d.join(".obsidian/app.json"),
        r#"{"attachmentFolderPath": "Files/Attachments"}"#,
    )
    .unwrap();
    std::fs::create_dir_all(d.join("Daily")).unwrap();
    let note = d.join("Daily/today.md");
    std::fs::write(&note, NOTE).unwrap();
    let mut h = open(vec![d.clone(), note.clone()], &note, AppOptions::default());
    wait_until(&mut h, "vault settings", |a| {
        a.vault_index()
            .is_some_and(|ix| ix.settings().attachment_folder.is_some())
    });
    focus_end(&mut h);
    paste_image(&mut h);
    assert!(is_png(&d.join("Files/Attachments").join(PASTED)));
    assert!(text(&h).contains(&format!("![](../Files/Attachments/{PASTED_ENC})")));
    assert!(!d.join("Daily/assets").exists());
    let listed: Vec<_> = std::fs::read_dir(d.join(".obsidian")).unwrap().flatten().collect();
    assert_eq!(listed.len(), 1, ".obsidian untouched");
    // switched off: Markise's own location
    set_images(
        &mut h,
        ImageSettings {
            obsidian: false,
            ..Default::default()
        },
    );
    paste_image(&mut h);
    assert!(is_png(&d.join("Daily/assets").join(PASTED)));
    let _ = std::fs::remove_dir_all(&d);
}

/// A never-saved document: pasted images go to the app-data folder for user images, linked
/// with an absolute path; without a settings folder the user is asked to save first.
#[test]
fn untitled_document_pastes_into_user_images() {
    let d = tmpdir("untitled");
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    let mut h = harness_with(AppOptions {
        config: ConfigDir::Dir(cfg.clone()),
        ..Default::default()
    });
    h.run_steps(3);
    h.state_mut().new_document();
    wait_until(&mut h, "untitled", |a| a.is_untitled() && !a.is_busy());
    focus_end(&mut h);
    paste_image(&mut h);
    let png = cfg.join("markise/user-images").join(PASTED);
    assert!(is_png(&png), "{}", png.display());
    let want = markise_engine::attach::destination(None, None, &png, false);
    assert!(!want.contains('\\') && !want.contains("%3F"), "{want}");
    assert_eq!(text(&h).trim_end(), format!("![]({want})"));

    // no settings folder: nothing written, a message instead
    let mut h = harness_with(AppOptions::default());
    h.run_steps(3);
    h.state_mut().new_document();
    wait_until(&mut h, "untitled", |a| a.is_untitled() && !a.is_busy());
    focus_end(&mut h);
    paste_image(&mut h);
    assert!(text(&h).trim().is_empty());
    assert!(
        h.state()
            .banner_text()
            .is_some_and(|b| b.contains("Save the document first"))
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// Dropped files go in at the drop point: images copied into the image folder, other files
/// linked where they are (nothing copied).
#[test]
fn dropped_files_insert_at_the_drop_point() {
    let d = tmpdir("drop");
    let src = tmpdir("drop-src");
    std::fs::write(src.join("photo one.jpg"), b"not really a jpeg").unwrap();
    std::fs::write(src.join("report.pdf"), b"%PDF").unwrap();
    let note = d.join("note.md");
    std::fs::write(&note, NOTE).unwrap();
    let mut h = open(vec![note.clone()], &note, AppOptions::default());
    focus_end(&mut h);
    let first = NOTE.find("First").unwrap();
    let r = h.state().viewer_item_rect(first).expect("first paragraph visible");
    let at = egui::pos2(r.left() + 2.0, r.center().y);
    drop_files(&mut h, at, &[src.join("photo one.jpg"), src.join("report.pdf")]);
    let t = text(&h);
    let pdf = markise_engine::attach::destination(Some(&d), None, &src.join("report.pdf"), true);
    let want = format!("![](assets/photo%20one.jpg)\n[report.pdf]({pdf})");
    let pos = t.find(&want).unwrap_or_else(|| panic!("{t}"));
    assert!(pos < t.find("Second").unwrap(), "inserted in the first paragraph: {t}");
    assert_eq!(
        std::fs::read(d.join("assets/photo one.jpg")).unwrap(),
        b"not really a jpeg"
    );
    assert!(
        !d.join("assets/report.pdf").exists(),
        "non-image files are linked, not copied"
    );
    // one undo step for the whole drop
    h.key_press_modifiers(cmd(), Key::Z);
    h.step();
    assert_eq!(text(&h), NOTE);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&src);
}

/// A file manager copy (Finder ⌘C) pasted: the clipboard seam's file list wins over the
/// file-name text egui delivers.
#[test]
fn pasted_file_manager_copy_is_like_a_drop() {
    let d = tmpdir("finder");
    let src = tmpdir("finder-src");
    std::fs::write(src.join("chart.png"), b"png-ish").unwrap();
    let note = d.join("note.md");
    std::fs::write(&note, NOTE).unwrap();
    let clip = Arc::new(Scripted::default());
    let mut h = open(
        vec![note.clone()],
        &note,
        AppOptions {
            clipboard_files: clip.clone() as Arc<dyn ClipboardFiles>,
            ..Default::default()
        },
    );
    focus_end(&mut h);
    clip.set(vec![src.join("chart.png")]);
    h.event(Event::Paste("chart.png".into()));
    h.step();
    wait_until(&mut h, "copied", |a| !a.is_attaching());
    assert!(text(&h).contains("![](assets/chart.png)"), "{}", text(&h));
    assert!(!text(&h).contains("paragraph.chart.png"));
    assert!(d.join("assets/chart.png").is_file());
    // an ordinary text paste stays text
    clip.set(Vec::new());
    h.event(Event::Paste(" plain".into()));
    h.step();
    wait_until(&mut h, "text paste", |a| !a.is_attaching());
    // (the caret after an image at a paragraph end sits on the next caret stop)
    assert!(
        text(&h).contains(" plain") && text(&h).matches("chart.png").count() == 1,
        "{:?}",
        text(&h)
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn pasted_image_url_becomes_an_image_and_large_files_are_refused() {
    let d = tmpdir("url");
    let note = d.join("note.md");
    std::fs::write(&note, NOTE).unwrap();
    let mut h = open(vec![note.clone()], &note, AppOptions::default());
    focus_end(&mut h);
    h.event(Event::Paste("https://example.com/pics/cat.png".into()));
    h.step();
    assert!(
        text(&h).contains("![](https://example.com/pics/cat.png)"),
        "{}",
        text(&h)
    );
    // off: the URL is plain text
    set_images(
        &mut h,
        ImageSettings {
            paste_urls: false,
            ..Default::default()
        },
    );
    h.event(Event::Paste(" https://example.com/dog.png".into()));
    h.step();
    assert!(text(&h).contains(" https://example.com/dog.png") && !text(&h).contains("![](https://example.com/dog"));
    // above 50 MB: refused with a message, nothing copied
    let src = tmpdir("url-big");
    let big = src.join("huge.png");
    std::fs::File::create(&big)
        .unwrap()
        .set_len(markise_engine::attach::MAX_BYTES + 1)
        .unwrap();
    let before = text(&h);
    let p = egui::pos2(600.0, 200.0);
    drop_files(&mut h, p, &[big]);
    assert_eq!(text(&h), before);
    assert!(
        h.state().banner_text().is_some_and(|b| b.contains("too large")),
        "{:?}",
        h.state().banner_text()
    );
    assert!(!d.join("assets/huge.png").exists());
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&src);
}

fn open_files_section(h: &mut H) {
    let ctx = h.ctx.clone();
    assert!(h.state_mut().menu_action(&ctx, &MenuAction::Command("app.preferences")));
    h.run_steps(3);
    h.get_by_role_and_label(egui::Role::Button, Section::Files.label())
        .click();
    h.run_steps(2);
}

fn toggled(h: &H, label: &str) -> bool {
    h.get_by_role_and_label(egui::Role::Switch, label)
        .accesskit_node()
        .toggled()
        == Some(Toggled::True)
}

/// Preferences → Files & Images: the image settings are shown, change the app and are written
/// to settings.json; Reset to Defaults covers them.
#[test]
fn image_settings_in_preferences() {
    let d = tmpdir("prefs");
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    let mut h = harness_with(AppOptions {
        config: ConfigDir::Dir(cfg.clone()),
        ..Default::default()
    });
    h.run_steps(3);
    open_files_section(&mut h);
    for l in [
        "Save pasted and dropped images to",
        "Image folder",
        "Use the Obsidian attachment folder when set",
        "Use relative paths",
        "Copy dropped image files into the image folder",
        "Copy other dropped files too",
        "Paste image URLs as images",
    ] {
        assert!(h.query_all_by_label(l).count() >= 1, "{l}");
    }
    assert!(toggled(&h, "Use relative paths"));
    assert!(!toggled(&h, "Copy other dropped files too"));
    h.get_by_role_and_label(egui::Role::Switch, "Use relative paths")
        .focus();
    h.run_steps(1);
    h.key_press(Key::Space);
    h.run_steps(2);
    assert!(!h.state().preference_values().images.relative_paths);
    h.get_by_role_and_label(egui::Role::Switch, "Copy other dropped files too")
        .focus();
    h.run_steps(1);
    h.key_press(Key::Space);
    h.run_steps(2);
    assert!(h.state().preference_values().images.copy_files);
    // the folder name field
    h.get_by_role_and_label(egui::Role::TextInput, "Image folder").focus();
    h.run_steps(1);
    h.key_press_modifiers(cmd(), Key::A);
    h.get_by_role_and_label(egui::Role::TextInput, "Image folder")
        .type_text("media");
    h.run_steps(2);
    assert_eq!(h.state().preference_values().images.folder, "media");
    let settings = cfg.join("markise/settings.json");
    wait_until(&mut h, "settings written", move |_| {
        std::fs::read_to_string(&settings)
            .is_ok_and(|s| s.contains("\"relative_paths\": false") && s.contains("\"folder\": \"media\""))
    });
    // search finds them
    h.state_mut().preferences().query = "obsidian".into();
    h.run_steps(2);
    assert!(
        h.query_by_label("Use the Obsidian attachment folder when set")
            .is_some()
    );
    h.state_mut().preferences().query.clear();
    h.run_steps(2);
    // Reset to Defaults (the confirmation answered directly)
    h.state_mut().reset_preferences(Section::Files);
    h.run_steps(2);
    assert_eq!(h.state().preference_values().images, ImageSettings::default());
    let _ = std::fs::remove_dir_all(&d);
}

/// A clipboard seam that blocks (a slow clipboard owner) and counts its calls.
struct Slow {
    files: Vec<PathBuf>,
    delay: Duration,
    calls: std::sync::atomic::AtomicUsize,
}

impl ClipboardFiles for Slow {
    fn files(&self) -> Vec<PathBuf> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        std::thread::sleep(self.delay);
        self.files.clone()
    }
}

fn slow_app(name: &str, files: Vec<PathBuf>, delay: Duration) -> (H, Arc<Slow>, PathBuf, PathBuf) {
    let d = tmpdir(name);
    let note = d.join("note.md");
    std::fs::write(&note, NOTE).unwrap();
    let slow = Arc::new(Slow {
        files,
        delay,
        calls: Default::default(),
    });
    let mut h = open(
        vec![note.clone()],
        &note,
        AppOptions {
            clipboard_files: slow.clone() as Arc<dyn ClipboardFiles>,
            ..Default::default()
        },
    );
    focus_end(&mut h);
    (h, slow, d, note)
}

/// Codex P1: the clipboard's file list is read on a worker; frames keep running while a slow
/// clipboard owner answers, and the paste is applied once, at the position it was made at.
#[test]
fn slow_clipboard_does_not_block_and_applies_once() {
    let src = tmpdir("slow-src");
    std::fs::write(src.join("pic.png"), b"png").unwrap();
    let (mut h, slow, d, _) = slow_app("slow-applies", vec![src.join("pic.png")], Duration::from_millis(400));
    let caret = h
        .state_mut()
        .viewer()
        .unwrap()
        .ed
        .session
        .selection()
        .primary()
        .focus
        .offset;
    h.event(Event::Paste("pic.png".into()));
    let t = Instant::now();
    h.step();
    assert!(
        t.elapsed() < Duration::from_millis(200),
        "the paste frame did not wait for the clipboard"
    );
    assert!(h.state().is_attaching());
    // the window stays responsive meanwhile (and nothing is inserted yet)
    for _ in 0..5 {
        let t = Instant::now();
        h.step();
        assert!(t.elapsed() < Duration::from_millis(200));
    }
    assert_eq!(text(&h), NOTE);
    wait_until(&mut h, "paste applied", |a| !a.is_attaching());
    h.run_steps(3);
    let t = text(&h);
    assert_eq!(t.matches("![](assets/pic.png)").count(), 1, "{t}");
    assert_eq!(t.find("![](assets/pic.png)"), Some(caret), "{t}");
    assert_eq!(slow.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(d.join("assets/pic.png").is_file());
    // one undo step
    h.key_press_modifiers(cmd(), Key::Z);
    h.step();
    assert_eq!(text(&h), NOTE);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&src);
}

/// Without files on the clipboard the original text goes in (one step); typing while the
/// clipboard is read cancels the paste instead of inserting at a stale position.
#[test]
fn slow_clipboard_text_fallback_and_cancel_on_edit() {
    let (mut h, slow, d, _) = slow_app("slow-fallback", Vec::new(), Duration::from_millis(150));
    h.event(Event::Paste(" pasted".into()));
    h.step();
    assert_eq!(text(&h), NOTE, "waits for the file list");
    wait_until(&mut h, "text paste", |a| !a.is_attaching());
    assert_eq!(text(&h), NOTE.replace("Second paragraph.", "Second paragraph. pasted"));
    h.key_press_modifiers(cmd(), Key::Z);
    h.step();
    assert_eq!(text(&h), NOTE);
    // an edit while the clipboard is read: cancelled, with a message
    h.event(Event::Paste(" late".into()));
    h.step();
    h.event(Event::Text("X".into()));
    h.step();
    wait_until(&mut h, "paste resolved", |a| !a.is_attaching());
    assert!(!text(&h).contains("late"), "{}", text(&h));
    assert!(text(&h).contains("Second paragraph.X"));
    assert!(h.state().banner_text().is_some_and(|b| b.contains("cancelled")));
    assert_eq!(slow.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    let _ = std::fs::remove_dir_all(&d);
}

/// Codex P2: two pastes made while the clipboard is still being read both go in, in order
/// (the second is re-based through the first's insertion, not cancelled).
#[test]
fn queued_pastes_apply_in_order() {
    let (mut h, slow, d, _) = slow_app("slow-queued", Vec::new(), Duration::from_millis(150));
    h.event(Event::Paste(" one".into()));
    h.step();
    h.event(Event::Paste(" two".into()));
    h.step();
    assert_eq!(text(&h), NOTE);
    wait_until(&mut h, "both pastes", |a| !a.is_attaching());
    assert_eq!(text(&h), NOTE.replace("Second paragraph.", "Second paragraph. one two"));
    assert!(h.state().banner_text().is_none(), "{:?}", h.state().banner_text());
    assert_eq!(slow.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    // two undo steps
    h.key_press_modifiers(cmd(), Key::Z);
    h.step();
    assert_eq!(text(&h), NOTE.replace("Second paragraph.", "Second paragraph. one"));
    let _ = std::fs::remove_dir_all(&d);
}

/// Codex P2: with absolute paths and a `typora-root-url`, a file below the root is written
/// root-relative and one elsewhere as a `file:` URL; both resolve to the written files.
#[test]
fn absolute_paths_with_a_resource_root() {
    let d = tmpdir("rooturl");
    std::fs::create_dir_all(d.join("site/docs")).unwrap();
    let note = d.join("site/docs/n.md");
    std::fs::write(&note, format!("---\ntypora-root-url: ..\n---\n\n{NOTE}")).unwrap();
    let mut h = open(vec![note.clone()], &note, AppOptions::default());
    focus_end(&mut h);
    set_images(
        &mut h,
        ImageSettings {
            relative_paths: false,
            ..Default::default()
        },
    );
    paste_image(&mut h);
    let png = d.join("site/docs/assets").join(PASTED);
    assert!(is_png(&png));
    let t = text(&h);
    let start = t.find("![](").unwrap() + 4;
    let dest = &t[start..start + t[start..].find(')').unwrap()];
    assert_eq!(
        dest,
        format!("/docs/assets/{PASTED_ENC}"),
        "root-relative below the root"
    );
    let docs = PathBuf::from(markise_engine::attach::simplify_verbatim(
        &d.join("site").join("docs").to_string_lossy(),
    ));
    let got = markise_doc::resource::local_file(&docs, Some(".."), dest).unwrap();
    assert_eq!(
        std::fs::canonicalize(got.path).unwrap(),
        std::fs::canonicalize(&png).unwrap(),
        "{dest}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// Codex P2 (round 3): one ordered queue for every kind. A text paste whose clipboard lookup
/// is slow and an image pasted right after it go in in paste order, and the other way round.
#[test]
fn mixed_pastes_keep_their_order() {
    // text, then image data
    let (mut h, _slow, d, _) = slow_app("slow-mixed1", Vec::new(), Duration::from_millis(200));
    h.event(Event::Paste(" one".into()));
    h.step();
    h.event(Event::PasteImage(image()));
    h.step();
    wait_until(&mut h, "both", |a| !a.is_attaching());
    let want = NOTE.replace(
        "Second paragraph.",
        &format!("Second paragraph. one![](assets/{PASTED_ENC})"),
    );
    assert_eq!(text(&h), want);
    assert!(is_png(&d.join("assets").join(PASTED)));
    let _ = std::fs::remove_dir_all(&d);

    // image data, then text
    let (mut h, _slow, d, _) = slow_app("slow-mixed2", vec![], Duration::from_millis(200));
    h.event(Event::PasteImage(image()));
    h.step();
    h.event(Event::Paste(" two".into()));
    h.step();
    wait_until(&mut h, "both", |a| !a.is_attaching());
    let want = NOTE.replace(
        "Second paragraph.",
        &format!("Second paragraph.![](assets/{PASTED_ENC}) two"),
    );
    assert_eq!(text(&h), want);
    // each is its own undo step: the later one goes first
    h.key_press_modifiers(cmd(), Key::Z);
    h.step();
    assert_eq!(
        text(&h),
        NOTE.replace(
            "Second paragraph.",
            &format!("Second paragraph.![](assets/{PASTED_ENC})")
        )
    );
    let _ = std::fs::remove_dir_all(&d);
}

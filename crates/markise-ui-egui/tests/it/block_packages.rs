//! Block packages in the app (ADR 0019): the question, the status bar item, and that documents
//! without a binding show nothing new.

use std::fs;
use std::path::{Path, PathBuf};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_package::{Sha256Hex, Store};
use markise_ui_egui::package_ui::{PackageAction, PackageStatus};
use markise_ui_egui::{AppOptions, ConfigDir, MarkiseApp};

use crate::common::{self, settle, wait_until};

type H = Harness<'static, MarkiseApp>;

const USE: &str = "Use Package";
const NOT_NOW: &str = "Not Now";

/// A folder with a document and a small neutral package, and a separate config folder.
struct Fixture {
    _dir: tempfile::TempDir,
    config: PathBuf,
    folder: PathBuf,
    doc: PathBuf,
}

fn archive(version: &str) -> Vec<u8> {
    let manifest =
        format!(r#"{{"format":"markise-block-package/2","library":"sample","version":"{version}","blocks":["tip"]}}"#);
    let files = [
        ("manifest.json", manifest.as_str()),
        (
            "components/tip/tip.json",
            r#"{"label":"Tip","fields":[{"name":"body","type":"text"}]}"#,
        ),
        ("components/tip/tip.js", "not run"),
        ("editor/palette.json", r#"{"format":1}"#),
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
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        // `open` canonicalizes (the temp folder is behind a link on macOS)
        let root = fs::canonicalize(dir.path()).unwrap();
        let config = root.join("config");
        let folder = root.join("content");
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(&folder).unwrap();
        let doc = folder.join("doc.md");
        fs::write(&doc, "# Doc\n\n:::tip\n\nHello\n\n:::\n").unwrap();
        Fixture {
            config,
            folder,
            doc,
            _dir: dir,
        }
    }

    fn bind(&self, version: &str) -> Sha256Hex {
        let bytes = archive(version);
        let sha = Sha256Hex::of(&bytes);
        fs::write(self.folder.join("p.tar.gz"), &bytes).unwrap();
        fs::write(
            self.folder.join(markise_package::BINDING_FILE),
            format!(r#"{{"library":"sample","version":"{version}","source":"p.tar.gz","sha256":"{sha}"}}"#),
        )
        .unwrap();
        sha
    }

    fn store(&self) -> Store {
        Store::in_config(&self.config)
    }

    fn harness(&self) -> H {
        let opts = AppOptions {
            config: ConfigDir::Dir(self.config.clone()),
            ..common::no_watch()
        };
        Harness::builder()
            .with_size(egui::vec2(1000.0, 700.0))
            .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), MarkiseApp::new(opts))
    }

    fn open(&self, h: &mut H) {
        open(h, &self.doc);
    }
}

fn open(h: &mut H, doc: &Path) {
    h.state_mut().open(doc.to_path_buf());
    wait_until(h, "document load", |a| {
        a.document_path() == Some(doc) && !a.is_loading()
    });
    settle(h);
    h.run();
}

fn has(h: &H, text: &str) -> bool {
    h.query_by_label_contains(text).is_some()
}

#[test]
fn an_unbound_document_shows_nothing_and_writes_nothing() {
    let f = Fixture::new();
    let mut h = f.harness();
    f.open(&mut h);
    assert!(matches!(h.state().block_package_status(), PackageStatus::None));
    assert!(!has(&h, "Blocks:"));
    assert!(!has(&h, USE));
    assert!(!f.store().root().exists());
}

#[test]
fn no_config_folder_means_no_lookup() {
    let f = Fixture::new();
    f.bind("1.0.0");
    let mut h = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_ui_state(
        |ui, app: &mut MarkiseApp| app.ui(ui),
        MarkiseApp::new(common::no_watch()),
    );
    open(&mut h, &f.doc);
    assert!(matches!(h.state().block_package_status(), PackageStatus::None));
    assert!(!has(&h, USE));
}

#[test]
fn asks_once_then_uses_the_package_on_the_next_launch() {
    let f = Fixture::new();
    let sha = f.bind("1.0.0");
    let mut h = f.harness();
    f.open(&mut h);
    assert!(has(&h, "Use the block package sample 1.0.0?"));
    assert!(has(&h, "Blocks: sample 1.0.0 · not in use"));
    assert!(!f.store().root().exists(), "nothing is written before the answer");
    assert!(h.state().block_package().is_none());

    h.get_by_label(USE).click();
    h.step();
    wait_until(&mut h, "the install", |a| a.block_package().is_some());
    settle(&mut h);
    h.run();
    assert!(!has(&h, USE));
    assert!(has(&h, "Blocks: sample 1.0.0"));
    assert!(!has(&h, "not in use"));
    assert!(f.store().is_accepted("sample", &sha));
    let package = h.state().block_package().unwrap();
    assert_eq!(package.index.block_count(), 1);
    assert!(package.asset_path("components/tip/tip.js").unwrap().is_file());
    drop(h);

    let mut h = f.harness();
    f.open(&mut h);
    assert!(h.state().block_package().is_some(), "no second question");
    assert!(!has(&h, USE));
}

#[test]
fn a_changed_hash_asks_again() {
    let f = Fixture::new();
    f.bind("1.0.0");
    let mut h = f.harness();
    f.open(&mut h);
    h.get_by_label(USE).click();
    h.step();
    wait_until(&mut h, "the install", |a| a.block_package().is_some());
    drop(h);

    f.bind("2.0.0");
    let mut h = f.harness();
    f.open(&mut h);
    assert!(h.state().block_package().is_none());
    assert!(has(&h, "Use the block package sample 2.0.0?"));
    assert!(has(
        &h,
        "It differs from the package you accepted before, sample 1.0.0."
    ));
}

#[test]
fn not_now_holds_for_the_session_and_the_status_bar_offers_it_again() {
    let f = Fixture::new();
    f.bind("1.0.0");
    let mut h = f.harness();
    f.open(&mut h);
    h.get_by_label(NOT_NOW).click();
    h.run();
    assert!(!has(&h, USE));
    assert!(has(&h, "Blocks: sample 1.0.0 · not in use"));
    assert!(!f.store().root().exists());

    // another document of the folder: no second card
    let other = f.folder.join("other.md");
    fs::write(&other, "text\n").unwrap();
    open(&mut h, &other);
    assert!(matches!(h.state().block_package_status(), PackageStatus::NotInUse(_)));
    assert!(!has(&h, USE));

    h.state_mut().block_package_action(PackageAction::Review);
    h.run();
    assert!(has(&h, USE));
}

#[test]
fn stop_using_withdraws_the_consent() {
    let f = Fixture::new();
    let sha = f.bind("1.0.0");
    let mut h = f.harness();
    f.open(&mut h);
    h.state_mut().block_package_action(PackageAction::Accept);
    wait_until(&mut h, "the install", |a| a.block_package().is_some());
    h.state_mut().block_package_action(PackageAction::Forget);
    wait_until(&mut h, "the question", |a| {
        matches!(a.block_package_status(), PackageStatus::NotInUse(_))
    });
    assert!(!f.store().is_accepted("sample", &sha));
    assert!(!f.store().files_dir(&sha).exists());
    assert!(!has(&h, USE), "no card right after stopping");
}

#[test]
fn problems_show_in_the_status_bar_and_leave_the_document_alone() {
    let f = Fixture::new();
    let sha = f.bind("1.0.0");
    fs::write(f.folder.join("p.tar.gz"), b"something else").unwrap();
    let mut h = f.harness();
    f.open(&mut h);
    assert!(h.state().model().is_some(), "the document is open");
    assert!(!has(&h, USE));
    let item = h.get_by_label("Block package problem");
    item.click();
    h.run();
    assert!(has(&h, "not the one the binding names"), "the menu says why");
    assert!(has(&h, sha.as_str()));
    assert!(!f.store().root().exists());

    // a future format
    let mut h = f.harness();
    let manifest = r#"{"format":"markise-block-package/99"}"#;
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    let mut header = tar::Header::new_gnu();
    header.set_size(manifest.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append_data(&mut header, "manifest.json", manifest.as_bytes())
        .unwrap();
    let bytes = tar.into_inner().unwrap().finish().unwrap();
    fs::write(f.folder.join("p.tar.gz"), &bytes).unwrap();
    fs::write(
        f.folder.join(markise_package::BINDING_FILE),
        format!(
            r#"{{"library":"sample","version":"1.0.0","source":"p.tar.gz","sha256":"{}"}}"#,
            Sha256Hex::of(&bytes)
        ),
    )
    .unwrap();
    f.open(&mut h);
    h.get_by_label("Block package problem").click();
    h.run();
    assert!(has(&h, "not supported by this version of Markise"));
}

#[test]
fn opening_another_document_follows_its_file() {
    let f = Fixture::new();
    f.bind("1.0.0");
    let mut h = f.harness();
    f.open(&mut h);
    assert!(has(&h, USE));

    let away = f.folder.parent().unwrap().join("away");
    fs::create_dir_all(&away).unwrap();
    let other = away.join("other.md");
    fs::write(&other, "text\n").unwrap();
    open(&mut h, &other);
    assert!(matches!(h.state().block_package_status(), PackageStatus::None));
    assert!(!has(&h, USE));
    assert!(!has(&h, "Blocks:"));
}

/// NFR-10 with the feature in play: nothing schedules repaints, with or without a question up.
#[test]
fn documents_stay_idle_bound_or_not() {
    // time passes between frames, so that a card's fade-in ends as it does in a window
    let clock = std::cell::Cell::new(0.0);
    let frame = |ctx: &egui::Context, app: &mut MarkiseApp| {
        clock.set(clock.get() + 1.0);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 780.0))),
            time: Some(clock.get()),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| app.ui(ui));
        out.textures_delta.clear();
        let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
        out.viewport_output[&egui::ViewportId::ROOT].repaint_delay
    };
    for bound in [false, true] {
        let f = Fixture::new();
        if bound {
            f.bind("1.0.0");
        }
        let ctx = egui::Context::default();
        let mut app = MarkiseApp::new(AppOptions {
            open: vec![f.doc.clone()],
            config: ConfigDir::Dir(f.config.clone()),
            ..common::no_watch()
        });
        // the config folder's theme worker may still deliver a late result; a repaint loop would
        // never reach a run of idle frames
        let t = std::time::Instant::now();
        loop {
            let delays = common::idle_delays(&mut app, |app| frame(&ctx, app), 6);
            if delays.iter().rev().take(3).all(|d| *d == std::time::Duration::MAX) {
                break;
            }
            assert!(t.elapsed().as_secs() < 30, "bound {bound}: {delays:?}");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(
            matches!(app.block_package_status(), PackageStatus::Asking(_)),
            bound,
            "the lookup finished"
        );
    }
}

/// A consent withdrawn on one document while another document bound to the same hash opens: the
/// other document must not end up using files that are gone.
#[test]
fn stop_using_is_ordered_before_the_next_documents_lookup() {
    use std::sync::{Arc, Mutex};
    let f = Fixture::new();
    let sha = f.bind("1.0.0");
    let other = f.folder.join("other.md");
    fs::write(&other, "text\n").unwrap();

    // holds the withdrawal until the gate's lock is released
    let gate = Arc::new(Mutex::new(()));
    let held = gate.lock().unwrap();
    let in_hook = gate.clone();
    let hooks = markise_engine::jobs::JobHooks {
        before_package_forget: Some(Box::new(move || drop(in_hook.lock()))),
        ..Default::default()
    };
    let opts = AppOptions {
        config: ConfigDir::Dir(f.config.clone()),
        hooks: Arc::new(hooks),
        ..common::no_watch()
    };
    let mut h: H = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), MarkiseApp::new(opts));
    f.open(&mut h);
    h.state_mut().block_package_action(PackageAction::Accept);
    wait_until(&mut h, "the install", |a| a.block_package().is_some());

    h.state_mut().block_package_action(PackageAction::Forget);
    h.state_mut().open(other.clone());
    wait_until(&mut h, "the other document", |a| {
        a.document_path() == Some(other.as_path()) && !a.is_loading()
    });
    // room for a lookup that does not wait for the withdrawal to answer
    for _ in 0..50 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    drop(held);
    settle(&mut h);
    h.run();
    assert!(!f.store().is_accepted("sample", &sha));
    assert!(
        h.state().block_package().is_none(),
        "{:?}",
        h.state().block_package_status()
    );
    assert!(matches!(h.state().block_package_status(), PackageStatus::NotInUse(_)));
}

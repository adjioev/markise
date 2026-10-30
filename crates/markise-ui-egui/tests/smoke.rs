//! UI smoke tests with egui_kittest: the real `MarkiseApp` driven through the accessibility
//! tree (no window, no GPU).

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_ui_egui::{AppOptions, MarkiseApp};

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-smoke-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn harness(open: Vec<PathBuf>) -> Harness<'static, MarkiseApp> {
    let app = MarkiseApp::new(AppOptions {
        open,
        ..Default::default()
    });
    Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        // the menu's ⌘/⇧ shortcut hints are not in egui's bundled fonts: tofu, as in the app
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app)
}

fn wait_until(h: &mut Harness<'static, MarkiseApp>, what: &str, f: impl Fn(&MarkiseApp) -> bool) {
    let t = Instant::now();
    while !f(h.state()) {
        assert!(t.elapsed() < common::HANG_GUARD, "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    h.step();
}

const DOC: &str = "---\ntitle: Smoke\n---\n\n# Alpha\n\nIntro with **bold**, *em*, `code` and a [link](https://example.com).\n\n- [ ] open task\n- [x] done task\n\n## Beta\n\n> quoted\n\n| a | b |\n|---|--:|\n| 1 | 2 |\n\n```rust\nfn main() {}\n```\n\n$$\nx^2\n$$\n\n## Gamma\n\nend\n";

fn loaded(a: &MarkiseApp) -> bool {
    a.model().is_some() && !a.is_loading()
}

#[test]
fn opens_file_from_cli_and_shows_title_outline_and_tree() {
    let dir = tmpdir("open");
    let p = dir.join("note.md");
    // long enough that "Gamma" is not on the first screen
    let filler: String = (0..80).map(|i| format!("Filler paragraph {i}.\n\n")).collect();
    std::fs::write(&p, DOC.replace("## Gamma", &format!("{filler}## Gamma")) + &filler).unwrap();
    std::fs::write(dir.join("other10.md"), "x").unwrap();
    std::fs::write(dir.join("other9.md"), "x").unwrap();
    let mut h = harness(vec![p.clone()]);
    wait_until(&mut h, "load", loaded);
    assert_eq!(h.state().title(), "note.md — Markise");

    // outline tab (default when a file is opened directly): headings are listed; clicking
    // one scrolls the viewer to it
    h.get_by_label("Outline").click();
    h.run_steps(2);
    h.get_by_label("Alpha");
    h.get_by_label("Beta");
    h.get_by_label("Gamma").click();
    h.run_steps(2);
    let gamma_item = h.state().model().unwrap().outline[2].item;
    let (anchor, _) = h.state_mut().viewer().unwrap().ed.view.anchor();
    assert_eq!(anchor, gamma_item, "outline click puts Gamma at the top");

    // file tree of the file's folder, natural order
    h.get_by_label("Files").click();
    // the folder is listed asynchronously: wait (bounded) until the rows are installed
    wait_until(&mut h, "listing", |a| {
        let rows = a.file_tree_rows();
        rows.iter().any(|r| r == "other9.md") && rows.iter().any(|r| r == "other10.md")
    });
    let rows = h.state().file_tree_rows();
    let pos = |n: &str| rows.iter().position(|r| r == n).unwrap();
    assert!(pos("other9.md") < pos("other10.md"), "natural order: {rows:?}");
    h.run_steps(2);
    h.get_by_label("other9.md");
    h.get_by_label("other10.md").click();
    wait_until(&mut h, "open other", |a| {
        a.document_path().is_some_and(|p| p.ends_with("other10.md")) && !a.is_loading()
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn source_mode_edit_and_save_round_trips() {
    let dir = tmpdir("source");
    let p = dir.join("doc.md");
    let original = "# T\r\n\r\n* one\r\n* two\r\n\r\nPara   with  spaces\r\n";
    std::fs::write(&p, original).unwrap();
    let mut h = harness(vec![p.clone()]);
    wait_until(&mut h, "load", loaded);

    // saving an unchanged document writes nothing and keeps the bytes
    h.state_mut().save();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), original);

    h.state_mut().toggle_source();
    h.run_steps(2);
    assert!(h.state().in_source_mode());
    h.state_mut().edit_source_text(|t| *t = t.replace("two", "zwei"));
    h.run_steps(1);
    assert!(
        h.state().title().contains('•'),
        "dirty marker in the title: {}",
        h.state().title()
    );
    h.state_mut().save();
    wait_until(&mut h, "save", |a| !a.is_busy() && !a.is_dirty());
    // only the edited word changed; CRLF, bullets and spacing untouched (spec 01 R1.2)
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        "# T\r\n\r\n* one\r\n* zwei\r\n\r\nPara   with  spaces\r\n"
    );
    h.state_mut().toggle_source();
    wait_until(&mut h, "leave source", |a| !a.is_busy());
    assert!(!h.state().in_source_mode());
    assert_eq!(h.state().title(), "doc.md — Markise");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reloads_when_the_file_changes_on_disk() {
    let dir = tmpdir("reload");
    let p = dir.join("live.md");
    std::fs::write(&p, "# Before\n").unwrap();
    let mut h = harness(vec![p.clone()]);
    wait_until(&mut h, "load", loaded);
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(&p, "# After\n\nnew text\n").unwrap();
    wait_until(&mut h, "reload", |a| {
        a.viewed_text().is_some_and(|t| t.starts_with("# After")) && !a.is_loading()
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn opening_a_folder_mounts_it() {
    let dir = tmpdir("folder");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub").join("deep.md"), "# Deep\n").unwrap();
    std::fs::write(dir.join("top.md"), "# Top\n").unwrap();
    let mut h = harness(vec![dir.clone()]);
    h.run_steps(3);
    wait_until(&mut h, "listing", |_| true);
    std::thread::sleep(Duration::from_millis(100));
    h.run_steps(3);
    h.get_by_label("top.md");
    h.get_by_label("sub").click();
    std::thread::sleep(Duration::from_millis(100));
    h.run_steps(3);
    std::thread::sleep(Duration::from_millis(100));
    h.run_steps(3);
    h.get_by_label("deep.md").click();
    wait_until(&mut h, "open deep", |a| {
        a.document_path().is_some_and(|p: &Path| p.ends_with("deep.md")) && !a.is_loading()
    });
    let _ = std::fs::remove_dir_all(&dir);
}

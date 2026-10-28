//! Sidebar presentation (issue #57): compact rows independent of the document font size,
//! Markdown extensions hidden in the tree (the inline rename still edits the full name), and
//! the hover / selection painting in the light and the dark appearance.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::Key;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_theme::Overrides;
use markise_ui_egui::sidebar::TreeAction;
use markise_ui_egui::{AppOptions, Appearance, MarkiseApp};

type H = Harness<'static, MarkiseApp>;

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-sidebar-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn harness(open: Vec<PathBuf>) -> H {
    let app = MarkiseApp::new(AppOptions {
        open,
        ..Default::default()
    });
    Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app)
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

/// A folder with `names` (files), mounted, the Files tab shown, every name listed.
fn tree(name: &str, names: &[&str]) -> (H, PathBuf) {
    let d = tmpdir(name);
    for n in names {
        std::fs::write(d.join(n), "# x\n").unwrap();
    }
    let mut h = harness(vec![d.clone()]);
    h.step();
    h.state_mut().show_files_tab(true);
    let want: Vec<String> = names.iter().map(|n| n.to_string()).collect();
    wait_until(&mut h, "the listing", move |s| {
        let rows = s.file_tree_rows();
        want.iter().all(|n| rows.contains(n))
    });
    (h, d)
}

/// Every text painted in the last frame.
fn painted_texts(h: &H) -> Vec<String> {
    fn walk(s: &egui::Shape, out: &mut Vec<String>) {
        match s {
            egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &h.output().shapes {
        walk(&c.shape, &mut out);
    }
    out
}

#[test]
fn row_height_does_not_follow_the_document_font_size() {
    let (mut h, d) = tree("row-height", &["a.md", "b.md"]);
    let before = h.get_by_label("a.md").rect().height();
    assert!((22.0..=28.0).contains(&before), "compact rows: {before}");
    // the owner's override (issue #57)
    h.state_mut().set_overrides(Overrides {
        font_size: Some(17.5),
        ..Default::default()
    });
    h.run_steps(3);
    assert_eq!(h.state().theme().font_size, 17.5, "the document font size changed");
    let after = h.get_by_label("a.md").rect().height();
    assert_eq!(before, after, "row height follows the document font size");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn markdown_extensions_are_hidden_but_rename_edits_the_full_name() {
    let (mut h, d) = tree("ext", &["note.md", "Draft.MARKDOWN", "crew.txt"]);
    let texts = painted_texts(&h);
    for shown in ["note", "Draft", "crew.txt"] {
        assert!(texts.iter().any(|t| t == shown), "{shown:?} painted: {texts:?}");
    }
    for hidden in ["note.md", "Draft.MARKDOWN"] {
        assert!(!texts.iter().any(|t| t == hidden), "{hidden:?} painted: {texts:?}");
    }
    // the rows are still named (accessibility, tooltip) with the full name
    h.get_by_label("note.md");

    // inline rename: the full name, the stem selected, the extension kept
    let ctx = h.ctx.clone();
    h.state_mut().tree_action(&ctx, TreeAction::Rename(d.join("note.md")));
    let t = Instant::now();
    let id = markise_ui_egui::sidebar::rename_field_id();
    while !h.ctx.memory(|m| m.has_focus(id)) {
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "the rename field never got the focus"
        );
        h.step();
    }
    h.step();
    assert_eq!(
        h.state().rename_state().map(|(_, n)| n.to_string()).as_deref(),
        Some("note.md")
    );
    assert!(
        painted_texts(&h).iter().any(|t| t == "note.md"),
        "the extension is visible while editing"
    );
    h.event(egui::Event::Text("ledger".into()));
    h.step();
    assert_eq!(
        h.state().rename_state().map(|(_, n)| n.to_string()).as_deref(),
        Some("ledger.md")
    );
    h.key_press(Key::Enter);
    wait_until(&mut h, "the renamed row", |s| {
        s.file_tree_rows().contains(&"ledger.md".to_string())
    });
    assert!(d.join("ledger.md").is_file() && !d.join("note.md").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn hover_and_selection_paint_in_light_and_dark() {
    for (i, appearance) in [Appearance::Light, Appearance::Dark].into_iter().enumerate() {
        let (mut h, d) = tree(&format!("paint-{i}"), &["open.md", "other.md", "plain.txt"]);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("sub").join("deep.md"), "x\n").unwrap();
        h.state_mut().set_theme_choice(appearance);
        let ctx = h.ctx.clone();
        h.state_mut().tree_action(&ctx, TreeAction::Open(d.join("open.md")));
        wait_until(&mut h, "the document", |s| s.model().is_some() && !s.is_busy());
        wait_until(&mut h, "the folder", |s| {
            s.file_tree_rows().contains(&"sub".to_string())
        });
        h.state_mut().toggle_folder(&d.join("sub"));
        wait_until(&mut h, "the nested row", |s| {
            s.file_tree_rows().contains(&"deep.md".to_string())
        });
        // hover a row, the open (selected) one, the header buttons and the tabs
        for label in ["other.md", "open.md", "deep.md", "New File", "Collapse All", "Outline"] {
            h.get_by_label(label).hover();
            h.run_steps(2);
        }
        // Collapse All folds the folder
        h.get_by_label("Collapse All").click();
        wait_until(&mut h, "collapsed", |s| {
            !s.file_tree_rows().contains(&"deep.md".to_string())
        });
        // the outline uses the same rows
        h.get_by_label("Outline").click();
        h.run_steps(3);
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[test]
fn recent_section_lists_the_session_files_and_marks_the_current_one() {
    let (mut h, d) = tree("recent", &["one.md", "two.md"]);
    let ctx = h.ctx.clone();
    for f in ["one.md", "two.md"] {
        h.state_mut().tree_action(&ctx, TreeAction::Open(d.join(f)));
        let p = d.join(f);
        wait_until(&mut h, f, move |s| {
            s.document_path() == Some(p.as_path()) && !s.is_busy()
        });
    }
    h.run_steps(2);
    // Recent rows are named by the full path (the tree has rows with the same file name)
    let one = d.join("one.md").display().to_string();
    let two = d.join("two.md").display().to_string();
    let r1 = h.get_by_label(&one).rect();
    let r2 = h.get_by_label(&two).rect();
    assert!(r2.top() < r1.top(), "the most recent first");
    assert!(r1.bottom() < h.get_by_label("one.md").rect().top(), "above the tree");
    // a click opens the file
    h.get_by_label(&one).click();
    let p = d.join("one.md");
    wait_until(&mut h, "reopened", move |s| {
        s.document_path() == Some(p.as_path()) && !s.is_busy()
    });
    // collapsing the section hides its rows
    h.get_by_label("Recent").click();
    h.run_steps(2);
    assert!(h.query_by_label(&one).is_none(), "collapsed");
    assert!(h.query_by_label("one.md").is_some(), "the tree stays");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn folder_section_collapses_and_the_activity_bar_switches_views() {
    let (mut h, d) = tree("sections", &["a.md"]);
    let folder = d.file_name().unwrap().to_string_lossy().into_owned();
    h.get_by_label(&folder).click();
    h.run_steps(2);
    assert!(h.query_by_label("a.md").is_none(), "the folder section is collapsed");
    h.get_by_label(&folder).click();
    h.run_steps(2);
    assert!(h.query_by_label("a.md").is_some());
    // the activity bar: Outline and back to Files; Search is a disabled placeholder
    h.get_by_label("Outline").click();
    h.run_steps(2);
    assert!(h.query_by_label("a.md").is_none(), "the outline is shown");
    h.get_by_label("Search").click();
    h.run_steps(2);
    assert!(h.query_by_label("a.md").is_none(), "Search does nothing yet");
    h.get_by_label("Files").click();
    h.run_steps(2);
    assert!(h.query_by_label("a.md").is_some());
    let _ = std::fs::remove_dir_all(&d);
}

//! Diagrams in the app (spec 01 §4.6, ADR 0009): on by default (DG-2) on a lazily started
//! worker, the View menu switch shows the source, the choice persists (DG-7), painting does not
//! panic.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use markise_layout::block::Prim;
use markise_ui_egui::{AppOptions, ConfigDir, MarkiseApp};

fn tmpfile(name: &str, text: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-dg-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join("doc.md");
    std::fs::write(&p, text).unwrap();
    p
}

fn harness(open: Vec<PathBuf>, config: ConfigDir) -> Harness<'static, MarkiseApp> {
    let app = MarkiseApp::new(AppOptions {
        open,
        config,
        ..Default::default()
    });
    Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app)
}

fn wait_until(h: &mut Harness<'static, MarkiseApp>, what: &str, mut f: impl FnMut(&mut MarkiseApp) -> bool) {
    let t = Instant::now();
    while !f(h.state_mut()) {
        assert!(t.elapsed() < common::HANG_GUARD, "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Whether the first diagram item's cached layout draws paths (a rendered scene).
fn rendered(a: &mut MarkiseApp) -> bool {
    let model = a.model().unwrap().clone();
    let idx = model
        .items
        .iter()
        .position(|i| matches!(i.kind, markise_layout::ItemKind::Diagram { .. }))
        .unwrap();
    let v = a.viewer().unwrap();
    let mut fonts = markise_layout::testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let l = v.ed.view.layout(idx, &mut f, &markise_layout::block::NoImages);
    l.prims.iter().any(|p| matches!(p, Prim::Path { .. }))
}

#[test]
fn diagrams_are_on_by_default_switchable_and_persisted() {
    let p = tmpfile(
        "toggle",
        "# T\n\n```mermaid\nflowchart LR\nA[Start] --> B{OK?}\nB -- yes --> C((Done))\n```\n\n```sequence\nA->B: hi\n```\n",
    );
    let cfg = p.parent().unwrap().join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    let settings = cfg.join("markise").join("settings.json");
    {
        let mut h = harness(vec![p.clone()], ConfigDir::Dir(cfg.clone()));
        wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
        assert!(h.state().diagrams(), "DG-2: on by default");
        wait_until(&mut h, "diagram", rendered);
        assert!(h.state().diagram_renderer().unwrap().started());
        // a few painted frames with the scene (paths, labels)
        for _ in 0..3 {
            h.step();
        }
        h.state_mut().set_diagrams(false);
        h.step();
        assert!(!rendered(h.state_mut()), "source when switched off");
        wait_until(&mut h, "settings written", |_| {
            std::fs::read_to_string(&settings).is_ok_and(|s| s.contains("\"diagrams\": false"))
        });
    }
    // DG-7: the next session keeps diagrams off
    let mut h = harness(vec![p], ConfigDir::Dir(cfg));
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    wait_until(&mut h, "settings applied", |a| !a.diagrams());
    for _ in 0..3 {
        h.step();
    }
    assert!(!rendered(h.state_mut()));
}

/// Codex review round 3 of PR #21: switching diagrams on while the caret is in a diagram's
/// source moves the caret to the stop after the block (it is never left in hidden source).
#[test]
fn caret_leaves_a_diagram_that_becomes_rendered() {
    let src = "# T\n\n```mermaid\nflowchart LR\nA --> B\n```\n\nafter\n";
    let p = tmpfile("caret", src);
    let cfg = p.parent().unwrap().join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    let mut h = harness(vec![p], ConfigDir::Dir(cfg));
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    h.state_mut().set_diagrams(false);
    h.step();
    let inside = src.find("A --> B").unwrap() + 2;
    h.state_mut()
        .viewer()
        .unwrap()
        .ed
        .set_caret(markise_doc::DocPos::new(inside), false);
    h.step();
    assert_eq!(
        h.state_mut()
            .viewer()
            .unwrap()
            .ed
            .session
            .selection()
            .primary()
            .focus
            .offset,
        inside,
        "a stop while the source is shown"
    );
    h.state_mut().set_diagrams(true);
    wait_until(&mut h, "diagram", rendered);
    h.step();
    h.step();
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
    assert_eq!(caret, src.find("after").unwrap(), "moved to the stop after the diagram");
}

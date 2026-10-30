//! Code highlighting in the app: lazy worker start, recoloring when results arrive, the
//! View menu switch.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use markise_layout::block::{Prim, color};
use markise_ui_egui::{AppOptions, MarkiseApp};

fn tmpfile(name: &str, text: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-hl-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join("doc.md");
    std::fs::write(&p, text).unwrap();
    p
}

fn harness(open: Vec<PathBuf>) -> Harness<'static, MarkiseApp> {
    let app = MarkiseApp::new(AppOptions {
        open,
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

/// Colors of the text sections of the first code item's cached layout.
fn code_colors(a: &mut MarkiseApp) -> Vec<egui::Color32> {
    let model = a.model().unwrap().clone();
    let idx = model
        .items
        .iter()
        .position(|i| matches!(i.kind, markise_layout::ItemKind::Code { .. }))
        .unwrap();
    let v = a.viewer().unwrap();
    let mut fonts = markise_layout::testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let l = v.ed.view.layout(idx, &mut f, &markise_layout::block::NoImages);
    let mut out = Vec::new();
    for p in &l.prims {
        if let Prim::Text { text, .. } = p {
            let galley = text.galley().unwrap();
            out.extend(galley.job.sections.iter().map(|s| s.format.color));
        }
    }
    out
}

#[test]
fn code_is_recolored_and_can_be_switched_off() {
    let p = tmpfile("recolor", "# T\n\n```rust\nfn main() { let x = 42; }\n```\n");
    let mut h = harness(vec![p]);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    let kw = |a: &MarkiseApp| color(a.theme().syntax.keyword);
    wait_until(&mut h, "highlight", |a| {
        let k = kw(a);
        code_colors(a).contains(&k)
    });
    assert!(h.state().highlighter().unwrap().started());

    h.state_mut().set_highlight_code(false);
    h.step();
    let k = kw(h.state());
    assert!(!code_colors(h.state_mut()).contains(&k), "plain when switched off");
    h.state_mut().set_highlight_code(true);
    h.step();
    assert!(
        code_colors(h.state_mut()).contains(&k),
        "cached result reused when switched on"
    );
}

#[test]
fn documents_without_code_do_not_start_the_worker() {
    let p = tmpfile("nocode", "# T\n\nJust text.\n\n```\nno language\n```\n");
    let mut h = harness(vec![p]);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    h.run_steps(4);
    assert!(
        !h.state().highlighter().unwrap().started(),
        "lazy: no worker without code"
    );
}

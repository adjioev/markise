//! Side by Side keystroke → frame (spec 04 §3.3 interim, NFR-4 budget p99 < 16 ms):
//! `cargo bench -p markise-ui-egui --bench side_by_side`.
//!
//! Opens corpora from 120 KB to 1.9 MB (just under the source editor's 2 MB edit limit), switches to Side by
//! Side, focuses the source editor with a click and types one character per frame. Reports the
//! CPU time of the whole frame (real app UI: `TextEdit` handling and its full-text relayout,
//! the incremental document/model/view update, the rendered pane's layout and paint,
//! tessellation; no GPU) and, separately, the incremental update alone.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Context, Event, Pos2, RawInput, vec2};
use markise_corpus::{Profile, generate_profile};
use markise_ui_egui::{AppOptions, MarkiseApp, ViewMode};

const W: f32 = 1300.0;
const H: f32 = 800.0;

fn raw(events: Vec<Event>) -> RawInput {
    RawInput {
        screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(W, H))),
        events,
        ..Default::default()
    }
}

fn frame(ctx: &Context, app: &mut MarkiseApp, events: Vec<Event>) -> Duration {
    let t = Instant::now();
    let mut out = ctx.run_ui(raw(events), |ui| app.ui(ui));
    out.textures_delta.clear();
    let _prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    t.elapsed()
}

fn corpus(name: &str, size: usize) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benches/corpus");
    let p = dir.join(name);
    if !p.exists() {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&p, generate_profile(size, 42, Profile::Mixed, false)).unwrap();
    }
    p
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn run(label: &str, path: &Path, highlight: bool) {
    let ctx = Context::default();
    let mut app = MarkiseApp::new(AppOptions::default());
    app.set_source_highlighting(highlight);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    app.open(path.to_path_buf());
    let t = Instant::now();
    while app.model().is_none() || app.is_loading() {
        frame(&ctx, &mut app, vec![]);
        std::thread::sleep(Duration::from_micros(200));
        assert!(t.elapsed() < Duration::from_secs(30));
    }
    app.set_view_mode(ViewMode::SideBySide);
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    // focus the editor: click in the middle of the source pane
    let p = Pos2::new(W * 0.35, H * 0.5);
    let click = |pressed| Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(&ctx, &mut app, vec![Event::PointerMoved(p), click(true)]);
    frame(&ctx, &mut app, vec![click(false)]);
    frame(&ctx, &mut app, vec![]);
    app.sbs_apply_ms.clear();
    let mut times = Vec::new();
    for i in 0..200 {
        let ev = if i % 20 == 19 {
            Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }
        } else {
            Event::Text("x".into())
        };
        times.push(frame(&ctx, &mut app, vec![ev]).as_secs_f64() * 1e3);
    }
    let typed = app.sbs_apply_ms.len();
    let mut apply = std::mem::take(&mut app.sbs_apply_ms);
    let (p50, p99, max) = (pct(&mut times, 0.5), pct(&mut times, 0.99), pct(&mut times, 1.0));
    println!(
        "{label}: keystroke -> frame p50 {p50:.2} ms, p99 {p99:.2} ms, max {max:.2} ms (budget p99 16 ms) -> {}",
        if p99 < 16.0 { "OK" } else { "OVER" }
    );
    if typed > 0 {
        let (a50, a99) = (pct(&mut apply, 0.5), pct(&mut apply, 0.99));
        println!("  incremental update alone ({typed} edits): p50 {a50:.2} ms, p99 {a99:.2} ms");
    } else {
        println!("  (no edits reached the document: the editor did not get focus)");
    }
}

fn main() {
    if std::env::args().any(|a| a == "--list") {
        return;
    }
    let eighth = corpus("120kb.md", 120 << 10);
    let quarter = corpus("256kb.md", 256 << 10);
    let one = corpus("1mb.md", 1 << 20);
    let two = corpus("1900kb.md", 1900 << 10);
    println!(
        "Side by Side headless measurements (release, {})",
        std::env::consts::ARCH
    );
    // colouring applies up to MARKISE_SOURCE_HIGHLIGHT_LIMIT (default 128 KB): run with a
    // larger limit to measure colouring of bigger files
    run("120 KB, plain", &eighth, false);
    run("120 KB, highlighted", &eighth, true);
    run("256 KB, plain", &quarter, false);
    run("256 KB, highlighted (if under the limit)", &quarter, true);
    run("1 MB, plain", &one, false);
    run("1.9 MB, plain", &two, false);
}

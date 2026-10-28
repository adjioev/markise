//! Source editor keystroke → frame (M2d, spec 02 §12.1; NFR-4 budget p99 < 16 ms):
//! `cargo bench -p markise-ui-egui --bench source_editor`.
//!
//! Opens corpora from 1 MB to 10 MB in full Source mode and in Side by Side (soft wrap and
//! line numbers on), focuses the source editor with a click and types one character per frame.
//! Reports the wall time of the whole frame (real app UI: input, session edit, incremental
//! rendered model + source view update, layout and paint of the panes, tessellation; no GPU),
//! the time from opening the file to the first source frame, and a scroll-to-end frame.

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

fn run(label: &str, path: &Path, mode: ViewMode) {
    let ctx = Context::default();
    let mut app = MarkiseApp::new(AppOptions::default());
    let mut prefs = app.source_prefs();
    prefs.line_numbers = true;
    prefs.soft_wrap = true;
    app.set_source_prefs(prefs);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    app.open(path.to_path_buf());
    let t = Instant::now();
    while app.model().is_none() || app.is_loading() {
        frame(&ctx, &mut app, vec![]);
        std::thread::sleep(Duration::from_micros(200));
        assert!(t.elapsed() < Duration::from_secs(30));
    }
    let open_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    app.set_view_mode(mode);
    frame(&ctx, &mut app, vec![]);
    let first_ms = t.elapsed().as_secs_f64() * 1e3;
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    println!("{label}: open {open_ms:.0} ms (load+parse+model), first source frame {first_ms:.1} ms");
    // scroll to the end with wrap (scrollbar drag to the bottom)
    let bar = Pos2::new(W - 4.0, H - 2.0);
    let press = |pressed| Event::PointerButton {
        pos: bar,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    let t = Instant::now();
    frame(&ctx, &mut app, vec![Event::PointerMoved(bar), press(true)]);
    println!("  scroll-to-end frame {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    frame(&ctx, &mut app, vec![press(false)]);
    // focus the editor: click in the middle of the source pane
    let p = Pos2::new(if mode == ViewMode::Source { W * 0.6 } else { W * 0.35 }, H * 0.5);
    let click = |pressed| Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(&ctx, &mut app, vec![Event::PointerMoved(p), click(true)]);
    frame(&ctx, &mut app, vec![click(false)]);
    frame(&ctx, &mut app, vec![]);
    app.source_input_ms.clear();
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
    let typed = app.source_input_ms.len();
    let mut apply = std::mem::take(&mut app.source_input_ms);
    let (p50, p99, max) = (pct(&mut times, 0.5), pct(&mut times, 0.99), pct(&mut times, 1.0));
    println!(
        "{label}: keystroke -> frame p50 {p50:.2} ms, p99 {p99:.2} ms, max {max:.2} ms (budget p99 16 ms) -> {}",
        if p99 < 16.0 { "OK" } else { "OVER" }
    );
    if typed > 0 {
        let (a50, a99) = (pct(&mut apply, 0.5), pct(&mut apply, 0.99));
        println!("  source editor input → laid out ({typed} frames): p50 {a50:.2} ms, p99 {a99:.2} ms");
    } else {
        println!("  (no edits reached the document: the editor did not get focus)");
    }
}

fn main() {
    if std::env::args().any(|a| a == "--list") {
        return;
    }
    let one = corpus("1mb.md", 1 << 20);
    let ten = corpus("10mb.md", 10 << 20);
    println!(
        "Source editor headless app measurements (release, {})",
        std::env::consts::ARCH
    );
    run("1 MB, Source", &one, ViewMode::Source);
    run("10 MB, Source", &ten, ViewMode::Source);
    run("1 MB, Side by Side", &one, ViewMode::SideBySide);
    run("10 MB, Side by Side", &ten, ViewMode::SideBySide);
}

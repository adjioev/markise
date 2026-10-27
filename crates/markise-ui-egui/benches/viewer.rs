//! Headless M1 measurements (`cargo bench -p markise-ui-egui --bench viewer`):
//!
//! - NFR-2 / NFR-3: open the 1 MB / 10 MB corpus to the first rendered frame — background
//!   read + parse + view model, then the first full egui frame of the real app UI (layout of
//!   the viewport, paint, tessellation). GPU upload/present is not included (no window).
//! - NFR-5: per-frame CPU time while wheel-scrolling through the 10 MB document (p50/p99/max);
//!   also a "jump" series (scrollbar drags to random positions).
//! - NFR-6 / NFR-7: resident memory after opening (process RSS, headless: no GPU driver).
//! - NFR-10: repaints requested after the document settled with no input.
//!
//! Runs the same `MarkiseApp::ui` as the desktop app, on a plain `egui::Context`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Context, Event, MouseWheelSource, MouseWheelUnit, Pos2, RawInput, TouchPhase, ViewportId, vec2};
use markise_corpus::{Profile, generate_profile};
use markise_ui_egui::{AppOptions, MarkiseApp, rss_mb};

const W: f32 = 1100.0;
const H: f32 = 780.0;

fn raw(events: Vec<Event>) -> RawInput {
    RawInput {
        screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(W, H))),
        events,
        ..Default::default()
    }
}

fn frame(ctx: &Context, app: &mut MarkiseApp, events: Vec<Event>) -> (Duration, Duration) {
    let t = Instant::now();
    let mut out = ctx.run_ui(raw(events), |ui| app.ui(ui));
    out.textures_delta.clear(); // headless: no renderer to apply them to
    let _prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    (t.elapsed(), out.viewport_output[&ViewportId::ROOT].repaint_delay)
}

fn corpus(name: &str, size: usize, profile: Profile) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benches/corpus");
    let p = dir.join(name);
    if !p.exists() {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&p, generate_profile(size, 42, profile, false)).unwrap();
    }
    p
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

/// Starts an app, waits until the fonts are in, then opens `path` and measures to the first
/// frame that paints the document.
fn open(path: &Path) -> (Context, MarkiseApp, f64) {
    let ctx = Context::default();
    let mut app = MarkiseApp::new(AppOptions::default());
    frame(&ctx, &mut app, vec![]); // installs fonts
    frame(&ctx, &mut app, vec![]);
    let t = Instant::now();
    app.open(path.to_path_buf());
    loop {
        frame(&ctx, &mut app, vec![]);
        if app.model().is_some() && !app.is_loading() {
            break;
        }
        std::thread::sleep(Duration::from_micros(200));
        assert!(t.elapsed() < Duration::from_secs(30));
    }
    (ctx, app, t.elapsed().as_secs_f64() * 1e3)
}

/// Memory of one document in a fresh process: open, scroll through, report RSS.
fn child(path: &Path) {
    let (ctx, mut app, _) = open(path);
    let after_open = rss_mb();
    let center = Pos2::new(W * 0.6, H * 0.5);
    frame(&ctx, &mut app, vec![Event::PointerMoved(center)]);
    for _ in 0..300 {
        let ev = Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: vec2(0.0, -200.0),
            phase: TouchPhase::Move,
            source: MouseWheelSource::Unknown,
            modifiers: Default::default(),
        };
        frame(&ctx, &mut app, vec![ev]);
    }
    println!("{after_open} {}", rss_mb());
}

fn main() {
    if std::env::args().any(|a| a == "--list") {
        return; // `cargo test --benches` probes the harness
    }
    if let Ok(p) = std::env::var("MARKISE_BENCH_CHILD") {
        child(&PathBuf::from(p));
        return;
    }
    let one = corpus("1mb.md", 1 << 20, Profile::Mixed);
    let ten = corpus("10mb.md", 10 << 20, Profile::Mixed);

    println!("M1 headless measurements (release, {})", std::env::consts::ARCH);
    for (label, path, budget, mem_budget) in [("1 MB", &one, 200.0, 80), ("10 MB", &ten, 1000.0, 300)] {
        let mut runs = Vec::new();
        for _ in 0..5 {
            let (_ctx, app, ms) = open(path);
            runs.push(ms);
            drop(app);
        }
        let med = pct(&mut runs, 0.5);
        println!(
            "open {label} to first rendered frame: median {med:.1} ms (runs {:?}) budget {budget} ms -> {}",
            runs.iter().map(|x| format!("{x:.0}")).collect::<Vec<_>>(),
            if med < budget { "OK" } else { "OVER" }
        );
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .env("MARKISE_BENCH_CHILD", path)
            .output()
            .unwrap();
        let s = String::from_utf8_lossy(&out.stdout);
        let mut it = s.split_whitespace();
        let (a, b) = (it.next().unwrap_or("?"), it.next().unwrap_or("?"));
        println!(
            "  RSS in a fresh process (headless, no window/GPU driver): {a} MB after open, {b} MB after scrolling 60k pt; budget {mem_budget} MB for the whole app"
        );
    }

    // ---- scrolling the 10 MB document (from item 2000: past the corpus's very tall [TOC])
    let (ctx, mut app, _) = open(&ten);
    app.viewer().unwrap().ed.view.scroll_to_item(2000);
    let center = Pos2::new(W * 0.6, H * 0.5);
    frame(&ctx, &mut app, vec![Event::PointerMoved(center)]);
    let mut times = Vec::new();
    for _ in 0..600 {
        let ev = Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: vec2(0.0, -60.0),
            phase: TouchPhase::Move,
            source: MouseWheelSource::Unknown,
            modifiers: Default::default(),
        };
        let (t, _) = frame(&ctx, &mut app, vec![ev]);
        times.push(t.as_secs_f64() * 1e3);
        if std::env::var_os("MARKISE_BENCH_DEBUG").is_some() && t.as_secs_f64() > 0.005 {
            let v = app.viewer().unwrap();
            eprintln!(
                "slow frame {} {:.1}ms anchor={:?} laid_out={} cache={}",
                times.len(),
                t.as_secs_f64() * 1e3,
                v.ed.view.anchor(),
                v.ed.view.laid_out.len(),
                v.ed.view.cache.len()
            );
        }
    }
    let (p50, p99, max) = (pct(&mut times, 0.5), pct(&mut times, 0.99), pct(&mut times, 1.0));
    let anchor = app.viewer().unwrap().ed.view.anchor();
    println!("  (scrolled from item 2000 to item {} of the 10 MB document)", anchor.0);
    println!(
        "wheel scroll 10 MB, 600 frames: p50 {p50:.2} ms, p99 {p99:.2} ms, max {max:.2} ms (60 fps budget 16.7 ms, 120 fps 8.3 ms) -> {}",
        if p99 < 8.3 {
            "OK (CPU side)"
        } else if p99 < 16.7 {
            "OK at 60 fps (CPU side)"
        } else {
            "OVER"
        }
    );
    // jumps: scrollbar drags to pseudo-random positions (each frame lays out a new screen)
    let mut times = Vec::new();
    let mut seed = 12345u64;
    for _ in 0..200 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let y = 30.0 + ((seed >> 33) as f32 / (1u64 << 31) as f32) * (H - 80.0);
        let p = Pos2::new(W - 4.0, y);
        let evs = vec![
            Event::PointerMoved(p),
            Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
            Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            },
        ];
        // the jump is applied and the new screen laid out in this frame
        let (t, _) = frame(&ctx, &mut app, evs);
        times.push(t.as_secs_f64() * 1e3);
    }
    let (p50, p99, max) = (pct(&mut times, 0.5), pct(&mut times, 0.99), pct(&mut times, 1.0));
    println!(
        "scrollbar jump 10 MB, 200 jumps (frame that lays out the new screen): p50 {p50:.2} ms, p99 {p99:.2} ms, max {max:.2} ms"
    );
    let cache = &app.viewer().unwrap().ed.view.cache;
    println!(
        "  process RSS after scrolling: {} MB (layout cache: {} entries, ~{} MB)",
        rss_mb(),
        cache.len(),
        cache.bytes() >> 20
    );

    // ---- idle
    frame(&ctx, &mut app, vec![Event::PointerGone]);
    let mut delays = Vec::new();
    for _ in 0..30 {
        let (_, d) = frame(&ctx, &mut app, vec![]);
        delays.push(d);
    }
    let settled = delays.iter().rposition(|d| *d != Duration::MAX).map_or(0, |i| i + 1);
    let requested = delays.iter().skip(settled).filter(|d| **d != Duration::MAX).count();
    println!(
        "idle after input stops: animation frames until settled {settled}, repaints requested afterwards {requested} (NFR-10 wants 0)"
    );
}

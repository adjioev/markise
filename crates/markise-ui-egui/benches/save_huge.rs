//! Save, autosave and draft-backup costs on a very large document (issue #29, bottleneck 4),
//! headless, through the real app UI on a plain `egui::Context`.
//!
//! Opt-in (CI never runs it): `MARKISE_BENCH_HUGE=1 cargo bench -p markise-ui-egui --bench save_huge`.
//!
//! - `MARKISE_HUGE_DIR`: folder with the corpus (default `benches/corpus`, gitignored); a
//!   missing `huge-mixed.md` is generated there (the same file `markise-corpus --huge` writes).
//!   The bench works on a temporary copy next to it: use a scratch folder and delete it afterwards.
//! - `MARKISE_HUGE_MIB`: size of a generated file (default 100).
//! - `MARKISE_SAVE_AUTOSAVE=0`: skip the autosave phase (it runs frames for 70 s).
//!
//! Measured: the UI-thread part of a save (the `save()` call), the longest UI frame until the
//! file is written, and what follows our own write — the file watcher's event and the reload
//! check (loader stages run, process CPU and RSS over a settle window). Then, with autosave
//! (1 minute) and draft backups on, every frame for 70 s: the frames in which the backup
//! and the autosave start.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use egui::{Context, Event, Pos2, RawInput, vec2};
use markise_corpus::{Profile, generate_profile};
use markise_doc::DocPos;
use markise_engine::load::{Stage, StageHook};
use markise_ui_egui::{AppOptions, MarkiseApp, rss_mb};

const W: f32 = 1300.0;
const H: f32 = 800.0;

fn raw(events: Vec<Event>) -> RawInput {
    RawInput {
        screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(W, H))),
        events,
        ..Default::default()
    }
}

fn thread_cpu() -> Duration {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: valid pointer to a timespec; CLOCK_THREAD_CPUTIME_ID is supported on macOS/Linux.
    unsafe {
        libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts);
    }
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

/// User + system CPU of the whole process (all threads).
fn process_cpu() -> Duration {
    // SAFETY: getrusage fills the zeroed struct.
    let ru = unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut ru);
        ru
    };
    let tv = |t: libc::timeval| Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1000);
    tv(ru.ru_utime) + tv(ru.ru_stime)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// One frame of the real app; returns (wall, thread CPU) in ms.
fn frame(ctx: &Context, app: &mut MarkiseApp, events: Vec<Event>) -> (f64, f64) {
    let c0 = thread_cpu();
    let t = Instant::now();
    let mut out = ctx.run_ui(raw(events), |ui| app.ui(ui));
    out.textures_delta.clear();
    let _prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    (ms(t.elapsed()), ms(thread_cpu().saturating_sub(c0)))
}

/// An offset inside a word of a text line near `frac` of the text.
fn caret_near(text: &str, frac: f64) -> usize {
    let b = text.as_bytes();
    let mut p = ((text.len() as f64) * frac) as usize;
    p = p.clamp(1, text.len().saturating_sub(3));
    while p + 2 < b.len() && !(b[p].is_ascii_lowercase() && b[p + 1].is_ascii_lowercase() && b[p - 1] != b'\n') {
        p += 1;
    }
    p + 1
}

/// Puts the caret near the middle and types one letter (the document becomes dirty).
fn type_letter(ctx: &Context, app: &mut MarkiseApp) {
    let v = app.viewer().unwrap();
    let at = caret_near(v.ed.session.text(), 0.5);
    let item = v.model().item_at_offset(at);
    v.ed.view.scroll_to_item(item.saturating_sub(1));
    frame(ctx, app, vec![]);
    app.viewer().unwrap().ed.set_caret(DocPos::new(at), false);
    frame(ctx, app, vec![]);
    frame(ctx, app, vec![Event::Text("s".into())]);
    assert!(app.is_dirty(), "typing made the document dirty");
}

fn child(src: &Path, autosave: bool) {
    let path = src.with_extension("save-work.md");
    std::fs::copy(src, &path).expect("copy corpus");
    // a config folder with autosave every minute (and draft backups, on by default)
    let config = src.with_extension("save-config");
    let _ = std::fs::remove_dir_all(&config);
    std::fs::create_dir_all(config.join("markise")).unwrap();
    std::fs::write(
        config.join("markise/settings.json"),
        r#"{"files":{"enableAutoSave":true,"autoSaveTimer":1}}"#,
    )
    .unwrap();
    let parses = Arc::new(AtomicU64::new(0));
    let reads = Arc::new(AtomicU64::new(0));
    let (p2, r2) = (parses.clone(), reads.clone());
    let hook: StageHook = Arc::new(move |st, _| match st {
        Stage::Read => {
            r2.fetch_add(1, Ordering::Relaxed);
        }
        Stage::Parse => {
            p2.fetch_add(1, Ordering::Relaxed);
        }
        Stage::Model => {}
    });
    let ctx = Context::default();
    let mut app = MarkiseApp::new(AppOptions {
        timing: true,
        load_hook: Some(hook),
        config: markise_ui_egui::ConfigDir::Dir(config.clone()),
        ..Default::default()
    });
    frame(&ctx, &mut app, vec![]);
    let t0 = Instant::now();
    app.open(path.clone());
    loop {
        frame(&ctx, &mut app, vec![]);
        if app.model().is_some() && !app.is_loading() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
        assert!(t0.elapsed() < Duration::from_secs(600), "open timed out");
    }
    for _ in 0..5 {
        frame(&ctx, &mut app, vec![]);
    }
    // focus the editing surface
    let center = Pos2::new(W * 0.5, H * 0.5);
    let b = |pressed| Event::PointerButton {
        pos: center,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(&ctx, &mut app, vec![Event::PointerMoved(center), b(true), b(false)]);
    frame(&ctx, &mut app, vec![]);
    // let the watcher's catch-up after the open settle
    let settle = Instant::now();
    while settle.elapsed() < Duration::from_secs(2) {
        frame(&ctx, &mut app, vec![]);
        std::thread::sleep(Duration::from_millis(5));
    }
    println!(
        "  opened: RSS {} MB; loader reads {} parses {}",
        rss_mb(),
        reads.load(Ordering::Relaxed),
        parses.load(Ordering::Relaxed)
    );

    for round in 0..3 {
        type_letter(&ctx, &mut app);
        // let the edit's own follow-up work (debounced jobs ~0.3 s later) finish first, so the
        // windows below see only what the save sets off
        let q = Instant::now();
        while q.elapsed() < Duration::from_secs(2) {
            frame(&ctx, &mut app, vec![]);
            std::thread::sleep(Duration::from_millis(5));
        }
        // loader stages from here on: the save's own write sets off the watcher event and the
        // reload check, possibly before the save is seen as finished
        let (r0, q0) = (reads.load(Ordering::Relaxed), parses.load(Ordering::Relaxed));
        let (c0, p0) = (thread_cpu(), process_cpu());
        let t = Instant::now();
        app.save();
        let ui = ms(t.elapsed());
        let ui_cpu = ms(thread_cpu().saturating_sub(c0));
        let mut fmax: f64 = 0.0;
        while app.is_saving() {
            fmax = fmax.max(frame(&ctx, &mut app, vec![]).0);
            std::thread::sleep(Duration::from_millis(1));
            assert!(t.elapsed() < Duration::from_secs(120), "save timed out");
        }
        let written = ms(t.elapsed());
        let save_cpu = ms(process_cpu().saturating_sub(p0));
        // what our own write sets off: watcher event → reload check (→ parse + model before)
        let (p1, u1, rss0) = (process_cpu(), thread_cpu(), rss_mb());
        let (mut rss_max, mut fmax2, mut fcpu2, mut loading_seen) = (rss0, 0f64, 0f64, false);
        let s = Instant::now();
        while s.elapsed() < Duration::from_secs(4) || app.is_loading() {
            let (w, c) = frame(&ctx, &mut app, vec![]);
            (fmax2, fcpu2) = (fmax2.max(w), fcpu2.max(c));
            loading_seen |= app.is_loading();
            rss_max = rss_max.max(rss_mb());
            std::thread::sleep(Duration::from_millis(5));
            assert!(s.elapsed() < Duration::from_secs(120), "reload timed out");
        }
        // CPU of every thread but the UI thread (loader, watcher, workers): the frame loop of
        // the settle window itself is not part of what our own write costs
        let after_cpu = ms(process_cpu().saturating_sub(p1)) - ms(thread_cpu().saturating_sub(u1));
        println!(
            "  save #{round}: save() on the UI thread {ui:.2} ms wall / {ui_cpu:.2} ms cpu; written after {written:.0} ms \
             (process CPU {save_cpu:.0} ms, max UI frame {fmax:.1} ms); dirty after: {}",
            app.is_dirty()
        );
        println!(
            "           after our write (4 s): loader reads +{} parses +{}, reload ran: {loading_seen}, CPU off the UI thread {after_cpu:.0} ms, \
             RSS {rss0} → max {rss_max} MB, max UI frame {fmax2:.1} ms wall / {fcpu2:.1} ms cpu; banner: {:?}",
            reads.load(Ordering::Relaxed) - r0,
            parses.load(Ordering::Relaxed) - q0,
            app.banner_text()
        );
    }

    if autosave {
        // dirty again; the backup fires 30 s after the last one, the autosave 60 s after the
        // last save — every frame is recorded
        type_letter(&ctx, &mut app);
        let start = Instant::now();
        let mut frames: Vec<(f64, f64, f64, bool)> = Vec::new();
        let mut was_saving = false;
        let mut save_started = None;
        while start.elapsed() < Duration::from_secs(70) {
            let (w, c) = frame(&ctx, &mut app, vec![]);
            let saving = app.is_saving();
            if saving && !was_saving {
                save_started = Some((ms(start.elapsed()), w, c));
            }
            frames.push((ms(start.elapsed()), w, c, saving && !was_saving));
            was_saving = saving;
            std::thread::sleep(Duration::from_millis(5));
        }
        // by UI-thread CPU: wall time on a busy machine includes scheduling noise
        frames.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
        let mut walls: Vec<f64> = frames.iter().map(|f| f.1).collect();
        walls.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p = |q: f64| walls[((walls.len() as f64 - 1.0) * q).round() as usize];
        println!(
            "  autosave phase: {} frames, wall p50 {:.2} p99 {:.2} ms; dirty after: {}",
            walls.len(),
            p(0.5),
            p(0.99),
            app.is_dirty()
        );
        match save_started {
            Some((at, w, c)) => {
                println!("    autosave started at {at:.0} ms in a frame of {w:.2} ms wall / {c:.2} ms cpu")
            }
            None => println!("    autosave did not start"),
        }
        for (at, w, c, s) in frames.iter().take(5) {
            println!(
                "    slowest frame (cpu) at {at:>6.0} ms: {w:>7.2} ms wall / {c:>7.2} ms cpu{}",
                if *s { " (autosave start)" } else { "" }
            );
        }
    }
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(&config);
}

fn main() {
    if std::env::args().any(|a| a == "--list") {
        return; // `cargo test --benches` probes the harness
    }
    if let Ok(p) = std::env::var("MARKISE_SAVE_CHILD") {
        let autosave = std::env::var_os("MARKISE_SAVE_AUTOSAVE").is_none_or(|v| v != "0");
        child(&PathBuf::from(p), autosave);
        return;
    }
    if std::env::var_os("MARKISE_BENCH_HUGE").is_none_or(|v| v == "0") {
        println!("save_huge: skipped (opt-in: MARKISE_BENCH_HUGE=1, see the file header)");
        return;
    }
    let dir = std::env::var("MARKISE_HUGE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benches/corpus"));
    let mib: usize = std::env::var("MARKISE_HUGE_MIB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("huge-mixed.md");
    if !path.exists() {
        std::fs::write(&path, generate_profile(mib << 20, 42, Profile::Mixed, false)).unwrap();
    }
    println!(
        "save_huge: {} ({} bytes), release, {}",
        path.display(),
        std::fs::metadata(&path).map_or(0, |m| m.len()),
        std::env::consts::ARCH
    );
    // a fresh process: clean RSS
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .env("MARKISE_SAVE_CHILD", &path)
        .stdout(std::process::Stdio::inherit())
        .output()
        .unwrap();
    for l in String::from_utf8_lossy(&out.stderr).lines() {
        if l.contains(" loaded ") || l.contains("saved") || l.contains("panicked") {
            println!("  log: {l}");
        }
    }
    if !out.status.success() {
        println!("  child failed: {}", out.status);
    }
}

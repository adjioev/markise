//! Scaling measurements on very large documents (default 100 MiB), headless, through the real
//! app UI on a plain `egui::Context` (layout, paint, tessellation; no GPU upload/present).
//!
//! Opt-in (CI never runs it): `MARKISE_BENCH_HUGE=1 cargo bench -p markise-ui-egui --bench huge`.
//!
//! - `MARKISE_HUGE_DIR`: folder with the corpus (default `benches/corpus`, gitignored); missing
//!   files are generated there (`markise-corpus --huge` writes the same files). ~100 MiB per
//!   profile plus a temporary working copy: use a scratch folder and delete it afterwards.
//! - `MARKISE_HUGE_MIB`: size of the huge files (default 100).
//! - `MARKISE_HUGE_PROFILES`: comma list out of `10mb,mixed,prose,log` (default all; `10mb` is
//!   the 10 MiB mixed corpus, the NFR reference size, for comparison).
//!
//! Each document is measured in a fresh child process (clean RSS / peak RSS): open (read +
//! parse + model on the loader thread, first painted frame), scrolling (wheel at top, bottom,
//! middle; scrollbar jumps), typing / Backspace / Enter at start, middle and end of the
//! rendered view, a large paste and its undo, 1,000 edits (memory growth, document copies),
//! save (UI-thread part and total), whole-text snapshot / clone costs, Source mode and Side by
//! Side (typing where the mode is editable), HTML export. Every frame series is reported as
//! wall time and thread CPU time (the UI thread's own CPU; separates scheduling noise on a
//! loaded machine from work), with the load average at start and end.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Context, Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, Pos2, RawInput, TouchPhase, vec2};
use markise_corpus::{Profile, generate_profile};
use markise_doc::DocPos;
use markise_ui_egui::{AppOptions, MarkiseApp, ViewMode, rss_mb};

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

/// User + system CPU of the whole process (all threads: loader, save, workers).
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

/// Peak resident memory of the process in MB.
fn peak_rss_mb() -> usize {
    // SAFETY: getrusage fills the zeroed struct.
    let max = unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut ru);
        ru.ru_maxrss as usize
    };
    // bytes on macOS, KiB on Linux
    if cfg!(target_os = "macos") {
        max >> 20
    } else {
        max >> 10
    }
}

fn loadavg() -> String {
    let mut l = [0f64; 3];
    // SAFETY: valid buffer of three doubles.
    let n = unsafe { libc::getloadavg(l.as_mut_ptr(), 3) };
    if n == 3 {
        format!("{:.1} {:.1} {:.1}", l[0], l[1], l[2])
    } else {
        "?".into()
    }
}

/// One frame of the real app; returns (wall, thread CPU) in ms.
fn frame(ctx: &Context, app: &mut MarkiseApp, events: Vec<Event>) -> (f64, f64) {
    let c0 = thread_cpu();
    let t = Instant::now();
    let mut out = ctx.run_ui(raw(events), |ui| app.ui(ui));
    out.textures_delta.clear();
    let _prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    (
        t.elapsed().as_secs_f64() * 1e3,
        thread_cpu().saturating_sub(c0).as_secs_f64() * 1e3,
    )
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

#[derive(Default)]
struct Series {
    wall: Vec<f64>,
    cpu: Vec<f64>,
}

impl Series {
    fn push(&mut self, (w, c): (f64, f64)) {
        self.wall.push(w);
        self.cpu.push(c);
    }
    fn report(mut self, label: &str) {
        let n = self.wall.len();
        let (w50, w99, wmax) = (
            pct(&mut self.wall, 0.5),
            pct(&mut self.wall, 0.99),
            pct(&mut self.wall, 1.0),
        );
        let (c50, c99) = (pct(&mut self.cpu, 0.5), pct(&mut self.cpu, 0.99));
        println!(
            "  {label:<44} n={n:<4} wall p50 {w50:>8.2} p99 {w99:>8.2} max {wmax:>8.2} ms | cpu p50 {c50:>8.2} p99 {c99:>8.2} ms"
        );
    }
}

fn wheel(dy: f32) -> Event {
    Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: vec2(0.0, dy),
        phase: TouchPhase::Move,
        source: MouseWheelSource::Unknown,
        modifiers: Default::default(),
    }
}

fn click_events(p: Pos2) -> Vec<Event> {
    let b = |pressed| Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    vec![Event::PointerMoved(p), b(true), b(false)]
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

fn key(k: Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

/// The keystroke script: letters, Backspace every 10th, Enter every 30th.
fn keystroke(i: usize) -> Event {
    if i % 30 == 29 {
        key(Key::Enter, Modifiers::default())
    } else if i % 10 == 9 {
        key(Key::Backspace, Modifiers::default())
    } else {
        Event::Text(((b'a' + (i % 26) as u8) as char).to_string())
    }
}

/// An offset inside a word of a text line near `frac` of the text.
fn caret_near(text: &str, frac: f64) -> usize {
    let b = text.as_bytes();
    let mut p = ((text.len() as f64) * frac) as usize;
    p = p.clamp(1, text.len().saturating_sub(3));
    while p + 2 < b.len() && !(b[p].is_ascii_lowercase() && b[p + 1].is_ascii_lowercase() && b[p - 1] != b'\n') {
        p += 1;
    }
    if p + 2 >= b.len() {
        return caret_near(text, frac * 0.9);
    }
    p + 1
}

fn text_len(app: &mut MarkiseApp) -> usize {
    app.viewer().map_or(0, |v| v.ed.session.text().len())
}

fn copies(app: &mut MarkiseApp) -> u64 {
    app.viewer().map_or(0, |v| v.ed.session.copies)
}

/// Scrolls the rendered view to `at` and puts the caret there.
fn caret_to(ctx: &Context, app: &mut MarkiseApp, at: usize) {
    let v = app.viewer().unwrap();
    let item = v.model().item_at_offset(at);
    v.ed.view.scroll_to_item(item.saturating_sub(1));
    frame(ctx, app, vec![]);
    app.viewer().unwrap().ed.set_caret(DocPos::new(at), false);
    frame(ctx, app, vec![]);
    frame(ctx, app, vec![]);
}

fn child(src: &Path) {
    // work on a copy: the bench saves the document
    let path = src.with_extension("work.md");
    std::fs::copy(src, &path).expect("copy corpus");
    let ctx = Context::default();
    let mut app = MarkiseApp::new(AppOptions {
        timing: true,
        ..Default::default()
    });
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    let rss0 = rss_mb();
    println!("  load average at start: {}", loadavg());
    println!("  RSS before open: {rss0} MB");

    // ---- open
    let (p0, t0) = (process_cpu(), Instant::now());
    app.open(path.clone());
    let mut load_frames = Series::default();
    loop {
        load_frames.push(frame(&ctx, &mut app, vec![]));
        if app.model().is_some() && !app.is_loading() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
        assert!(t0.elapsed() < Duration::from_secs(600), "open timed out");
    }
    let open_ms = t0.elapsed().as_secs_f64() * 1e3;
    let open_cpu = process_cpu().saturating_sub(p0).as_secs_f64() * 1e3;
    let first = *load_frames.wall.last().unwrap();
    let first_cpu = *load_frames.cpu.last().unwrap();
    let items = app.model().map_or(0, |m| m.items.len());
    println!(
        "  open → first painted frame: wall {open_ms:.0} ms, process CPU {open_cpu:.0} ms (all threads); install+first frame {first:.1} ms wall / {first_cpu:.1} ms cpu; items {items}"
    );
    load_frames.wall.pop();
    load_frames.cpu.pop();
    load_frames.report("UI frames while loading (responsiveness)");
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    println!("  RSS after open: {} MB (peak so far {} MB)", rss_mb(), peak_rss_mb());

    // ---- whole-text snapshot / clone costs (what a save, worker search or export pays)
    {
        let doc = app.viewer().unwrap().doc().clone();
        let mut snap = Series::default();
        for _ in 0..10 {
            let c0 = thread_cpu();
            let t = Instant::now();
            let s: Arc<str> = doc.snapshot();
            std::hint::black_box(&s);
            let w = t.elapsed().as_secs_f64() * 1e3;
            snap.push((w, thread_cpu().saturating_sub(c0).as_secs_f64() * 1e3));
            drop(s);
        }
        snap.report("Arc<str> snapshot of the text");
        let mut fp = Series::default();
        for _ in 0..5 {
            use std::hash::{Hash, Hasher};
            let c0 = thread_cpu();
            let t = Instant::now();
            let mut h = ahash::AHasher::default();
            doc.text().hash(&mut h);
            std::hint::black_box(h.finish());
            fp.push((
                t.elapsed().as_secs_f64() * 1e3,
                thread_cpu().saturating_sub(c0).as_secs_f64() * 1e3,
            ));
        }
        fp.report("text fingerprint (save, UI thread)");
        let mut cl = Series::default();
        for _ in 0..3 {
            let c0 = thread_cpu();
            let t = Instant::now();
            let d = doc.clone_box();
            let w = t.elapsed().as_secs_f64() * 1e3;
            cl.push((w, thread_cpu().saturating_sub(c0).as_secs_f64() * 1e3));
            drop(d);
        }
        cl.report("FormatDoc clone (copy-on-write fallback)");
    }

    // ---- scrolling: top → bottom → middle
    let center = Pos2::new(W * 0.6, H * 0.5);
    app.viewer().unwrap().ed.view.scroll_to_item(0);
    frame(&ctx, &mut app, vec![Event::PointerMoved(center)]);
    let mut s = Series::default();
    for _ in 0..300 {
        s.push(frame(&ctx, &mut app, vec![wheel(-60.0)]));
    }
    s.report("wheel scroll at top (300 frames)");
    let bar = |y: f32| Pos2::new(W - 4.0, y);
    let mut j = Series::default();
    j.push(frame(&ctx, &mut app, click_events(bar(H - 2.0))));
    frame(&ctx, &mut app, vec![Event::PointerMoved(center)]);
    let mut s = Series::default();
    for _ in 0..300 {
        s.push(frame(&ctx, &mut app, vec![wheel(60.0)]));
    }
    s.report("wheel scroll at bottom (300 frames)");
    j.push(frame(&ctx, &mut app, click_events(bar(H * 0.5))));
    frame(&ctx, &mut app, vec![Event::PointerMoved(center)]);
    let mut s = Series::default();
    for _ in 0..300 {
        s.push(frame(&ctx, &mut app, vec![wheel(-60.0)]));
    }
    s.report("wheel scroll in the middle (300 frames)");
    let mut seed = 12345u64;
    for _ in 0..100 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let y = 30.0 + ((seed >> 33) as f32 / (1u64 << 31) as f32) * (H - 80.0);
        j.push(frame(&ctx, &mut app, click_events(bar(y))));
    }
    j.report("scrollbar jumps (bottom, middle, 100 random)");
    println!("  RSS after scrolling: {} MB", rss_mb());

    // ---- typing in the rendered view
    frame(&ctx, &mut app, click_events(center)); // focus the editing surface
    frame(&ctx, &mut app, vec![]);
    let copies0 = copies(&mut app);
    for (label, frac) in [("start", 0.0), ("middle", 0.5), ("end", 0.999)] {
        let at = {
            let v = app.viewer().unwrap();
            caret_near(v.ed.session.text(), frac)
        };
        caret_to(&ctx, &mut app, at);
        let (len0, cp0) = (text_len(&mut app), copies(&mut app));
        let mut s = Series::default();
        for i in 0..150 {
            s.push(frame(&ctx, &mut app, vec![keystroke(i)]));
        }
        let grew = text_len(&mut app) as isize - len0 as isize;
        let cp = copies(&mut app) - cp0;
        s.report(&format!(
            "rendered: keystroke → frame [{label}] (Δ{grew} B, {cp} copies)"
        ));
    }
    println!(
        "  document copies during rendered typing: {}",
        copies(&mut app) - copies0
    );

    // ---- large paste and its undo
    for (label, size) in [("1 MiB", 1usize << 20), ("10 MiB", 10 << 20)] {
        let paste = generate_profile(size, 7, Profile::Prose, false);
        let at = {
            let v = app.viewer().unwrap();
            caret_near(v.ed.session.text(), 0.5)
        };
        caret_to(&ctx, &mut app, at);
        let len0 = text_len(&mut app);
        let p = frame(&ctx, &mut app, vec![Event::Paste(paste)]);
        let pasted = text_len(&mut app) - len0;
        let u = frame(&ctx, &mut app, vec![key(Key::Z, cmd())]);
        let back = text_len(&mut app) == len0;
        println!(
            "  paste {label} (+{pasted} B): frame {:.1} ms wall / {:.1} ms cpu; undo frame {:.1} ms wall / {:.1} ms cpu (restored: {back})",
            p.0, p.1, u.0, u.1
        );
    }

    // ---- 1,000 edits: memory growth and document copies
    {
        let at = {
            let v = app.viewer().unwrap();
            caret_near(v.ed.session.text(), 0.3)
        };
        caret_to(&ctx, &mut app, at);
        let (r0, c0) = (rss_mb(), copies(&mut app));
        let mut s = Series::default();
        for i in 0..1000 {
            let ev = if i % 7 == 6 {
                key(Key::Space, Modifiers::default())
            } else {
                keystroke(i)
            };
            s.push(frame(&ctx, &mut app, vec![ev]));
        }
        s.report("1,000 edits (rendered, mid-document)");
        let undo = app.viewer().unwrap().ed.session.history().undo_len();
        println!(
            "  1,000 edits: RSS {r0} → {} MB, document copies {}, undo steps {undo}",
            rss_mb(),
            copies(&mut app) - c0
        );
    }

    // ---- save: snapshot + fingerprint on the UI thread, write on the worker
    for round in 0..3 {
        frame(&ctx, &mut app, vec![Event::Text("s".into())]);
        let (c0, p0) = (thread_cpu(), process_cpu());
        let t = Instant::now();
        app.save();
        let ui = t.elapsed().as_secs_f64() * 1e3;
        let ui_cpu = thread_cpu().saturating_sub(c0).as_secs_f64() * 1e3;
        let mut f = Series::default();
        while app.is_saving() {
            f.push(frame(&ctx, &mut app, vec![]));
            std::thread::sleep(Duration::from_millis(1));
            assert!(t.elapsed() < Duration::from_secs(120), "save timed out");
        }
        let total = t.elapsed().as_secs_f64() * 1e3;
        let pcpu = process_cpu().saturating_sub(p0).as_secs_f64() * 1e3;
        let fmax = f.wall.iter().cloned().fold(0.0, f64::max);
        println!(
            "  save #{round}: UI-thread call {ui:.1} ms wall / {ui_cpu:.1} ms cpu; done after {total:.0} ms wall, process CPU {pcpu:.0} ms; dirty after: {}; max UI frame meanwhile {fmax:.1} ms",
            app.is_dirty()
        );
    }
    println!("  RSS after edits+saves: {} MB (peak {} MB)", rss_mb(), peak_rss_mb());

    // ---- HTML export of the whole document (worker-side work, measured inline)
    {
        let doc = app.viewer().unwrap().doc().clone();
        let md = doc
            .as_any()
            .downcast_ref::<markise_md::Document>()
            .expect("a Markdown document");
        // every `[TOC]` renders the whole heading list: the mixed corpus has one TOC per ~200
        // blocks, so its HTML grows quadratically with size (10 MiB → ~0.4 GB); skip above 20 MiB
        let tocs = md.text().matches("[TOC]").count();
        if tocs > 1 && md.text().len() > 20 << 20 {
            println!("  HTML export: skipped ({tocs} [TOC] markers; output grows with TOCs × headings)");
        } else {
            let (r0, c0) = (rss_mb(), thread_cpu());
            let t = Instant::now();
            let html = markise_export::render_document(md, &markise_export::HtmlOptions::default());
            let w = t.elapsed().as_secs_f64() * 1e3;
            let c = thread_cpu().saturating_sub(c0).as_secs_f64() * 1e3;
            println!(
                "  HTML export (render_document): {w:.0} ms wall / {c:.0} ms cpu, {} MB output, RSS {r0} → {} MB (peak {} MB)",
                html.len() >> 20,
                rss_mb(),
                peak_rss_mb()
            );
        }
    }

    // ---- Source mode and Side by Side
    for (label, mode, focus_x) in [
        ("Source", ViewMode::Source, W * 0.6),
        ("Side by Side", ViewMode::SideBySide, W * 0.25),
    ] {
        let r0 = rss_mb();
        let c0 = thread_cpu();
        let t = Instant::now();
        app.set_view_mode(mode);
        let f = frame(&ctx, &mut app, vec![]);
        let switch = t.elapsed().as_secs_f64() * 1e3;
        let switch_cpu = thread_cpu().saturating_sub(c0).as_secs_f64() * 1e3;
        for _ in 0..3 {
            frame(&ctx, &mut app, vec![]);
        }
        println!(
            "  {label}: switch + first frame {switch:.1} ms wall / {switch_cpu:.1} ms cpu (frame alone {:.1} ms), RSS {r0} → {} MB",
            f.0,
            rss_mb()
        );
        let p = Pos2::new(focus_x, H * 0.5);
        frame(&ctx, &mut app, vec![Event::PointerMoved(p)]);
        let mut s = Series::default();
        for _ in 0..200 {
            s.push(frame(&ctx, &mut app, vec![wheel(-60.0)]));
        }
        s.report(&format!("{label}: wheel scroll (200 frames)"));
        let t = Instant::now();
        let jb = frame(&ctx, &mut app, click_events(bar(H * 0.5)));
        println!(
            "  {label}: scrollbar jump to middle {:.1} ms wall / {:.1} ms cpu ({:.1} ms incl. call)",
            jb.0,
            jb.1,
            t.elapsed().as_secs_f64() * 1e3
        );
        frame(&ctx, &mut app, click_events(p));
        frame(&ctx, &mut app, vec![]);
        let len0 = app.document_text().map_or(0, |t| t.len());
        let mut s = Series::default();
        for i in 0..100 {
            s.push(frame(&ctx, &mut app, vec![keystroke(i)]));
        }
        for _ in 0..3 {
            frame(&ctx, &mut app, vec![]);
        }
        let grew = app.document_text().map_or(0, |t| t.len()) as isize - len0 as isize;
        s.report(&format!("{label}: keystroke → frame (Δ{grew} B)"));
        app.set_view_mode(ViewMode::Rendered);
        frame(&ctx, &mut app, vec![]);
    }
    println!(
        "  final RSS {} MB, peak RSS {} MB; load average at end: {}",
        rss_mb(),
        peak_rss_mb(),
        loadavg()
    );
    drop(app);
    let _ = std::fs::remove_file(&path);
}

/// Memory split without the app, in its own process: text, parsed document, view model.
fn mem_child(src: &Path) {
    use markise_doc::{DocumentFormat, FormatOptions};
    let r0 = rss_mb();
    let text = std::fs::read_to_string(src).expect("read corpus");
    let r1 = rss_mb();
    let t = Instant::now();
    let doc = markise_md::MarkdownFormat::default().parse(text, &FormatOptions::default());
    let parse = t.elapsed().as_secs_f64() * 1e3;
    let r2 = rss_mb();
    let t = Instant::now();
    let model = markise_layout::DocModel::build_owned(&*doc, None);
    let build = t.elapsed().as_secs_f64() * 1e3;
    let r3 = rss_mb();
    let items = model.items.len();
    println!(
        "  memory split (engine only, fresh process): text {} MB, parsed document +{} MB ({parse:.0} ms), view model +{} MB ({build:.0} ms, {items} items, ~{} B/item)",
        r1 - r0,
        r2 - r1,
        r3 - r2,
        ((r3 - r2) << 20) / items.max(1)
    );
    drop(model);
    drop(doc);
}

fn main() {
    if std::env::args().any(|a| a == "--list") {
        return; // `cargo test --benches` probes the harness
    }
    if let Ok(p) = std::env::var("MARKISE_HUGE_MEM_CHILD") {
        mem_child(&PathBuf::from(p));
        return;
    }
    if let Ok(p) = std::env::var("MARKISE_HUGE_CHILD") {
        child(&PathBuf::from(p));
        return;
    }
    if std::env::var_os("MARKISE_BENCH_HUGE").is_none_or(|v| v == "0") {
        println!("huge: skipped (opt-in: MARKISE_BENCH_HUGE=1, see the file header)");
        return;
    }
    let dir = std::env::var("MARKISE_HUGE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benches/corpus"));
    let mib: usize = std::env::var("MARKISE_HUGE_MIB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let profiles = std::env::var("MARKISE_HUGE_PROFILES").unwrap_or_else(|_| "10mb,mixed,prose,log".into());
    std::fs::create_dir_all(&dir).unwrap();
    println!(
        "huge: headless app measurements (release, {}, {} CPUs)",
        std::env::consts::ARCH,
        std::thread::available_parallelism().map_or(0, |n| n.get())
    );
    for name in profiles.split(',').map(str::trim) {
        let (file, size, profile) = match name {
            "10mb" => ("10mb.md".to_string(), 10usize << 20, Profile::Mixed),
            "mixed" => ("huge-mixed.md".into(), mib << 20, Profile::Mixed),
            "prose" => ("huge-prose.md".into(), mib << 20, Profile::Prose),
            "log" => ("huge-log.md".into(), mib << 20, Profile::Log),
            other => panic!("unknown profile {other}"),
        };
        let path = dir.join(file);
        if !path.exists() {
            std::fs::write(&path, generate_profile(size, 42, profile, false)).unwrap();
        }
        let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
        let lines = std::fs::read(&path).map_or(0, |b| b.iter().filter(|&&c| c == b'\n').count());
        println!(
            "\n== {name}: {} ({bytes} bytes, {lines} lines) — load average {}",
            path.display(),
            loadavg()
        );
        let _ = std::process::Command::new(std::env::current_exe().unwrap())
            .env("MARKISE_HUGE_MEM_CHILD", &path)
            .status();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .env("MARKISE_HUGE_CHILD", &path)
            .stdout(std::process::Stdio::inherit())
            .output()
            .unwrap();
        // the app's timing log (stderr): loader stage split and save sizes
        for l in String::from_utf8_lossy(&out.stderr).lines() {
            if l.contains(" loaded ") || l.contains("first-frame") || l.contains("panicked") {
                println!("  log: {l}");
            }
        }
        if !out.status.success() {
            println!("  child failed: {}", out.status);
        }
    }
}

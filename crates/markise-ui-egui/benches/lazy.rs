//! PROTOTYPE measurements of the lazy (paged) view model against the normal one (issue #29,
//! docs/research/lazy-view-prototype.md), headless, through the real app UI on a plain
//! `egui::Context` (layout, paint, tessellation; no GPU).
//!
//! Opt-in: `MARKISE_BENCH_LAZY=1 MARKISE_LAZY_FILES=a.md,b.md cargo bench -p markise-ui-egui
//! --bench lazy`. `MARKISE_LAZY_MODES` (default `normal,lazy`) selects the modes; each file and
//! mode runs in a fresh child process (`MARKISE_LAZY_VIEW` set for the lazy one).
//!
//! A counting global allocator reports live heap bytes; footprint is the macOS physical
//! footprint (Activity Monitor's "Memory").

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use egui::{Context, Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, Pos2, RawInput, TouchPhase, vec2};
use markise_doc::DocPos;
use markise_ui_egui::{AppOptions, MarkiseApp, rss_mb};

struct Counting;
static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards every call to `System` unchanged; only counts sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let now = LIVE.fetch_add(l.size() as isize, Ordering::Relaxed) + l.size() as isize;
            PEAK.fetch_max(now.max(0) as usize, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(p, l) };
        LIVE.fetch_sub(l.size() as isize, Ordering::Relaxed);
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            let now = LIVE.fetch_add(l.size() as isize, Ordering::Relaxed) + l.size() as isize;
            PEAK.fetch_max(now.max(0) as usize, Ordering::Relaxed);
        }
        p
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let q = unsafe { System.realloc(p, l, new) };
        if !q.is_null() {
            let d = new as isize - l.size() as isize;
            let now = LIVE.fetch_add(d, Ordering::Relaxed) + d;
            PEAK.fetch_max(now.max(0) as usize, Ordering::Relaxed);
        }
        q
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn footprint_mb() -> usize {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: `proc_pid_rusage` fills the zeroed `rusage_info_v2` of the flavor passed.
        unsafe {
            let mut ri: libc::rusage_info_v2 = std::mem::zeroed();
            let p = (&mut ri as *mut libc::rusage_info_v2).cast::<libc::rusage_info_t>();
            if libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V2, p) == 0 {
                return (ri.ri_phys_footprint >> 20) as usize;
            }
        }
    }
    rss_mb()
}

fn mem(label: &str) {
    println!(
        "  MEM {label:<28} live {:>6} MB | footprint {:>6} MB | RSS {:>6} MB | peak live {:>6} MB",
        LIVE.load(Ordering::Relaxed).max(0) >> 20,
        footprint_mb(),
        rss_mb(),
        PEAK.load(Ordering::Relaxed) >> 20
    );
}

const W: f32 = 1300.0;
const H: f32 = 800.0;

fn raw(events: Vec<Event>) -> RawInput {
    RawInput {
        screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(W, H))),
        events,
        ..Default::default()
    }
}

fn frame(ctx: &Context, app: &mut MarkiseApp, events: Vec<Event>) -> f64 {
    let t = Instant::now();
    let mut out = ctx.run_ui(raw(events), |ui| app.ui(ui));
    out.textures_delta.clear();
    let _prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    t.elapsed().as_secs_f64() * 1e3
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn report(label: &str, mut v: Vec<f64>) {
    let n = v.len();
    let (a, b, c) = (pct(&mut v, 0.5), pct(&mut v, 0.99), pct(&mut v, 1.0));
    println!("  {label:<44} n={n:<5} p50 {a:>8.2}  p99 {b:>8.2}  max {c:>8.2} ms");
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
    Modifiers {
        mac_cmd: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        command: true,
        ..Default::default()
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

fn keystroke(i: usize) -> Event {
    if i % 30 == 29 {
        key(Key::Enter, Modifiers::default())
    } else if i % 10 == 9 {
        key(Key::Backspace, Modifiers::default())
    } else {
        Event::Text(((b'a' + (i % 26) as u8) as char).to_string())
    }
}

fn line_end(text: &str, p: usize) -> usize {
    text[p..].find('\n').map_or(text.len(), |i| p + i)
}

/// An offset in the middle of a plain paragraph line at or after `frac` of the text.
fn para_near(text: &str, frac: f64) -> usize {
    let mut p = ((text.len() as f64) * frac) as usize;
    while !text.is_char_boundary(p) {
        p += 1;
    }
    loop {
        let Some(i) = text[p..].find("\n\n") else {
            return text.len() / 2;
        };
        let ls = p + i + 2;
        let le = line_end(text, ls);
        let line = &text[ls..le];
        if line.len() > 30
            && line
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c == b' ' || c == b',' || c == b'.')
        {
            return ls + line.len() / 2;
        }
        p = ls;
    }
}

struct Probe {
    scroll_y: f64,
    total: f64,
}

fn probe(app: &mut MarkiseApp) -> Probe {
    let v = &app.viewer().unwrap().ed.view;
    Probe {
        scroll_y: v.scroll_y(),
        total: v.total_height(),
    }
}

fn lazy_line(app: &mut MarkiseApp) -> String {
    let v = &app.viewer().unwrap().ed.view;
    let l = v.lazy();
    let s = markise_layout::paged_stats();
    format!(
        "items {} | materialized {} evicted {} rebuilds {} | {:.0} ms in paging | placeholder layouts {} | max anchor jump {:.0} px",
        v.len(),
        l.materialized,
        l.evicted,
        s.rebuilds,
        l.ms,
        l.placeholder_layouts,
        l.max_jump
    )
}

fn child(src: &Path) {
    let lazy = markise_layout::lazy_enabled();
    let path = src.with_extension(if lazy { "lazy-work.md" } else { "normal-work.md" });
    std::fs::copy(src, &path).expect("copy corpus");
    let ctx = Context::default();
    let mut app = MarkiseApp::new(AppOptions {
        timing: true,
        ..Default::default()
    });
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    mem("before open");

    // ---- open → first painted frame
    let t0 = Instant::now();
    app.open(path.clone());
    let mut install;
    loop {
        install = frame(&ctx, &mut app, vec![]);
        if app.model().is_some() && !app.is_loading() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
        assert!(t0.elapsed() < Duration::from_secs(600), "open timed out");
    }
    let open_ms = t0.elapsed().as_secs_f64() * 1e3;
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    let items = app.model().map_or(0, |m| m.items.len());
    println!("  open → first painted frame {open_ms:.0} ms (install + first frame {install:.1} ms), {items} items");
    mem("after open");
    let total0 = probe(&mut app).total;
    // offsets the phases need (no copy of the text stays alive: it would count as live heap)
    let (finds, mid, start0) = {
        let text = app.viewer().unwrap().ed.session.text();
        let finds: Vec<usize> = [0.2, 0.4, 0.6, 0.8, 0.95, 0.1]
            .iter()
            .map(|&f| para_near(text, f))
            .collect();
        (finds, para_near(text, 0.5), para_near(text, 0.0))
    };
    let r = app.viewer().unwrap().surface_rect().expect("the viewer was shown");
    let center = r.center();
    let bar = |f: f32| Pos2::new(r.right() - 4.0, r.top() + 1.0 + f.clamp(0.0, 1.0) * (r.height() - 2.0));

    // ---- jumps: outline entries (materialize the heading's page, scroll, frame)
    let n_out = app.model().map_or(0, |m| m.outline.len());
    let mut outline_ms = Vec::new();
    let mut misses = 0;
    for f in [0.25, 0.5, 0.75, 0.99, 0.33, 0.66] {
        let k = ((n_out as f64 - 1.0) * f) as usize;
        let t = Instant::now();
        {
            let v = &mut app.viewer().unwrap().ed.view;
            if let Some(i) = v.ensure_heading(k) {
                v.scroll_to_item(i);
            }
        }
        frame(&ctx, &mut app, vec![]);
        outline_ms.push(t.elapsed().as_secs_f64() * 1e3);
        // the heading is shown
        let want = app.model().unwrap().outline[k].item;
        if !app.viewer().unwrap().ed.view.last_visible.contains(&want) {
            misses += 1;
        }
    }
    report(
        &format!("jump: outline click (cold, 6 targets, {misses} missed)"),
        outline_ms,
    );

    // ---- jumps: find next (selection far away → scroll to caret)
    let mut find_ms = Vec::new();
    for &at in &finds {
        let t = Instant::now();
        {
            let v = app.viewer().unwrap();
            v.ed.session.set_selection(markise_doc::Selection::range(at, at + 3));
            v.ed.refresh_focus();
            v.find_reveal_for_bench();
        }
        frame(&ctx, &mut app, vec![]);
        find_ms.push(t.elapsed().as_secs_f64() * 1e3);
    }
    report("jump: find next (cold, 6 targets)", find_ms);

    // ---- Cmd+Down / Cmd+Up (caret in the text first)
    let at = mid;
    {
        let v = app.viewer().unwrap();
        v.ed.set_caret(DocPos::new(at), false);
        v.find_reveal_for_bench();
    }
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, click_events(center));
    frame(&ctx, &mut app, vec![]);
    let mut edge_ms = Vec::new();
    for k in [Key::ArrowDown, Key::ArrowUp, Key::ArrowDown, Key::ArrowUp] {
        edge_ms.push(frame(&ctx, &mut app, vec![key(k, cmd())]));
        let c = app.viewer().unwrap().ed.session.selection().primary().focus.offset;
        print!(" {c}");
        frame(&ctx, &mut app, vec![]);
    }
    println!();
    report("jump: Cmd+Down/Up (carets above)", edge_ms);
    mem("after jumps");

    // ---- scrollbar jumps: frame time and stability
    frame(&ctx, &mut app, vec![Event::PointerMoved(center)]);
    let mut bar_ms = Vec::new();
    let mut drift = Vec::new();
    for f in [0.1f32, 0.3, 0.5, 0.7, 0.9, 0.2, 0.4, 0.6, 0.8] {
        bar_ms.push(frame(&ctx, &mut app, click_events(bar(f))));
        let a = probe(&mut app);
        for _ in 0..10 {
            frame(&ctx, &mut app, vec![Event::PointerMoved(center)]);
        }
        let b = probe(&mut app);
        // thumb position right after the jump vs after the estimates around it settled
        let fa = a.scroll_y / (a.total - r.height() as f64).max(1.0);
        let fb = b.scroll_y / (b.total - r.height() as f64).max(1.0);
        drift.push(((fb - fa) * r.height() as f64).abs());
    }
    report("scrollbar click jump (cold, 9 targets)", bar_ms);
    let mut d2 = drift.clone();
    println!(
        "  scrollbar thumb drift after a jump (px of a {:.0} px bar): p50 {:.2} max {:.2}",
        r.height(),
        pct(&mut d2, 0.5),
        pct(&mut drift, 1.0)
    );

    // ---- wheel scrolling: cold (new region) and warm (the same region again)
    {
        let v = &mut app.viewer().unwrap().ed.view;
        let y = v.total_height() * 0.37;
        v.scroll_to_y(y);
    }
    frame(&ctx, &mut app, vec![Event::PointerMoved(center)]);
    let mut cold = Vec::new();
    for _ in 0..400 {
        cold.push(frame(&ctx, &mut app, vec![wheel(-60.0)]));
    }
    report("wheel scroll down, cold (400 frames)", cold);
    let mut warm = Vec::new();
    for _ in 0..400 {
        warm.push(frame(&ctx, &mut app, vec![wheel(60.0)]));
    }
    report("wheel scroll up, same region (warm)", warm);

    // ---- typing in the middle
    let at = mid;
    {
        let v = app.viewer().unwrap();
        v.ed.set_caret(DocPos::new(at), false);
        v.find_reveal_for_bench();
    }
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    let mut typing = Vec::new();
    for i in 0..300 {
        let m0 = app.viewer().unwrap().ed.view.lazy().clone();
        let ms = frame(&ctx, &mut app, vec![keystroke(i)]);
        if ms > 8.0 {
            let v = &app.viewer().unwrap().ed.view;
            let m1 = v.lazy();
            println!(
                "  slow keystroke {i}: {ms:.1} ms, materialized +{} evicted +{} paging +{:.1} ms, items {}",
                m1.materialized - m0.materialized,
                m1.evicted - m0.evicted,
                m1.ms - m0.ms,
                v.len()
            );
        }
        typing.push(ms);
    }
    report("typing in the middle (letters, BS, Enter)", typing);
    let mut undo = Vec::new();
    for _ in 0..20 {
        undo.push(frame(&ctx, &mut app, vec![key(Key::Z, cmd())]));
    }
    report("undo (20 steps)", undo);
    // typing at the very start
    {
        let v = app.viewer().unwrap();
        v.ed.set_caret(DocPos::new(start0), false);
        v.find_reveal_for_bench();
    }
    frame(&ctx, &mut app, vec![]);
    let mut typing0 = Vec::new();
    for i in 0..100 {
        typing0.push(frame(&ctx, &mut app, vec![keystroke(i)]));
    }
    report("typing near the start", typing0);
    // edits at block starts all over the document (Backspace merges with the block before,
    // which may sit in another page), then a consistency check against a full build
    let mut merge_ms = Vec::new();
    for k in 0..12 {
        let ls = {
            let text = app.viewer().unwrap().ed.session.text();
            let mid = para_near(text, 0.05 + 0.08 * k as f64);
            text[..mid].rfind('\n').map_or(0, |i| i + 1)
        };
        {
            let v = app.viewer().unwrap();
            v.ed.set_caret(DocPos::new(ls), false);
            v.find_reveal_for_bench();
        }
        frame(&ctx, &mut app, vec![]);
        for e in [
            key(Key::Backspace, Modifiers::default()),
            key(Key::Enter, Modifiers::default()),
            Event::Text("z".into()),
            key(Key::Backspace, Modifiers::default()),
        ] {
            merge_ms.push(frame(&ctx, &mut app, vec![e]));
        }
    }
    report("Backspace/Enter at block starts (12 places)", merge_ms);
    if lazy {
        let v = app.viewer().unwrap();
        let r = v.ed.view.model().check_paged(&**v.ed.view.doc());
        println!("  lazy: consistency with a full build after edits: {r:?}");
    }
    if lazy {
        println!("  lazy: {}", lazy_line(&mut app));
    }
    mem("after typing");

    // ---- traversal of the whole document (scrollbar steps)
    let steps = std::env::var("MARKISE_LAZY_STEPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000usize);
    let t = Instant::now();
    let mut trav = Vec::new();
    for s in 0..=steps {
        {
            let v = &mut app.viewer().unwrap().ed.view;
            let y = v.total_height() * s as f64 / steps as f64;
            v.scroll_to_y(y);
        }
        trav.push(frame(&ctx, &mut app, vec![]));
    }
    println!(
        "  traversal: {steps} scrollbar steps in {:.1} s",
        t.elapsed().as_secs_f64()
    );
    report("traversal frames", trav);
    for _ in 0..5 {
        frame(&ctx, &mut app, vec![]);
    }
    mem("after traversal");
    let total1 = probe(&mut app).total;
    println!(
        "  document height: after open {total0:.0} px, after traversal {total1:.0} px ({:+.1} %)",
        (total1 / total0 - 1.0) * 100.0
    );
    if lazy {
        println!("  lazy: {}", lazy_line(&mut app));
        let v = app.viewer().unwrap();
        let r = v.ed.view.model().check_paged(&**v.ed.view.doc());
        println!("  lazy: consistency with a full build after traversal: {r:?}");
    }
    drop(app);
    let _ = std::fs::remove_file(&path);
}

fn main() {
    if std::env::args().any(|a| a == "--list") {
        return;
    }
    if let Ok(p) = std::env::var("MARKISE_LAZY_CHILD") {
        child(&PathBuf::from(p));
        return;
    }
    if std::env::var_os("MARKISE_BENCH_LAZY").is_none_or(|v| v == "0") {
        println!("lazy: skipped (opt-in: MARKISE_BENCH_LAZY=1, see the file header)");
        return;
    }
    let files = std::env::var("MARKISE_LAZY_FILES").expect("MARKISE_LAZY_FILES");
    let modes = std::env::var("MARKISE_LAZY_MODES").unwrap_or_else(|_| "normal,lazy".into());
    for f in files.split(',').map(str::trim) {
        for m in modes.split(',').map(str::trim) {
            println!("\n== {f} — {m}");
            let mut c = std::process::Command::new(std::env::current_exe().unwrap());
            c.env("MARKISE_LAZY_CHILD", f).env_remove("MARKISE_LAZY_VIEW");
            if m == "lazy" {
                c.env("MARKISE_LAZY_VIEW", "1");
            }
            let out = c.stdout(std::process::Stdio::inherit()).output().unwrap();
            for l in String::from_utf8_lossy(&out.stderr).lines() {
                if l.contains(" loaded ") || l.contains("panicked") || l.contains("lazy view") {
                    println!("  log: {l}");
                }
            }
            if !out.status.success() {
                println!("  child failed: {}", out.status);
            }
        }
    }
}

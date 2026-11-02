//! Memory breakdown of a large open document (issue #29, part 4), headless, through the real
//! app UI on a plain `egui::Context`.
//!
//! Opt-in (CI never runs it): `MARKISE_BENCH_MEMORY=1 cargo bench -p markise-ui-egui --bench memory`.
//!
//! - `MARKISE_HUGE_DIR`: folder with the corpus (default `benches/corpus`, gitignored); missing
//!   files are generated there (`markise-corpus --huge` writes the same files).
//! - `MARKISE_HUGE_MIB`: size of the huge files (default 100).
//! - `MARKISE_HUGE_PROFILES`: comma list out of `10mb,mixed,prose,log` (default `mixed,prose,log`).
//!
//! A counting global allocator tracks live heap bytes and live allocations; every phase prints
//! its delta together with the process RSS. Each document runs in two fresh child processes:
//! the engine pipeline alone (read → parse → model, with a walk over the parsed document and the
//! model attributing bytes to their structures), then the real app (open on the loader thread,
//! first frames, steady state). Live bytes are what the program asked for; RSS minus live bytes
//! is allocator overhead, fragmentation, code, stacks and GPU-side buffers (none headless).

use std::alloc::{GlobalAlloc, Layout, System};
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use egui::{Context, Pos2, RawInput, vec2};
use markise_corpus::{Profile, generate_profile};
use markise_layout::model::{Container, Item, ItemKind, Marker, OutlineEntry, Segment};
use markise_ui_egui::{AppOptions, MarkiseApp, rss_mb};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);
static ALLOCS: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
/// Live bytes as the allocator rounds them (`malloc_good_size` on macOS).
static ROUNDED: AtomicIsize = AtomicIsize::new(0);

#[cfg(target_os = "macos")]
fn good_size(n: usize) -> usize {
    unsafe extern "C" {
        fn malloc_good_size(size: usize) -> usize;
    }
    // SAFETY: pure libmalloc query without side effects.
    unsafe { malloc_good_size(n) }
}

#[cfg(not(target_os = "macos"))]
fn good_size(n: usize) -> usize {
    n
}

thread_local! { static IN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
static TRACE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
fn trace(n: usize) {
    if n >= (4 << 20) && TRACE.load(Ordering::Relaxed) && !IN.with(|c| c.replace(true)) {
        let bt = std::backtrace::Backtrace::force_capture().to_string();
        let lines: Vec<&str> = bt.lines().filter(|l| l.contains("markise")).take(12).collect();
        eprintln!("BIG {} MB\n{}", n >> 20, lines.join("\n"));
        IN.with(|c| c.set(false));
    }
}
// SAFETY: forwards every call to `System` unchanged; only counts sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        trace(l.size());
        // SAFETY: same contract as the caller's.
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let now = LIVE.fetch_add(l.size() as isize, Ordering::Relaxed) + l.size() as isize;
            ROUNDED.fetch_add(good_size(l.size()) as isize, Ordering::Relaxed);
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            PEAK.fetch_max(now.max(0) as usize, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(p, l) };
        LIVE.fetch_sub(l.size() as isize, Ordering::Relaxed);
        ROUNDED.fetch_sub(good_size(l.size()) as isize, Ordering::Relaxed);
        ALLOCS.fetch_sub(1, Ordering::Relaxed);
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            let now = LIVE.fetch_add(l.size() as isize, Ordering::Relaxed) + l.size() as isize;
            ROUNDED.fetch_add(good_size(l.size()) as isize, Ordering::Relaxed);
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            PEAK.fetch_max(now.max(0) as usize, Ordering::Relaxed);
        }
        p
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        trace(new);
        // SAFETY: same contract as the caller's.
        let q = unsafe { System.realloc(p, l, new) };
        if !q.is_null() {
            let d = new as isize - l.size() as isize;
            ROUNDED.fetch_add(
                good_size(new) as isize - good_size(l.size()) as isize,
                Ordering::Relaxed,
            );
            let now = LIVE.fetch_add(d, Ordering::Relaxed) + d;
            PEAK.fetch_max(now.max(0) as usize, Ordering::Relaxed);
        }
        q
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[derive(Clone, Copy)]
struct Snap {
    live: isize,
    allocs: isize,
    rss: usize,
}

fn snap() -> Snap {
    Snap {
        live: LIVE.load(Ordering::Relaxed),
        allocs: ALLOCS.load(Ordering::Relaxed),
        rss: rss_mb(),
    }
}

/// Physical footprint in MB (what Activity Monitor shows as "Memory"; macOS only, else RSS):
/// dirty memory without pages the allocator has freed but not yet returned.
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

fn mb(b: isize) -> f64 {
    b as f64 / (1 << 20) as f64
}

fn phase(label: &str, a: Snap, b: Snap) {
    println!(
        "  {label:<34} live {:>+9.1} MB  allocs {:>+10}  RSS {:>+6} MB  (now live {:.0} MB, RSS {} MB)",
        mb(b.live - a.live),
        b.allocs - a.allocs,
        b.rss as isize - a.rss as isize,
        mb(b.live),
        b.rss
    );
}

#[derive(Default)]
struct Tally {
    rows: Vec<(&'static str, usize, usize)>,
}

impl Tally {
    fn add(&mut self, name: &'static str, bytes: usize, count: usize) {
        if let Some(r) = self.rows.iter_mut().find(|r| r.0 == name) {
            r.1 += bytes;
            r.2 += count;
        } else {
            self.rows.push((name, bytes, count));
        }
    }
    fn print(&self, title: &str, n: usize) {
        let total: usize = self.rows.iter().map(|r| r.1).sum();
        println!(
            "  {title}: {:.1} MB walked ({} B per unit over {n})",
            mb(total as isize),
            total / n.max(1)
        );
        for (name, bytes, count) in &self.rows {
            println!("    {name:<40} {:>8.1} MB  {count:>10}", mb(*bytes as isize));
        }
    }
}

fn heap_string(s: &str) -> usize {
    s.len()
}

fn walk_block(b: &markise_md::Block, t: &mut Tally) {
    use markise_md::BlockKind;
    t.add(
        "block: Vec<Span> lines (capacity)",
        b.lines.capacity() * size_of::<markise_md::Span>(),
        b.lines.len(),
    );
    t.add(
        "block: Vec<Block> children (capacity)",
        b.children.capacity() * size_of::<markise_md::Block>(),
        b.children.len(),
    );
    if let BlockKind::Table { aligns } = &b.kind {
        t.add("block: table aligns", aligns.capacity(), 1);
    }
    let kind = match b.kind {
        BlockKind::Paragraph => "kind: paragraph",
        BlockKind::Heading { .. } => "kind: heading",
        BlockKind::FencedCode { .. } | BlockKind::IndentedCode => "kind: code",
        BlockKind::MathBlock { .. } => "kind: math",
        BlockKind::Table { .. } => "kind: table",
        BlockKind::List { .. } => "kind: list",
        BlockKind::ListItem { .. } => "kind: list item",
        BlockKind::BlockQuote { .. } => "kind: quote",
        _ => "kind: other",
    };
    t.add(kind, 0, 1);
    for c in &b.children {
        walk_block(c, t);
    }
}

fn walk_doc(doc: &markise_md::Document) {
    let mut t = Tally::default();
    let mut tops = 0;
    for top in doc.tops() {
        tops += 1;
        walk_block(top.block, &mut t);
    }
    t.add(
        "top blocks inline (RelChunks items)",
        tops * size_of::<markise_md::Block>(),
        tops,
    );
    t.add("top block origins (RelChunks rel)", tops * size_of::<usize>(), tops);
    t.print("parsed document (excluding the text)", tops);
}

fn walk_container(c: &Container, t: &mut Tally) {
    match c {
        Container::ListItem {
            marker: Some(Marker::Number(s)),
            ..
        } => t.add("item: list number String", heap_string(s), 1),
        Container::Footnote { label: Some(s) } => t.add("item: footnote label String", heap_string(s), 1),
        _ => {}
    }
}

fn walk_item(it: &Item, t: &mut Tally) {
    t.add("item: inline (size_of::<Item>)", size_of::<Item>(), 1);
    if !it.containers.is_empty() {
        t.add(
            "item: Box<[Container]>",
            it.containers.len() * size_of::<Container>(),
            1,
        );
    }
    for c in it.containers.iter() {
        walk_container(c, t);
    }
    match &it.kind {
        ItemKind::Code { lang } | ItemKind::Diagram { lang } if !lang.is_empty() => {
            t.add("item: code lang", lang.len(), 1)
        }
        _ => {}
    }
    if let Some(s) = &it.segment {
        match s {
            Segment::Lines { spans, .. } => t.add("item: segment spans", spans.len() * 16, 1),
            Segment::Rows { data, .. } => t.add(
                "item: segment rows data",
                size_of::<markise_layout::model::RowsData>() + data.col_chars.len() * 4,
                1,
            ),
            Segment::Toc { .. } => {}
        }
    }
    if it.code.is_some() {
        t.add("item: Box<CodeRef>", size_of::<markise_layout::CodeRef>(), 1);
    }
    if it.raw.is_some() {
        t.add(
            "item: Box<RawPresentation>",
            size_of::<markise_doc::RawPresentation>(),
            1,
        );
    }
}

fn walk_model(m: &markise_layout::DocModel) {
    let mut t = Tally::default();
    for it in m.items.iter() {
        walk_item(it, &mut t);
    }
    t.add(
        "item origins (RelChunks rel)",
        m.items.len() * size_of::<usize>(),
        m.items.len(),
    );
    for o in &m.outline {
        t.add("outline: entries inline", size_of::<OutlineEntry>(), 1);
        t.add(
            "outline: text+anchor+key Strings",
            o.text.capacity() + o.anchor.capacity() + o.key.capacity(),
            3,
        );
    }
    t.print("view model", m.items.len());
}

fn sizes() {
    println!(
        "  size_of: Block {} B, BlockKind {} B, Span {} B, Item {} B, ItemKind {} B, Segment {} B, Container {} B, OutlineEntry {} B",
        size_of::<markise_md::Block>(),
        size_of::<markise_md::BlockKind>(),
        size_of::<markise_md::Span>(),
        size_of::<Item>(),
        size_of::<ItemKind>(),
        size_of::<Segment>(),
        size_of::<Container>(),
        size_of::<OutlineEntry>(),
    );
}

/// Engine pipeline alone: text, parsed document, model (with structure walks).
fn engine_child(src: &Path) {
    use markise_doc::{DocumentFormat, FormatOptions};
    sizes();
    let s0 = snap();
    let text = std::fs::read_to_string(src).expect("read corpus");
    let s1 = snap();
    phase("read text", s0, s1);
    let doc = markise_md::MarkdownFormat::default().parse(text, &FormatOptions::default());
    let s2 = snap();
    phase("parse (document minus text)", s1, s2);
    let md = doc
        .as_any()
        .downcast_ref::<markise_md::Document>()
        .expect("markdown document");
    walk_doc(md);
    let model = markise_layout::DocModel::build_owned(&*doc, None);
    let s3 = snap();
    phase("model build", s2, s3);
    println!(
        "  model: {} items, {} outline entries, {} B/item live",
        model.items.len(),
        model.outline.len(),
        (s3.live - s2.live) / model.items.len().max(1) as isize
    );
    walk_model(&model);
    let doc: std::sync::Arc<dyn markise_doc::FormatDoc> = std::sync::Arc::from(doc);
    let model = std::sync::Arc::new(model);
    let view = markise_layout::View::new(doc.clone(), model.clone(), markise_theme::Theme::light(), 600.0, 1.0);
    let s4 = snap();
    phase("View::new (heights, disclosures)", s3, s4);
    drop(view);
    let s5 = snap();
    let viewer = markise_ui_egui::viewer::DocViewer::new(
        std::sync::Arc::new(markise_md::MarkdownFormat::default()),
        doc.clone(),
        model.clone(),
        &markise_theme::Theme::light(),
        None,
    );
    let s6 = snap();
    phase("DocViewer::new (session + view)", s5, s6);
    drop(viewer);
    println!(
        "  peak live during the pipeline: {:.0} MB",
        mb(PEAK.load(Ordering::Relaxed) as isize)
    );
}

const W: f32 = 1300.0;
const H: f32 = 800.0;

fn frame(ctx: &Context, app: &mut MarkiseApp) {
    let raw = RawInput {
        screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(W, H))),
        ..Default::default()
    };
    let mut out = ctx.run_ui(raw, |ui| app.ui(ui));
    out.textures_delta.clear();
    let _prims = ctx.tessellate(out.shapes, out.pixels_per_point);
}

/// The real app: open, first frames, steady state.
fn app_child(src: &Path) {
    let s0 = snap();
    let ctx = Context::default();
    let mut app = MarkiseApp::new(AppOptions::default());
    frame(&ctx, &mut app);
    frame(&ctx, &mut app);
    let s1 = snap();
    phase("app without a document (fonts, UI)", s0, s1);
    PEAK.store(s1.live.max(0) as usize, Ordering::Relaxed);
    TRACE.store(std::env::var_os("MARKISE_MEM_TRACE").is_some(), Ordering::Relaxed);
    let t0 = Instant::now();
    app.open(src.to_path_buf());
    loop {
        frame(&ctx, &mut app);
        if app.model().is_some() && !app.is_loading() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
        assert!(t0.elapsed() < Duration::from_secs(600), "open timed out");
    }
    let s2 = snap();
    phase("open → first document frame", s1, s2);
    println!(
        "  peak live during open: {:.0} MB",
        mb(PEAK.load(Ordering::Relaxed) as isize)
    );
    // steady state: let background work (word count, the watcher's first read) settle — at
    // least 3 s, then until live bytes and RSS stay within 1 MB for 1 s (at most 20 s)
    let t1 = Instant::now();
    let mut last = (snap(), Instant::now());
    loop {
        frame(&ctx, &mut app);
        std::thread::sleep(Duration::from_millis(20));
        let now = snap();
        if (now.live - last.0.live).abs() > (1 << 20) || now.rss.abs_diff(last.0.rss) > 1 {
            last = (now, Instant::now());
        }
        if t1.elapsed() > Duration::from_secs(20)
            || (t1.elapsed() > Duration::from_secs(3) && last.1.elapsed() > Duration::from_secs(1))
        {
            break;
        }
    }
    let s3 = snap();
    phase(
        &format!("steady state ({:.1} s of frames)", t1.elapsed().as_secs_f64()),
        s2,
        s3,
    );
    phase("TOTAL document cost", s1, s3);
    let items = app.model().map_or(0, |m| m.items.len());
    let bytes = std::fs::metadata(src).map_or(1, |m| m.len()) as f64;
    let fp = footprint_mb();
    println!(
        "  steady state: live {:.0} MB = {:.2}× file (allocator-rounded {:.0} MB), RSS {} MB = {:.2}× file, footprint {fp} MB = {:.2}× file, {items} items, {} live allocations",
        mb(s3.live),
        s3.live as f64 / bytes,
        mb(ROUNDED.load(Ordering::Relaxed)),
        s3.rss,
        (s3.rss as f64 * (1 << 20) as f64) / bytes,
        (fp as f64 * (1 << 20) as f64) / bytes,
        s3.allocs
    );
    if let Some(secs) = std::env::var("MARKISE_MEM_PAUSE").ok().and_then(|v| v.parse().ok()) {
        // inspect the process from outside (`vmmap -summary`, `footprint`)
        println!("  pid {}: pausing {secs} s", std::process::id());
        std::thread::sleep(Duration::from_secs(secs));
    }
    drop(app);
}

fn main() {
    if std::env::args().any(|a| a == "--list") {
        return; // `cargo test --benches` probes the harness
    }
    if let Ok(p) = std::env::var("MARKISE_MEM_ENGINE_CHILD") {
        engine_child(&PathBuf::from(p));
        return;
    }
    if let Ok(p) = std::env::var("MARKISE_MEM_APP_CHILD") {
        app_child(&PathBuf::from(p));
        return;
    }
    if std::env::var_os("MARKISE_BENCH_MEMORY").is_none_or(|v| v == "0") {
        println!("memory: skipped (opt-in: MARKISE_BENCH_MEMORY=1, see the file header)");
        return;
    }
    let dir = std::env::var("MARKISE_HUGE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benches/corpus"));
    let mib: usize = std::env::var("MARKISE_HUGE_MIB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let profiles = std::env::var("MARKISE_HUGE_PROFILES").unwrap_or_else(|_| "mixed,prose,log".into());
    std::fs::create_dir_all(&dir).unwrap();
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
        println!("\n== {name}: {} ({bytes} bytes)", path.display());
        for var in ["MARKISE_MEM_ENGINE_CHILD", "MARKISE_MEM_APP_CHILD"] {
            let st = std::process::Command::new(std::env::current_exe().unwrap())
                .env(var, &path)
                .status()
                .unwrap();
            if !st.success() {
                println!("  child failed: {st}");
            }
        }
    }
}

//! Source editor (M2d slice 1, spec 02 §12.1, ADR 0006 §5.5), headless: `type-10mb` with soft
//! wrap on, open, and scroll-to-end with wrap — keystroke → source command → session edit →
//! incremental rendered model + source view update → caret follow → layout of the source
//! viewport, with the CPU test fonts (gate G3 budget: p99 ≤ 12 ms).
//!
//! Reported per kind and overall: p50 / p99 / max, wall and thread CPU. Painting, the gutter
//! (paint-time only) and presenting are not included; the app bench
//! (`cargo bench -p markise-ui-egui --bench source_editor`) measures whole frames.
//!
//! Run: `cargo bench -p markise-engine --bench source`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use markise_corpus::{Profile, generate_profile};
use markise_doc::{DocumentFormat, Params, Selection};
use markise_engine::editor::EditorView;
use markise_layout::source::SourceParams;
use markise_layout::testing;
use markise_md::MarkdownFormat;
use markise_theme::Theme;

const VH: f32 = 900.0;

fn thread_cpu() -> Duration {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: valid pointer to a timespec; CLOCK_THREAD_CPUTIME_ID is supported on macOS/Linux.
    unsafe {
        libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts);
    }
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

fn pct(v: &mut [Duration], q: f64) -> Duration {
    v.sort();
    v[((v.len() as f64 - 1.0) * q).round() as usize]
}

fn report(name: &str, mut v: Vec<Duration>) -> Duration {
    if v.is_empty() {
        return Duration::ZERO;
    }
    let n = v.len();
    let p50 = pct(&mut v, 0.5);
    let p99 = pct(&mut v, 0.99);
    let max = *v.last().unwrap();
    println!(
        "{name:<34} n={n:<4} p50 {:>7.3} ms   p99 {:>7.3} ms   max {:>7.3} ms",
        p50.as_secs_f64() * 1e3,
        p99.as_secs_f64() * 1e3,
        max.as_secs_f64() * 1e3
    );
    p99
}

fn run(label: &str, text: String) -> Duration {
    let mut fonts = testing::fonts();
    let format: Arc<dyn DocumentFormat> = Arc::new(MarkdownFormat::default());
    let t = Instant::now();
    let mut ed = EditorView::open(format, text, Theme::light(), 700.0);
    let parsed = t.elapsed();
    let t = Instant::now();
    ed.open_source(SourceParams::default(), 700.0, 2.0);
    let opened = t.elapsed();
    {
        let mut f = fonts.with_pixels_per_point(2.0);
        let t = Instant::now();
        ed.source_frame(VH, VH * 0.5, &mut f);
        println!(
            "{label}: parse+model {:.0} ms, source view build {:.1} ms, first source viewport {:.1} ms",
            parsed.as_secs_f64() * 1e3,
            opened.as_secs_f64() * 1e3,
            t.elapsed().as_secs_f64() * 1e3
        );
    }
    // scroll to the end with wrap (no O(lines) walk)
    {
        let mut f = fonts.with_pixels_per_point(2.0);
        let t = Instant::now();
        if let Some(p) = ed.source.as_mut() {
            p.view.scroll_to_y(f64::MAX);
        }
        let vis = ed.source_frame(VH, VH * 0.5, &mut f);
        let n = ed.source.as_ref().map_or(0, |p| p.view.len());
        println!(
            "{label}: scroll-to-end with wrap {:.2} ms (last item shown: {}, {} items)",
            t.elapsed().as_secs_f64() * 1e3,
            vis.last().is_some_and(|v| v.idx + 1 == n),
            n
        );
    }
    // caret in the middle of a paragraph line
    let text = ed.session.text().to_string();
    let mut at = text.len() / 2;
    while !text.is_char_boundary(at) || !text.as_bytes()[at].is_ascii_lowercase() {
        at += 1;
    }
    ed.source_set_selection(Selection::caret(at));
    {
        let mut f = fonts.with_pixels_per_point(2.0);
        ed.source_scroll_to_caret(VH, &mut f);
        ed.source_frame(VH, VH * 0.5, &mut f);
    }
    let mut script: Vec<(&str, &str, Params)> = Vec::new();
    for (i, c) in "the quick brown fox jumps over the lazy dog "
        .repeat(6)
        .chars()
        .enumerate()
    {
        script.push(("letter", "edit.insertText", Params::text("text", c.to_string())));
        if i % 17 == 16 {
            script.push(("backspace", "edit.deleteBackward", Params::new()));
        }
        if i % 41 == 40 {
            script.push(("enter", "edit.newline", Params::new()));
        }
        if i % 23 == 22 {
            script.push(("move", "caret.left", Params::new()));
            script.push(("move", "caret.right", Params::new()));
        }
        if i % 59 == 58 {
            script.push(("move", "caret.up", Params::new()));
            script.push(("move", "caret.down", Params::new()));
            script.push(("move", "caret.pageDown", Params::new()));
            script.push(("move", "caret.pageUp", Params::new()));
        }
        if i % 97 == 96 {
            script.push(("undo", "edit.undo", Params::new()));
        }
    }
    let mut by: std::collections::BTreeMap<&str, Vec<Duration>> = Default::default();
    let mut all = Vec::new();
    let mut cpu = Vec::new();
    for (kind, id, p) in &script {
        let mut f = fonts.with_pixels_per_point(2.0);
        let c0 = thread_cpu();
        let t0 = Instant::now();
        let _ = ed.source_execute(id, p, &mut f, VH);
        ed.source_scroll_to_caret(VH, &mut f);
        ed.source_frame(VH, VH * 0.5, &mut f);
        let d = t0.elapsed();
        cpu.push(thread_cpu().saturating_sub(c0));
        by.entry(kind).or_default().push(d);
        all.push(d);
    }
    for (k, v) in by {
        report(&format!("{label} {k}"), v);
    }
    report(&format!("{label} all keystrokes (wall)"), all);
    let p99 = report(&format!("{label} all keystrokes (thread CPU)"), cpu);
    println!(
        "{label}: document copies during the script: {} (expected 0: edits in place)",
        ed.session.copies
    );
    p99
}

fn main() {
    if std::env::args().any(|a| a == "--list") {
        return;
    }
    let path10 = format!("{}/../../benches/corpus/10mb.md", env!("CARGO_MANIFEST_DIR"));
    let ten =
        std::fs::read_to_string(&path10).unwrap_or_else(|_| generate_profile(10 << 20, 42, Profile::Mixed, false));
    let one = generate_profile(1 << 20, 42, Profile::Mixed, false);
    let a = run("source-type-1mb", one);
    let b = run("source-type-10mb", ten);
    // one 10 MB line: the piece path
    let c = run("source-type-long-line", "word ".repeat(2 << 20));
    let worst = a.max(b).max(c);
    println!(
        "source editor headless (event → laid-out viewport, thread CPU) worst p99 {:.3} ms vs budget 12 ms: {}",
        worst.as_secs_f64() * 1e3,
        if worst <= Duration::from_millis(12) {
            "PASS"
        } else {
            "FAIL"
        }
    );
}

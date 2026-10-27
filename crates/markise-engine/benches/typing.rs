//! `type-1mb` / `type-10mb` (ADR 0006 §7.6 gate G3, headless part; M2b): keystroke → command
//! → session edit → incremental model/view update → focus (reveal) → caret follow → layout of
//! the viewport, through the editing surface (`EditorView`) with the CPU test fonts, in the
//! middle of the 1 MB and 10 MB corpora.
//!
//! The script mixes letters (typing groups, word boundaries), Backspace, Enter (paragraph
//! split), caret moves (Left/Right/Up/Down) and Undo, then M2c keys (bullet rule, list
//! Enter/Tab/Shift+Tab/exit, heading rule, auto-pair, Select Word + Strong). Reported per kind and overall: p50 /
//! p99 / max. Painting/tessellation and presenting are not included (the desktop
//! `MARKISE_TIMING=1` log measures handle+layout+paint per keystroke).
//!
//! Run: `cargo bench -p markise-engine --bench typing`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use markise_corpus::{Profile, generate_profile};
use markise_doc::{DocPos, DocumentFormat, Params};
use markise_engine::editor::EditorView;
use markise_layout::block::NoImages;
use markise_layout::testing;
use markise_md::MarkdownFormat;
use markise_theme::Theme;

const VH: f32 = 900.0;

fn caret_near(text: &str, frac: f64) -> usize {
    let b = text.as_bytes();
    let mut p = ((text.len() as f64) * frac) as usize;
    while p + 2 < b.len()
        && !(b[p].is_ascii_lowercase() && b[p + 1].is_ascii_lowercase() && b[p - 1] != b'\n' && b[p - 1] != b'|')
    {
        p += 1;
    }
    p + 1
}

/// CPU time of the calling thread (separates scheduling noise on a loaded machine from work).
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
        "{name:<28} n={n:<4} p50 {:>7.3} ms   p99 {:>7.3} ms   max {:>7.3} ms",
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
    println!(
        "{label}: open (parse + model, setup) {:.0} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    let at = caret_near(ed.session.text(), 0.5);
    {
        let mut f = fonts.with_pixels_per_point(2.0);
        ed.set_caret(DocPos::new(at), false);
        ed.refresh_focus();
        ed.scroll_to_caret(VH, &mut f, &NoImages);
        ed.view.frame(VH, VH * 0.5, &mut f, &NoImages);
    }
    let script: Vec<(&str, &str, Params)> = {
        let mut v = Vec::new();
        let words = "the quick brown fox jumps over the lazy dog ";
        for (i, c) in words.repeat(5).chars().enumerate() {
            v.push(("letter", "edit.insertText", Params::text("text", c.to_string())));
            if i % 17 == 16 {
                v.push(("backspace", "edit.deleteBackward", Params::new()));
            }
            if i % 41 == 40 {
                v.push(("enter", "edit.newline", Params::new()));
            }
            if i % 23 == 22 {
                v.push(("move", "caret.left", Params::new()));
                v.push(("move", "caret.right", Params::new()));
            }
            if i % 59 == 58 {
                v.push(("move", "caret.up", Params::new()));
                v.push(("move", "caret.down", Params::new()));
            }
            if i % 97 == 96 {
                v.push(("undo", "edit.undo", Params::new()));
            }
        }
        // M2c: input rules, list keys, format commands, auto-pair (spec 02 §2–§5)
        let t = |s: &str| Params::text("text", s.to_string());
        for _ in 0..5 {
            v.push(("enter", "edit.newline", Params::new()));
            v.push(("rule", "edit.insertText", t("-")));
            v.push(("rule", "edit.insertText", t(" ")));
            v.push(("letter", "edit.insertText", t("a")));
            v.push(("list", "edit.newline", Params::new()));
            v.push(("letter", "edit.insertText", t("b")));
            v.push(("list", "edit.indentOrNextCell", Params::new()));
            v.push(("list", "edit.outdentOrPrevCell", Params::new()));
            v.push(("list", "edit.newline", Params::new()));
            v.push(("list", "edit.newline", Params::new()));
            for c in ["#", "#", " ", "H"] {
                v.push(("rule", "edit.insertText", t(c)));
            }
            v.push(("rule", "edit.newline", Params::new()));
            for c in ["w", "o", "r", "d", " ", "("] {
                v.push(("letter", "edit.insertText", t(c)));
            }
            v.push(("letter", "edit.insertText", t(")")));
            v.push(("move", "caret.left", Params::new()));
            v.push(("move", "caret.left", Params::new()));
            v.push(("format", "select.word", Params::new()));
            v.push(("format", "format.strong", Params::new()));
            v.push(("format", "format.strong", Params::new()));
            v.push(("undo", "edit.undo", Params::new()));
        }
        v
    };
    let kind = ed.session.doc().leaf(at).map(|l| l.kind.name());
    let len0 = ed.session.text().len();
    println!("{label}: caret at {at} in a {kind:?}");
    let mut by: std::collections::BTreeMap<&str, Vec<Duration>> = Default::default();
    let mut all = Vec::new();
    let mut cpu = Vec::new();
    for (kind, id, p) in &script {
        let mut f = fonts.with_pixels_per_point(2.0);
        let c0 = thread_cpu();
        let t0 = Instant::now();
        let _ = ed.execute(id, p, &mut f, &NoImages, VH);
        ed.refresh_focus();
        ed.scroll_to_caret(VH, &mut f, &NoImages);
        ed.view.frame(VH, VH * 0.5, &mut f, &NoImages);
        let d = t0.elapsed();
        cpu.push(thread_cpu().saturating_sub(c0));
        by.entry(kind).or_default().push(d);
        all.push(d);
    }
    for (k, v) in by {
        report(&format!("{label} {k}"), v);
    }
    report(&format!("{label} all keystrokes"), all);
    let p99 = report(&format!("{label} all (thread CPU)"), cpu);
    println!(
        "{label}: text grew by {} bytes, {} undo steps",
        ed.session.text().len() as isize - len0 as isize,
        ed.session.history().undo_len()
    );
    println!(
        "{label}: document copies during the script: {} (expected 0: edits in place)",
        ed.session.copies
    );
    p99
}

fn main() {
    let path10 = format!("{}/../../benches/corpus/10mb.md", env!("CARGO_MANIFEST_DIR"));
    let ten =
        std::fs::read_to_string(&path10).unwrap_or_else(|_| generate_profile(10 << 20, 42, Profile::Mixed, false));
    let one = generate_profile(1 << 20, 42, Profile::Mixed, false);
    let a = run("type-1mb", one);
    let b = run("type-10mb", ten);
    // one 2 MB list (Codex review of PR #35: typing must not scan the siblings)
    let mut list = String::with_capacity(2 << 20);
    let mut n = 0u64;
    while list.len() < 2 << 20 {
        n += 1;
        list.push_str(&format!("{n}. item {n} with some words in it\n"));
        if n.is_multiple_of(7) {
            list.push_str("   - nested child\n");
        }
    }
    let c = run("type-list-2mb", list);
    let worst = a.max(b).max(c);
    println!(
        "G3 headless (event → laid-out viewport, thread CPU) worst p99 {:.3} ms vs budget 12 ms: {}",
        worst.as_secs_f64() * 1e3,
        if worst <= Duration::from_millis(12) {
            "PASS"
        } else {
            "FAIL"
        }
    );
}

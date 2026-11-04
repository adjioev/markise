//! `:::` blocks on large documents (ADR 0018), headless through the editing surface with the CPU
//! test fonts (no painting): open, the first screen, and keystrokes in the middle of the same
//! corpus without blocks, with many small blocks, inside one block holding it all, and below a
//! stray opener. Also the edits that change a pair: a closer added or removed at the far end of
//! the text and undone, and an opener typed in the middle.
//!
//! Run: `cargo bench -p markise-engine --bench directives` (`MARKISE_DIRECTIVE_MB`, default 10).

use std::sync::Arc;
use std::time::{Duration, Instant};

use markise_corpus::generate;
use markise_doc::{DocPos, DocumentFormat, Params, PatchSet};
use markise_engine::editor::EditorView;
use markise_layout::block::NoImages;
use markise_layout::testing;
use markise_md::MarkdownFormat;
use markise_theme::Theme;

const VH: f32 = 900.0;

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn report(name: &str, mut v: Vec<Duration>) {
    if v.is_empty() {
        return;
    }
    v.sort();
    let q = |x: f64| ms(v[((v.len() as f64 - 1.0) * x).round() as usize]);
    println!(
        "  {name:<24} n={:<4} p50 {:>8.3}  p99 {:>8.3}  max {:>8.3} ms",
        v.len(),
        q(0.5),
        q(0.99),
        q(1.0)
    );
}

/// `text` with every `every` blocks that start on a clean line wrapped in a `:::` block.
fn with_cards(text: &str, every: usize) -> String {
    let d = markise_md::parse(text);
    let starts: Vec<usize> = d
        .tops()
        .filter_map(|t| t.clean_line())
        .enumerate()
        .filter(|(i, _)| i % every == 0)
        .map(|(_, s)| s)
        .skip(1)
        .collect();
    let mut out = String::with_capacity(text.len() + starts.len() * 24);
    let mut pos = starts.first().copied().unwrap_or(0);
    out.push_str(&text[..pos]);
    for (i, w) in starts.windows(2).enumerate() {
        out.push_str(&format!(":::card {i}\n\n"));
        out.push_str(&text[w[0]..w[1]]);
        out.push_str("\n\n:::\n\n");
        pos = w[1];
    }
    out.push_str(&text[pos..]);
    out
}

fn run(label: &str, text: String) {
    let markers = text.lines().filter(|l| l.starts_with(":::")).count();
    println!(
        "{label}: {:.1} MiB, {markers} marker-like lines",
        text.len() as f64 / (1 << 20) as f64
    );
    let mut fonts = testing::fonts();
    let format: Arc<dyn DocumentFormat> = Arc::new(MarkdownFormat::default());
    let t = Instant::now();
    let mut ed = EditorView::open(format, text, Theme::light(), 700.0);
    let open = t.elapsed();
    let t = Instant::now();
    {
        let mut f = fonts.with_pixels_per_point(2.0);
        ed.view.frame(VH, VH * 0.5, &mut f, &NoImages);
    }
    println!(
        "  open (parse + model) {:.0} ms, first screen {:.1} ms",
        ms(open),
        ms(t.elapsed())
    );
    // a paragraph's text in the middle
    let at = {
        let s = ed.session.text();
        let b = s.as_bytes();
        let mut p = s.len() / 2;
        while p + 2 < b.len()
            && !(b[p].is_ascii_lowercase() && b[p + 1].is_ascii_lowercase() && b[p - 1] != b'\n' && b[p - 1] != b'|')
            || !s.is_char_boundary(p + 1)
        {
            p += 1;
        }
        p + 1
    };
    {
        let mut f = fonts.with_pixels_per_point(2.0);
        ed.set_caret(DocPos::new(at), false);
        ed.refresh_focus();
        ed.scroll_to_caret(VH, &mut f, &NoImages);
        ed.view.frame(VH, VH * 0.5, &mut f, &NoImages);
    }
    macro_rules! key {
        ($id:expr, $p:expr) => {{
            let mut f = fonts.with_pixels_per_point(2.0);
            let t0 = Instant::now();
            let _ = ed.execute($id, $p, &mut f, &NoImages, VH);
            ed.refresh_focus();
            ed.scroll_to_caret(VH, &mut f, &NoImages);
            ed.view.frame(VH, VH * 0.5, &mut f, &NoImages);
            t0.elapsed()
        }};
    }
    let ch = |c: char| Params::text("text", c.to_string());
    let (mut letters, mut enters, mut backs) = (Vec::new(), Vec::new(), Vec::new());
    for (i, c) in "the quick brown fox jumps over the lazy dog "
        .repeat(5)
        .chars()
        .enumerate()
    {
        letters.push(key!("edit.insertText", &ch(c)));
        if i % 17 == 16 {
            backs.push(key!("edit.deleteBackward", &Params::new()));
        }
        if i % 41 == 40 {
            enters.push(key!("edit.newline", &Params::new()));
        }
    }
    report("letter", letters);
    report("backspace", backs);
    report("enter", enters);
    // a closer added or removed far from its opener: the whole range between changes meaning
    let (mut far, mut undo) = (Vec::new(), Vec::new());
    for _ in 0..3 {
        let mut f = fonts.with_pixels_per_point(2.0);
        let end = ed.session.text().len();
        let patch = if ed.session.text().ends_with("\n:::\n") {
            PatchSet::single_verbatim(end - 4..end, "")
        } else {
            PatchSet::single_verbatim(end..end, "\n:::\n")
        };
        let t0 = Instant::now();
        let _ = ed.session.apply_external(&patch, "Far", false);
        ed.sync();
        ed.view.frame(VH, VH * 0.5, &mut f, &NoImages);
        far.push(t0.elapsed());
        let t0 = Instant::now();
        ed.session.undo();
        ed.sync();
        ed.view.frame(VH, VH * 0.5, &mut f, &NoImages);
        undo.push(t0.elapsed());
    }
    report("far closer added/removed", far);
    report("far closer undone", undo);
    // an opener typed on a new line, Enter writes the closer
    let _ = key!("edit.newline", &Params::new());
    let mut opener = Vec::new();
    for c in ":::note".chars() {
        opener.push(key!("edit.insertText", &ch(c)));
    }
    opener.push(key!("edit.newline", &Params::new()));
    report("opener typed + Enter", opener);
}

fn main() {
    let mb: usize = std::env::var("MARKISE_DIRECTIVE_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let base = generate(mb << 20, 7);
    run("no blocks", base.clone());
    run("many small blocks", with_cards(&base, 8));
    run("one block holding it all", format!(":::whole\n\n{base}\n\n:::\n"));
    run("a stray opener on top", format!(":::stray\n\n{base}"));
}

//! Native block views on a large document (ADR 0021), headless through the editing surface with
//! the CPU test fonts (no painting): open, the first screen, scrolling and typing on the same
//! corpus with no blocks, with many blocks and no package, and with the same blocks shown natively;
//! and reading the document's language.
//!
//! Run: `cargo bench -p markise-engine --bench native_blocks` (`MARKISE_NATIVE_MB`, default 10).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use markise_corpus::generate;
use markise_doc::{DocPos, DocumentFormat, Params};
use markise_engine::block_page::document_language;
use markise_engine::block_view::PackageViews;
use markise_engine::editor::EditorView;
use markise_layout::block::NoImages;
use markise_layout::{BlockViews, testing};
use markise_md::MarkdownFormat;
use markise_package::PackageIndex;
use markise_theme::Theme;

const VH: f32 = 900.0;

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn report(name: &str, mut v: Vec<Duration>) {
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

fn package() -> Arc<PackageViews> {
    package_with(r#"{"tip": {"kind": "callout", "accent": "green"}, "poll": {"kind": "choices", "accent": "purple"}}"#)
}

/// The same blocks, with a practice for each kind of them.
fn practice_package() -> Arc<PackageViews> {
    package_with(
        r#"{"tip": {"kind": "callout", "practice": {"reveal": ["body"]}},
            "poll": {"kind": "choices", "practice": {"reveal": ["why"]}}}"#,
    )
}

fn package_with(views: &str) -> Arc<PackageViews> {
    let mut files = BTreeMap::new();
    let manifest = r#"{"format":"markise-block-package/2","library":"sample","version":"1","blocks":["tip","poll"]}"#;
    files.insert("manifest.json".to_string(), manifest.as_bytes().to_vec());
    let tip = r#"{"label": "Tip", "fields": [
        {"name": "title", "type": "heading", "level": 3, "min": 0}, {"name": "body", "type": "text", "min": 1}]}"#;
    let poll = r#"{"label": "Poll", "fields": [
        {"name": "question", "type": "heading", "level": 2},
        {"name": "answers", "type": "choices", "ids": "required", "minItems": 2, "correct": "exactly-one"},
        {"name": "why", "type": "quote", "min": 0}]}"#;
    files.insert("components/tip/tip.json".to_string(), tip.as_bytes().to_vec());
    files.insert("components/poll/poll.json".to_string(), poll.as_bytes().to_vec());
    files.insert("editor/palette.json".to_string(), br#"{"format": 1}"#.to_vec());
    files.insert("editor/views.json".to_string(), views.as_bytes().to_vec());
    let index = PackageIndex::from_files(&files).expect("a valid package");
    Arc::new(PackageViews::new(Arc::new(index), "en"))
}

/// `text` with a block after every `every` top-level blocks on a clean line.
fn with_blocks(text: &str, every: usize) -> String {
    let d = markise_md::parse(text);
    let starts: Vec<usize> = (d.tops().filter_map(|t| t.clean_line()).enumerate())
        .filter(|(i, _)| i % every == 0)
        .map(|(_, s)| s)
        .skip(1)
        .collect();
    let mut out = String::with_capacity(text.len() + starts.len() * 120);
    let mut pos = 0;
    for (i, &s) in starts.iter().enumerate() {
        out.push_str(&text[pos..s]);
        pos = s;
        if i % 2 == 0 {
            out.push_str(&format!(
                ":::tip\n\n### Title {i}\n\nThe body of the tip, a line of text.\n\n:::\n\n"
            ));
        } else {
            out.push_str(&format!(
                ":::poll\n\n## Which of these, number {i}?\n\n- [ ] This one {{#a1}}\n- [x] That one {{#a2}}\n\n> Because of how it is, number {i}.\n\n:::\n\n"
            ));
        }
    }
    out.push_str(&text[pos..]);
    out
}

fn run(label: &str, text: String, views: Option<Arc<PackageViews>>) {
    run_in(label, text, views, false);
}

fn run_in(label: &str, text: String, views: Option<Arc<PackageViews>>, practice: bool) {
    let fields = views.clone();
    let blocks = text.lines().filter(|l| l.starts_with(":::") && l.len() > 3).count();
    println!(
        "{label}: {:.1} MiB, {blocks} blocks",
        text.len() as f64 / (1 << 20) as f64
    );
    let mut fonts = testing::fonts();
    let format: Arc<dyn DocumentFormat> = Arc::new(MarkdownFormat::default());
    let t = Instant::now();
    let mut ed = EditorView::open(format, text, Theme::light(), 700.0);
    ed.view.set_block_views(views.map(|v| v as Arc<dyn BlockViews>));
    ed.set_block_fields(fields);
    if practice {
        ed.set_read_mode(true);
    }
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
    // a frame per wheel notch, as far as this goes: new items come into view on most of them
    let mut scroll = Vec::new();
    for _ in 0..200 {
        let mut f = fonts.with_pixels_per_point(2.0);
        let t0 = Instant::now();
        ed.view.scroll_by(120.0);
        ed.view.frame(VH, VH * 0.5, &mut f, &NoImages);
        scroll.push(t0.elapsed());
    }
    report("scroll frame", scroll);
    println!("  total height {:.0}", ed.view.total_height());
    if practice {
        // an answer, a check and a reset on a block on screen
        let mut press = Vec::new();
        let first = ed.session.text().find(":::poll").unwrap_or(0);
        let answer = first + ed.session.text()[first..].find("That one").unwrap_or(0);
        {
            let mut f = fonts.with_pixels_per_point(2.0);
            ed.view.scroll_to_y(0.0);
            ed.set_caret(DocPos::new(answer), false);
            ed.refresh_focus();
            ed.scroll_to_caret(VH, &mut f, &NoImages);
            ed.view.frame(VH, VH * 0.5, &mut f, &NoImages);
        }
        for _ in 0..50 {
            let mut f = fonts.with_pixels_per_point(2.0);
            let t0 = Instant::now();
            ed.practice_press(answer);
            ed.practice_check(first);
            ed.view.frame(VH, VH * 0.5, &mut f, &NoImages);
            ed.practice_reset(first);
            ed.view.frame(VH, VH * 0.5, &mut f, &NoImages);
            press.push(t0.elapsed() / 2);
        }
        report("check or reset, a frame", press);
    }
    // prose in the middle
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
    let mut letters = Vec::new();
    for c in "the quick brown fox jumps over the lazy dog ".repeat(5).chars() {
        let mut f = fonts.with_pixels_per_point(2.0);
        let t0 = Instant::now();
        let _ = ed.execute(
            "edit.insertText",
            &Params::text("text", c.to_string()),
            &mut f,
            &NoImages,
            VH,
        );
        ed.refresh_focus();
        ed.scroll_to_caret(VH, &mut f, &NoImages);
        ed.view.frame(VH, VH * 0.5, &mut f, &NoImages);
        letters.push(t0.elapsed());
    }
    report("typing in prose", letters);
    // a field of the block nearest to the middle
    let in_field = {
        let s = ed.session.text();
        s[s.len() / 2..]
            .find("The body of the tip")
            .map(|i| s.len() / 2 + i + 3)
    };
    if let Some(at) = in_field.filter(|&at| ed.view.native_block(at).is_some()) {
        {
            let mut f = fonts.with_pixels_per_point(2.0);
            ed.set_caret(DocPos::new(at), false);
            ed.refresh_focus();
            ed.scroll_to_caret(VH, &mut f, &NoImages);
            ed.view.frame(VH, VH * 0.5, &mut f, &NoImages);
        }
        let mut field = Vec::new();
        for c in "the quick brown fox jumps over the lazy dog ".repeat(5).chars() {
            let mut f = fonts.with_pixels_per_point(2.0);
            let t0 = Instant::now();
            let _ = ed.execute(
                "edit.insertText",
                &Params::text("text", c.to_string()),
                &mut f,
                &NoImages,
                VH,
            );
            ed.refresh_focus();
            ed.scroll_to_caret(VH, &mut f, &NoImages);
            ed.view.frame(VH, VH * 0.5, &mut f, &NoImages);
            field.push(t0.elapsed());
        }
        report("typing in a field", field);
    }
}

/// What an edit adds for a document with a package: the language read again, from the first block.
fn language_read(base: &str) {
    for (label, text) in [
        ("no front matter", base.to_string()),
        ("front matter", format!("---\ntitle: T\nlang: ru-RU\n---\n\n{base}")),
    ] {
        let doc = markise_md::parse(&text);
        let reads = (0..200)
            .map(|_| {
                let t = Instant::now();
                std::hint::black_box(document_language(&doc));
                t.elapsed()
            })
            .collect();
        report(&format!("language, {label}"), reads);
    }
}

fn main() {
    let mb: usize = (std::env::var("MARKISE_NATIVE_MB").ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let base = generate(mb << 20, 7);
    let blocks = with_blocks(&base, 3);
    language_read(&base);
    run("no blocks, no package", base.clone(), None);
    run("no blocks, a package", base, Some(package()));
    run("many blocks, no package", blocks.clone(), None);
    run("many blocks, shown natively", blocks.clone(), Some(package()));
    run(
        "many blocks, a practice, not in it",
        blocks.clone(),
        Some(practice_package()),
    );
    run_in("many blocks, in Practice", blocks, Some(practice_package()), true);
}

//! `cargo bench -p markise-theme`: CSS theme import (spec 03 §2).
//!
//! Theme import runs off the UI thread when a theme is selected or a theme file changes; the
//! budget is "well below a frame" for typical themes so switching feels instant. Uses an
//! original synthetic theme; the community corpus (if fetched into `.theme-cache/`) is timed by
//! `cargo test -p markise-theme --release --test corpus -- --nocapture`.

use std::path::Path;

use criterion::{Criterion, criterion_group, criterion_main};
use markise_theme::{ImportOptions, import_css};

/// An original theme of roughly community-theme size (~300 rules, variables, media queries).
fn synthetic_theme() -> String {
    let mut css = String::from(
        ":root { --bg-color: #fbfaf7; --text-color: #2b2b2b; --accent: #9a3b1b; --monospace: \"Mono\", monospace; }\n\
         html { font-size: 16px } body { background: var(--bg-color); color: var(--text-color) }\n\
         #write { max-width: 860px; padding: 30px 40px; line-height: 1.6 }\n",
    );
    for i in 1..=6 {
        css.push_str(&format!(
            "#write h{i} {{ font-size: {}rem; margin: 1.2em 0 .6em; color: var(--accent, #333) }}\n",
            2.2 - i as f32 * 0.2
        ));
    }
    for i in 0..280 {
        css.push_str(&format!(
            "#typora-sidebar .item-{i}:hover, .cm-s-inner .cm-t{i} {{ color: #{:06x}; border-left: 2px solid var(--accent); transition: all .2s }}\n",
            i * 997 % 0xffffff
        ));
    }
    css.push_str("@media (prefers-color-scheme: dark) { :root { --bg-color: #1e1d1b; --text-color: #e6e1d6 } }\n");
    css.push_str("@media print { #write { max-width: none } }\n");
    css
}

fn bench(c: &mut Criterion) {
    let css = synthetic_theme();
    let opts = ImportOptions::default();
    let mut g = c.benchmark_group("theme-import");
    g.bench_function("synthetic-300-rules", |b| {
        b.iter(|| import_css(&css, Path::new("."), &opts))
    });
    let big: String = std::iter::repeat_n(css.as_str(), 20).collect();
    g.sample_size(20);
    g.bench_function("synthetic-6000-rules", |b| {
        b.iter(|| import_css(&big, Path::new("."), &opts))
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);

//! Community theme corpus (spec 03 §2, C6): every theme imports without panic, yields sane
//! colors and fonts, and the import report's coverage is printed.
//!
//! The themes are test input only, fetched by `scripts/fetch-theme-corpus.sh` into the
//! gitignored `.theme-cache/` (override with `MARKISE_THEME_CACHE`). Without the cache the test
//! prints a skip notice, and fails when `CI=true`.
//!
//! `cargo test -p markise-theme --test corpus -- --nocapture` prints the per-theme table and
//! the corpus-wide frequency of unsupported properties.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use markise_theme::{ImportOptions, Theme, import_theme_file};

struct Entry {
    id: String,
    repo: String,
    commit: String,
    css: String,
}

fn manifest() -> Vec<Entry> {
    let text = include_str!("theme-corpus.tsv");
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            assert!(f.len() >= 4, "bad manifest line: {l}");
            Entry {
                id: f[0].into(),
                repo: f[1].into(),
                commit: f[2].into(),
                css: f[3].into(),
            }
        })
        .collect()
}

fn cache_dir() -> PathBuf {
    std::env::var_os("MARKISE_THEME_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.theme-cache"))
}

fn skip_or_fail(msg: &str) {
    let ci = std::env::var("CI").map(|v| v == "true" || v == "1").unwrap_or(false);
    if ci {
        panic!("theme corpus unavailable in CI: {msg}");
    }
    // written to the raw stderr handle so libtest's output capture does not hide it
    use std::io::Write;
    let _ = writeln!(
        std::io::stderr(),
        "THEME CORPUS SKIPPED: {msg} — run scripts/fetch-theme-corpus.sh to download the test themes"
    );
}

#[test]
fn corpus_themes_import() {
    let entries = manifest();
    assert!(entries.len() >= 15, "corpus should list 15-20 themes");
    let cache = cache_dir();
    if !cache.is_dir() {
        skip_or_fail(&format!("{} does not exist", cache.display()));
        return;
    }
    let missing: Vec<&str> = entries
        .iter()
        .filter(|e| !path_of(&cache, e).is_file())
        .map(|e| e.id.as_str())
        .collect();
    if !missing.is_empty() {
        skip_or_fail(&format!("theme files missing for {missing:?}"));
        return;
    }

    let light = Theme::light();
    let dark = Theme::dark();
    let mut freq: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut at_rules: BTreeMap<String, usize> = BTreeMap::new();
    let mut ignored: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut total_cov = 0.0;
    println!(
        "{:<17} {:>4} {:>6} {:>6} {:>7} {:>6} {:>6} {:>5}  notable unsupported",
        "theme", "dark", "scheme", "rules", "matched", "cov%", "ign%", "ms"
    );
    for e in &entries {
        let path = path_of(&cache, e);
        let start = Instant::now();
        let r = match std::panic::catch_unwind(|| import_theme_file(&path, &ImportOptions::default())) {
            Ok(Ok(r)) => r,
            Ok(Err(err)) => {
                failures.push(format!("{}: io error {err}", e.id));
                continue;
            }
            Err(_) => {
                failures.push(format!("{}: PANIC", e.id));
                continue;
            }
        };
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        let t = &r.theme;
        let rep = &r.report;
        let cov = rep.coverage();
        total_cov += cov;

        // sanity: colors are imported. Background/text must change where the theme defines
        // them through the well-known variables; every theme must change several color slots.
        let base = if t.page_bg.brightness() < 0.5 { &dark } else { &light };
        let css = std::fs::read_to_string(&path).unwrap_or_default();
        let defines_page = css.contains("--bg-color:") || css.contains("--text-color:");
        if defines_page && t.page_bg == base.page_bg && t.text == base.text {
            failures.push(format!("{}: neither background nor text color imported", e.id));
        }
        let changed = changed_color_slots(t, base);
        if changed < 3 {
            failures.push(format!(
                "{}: only {changed} color slots differ from the base theme",
                e.id
            ));
        }
        for v in std::iter::once(t).chain(r.dark_variant.as_ref()) {
            if !(v.font_size.is_finite() && (6.0..=72.0).contains(&v.font_size)) {
                failures.push(format!("{}: font size {}", e.id, v.font_size));
            }
            if !(0.8..=4.0).contains(&v.line_height) {
                failures.push(format!("{}: line height {}", e.id, v.line_height));
            }
            if v.headings.iter().any(|h| !(6.0..=120.0).contains(&h.size)) {
                failures.push(format!("{}: heading sizes", e.id));
            }
            // text must be distinguishable from the page
            let contrast = (v.text.brightness() - v.page_bg.brightness()).abs();
            if contrast < 0.25 {
                failures.push(format!("{}: low text/page contrast {contrast:.2}", e.id));
            }
        }
        if cov < 0.5 {
            failures.push(format!("{}: coverage {:.0}%", e.id, cov * 100.0));
        }

        for i in &rep.unsupported_properties {
            let f = freq.entry(i.name.clone()).or_default();
            f.0 += i.count;
            f.1 += 1;
        }
        for i in &rep.unsupported_values {
            let name = format!("value of {}", i.name.split(':').next().unwrap_or(""));
            let f = freq.entry(name).or_default();
            f.0 += i.count;
            f.1 += 1;
        }
        for i in &rep.ignored_properties {
            let f = ignored.entry(i.name.clone()).or_default();
            f.0 += i.count;
            f.1 += 1;
        }
        for i in &rep.unsupported_at_rules {
            *at_rules.entry(i.name.clone()).or_default() += i.count;
        }
        let notable: Vec<String> = rep
            .unsupported_properties
            .iter()
            .take(3)
            .map(|i| format!("{}×{}", i.name, i.count))
            .chain(
                rep.unsupported_values
                    .iter()
                    .take(2)
                    .map(|i| format!("[{}]×{}", i.name, i.count)),
            )
            .collect();
        println!(
            "{:<17} {:>4} {:>6} {:>6} {:>7} {:>6.1} {:>6.1} {:>5.1}  {}",
            e.id,
            if t.dark { "yes" } else { "no" },
            if rep.scheme_aware { "yes" } else { "no" },
            rep.rules,
            rep.selectors_matched,
            cov * 100.0,
            rep.ignored as f64 / rep.content_declarations.max(1) as f64 * 100.0,
            ms,
            notable.join(", ")
        );
        let _ = (&e.repo, &e.commit);
    }
    println!("\nmean coverage: {:.1}%", total_cov / entries.len() as f64 * 100.0);
    let mut f: Vec<_> = freq.into_iter().collect();
    f.sort_by_key(|a| std::cmp::Reverse(a.1.0));
    println!("\nunsupported properties/values across the corpus (occurrences, themes):");
    for (name, (n, themes)) in f.iter().take(25) {
        println!("  {name:<40} {n:>5} {themes:>3}");
    }
    let mut ig: Vec<_> = ignored.into_iter().collect();
    ig.sort_by_key(|a| std::cmp::Reverse(a.1.0));
    println!("\ndeliberately ignored properties across the corpus (occurrences, themes):");
    for (name, (n, themes)) in ig.iter().take(20) {
        println!("  {name:<40} {n:>5} {themes:>3}");
    }
    let mut a: Vec<_> = at_rules.into_iter().collect();
    a.sort_by_key(|x| std::cmp::Reverse(x.1));
    println!("\nskipped at-rules / media features:");
    for (name, n) in a.iter().take(15) {
        println!("  {name:<40} {n:>5}");
    }
    assert!(failures.is_empty(), "corpus failures:\n{}", failures.join("\n"));
}

fn changed_color_slots(t: &Theme, b: &Theme) -> usize {
    let pairs = [
        (t.page_bg, b.page_bg),
        (t.text, b.text),
        (t.link, b.link),
        (t.headings[0].color, b.headings[0].color),
        (t.headings[1].color, b.headings[1].color),
        (t.inline_code.background, b.inline_code.background),
        (t.inline_code.text, b.inline_code.text),
        (t.code_block.background, b.code_block.background),
        (t.code_block.text, b.code_block.text),
        (t.quote_bar, b.quote_bar),
        (t.quote_text, b.quote_text),
        (t.table_border, b.table_border),
        (t.table_header_bg, b.table_header_bg),
        (t.rule, b.rule),
        (t.highlight_bg, b.highlight_bg),
        (t.chrome.sidebar_bg, b.chrome.sidebar_bg),
        (t.syntax.keyword, b.syntax.keyword),
    ];
    pairs.iter().filter(|(a, b)| a != b).count()
}

fn path_of(cache: &std::path::Path, e: &Entry) -> PathBuf {
    let short = &e.commit[..12.min(e.commit.len())];
    cache
        .join(format!("{}-{short}", e.repo.replace('/', "__")))
        .join(&e.css)
}

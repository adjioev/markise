//! `editor-keystroke` (ADR 0006 §7.7, gate G1 at API level): apply + undo of one keystroke patch
//! on the 10 MB corpus, caret at start / middle / end, through
//! - `FormatDoc::apply` alone (memmove + bounded re-parse + span shift; the inverse patch undoes),
//! - the session typing pipeline (`insert_text`: rules check, canonical patch, apply, marks,
//!   history) and `undo`.
//!
//! - `DocModel::apply` (items splice + re-flatten of the re-parsed blocks, dependency checks) and
//!   `View::apply` (heights splice) for the same keystroke (G1 second half: items/heights
//!   splice p99 ≤ 1.5 ms);
//! - G2 cases (`FormatDoc::apply` + `DocModel::apply` p99 ≤ 4 ms): typing, Backspace and Enter
//!   in the middle and at the end of a 2 MB list, nested list and block quote (nested restart
//!   checkpoints, ADR 0006 §7.2.3), typing in a 2 MB code block and a 512 KB table, edits far
//!   below an unclosed `$$` near the top; fence completion through the session.
//!
//! Reports p50 / p99 / max per stage (custom harness: criterion reports means, the gates are
//! p99). G1 budget: memmove + span shift p99 ≤ 1.5 ms.
//!
//! Run: `cargo bench -p markise-engine --bench editor` (`MARKISE_BENCH_CORPUS=<file>` replaces
//! the 10 MB corpus).

use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use markise_corpus::{Profile, generate_profile};
use markise_doc::{DocumentFormat, FormatDoc, FormatOptions, PatchSet, Selection};
use markise_engine::editor::EditorSession;
use markise_layout::{DocModel, View};
use markise_md::MarkdownFormat;
use markise_theme::Theme;

const ROUNDS: usize = 400;
const WARMUP: usize = 20;

/// The 10 MB corpus, or the file named by `MARKISE_BENCH_CORPUS` (e.g. a 100 MiB scaling
/// corpus from `markise-corpus --huge`).
fn corpus() -> String {
    let path = std::env::var("MARKISE_BENCH_CORPUS")
        .unwrap_or_else(|_| format!("{}/../../benches/corpus/10mb.md", env!("CARGO_MANIFEST_DIR")));
    std::fs::read_to_string(&path).unwrap_or_else(|_| generate_profile(10 << 20, 42, Profile::Mixed, false))
}

/// A caret position inside a paragraph line near `frac` of the text: after an ASCII letter
/// followed by another letter (an intra-word keystroke).
fn caret_near(text: &str, frac: f64) -> usize {
    let b = text.as_bytes();
    let mut p = ((text.len() as f64) * frac) as usize;
    p = p.min(text.len().saturating_sub(2));
    while p + 1 < b.len() && !(b[p].is_ascii_lowercase() && b[p + 1].is_ascii_lowercase()) {
        p += 1;
    }
    if p + 1 >= b.len() {
        p = 0;
        while !(b[p].is_ascii_lowercase() && b[p + 1].is_ascii_lowercase()) {
            p += 1;
        }
    }
    p + 1
}

fn pct(v: &mut [Duration], q: f64) -> Duration {
    v.sort();
    v[((v.len() as f64 - 1.0) * q).round() as usize]
}

fn report(name: &str, mut v: Vec<Duration>) -> Duration {
    let p50 = pct(&mut v, 0.5);
    let p99 = pct(&mut v, 0.99);
    let max = *v.last().unwrap();
    println!(
        "{name:<34} p50 {:>8.3} ms   p99 {:>8.3} ms   max {:>8.3} ms",
        p50.as_secs_f64() * 1e3,
        p99.as_secs_f64() * 1e3,
        max.as_secs_f64() * 1e3
    );
    p99
}

fn main() {
    let text = corpus();
    println!(
        "editor-keystroke: corpus {} bytes, {ROUNDS} rounds per position",
        text.len()
    );
    let format: Arc<dyn DocumentFormat> = Arc::new(MarkdownFormat::default());
    let t = Instant::now();
    let mut doc: Box<dyn FormatDoc> = format.parse(text.clone(), &FormatOptions::default());
    println!(
        "parse (setup)                      {:>8.1} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    // baseline: the memmove alone (String insert + remove of one byte)
    {
        let mut raw = text.clone();
        raw.reserve(16);
        for (label, frac) in [("start", 0.0), ("middle", 0.5)] {
            let at = caret_near(&raw, frac);
            let mut v = Vec::with_capacity(ROUNDS);
            for i in 0..ROUNDS + WARMUP {
                let t0 = Instant::now();
                raw.insert(at, 'x');
                black_box(&raw);
                raw.remove(at);
                if i >= WARMUP {
                    v.push(t0.elapsed() / 2);
                }
            }
            report(&format!("memmove only (per op)      [{label}]"), v);
        }
    }
    let mut worst_apply = Duration::ZERO;
    for (label, frac) in [("start", 0.0), ("middle", 0.5), ("end", 0.999)] {
        let at = caret_near(doc.text(), frac);
        let fwd = PatchSet::single_verbatim(at..at, "x");
        let removed = fwd.removed_from(doc.text());
        let inv = fwd.inverse(&removed).unwrap();
        let (mut a, mut u) = (Vec::with_capacity(ROUNDS), Vec::with_capacity(ROUNDS));
        for i in 0..ROUNDS + WARMUP {
            let t0 = Instant::now();
            black_box(doc.apply(&fwd));
            let t1 = Instant::now();
            black_box(doc.apply(&inv));
            let t2 = Instant::now();
            if i >= WARMUP {
                a.push(t1 - t0);
                u.push(t2 - t1);
            }
        }
        assert_eq!(doc.text().len(), text.len());
        worst_apply = worst_apply.max(report(&format!("FormatDoc::apply keystroke [{label}]"), a));
        worst_apply = worst_apply.max(report(&format!("FormatDoc::apply inverse   [{label}]"), u));
    }

    // G1 second half + G2 fast case: model and view updates for the same keystrokes
    let mut worst_model = Duration::ZERO;
    {
        let mut model = DocModel::build_owned(&*doc, None);
        let shared: Arc<dyn FormatDoc> = Arc::from(doc.clone_box());
        let mut view = View::new(shared.clone(), Arc::new(model.clone()), Theme::light(), 700.0, 2.0);
        println!("items: {}", model.items.len());
        for (label, frac) in [("start", 0.0), ("middle", 0.5), ("end", 0.999)] {
            let at = caret_near(doc.text(), frac);
            let fwd = PatchSet::single_verbatim(at..at, "x");
            let removed = fwd.removed_from(doc.text());
            let inv = fwd.inverse(&removed).unwrap();
            let (mut a, mut m, mut v) = (Vec::new(), Vec::new(), Vec::new());
            for i in 0..ROUNDS + WARMUP {
                for ps in [&fwd, &inv] {
                    let t0 = Instant::now();
                    let e = doc.apply(ps);
                    let t1 = Instant::now();
                    let ch = model.apply(&*doc, &e);
                    let t2 = Instant::now();
                    // the Arc<DocModel> clone stands for handing the model to the view; it is
                    // not part of the measured splice
                    let shared_model = Arc::new(model.clone());
                    // keep the previous model alive: freeing a whole model clone is a bench
                    // artifact (a session hands the view an updated model without cloning)
                    let prev = view.model().clone();
                    let t3 = Instant::now();
                    view.apply(shared.clone(), shared_model, &ch);
                    let t4 = Instant::now();
                    drop(prev);
                    if i >= WARMUP {
                        a.push(t1 - t0);
                        m.push(t2 - t1);
                        v.push(t4 - t3);
                    }
                }
            }
            report(&format!("  apply (again)          [{label}]"), a);
            worst_model = worst_model.max(report(&format!("DocModel::apply keystroke [{label}]"), m));
            worst_model = worst_model.max(report(&format!("View::apply heights      [{label}]"), v));
        }
        // a structural keystroke: Enter splits a paragraph (one more item, heights spliced)
        let at = caret_near(doc.text(), 0.0);
        let fwd = PatchSet::single_verbatim(at..at, "\n\n");
        let removed = fwd.removed_from(doc.text());
        let inv = fwd.inverse(&removed).unwrap();
        let (mut m, mut v) = (Vec::new(), Vec::new());
        for i in 0..ROUNDS / 2 + WARMUP {
            for ps in [&fwd, &inv] {
                let e = doc.apply(ps);
                let t1 = Instant::now();
                let ch = model.apply(&*doc, &e);
                let t2 = Instant::now();
                let shared_model = Arc::new(model.clone());
                let prev = view.model().clone();
                let t3 = Instant::now();
                view.apply(shared.clone(), shared_model, &ch);
                let t4 = Instant::now();
                drop(prev);
                if i >= WARMUP {
                    m.push(t2 - t1);
                    v.push(t4 - t3);
                }
            }
        }
        worst_model = worst_model.max(report("DocModel::apply Enter    [start]", m));
        worst_model = worst_model.max(report("View::apply heights Enter [start]", v));
        model
            .diff(&DocModel::build_owned(&*doc, None))
            .expect("incremental == full");
    }
    println!(
        "G1 (items/heights splice) worst p99 {:.3} ms vs budget 1.5 ms: {}",
        worst_model.as_secs_f64() * 1e3,
        if worst_model <= Duration::from_micros(1500) {
            "PASS"
        } else {
            "FAIL"
        }
    );
    g2(&format);

    let mut s = EditorSession::new(format, text.clone());
    let mut worst_session = Duration::ZERO;
    for (label, frac) in [("start", 0.0), ("middle", 0.5), ("end", 0.999)] {
        let at = caret_near(s.text(), frac);
        let (mut a, mut u) = (Vec::with_capacity(ROUNDS), Vec::with_capacity(ROUNDS));
        for i in 0..ROUNDS + WARMUP {
            s.set_selection(Selection::caret(at));
            let t0 = Instant::now();
            s.insert_text("x").unwrap();
            let t1 = Instant::now();
            assert!(s.undo());
            let t2 = Instant::now();
            if i >= WARMUP {
                a.push(t1 - t0);
                u.push(t2 - t1);
            }
        }
        assert_eq!(s.text(), text);
        worst_session = worst_session.max(report(&format!("session insert_text  [{label}]"), a));
        worst_session = worst_session.max(report(&format!("session undo         [{label}]"), u));
    }
    // G2 case (Codex review): the keystroke completing ``` on a fresh paragraph mid-document.
    // Text and the EnterPending hint are applied atomically, so no suffix re-parse as code.
    {
        let b = s.text().as_bytes();
        let mut at = s.text().len() / 2;
        while !(b[at - 1] == b'\n' && b[at] == b'\n' && b[at + 1] == b'\n') {
            at += 1;
        }
        let (mut a, mut u) = (Vec::with_capacity(ROUNDS), Vec::with_capacity(ROUNDS));
        for i in 0..ROUNDS / 4 + WARMUP {
            s.set_selection(Selection::caret(at));
            s.insert_text("`").unwrap();
            s.insert_text("`").unwrap();
            let t0 = Instant::now();
            s.insert_text("`").unwrap();
            let t1 = Instant::now();
            while s.text().len() != text.len() {
                assert!(s.undo());
            }
            let t2 = Instant::now();
            if i >= WARMUP {
                a.push(t1 - t0);
                u.push(t2 - t1);
            }
        }
        assert_eq!(s.text(), text);
        report("session ``` completion [middle]", a);
        report("session undo of ``` group [middle]", u);
    }
    println!(
        "G1 (memmove + span shift, incl. bounded re-parse) worst p99 {:.3} ms vs budget 1.5 ms: {}",
        worst_apply.as_secs_f64() * 1e3,
        if worst_apply <= Duration::from_micros(1500) {
            "PASS"
        } else {
            "FAIL"
        }
    );
    println!(
        "session keystroke/undo worst p99 {:.3} ms (NFR-4 share: apply 3 ms + marks/history 0.2 ms)",
        worst_session.as_secs_f64() * 1e3
    );
}

/// Builds a document of about `size` bytes from `line(i)` lines.
fn lines_doc(size: usize, line: impl Fn(usize) -> String) -> String {
    let mut s = String::with_capacity(size + 64);
    let mut i = 0;
    while s.len() < size {
        s.push_str(&line(i));
        i += 1;
    }
    s
}

/// Offset just after "words" + 2 in the line containing `frac` of `text` (inside item text),
/// and the end of that line.
fn spot(text: &str, frac: f64) -> (usize, usize) {
    let mut at = (text.len() as f64 * frac) as usize;
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    let w = text[..at]
        .rfind("words")
        .or_else(|| text[at..].find("words").map(|k| k + at))
        .unwrap();
    let mid = w + 2;
    (mid, mid + text[mid..].find('\n').unwrap())
}

/// G2 (ADR 0006 §7.6): `FormatDoc::apply` + `DocModel::apply` per keystroke for the §7.2
/// cases: typing, Backspace and Enter in the middle and at the end of huge containers (flat
/// list, nested list, block quote), typing in a huge code block and table, edits below an
/// unclosed `$$`; plus fence completion through the session (text + hint atomically).
fn g2(format: &Arc<dyn DocumentFormat>) {
    let mut worst = Duration::ZERO;
    let mb2 = 2 << 20;
    let flat = lines_doc(mb2, |i| format!("- list item number {i} with some words\n"));
    let nested = lines_doc(mb2, |i| {
        if i % 2500 == 0 {
            format!("- top item {i} words\n")
        } else {
            format!("  - nested item {i} with some words\n")
        }
    });
    let quote = lines_doc(mb2, |i| format!("> quoted paragraph {i} with some words\n>\n"));
    let code = format!(
        "```rust\n{}```\n",
        lines_doc(mb2, |i| format!("let words_{i} = {i}; // words\n"))
    );
    let table = format!(
        "| a | b |\n|---|---|\n{}",
        lines_doc(mb2 / 4, |i| format!("| cell {i} words | {i} |\n"))
    );
    let docs: [(&str, String, &str); 3] = [
        ("2 MB list", flat, "\n- "),
        ("2 MB nested list", nested, "\n  - "),
        ("2 MB quote", quote, "\n> "),
    ];
    for (name, body, enter) in &docs {
        let text = format!("# Doc\n\nintro\n\n{body}\nafter\n");
        for (pos, frac) in [("middle", 0.5), ("end", 0.995)] {
            let (mid, eol) = spot(&text, frac);
            let cases: [(&str, std::ops::Range<usize>, &str); 4] = [
                ("type", mid..mid, "x"),
                ("Backspace", mid - 1..mid, ""),
                ("Enter mid-line", mid..mid, enter),
                ("Enter at line end", eol..eol, enter),
            ];
            for (what, r, ins) in cases {
                worst = worst.max(case(format, &text, r, ins, &format!("{what} {name} [{pos}]")));
            }
        }
    }
    for (name, body) in [("2 MB code block", &code), ("512 KB table", &table)] {
        let text = format!("# Doc\n\nintro\n\n{body}\nafter\n");
        let (mid, _) = spot(&text, 0.5);
        worst = worst.max(case(format, &text, mid..mid, "x", &format!("type {name} [middle]")));
        worst = worst.max(case(
            format,
            &text,
            mid - 1..mid,
            "",
            &format!("Backspace {name} [middle]"),
        ));
    }
    // an unclosed `$$` near the top of the 10 MB corpus, edits far below
    let body = corpus();
    let text = format!("intro\n\n$$\nunclosed\n\n{body}");
    let at = caret_near(&text, 0.8);
    worst = worst.max(case(format, &text, at..at, "x", "type below unclosed $$"));
    let b = text.as_bytes();
    let mut blank = text.len() * 4 / 5;
    while !(b[blank - 1] == b'\n' && b[blank] == b'\n') {
        blank += 1;
    }
    worst = worst.max(case(
        format,
        &text,
        blank..blank,
        "\n# New heading\n",
        "heading below unclosed $$",
    ));
    println!(
        "G2 (FormatDoc::apply + DocModel::apply) worst p99 {:.3} ms vs budget 4 ms: {}",
        worst.as_secs_f64() * 1e3,
        if worst <= Duration::from_millis(4) {
            "PASS"
        } else {
            "FAIL"
        }
    );
    // Not keystroke-class (reported, outside the gate): a blank line that makes a tight 2 MB
    // list loose (and its undo) changes the rendering of every item, so the model re-flattens
    // the whole list once (§7.2.5 accepted block-local cost); the parse itself stays nested.
    let flat = &docs[0].1;
    let text = format!("# Doc\n\nintro\n\n{flat}\nafter\n");
    let (_, eol) = spot(&text, 0.5);
    case(
        format,
        &text,
        eol..eol,
        "\n",
        "tightness flip 2 MB list [middle] (info)",
    );
    // fence completion through the session: the keystroke completing ``` (text and the
    // EnterPending hint in one apply, no re-parse of the suffix as code) at a paragraph start
    // of the 10 MB corpus, and at an item start of a 2 MB list (no hint there: the fence opens
    // and runs to the end of its item)
    let line_start = text[..eol].rfind('\n').unwrap() + 1;
    for (label, src, at) in [
        ("``` at a paragraph start (10 MB)", corpus(), None),
        ("``` at an item start (2 MB list)", text.clone(), Some(line_start + 2)),
    ] {
        session_fence(format, &src, at, label);
    }
}

/// Session typing of three backticks; reports the third keystroke and the undo of the group.
fn session_fence(format: &Arc<dyn DocumentFormat>, text: &str, at: Option<usize>, label: &str) {
    let mut s = EditorSession::new(format.clone(), text.to_string());
    let at = at.unwrap_or_else(|| {
        let b = s.text().as_bytes();
        let mut at = s.text().len() / 2;
        while !(b[at - 1] == b'\n' && b[at] == b'\n' && b[at + 1] == b'\n') {
            at += 1;
        }
        at
    });
    let (mut a, mut u) = (Vec::new(), Vec::new());
    for i in 0..ROUNDS / 4 + WARMUP {
        s.set_selection(Selection::caret(at));
        s.insert_text("`").unwrap();
        s.insert_text("`").unwrap();
        let t0 = Instant::now();
        s.insert_text("`").unwrap();
        let t1 = Instant::now();
        while s.text().len() != text.len() {
            assert!(s.undo());
        }
        let t2 = Instant::now();
        if i >= WARMUP {
            a.push(t1 - t0);
            u.push(t2 - t1);
        }
    }
    assert_eq!(s.text(), text);
    report(&format!("session {label}"), a);
    report(&format!("session undo of the group {label}"), u);
}

fn case(format: &Arc<dyn DocumentFormat>, text: &str, r: std::ops::Range<usize>, ins: &str, label: &str) -> Duration {
    let mut doc = format.parse(text.to_string(), &FormatOptions::default());
    let mut model = DocModel::build_owned(&*doc, None);
    let fwd = PatchSet::single_verbatim(r, ins);
    let removed = fwd.removed_from(doc.text());
    let inv = fwd.inverse(&removed).unwrap();
    let (mut v, mut p, mut m) = (Vec::new(), Vec::new(), Vec::new());
    let rounds = 40;
    for i in 0..rounds + 3 {
        for ps in [&fwd, &inv] {
            let t0 = Instant::now();
            let e = doc.apply(ps);
            let t1 = Instant::now();
            model.apply(&*doc, &e);
            if i >= 3 {
                v.push(t0.elapsed());
                p.push(t1 - t0);
                m.push(t1.elapsed());
            }
        }
    }
    model
        .diff(&DocModel::build_owned(&*doc, None))
        .expect("incremental == full");
    let (pp, mm) = (pct(&mut p, 0.99), pct(&mut m, 0.99));
    let total = pct(&mut v, 0.99);
    let p50 = pct(&mut v, 0.5);
    println!(
        "G2 {label:<44} total p50 {:>7.3} p99 {:>7.3} ms  (apply p99 {:>7.3}, model p99 {:>7.3})",
        p50.as_secs_f64() * 1e3,
        total.as_secs_f64() * 1e3,
        pp.as_secs_f64() * 1e3,
        mm.as_secs_f64() * 1e3
    );
    total
}

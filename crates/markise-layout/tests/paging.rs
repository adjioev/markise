//! Paged view models (issue #125): a paged model equals a full build wherever it holds real
//! items, through builds, materialization, eviction and random edits (differential tests
//! against `DocModel::build_with(.., Paging::Full)`), and the view keeps only pages near the
//! viewport materialized without moving the content on screen.

use std::sync::Arc;

use markise_doc::{FormatDoc, LineEnding, PatchSet};
use markise_layout::block::NoImages;
use markise_layout::{DocModel, Paging, View, testing};
use markise_theme::Theme;
use proptest::prelude::*;

fn md(src: &str) -> markise_md::Document {
    markise_md::parse(src)
}

fn paged(doc: &dyn FormatDoc, tops: usize) -> DocModel {
    DocModel::build_with(doc, None, Paging::Paged { tops })
}

fn full(doc: &dyn FormatDoc) -> DocModel {
    DocModel::build_with(doc, None, Paging::Full)
}

/// Top-level block of item `i`.
fn top(m: &DocModel, doc: &dyn FormatDoc, i: usize) -> usize {
    doc.top_at(m.item_start(i)).unwrap_or(0)
}

/// Runs of real items made of whole top-level blocks (what the view may evict), each split
/// into runs of at most `tops` blocks.
fn evictable(m: &DocModel, doc: &dyn FormatDoc, tops: usize) -> Vec<std::ops::Range<usize>> {
    let n = m.items.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if m.is_page(i) {
            i += 1;
            continue;
        }
        let mut e = i + 1;
        let t0 = top(m, doc, i);
        while e < n && !m.is_page(e) && top(m, doc, e) < t0 + tops {
            e += 1;
        }
        // the run must end at a block boundary (the next item starts another block)
        while e > i + 1 && e < n && !m.is_page(e) && top(m, doc, e) == top(m, doc, e - 1) {
            e -= 1;
        }
        if e == n || m.is_page(e) || top(m, doc, e) != top(m, doc, e - 1) {
            out.push(i..e);
        }
        i = e;
    }
    out
}

/// Materializes every page (each step checked) and compares the result with a full build.
fn materialize_all(m: &mut DocModel, doc: &dyn FormatDoc) -> Result<(), String> {
    while let Some(i) = (0..m.items.len()).rev().find(|&i| m.is_page(i)) {
        let n = m.items.len();
        let ch = m.materialize(doc, i);
        if m.items.len() != n - 1 + ch.inserted.len() {
            return Err(format!("materialize {i}: {n} items → {} ({ch:?})", m.items.len()));
        }
        m.check_against_full(doc)
            .map_err(|e| format!("after materializing {i}: {e}"))?;
    }
    m.diff(&full(doc))
}

/// Disclosures of every shape (issue #125, phase 3): separate summaries, titles in the start
/// tag, open ones, nesting, quoted ones (their own scope), malformed ones (an unmatched end,
/// an unclosed start, a summary not right after its start).
fn disclosure_snippet(i: usize) -> String {
    match i % 9 {
        0 => format!("<details>\n<summary>Sum {i}</summary>\n\nhidden {i}\n\n</details>\n\n"),
        1 => format!("<details open>\n\nopen body {i}\n\n- list {i}\n\n</details>\n\n"),
        2 => format!(
            "<details>\n<summary>Outer {i}</summary>\n\nouter {i}\n\n<details>\n<summary>Inner {i}</summary>\n\ninner {i}\n\n</details>\n\nafter inner {i}\n\n</details>\n\n"
        ),
        3 => format!("> <details>\n> <summary>Quoted {i}</summary>\n>\n> quoted body {i}\n>\n> </details>\n\n"),
        4 => format!("</details>\n\nstray end above {i}\n\n"),
        5 => format!("<details>\n\nfar apart {i}\n\n"),
        6 => format!("text {i}\n\n</details>\n\n"),
        7 => format!("<details>\n\npara between {i}\n\n<summary>Late {i}</summary>\n\n</details>\n\n"),
        _ => format!("<details><summary>Inline {i}</summary>\n\nbody {i}\n\n</details>\n\n"),
    }
}

fn big_docs() -> Vec<String> {
    let mut v = Vec::new();
    let mut s = String::from("---\ntitle: t\n---\n\n[toc]\n\n");
    for i in 0..60 {
        s.push_str(&format!(
            "# H{i}\n\nPara {i} with [r] and *style*.\n\n- a {i}\n- b\n  > q\n\n```\ncode {i}\n```\n\n"
        ));
        if i % 7 == 3 {
            s.push_str("[toc]\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n[r]: /x\n\n");
        }
        if i % 11 == 5 {
            s.push_str(&"long code line\n".repeat(150).replace("long", "    long"));
            s.push('\n');
        }
        s.push_str(&disclosure_snippet(i));
    }
    v.push(s);
    // headings enough to segment the TOCs, repeated anchors
    let mut s = String::from("[toc]\n\n");
    for i in 0..300 {
        s.push_str(&format!("## Same\n\ntext {i}\n\n"));
    }
    s.push_str("[toc]\n");
    v.push(s);
    v.push(markise_corpus::generate(64 << 10, 3));
    v
}

#[test]
fn paged_builds_materialize_and_evict_like_full_builds() {
    for src in big_docs() {
        let doc = md(&src);
        for tops in [1, 2, 3, 7, 128] {
            let mut m = paged(&doc, tops);
            m.check_against_full(&doc)
                .unwrap_or_else(|e| panic!("tops {tops}: build: {e}"));
            assert!(!m.is_page(0) && !m.is_page(m.items.len() - 1), "the edges are real");
            if tops < 8 {
                assert!(m.page_count() > 3, "tops {tops}: {} pages", m.page_count());
            }
            // the middle page first, then the rest
            let pages: Vec<usize> = (0..m.items.len()).filter(|&i| m.is_page(i)).collect();
            if let Some(&mid) = pages.get(pages.len() / 2) {
                m.materialize(&doc, mid);
                m.check_against_full(&doc)
                    .unwrap_or_else(|e| panic!("tops {tops}: middle: {e}"));
            }
            materialize_all(&mut m, &doc).unwrap_or_else(|e| panic!("tops {tops}: {e}"));
            // evict every other run, then materialize again
            let runs = evictable(&m, &doc, tops.max(2));
            for r in runs.iter().rev().step_by(2) {
                let n = m.items.len();
                let ch = m.evict(&doc, r.clone(), None);
                assert_eq!(m.items.len(), n + 1 - r.len(), "{ch:?}");
                m.check_against_full(&doc)
                    .unwrap_or_else(|e| panic!("tops {tops}: evict {r:?}: {e}"));
            }
            materialize_all(&mut m, &doc).unwrap_or_else(|e| panic!("tops {tops}: again: {e}"));
        }
    }
}

#[test]
fn small_documents_are_full_models() {
    let src = "# T\n\npara\n\n- a\n- b\n";
    let doc = md(src);
    let m = paged(&doc, 128);
    assert!(m.is_paged());
    assert_eq!(m.page_count(), 0);
    m.diff(&full(&doc)).unwrap();
    // two pages: both edges, so both real
    let m = paged(&doc, 2);
    assert_eq!(m.page_count(), 0);
    m.diff(&full(&doc)).unwrap();
}

#[test]
fn the_toy_format_pages_too() {
    let mut s = String::new();
    for i in 0..40 {
        s.push_str(&format!("= Title {i}\n\npara *b* {i}\nmore\n\n* item\n\n"));
    }
    let doc = markise_testfmt::TestDoc::parse(&s, LineEnding::Lf);
    let mut m = paged(&doc, 3);
    assert!(m.page_count() > 5);
    m.check_against_full(&doc).unwrap();
    materialize_all(&mut m, &doc).unwrap();
}

// ---- random edits and paging operations == full build -------------------------------------

const SNIPPETS: &[&str] = &[
    "para text\n",
    "\n\n",
    "# Heading\n",
    "## Same\n",
    "- a\n- b\n",
    "  - nested\n",
    "> quote\n",
    "```\n",
    "```rust\nfn x() {}\n```\n",
    "| a | b |\n|---|--:|\n| 1 | 2 |\n",
    "[r]: http://x\n",
    "see [r]\n",
    "[toc]\n",
    "<details>\n",
    "</details>\n",
    "<summary>T</summary>\n",
    "<details open>\n",
    "[r]: http://other\n",
    "> <details>\n",
    "---\n",
    "x",
    "#",
    "-",
    "`",
    " ",
];

#[derive(Clone, Debug)]
enum Op {
    /// Replace `del` bytes at `pos` (modulo the text length; inside a real item when
    /// `real`) by snippet `snip` (nothing when `ins` is false).
    Edit {
        pos: usize,
        del: usize,
        snip: usize,
        ins: bool,
        real: bool,
    },
    /// Materialize the `k`-th page (modulo the pages).
    Materialize(usize),
    /// Evict the `k`-th evictable run (modulo the runs).
    Evict(usize),
}

fn ops() -> impl Strategy<Value = Vec<Op>> {
    let op = prop_oneof![
        3 => (0usize..100_000, 0usize..20, 0usize..SNIPPETS.len(), any::<bool>(), any::<bool>())
            .prop_map(|(pos, del, snip, ins, real)| Op::Edit { pos, del, snip, ins, real }),
        1 => (0usize..1000).prop_map(Op::Materialize),
        1 => (0usize..1000).prop_map(Op::Evict),
    ];
    proptest::collection::vec(op, 1..24)
}

fn char_floor(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn run_ops(doc: &mut dyn FormatDoc, tops: usize, ops: &[Op]) -> Result<(), String> {
    let mut m = paged(doc, tops);
    m.check_against_full(doc)?;
    for op in ops {
        match *op {
            Op::Edit {
                pos,
                del,
                snip,
                ins,
                real,
            } => {
                let t = doc.text();
                let mut a = if t.is_empty() { 0 } else { pos % (t.len() + 1) };
                if real {
                    // inside a real item (the incremental path, as at the caret)
                    let reals: Vec<usize> = (0..m.items.len()).filter(|&i| !m.is_page(i)).collect();
                    if !reals.is_empty() {
                        let i = reals[pos % reals.len()];
                        let sp = m.item_span(i);
                        a = sp.start + (pos / 7) % (sp.len() + 1);
                    }
                }
                let a = char_floor(t, a);
                let b = char_floor(t, a + del).max(a);
                let text = if ins { SNIPPETS[snip] } else { "" };
                if a == b && text.is_empty() {
                    continue;
                }
                let ps = PatchSet::single_verbatim(a..b, text);
                let e = doc.apply(&ps);
                m.apply(doc, &e);
                m.check_against_full(doc)
                    .map_err(|d| format!("after {ps:?} -> {:?}\n{e:?}\n{d}", doc.text()))?;
            }
            Op::Materialize(k) => {
                let pages: Vec<usize> = (0..m.items.len()).filter(|&i| m.is_page(i)).collect();
                if let Some(&i) = pages.get(k % pages.len().max(1)) {
                    m.materialize(doc, i);
                    m.check_against_full(doc).map_err(|d| format!("materialize {i}: {d}"))?;
                }
            }
            Op::Evict(k) => {
                let runs = evictable(&m, doc, tops.max(2));
                if let Some(r) = runs.get(k % runs.len().max(1)) {
                    m.evict(doc, r.clone(), None);
                    m.check_against_full(doc).map_err(|d| format!("evict {r:?}: {d}"))?;
                }
            }
        }
    }
    materialize_all(&mut m, doc)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(
        std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(64)
    ))]

    /// Random edits interleaved with materialization and eviction: after every step the
    /// paged model equals a full build where it holds real items; materialized completely at
    /// the end, it is the full build.
    #[test]
    fn prop_paged_model_equals_full_build_markdown(base in 0usize..2, tops in 1usize..5, ops in ops()) {
        let src = big_docs().swap_remove(base);
        let mut doc = md(&src);
        if let Err(e) = run_ops(&mut doc, tops, &ops) {
            return Err(TestCaseError::fail(e));
        }
    }

    #[test]
    fn prop_paged_model_equals_full_build_testfmt(tops in 1usize..4, ops in ops()) {
        let s = "= Title\n\npara *b*\nmore\n\n* item\n\n".repeat(12);
        let mut doc = markise_testfmt::TestDoc::parse(&s, LineEnding::Lf);
        if let Err(e) = run_ops(&mut doc, tops, &ops) {
            return Err(TestCaseError::fail(e));
        }
    }
}

// ---- the view ----------------------------------------------------------------------------

fn doc(n: usize) -> String {
    let mut s = String::from("# Title\n\n");
    for i in 0..n {
        s.push_str(&format!("Paragraph {i} with some *styled* text and `code`.\n\n"));
        if i % 50 == 25 {
            s.push_str(&format!("## Section {i}\n\n"));
        }
    }
    s
}

fn paged_view(src: &str, tops: usize) -> View {
    let doc: Arc<dyn FormatDoc> = Arc::new(md(src));
    let model = Arc::new(paged(&*doc, tops));
    View::new(doc, model, Theme::light(), 700.0, 1.0)
}

#[test]
fn the_view_materializes_near_the_viewport_and_stays_bounded() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = paged_view(&doc(5000), 16);
    let n0 = v.len();
    assert!(n0 < 400, "{n0} items at open (pages of 16 of ~5,100 blocks)");
    let vis = v.frame(800.0, 400.0, &mut f, &NoImages);
    assert_eq!(vis[0].idx, 0);
    assert!(v.laid_out.len() < 60, "laid out {} items", v.laid_out.len());
    // a traversal of the whole document by scrollbar steps
    let mut max_items = 0;
    for s in 0..=150 {
        let y = v.total_height() * s as f64 / 150.0;
        v.scroll_to_y(y);
        let vis = v.frame(800.0, 400.0, &mut f, &NoImages);
        assert!(!vis.is_empty());
        for x in &vis {
            assert!(!v.model().is_page(x.idx), "step {s}: a placeholder is shown");
        }
        // tiles the viewport without overlap
        for w in vis.windows(2) {
            assert!(w[1].y >= w[0].y + w[0].layout.height - 0.01);
        }
        max_items = max_items.max(v.len());
    }
    assert!(max_items < 1500, "items stay bounded: {max_items} of ~5,100 leaves");
    assert_eq!(v.paging_stats().placeholder_layouts, 0);
    assert!(v.paging_stats().evicted > 0 && v.paging_stats().materialized > 0);
    v.model().check_against_full(&**v.doc()).unwrap();
    // the end of the document is reachable
    v.scroll_to_y(f64::MAX);
    let vis = v.frame(800.0, 400.0, &mut f, &NoImages);
    assert_eq!(vis.last().unwrap().idx, v.len() - 1);
}

#[test]
fn materializing_pages_above_does_not_move_the_content() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let src = doc(3000);
    let mut v = paged_view(&src, 8);
    v.scroll_to_y(v.total_height() * 0.6);
    let vis = v.frame(800.0, 400.0, &mut f, &NoImages);
    let first = vis[1].idx;
    let (start, y) = (v.model().item_start(first), vis[1].y);
    // scroll up a screen in small steps: pages above come in, indices shift, the item stays
    let mut moved = 0.0;
    for _ in 0..40 {
        v.scroll_by(-20.0);
        moved += 20.0;
        let vis = v.frame(800.0, 400.0, &mut f, &NoImages);
        if let Some(x) = vis.iter().find(|x| v.model().item_start(x.idx) == start) {
            assert!(
                (x.y - (y + moved)).abs() < 0.5,
                "item at {start}: y {} != {}",
                x.y,
                y + moved
            );
        }
    }
}

#[test]
fn jumps_into_unloaded_pages_land_on_their_target() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let src = doc(3000);
    let mut v = paged_view(&src, 8);
    v.frame(800.0, 400.0, &mut f, &NoImages);
    // a source position in a placeholder
    let target = src.find("Paragraph 2222 ").unwrap();
    assert!(v.model().is_page(v.model().item_at_offset(target)));
    v.ensure_offset(target);
    let i = v.model().item_at_offset(target);
    assert!(!v.model().is_page(i));
    assert_eq!(v.model().item_start(i), target);
    v.scroll_to_item(i);
    let vis = v.frame(800.0, 400.0, &mut f, &NoImages);
    assert_eq!(vis[0].idx, v.model().item_at_offset(target));
    // an outline entry in a placeholder
    let k = v.model().outline.iter().position(|o| o.text == "Section 1525").unwrap();
    v.scroll_to_y(0.0);
    for _ in 0..3 {
        v.frame(800.0, 400.0, &mut f, &NoImages);
    }
    let i = v.ensure_heading(k).unwrap();
    assert!(!v.model().is_page(i));
    assert_eq!(
        v.model().item_start(i),
        src.find("## Section 1525").unwrap(),
        "the heading's own item"
    );
    // a scroll position by source survives eviction and comes back
    v.scroll_to_item(i);
    v.scroll_by(7.0);
    v.frame(800.0, 400.0, &mut f, &NoImages);
    let at = v.source_anchor();
    v.scroll_to_y(0.0);
    for _ in 0..3 {
        v.frame(800.0, 400.0, &mut f, &NoImages);
    }
    assert!(v.model().is_page(v.model().item_at_offset(at.0)), "evicted meanwhile");
    v.scroll_to_source(at.0, at.1, at.2);
    v.frame(800.0, 400.0, &mut f, &NoImages);
    assert_eq!(v.source_anchor(), at);
}

/// The differential test on a corpus document of 1 MiB with long deterministic sequences of
/// edits, materializations and evictions at the default page size and at tiny pages.
#[test]
#[ignore = "heavy: large input; cargo xtask heavy"]
fn heavy_suite_paged_model_equals_full_build_on_a_large_document() {
    let src = markise_corpus::generate(1 << 20, 11);
    // a small deterministic generator (xorshift) for the operations
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed as usize
    };
    for tops in [128, 16, 2] {
        let ops: Vec<Op> = (0..120)
            .map(|_| match next() % 5 {
                0 => Op::Materialize(next()),
                1 => Op::Evict(next()),
                _ => Op::Edit {
                    pos: next(),
                    del: next() % 40,
                    snip: next() % SNIPPETS.len(),
                    ins: next() % 3 != 0,
                    real: next() % 4 != 0,
                },
            })
            .collect();
        let mut doc = md(&src);
        run_ops(&mut doc, tops, &ops).unwrap_or_else(|e| panic!("tops {tops}: {e}"));
    }
}

/// A placeholder's height estimate is the sum of its items' estimates (paragraph wrapping
/// interpolated between the kept line lengths), so materializing a page barely changes the
/// document height and the scrollbar (found by the scrollbar drag test of #123: one-line
/// paragraphs were estimated 30 % too tall).
#[test]
fn page_estimates_match_the_items_estimates() {
    let mut short = String::new();
    for i in 0..3_000 {
        short.push_str(&format!("Paragraph {i} of a tall document with a line of text.\n\n"));
    }
    // (closed disclosures hide content a placeholder partly holding them still counts: the
    // documents here have none)
    for src in [short, big_docs().swap_remove(1), doc(2000)] {
        let doc: Arc<dyn FormatDoc> = Arc::new(md(&src));
        for width in [300.0, 700.0, 1200.0] {
            let p = View::new(doc.clone(), Arc::new(paged(&*doc, 16)), Theme::light(), width, 1.0);
            let f = View::new(doc.clone(), Arc::new(full(&*doc)), Theme::light(), width, 1.0);
            let (a, b) = (p.total_height(), f.total_height());
            assert!((a / b - 1.0).abs() < 0.02, "width {width}: paged {a} vs full {b}");
        }
    }
}

// ---- cross-block facts (phase 3) ---------------------------------------------------------

/// Hidden, depth, `(title, open)` of an item ([`View::disclosure_of`]).
type DisclosureState = (bool, usize, Option<(String, bool)>);

/// Disclosure state of every real item of `v`, by source position (start, segment ordinal).
fn disclosure_states(v: &View) -> Vec<((usize, usize), DisclosureState)> {
    let m = v.model();
    let mut out = Vec::new();
    let mut prev = (usize::MAX, 0);
    for i in 0..m.items.len() {
        if m.is_page(i) {
            prev = (usize::MAX, 0);
            continue;
        }
        let at = m.item_start(i);
        let nth = if prev.0 == at { prev.1 + 1 } else { 0 };
        prev = (at, nth);
        out.push(((at, nth), v.disclosure_of(i)));
    }
    out
}

/// The paged view's states equal the full view's for the same items.
fn same_states(paged: &View, full: &View) -> Result<(), String> {
    let f: std::collections::HashMap<_, _> = disclosure_states(full).into_iter().collect();
    for (k, st) in disclosure_states(paged) {
        match f.get(&k) {
            Some(x) if *x == st => {}
            other => return Err(format!("item at {k:?}: paged {st:?} != full {other:?}")),
        }
    }
    Ok(())
}

fn materialize_in_view(v: &mut View, i: usize) {
    let doc = v.doc().clone();
    let ch = v.model_mut().materialize(&*doc, i);
    v.apply_change(doc, &ch);
}

fn evict_in_view(v: &mut View, r: std::ops::Range<usize>) {
    let doc = v.doc().clone();
    let ch = v.model_mut().evict(&*doc, r, None);
    v.apply_change(doc, &ch);
}

/// Disclosures pair document-wide: whatever pages are materialized, in whatever order, and
/// whatever the reader toggled, every real item of a paged view (pages of 1 and 2 blocks) is
/// hidden, indented and titled as in the full view; nested, quoted, malformed and
/// separate-summary disclosures included.
#[test]
fn disclosures_pair_alike_in_every_load_and_evict_order() {
    let mut src = String::from("# Top\n\n");
    for i in 0..45 {
        src.push_str(&format!("Paragraph {i}.\n\n"));
        src.push_str(&disclosure_snippet(i));
    }
    let doc: Arc<dyn FormatDoc> = Arc::new(md(&src));
    let mut full_v = View::new(doc.clone(), Arc::new(full(&*doc)), Theme::light(), 700.0, 1.0);
    for tops in [1, 2] {
        for order in 0..3u64 {
            let mut v = View::new(doc.clone(), Arc::new(paged(&*doc, tops)), Theme::light(), 700.0, 1.0);
            assert_eq!(v.model().cross.disclosures(), full_v.model().cross.disclosures());
            same_states(&v, &full_v).unwrap_or_else(|e| panic!("tops {tops}: open: {e}"));
            // materialize in an order (back to front, front to back, from the middle out),
            // evicting every third run on the way
            let mut seed = 0x2545_f491_4f6c_dd1du64 ^ order;
            let mut step = 0;
            while let Some(i) = {
                let pages: Vec<usize> = (0..v.len()).filter(|&i| v.model().is_page(i)).collect();
                match order {
                    0 => pages.last().copied(),
                    1 => pages.first().copied(),
                    _ => pages.get(pages.len() / 2).copied(),
                }
            } {
                materialize_in_view(&mut v, i);
                step += 1;
                if step % 3 == 0 {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    let runs = evictable(v.model(), &**v.doc(), tops.max(2));
                    if !runs.is_empty() && step < 60 {
                        evict_in_view(&mut v, runs[seed as usize % runs.len()].clone());
                    }
                }
                same_states(&v, &full_v).unwrap_or_else(|e| panic!("tops {tops} order {order} step {step}: {e}"));
                if step > 400 {
                    break;
                }
            }
        }
    }
    // the reader's toggle survives the eviction of the disclosure's start marker
    let mut v = View::new(doc.clone(), Arc::new(paged(&*doc, 1)), Theme::light(), 700.0, 1.0);
    let at = src.find("<details>\n<summary>Sum 9</summary>").unwrap();
    for view in [&mut v, &mut full_v] {
        view.ensure_offset(at);
        let i = view.model().item_at_offset(at);
        assert!(view.disclosure_of(i).2.is_some_and(|(t, open)| t == "Sum 9" && !open));
        view.toggle_expanded(i);
    }
    same_states(&v, &full_v).unwrap();
    let i = v.model().item_at_offset(at);
    let runs = evictable(v.model(), &**v.doc(), 1);
    let r = runs.iter().find(|r| r.contains(&i)).unwrap().clone();
    evict_in_view(&mut v, r);
    assert!(v.model().is_page(v.model().item_at_offset(at)));
    v.ensure_offset(at);
    let i = v.model().item_at_offset(at);
    assert_eq!(v.disclosure_of(i).2, Some(("Sum 9".to_string(), true)), "still open");
    same_states(&v, &full_v).unwrap();
}

/// A placeholder's measured height is dropped when an input its blocks depend on changes:
/// the outline (a page holding a `[toc]`) and the resource root (a page showing images).
#[test]
fn measured_page_heights_follow_document_level_inputs() {
    let mut src = String::from("---\ntypora-root-url: /a\n---\n\n");
    for i in 0..40 {
        src.push_str(&format!("## Heading {i}\n\ntext {i}\n\n"));
        if i == 20 {
            src.push_str("[toc]\n\n![pic](p.png)\n\n");
        }
    }
    let mut d = md(&src);
    let doc: Arc<dyn FormatDoc> = Arc::new(d.clone());
    let mut v = View::new(doc.clone(), Arc::new(paged(&*doc, 2)), Theme::light(), 700.0, 1.0);
    let toc = src.find("[toc]").unwrap();
    v.ensure_offset(toc);
    let i = v.model().item_at_offset(toc);
    // evict the run holding the TOC with a deliberately wrong measured height
    let runs = evictable(v.model(), &*doc, 2);
    let r = runs.iter().find(|r| r.contains(&i)).unwrap().clone();
    let key_probe = v.estimate(r.start);
    let _ = key_probe;
    let docc = v.doc().clone();
    let ch = v
        .model_mut()
        .evict(&*docc, r.clone(), Some((2800, 0, 12345.0f32.to_bits())));
    v.apply_change(docc, &ch);
    let p = r.start;
    assert!(v.model().is_page(p));
    let before = v.height(p);
    // more headings: the outline grows, the TOC page is estimated again
    let end = d.text().len();
    let e = d.apply(&PatchSet::single_verbatim(end..end, "## New A\n\n## New B\n\n"));
    let ch = v.model_mut().apply(&d, &e);
    v.apply_change(Arc::new(d.clone()), &ch);
    assert!(v.model().is_page(p));
    assert_eq!(v.height(p), v.estimate(p), "re-estimated after the outline changed");
    assert_ne!(v.height(p), before);
    // the resource root changes: an image page is estimated again
    let at = d.text().find("/a").unwrap();
    let gens = v.model().cross.gens;
    let e = d.apply(&PatchSet::single_verbatim(at..at + 2, "/b"));
    let ch = v.model_mut().apply(&d, &e);
    v.apply_change(Arc::new(d.clone()), &ch);
    assert!(v.model().cross.gens.resources > gens.resources);
}

/// What the reader sees is the same: jumping to source positions all over a document with
/// disclosures, `[toc]`s, references and repeated headings, the paged view (pages of 1 and 2
/// blocks) shows the same items at the same places with the same rendered text as the full
/// view.
#[test]
fn paged_and_full_views_show_the_same() {
    let src = big_docs().swap_remove(0);
    let doc: Arc<dyn FormatDoc> = Arc::new(md(&src));
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let shown = |v: &mut View, f: &mut dyn markise_layout::TextShaper| -> Vec<(usize, i32, String)> {
        let vis = v.frame(800.0, 300.0, f, &NoImages);
        vis.iter()
            .map(|x| {
                let text: String = x
                    .layout
                    .prims
                    .iter()
                    .filter_map(|p| match p {
                        markise_layout::block::Prim::Text { text, .. } => Some(text.text().to_string()),
                        _ => None,
                    })
                    .collect();
                (v.model().item_start(x.idx), x.y.round() as i32, text)
            })
            .collect()
    };
    for tops in [1, 2] {
        let mut full_v = View::new(doc.clone(), Arc::new(full(&*doc)), Theme::light(), 700.0, 1.0);
        let mut v = View::new(doc.clone(), Arc::new(paged(&*doc, tops)), Theme::light(), 700.0, 1.0);
        for k in 0..25 {
            let at = src.len() * k / 25;
            let at = src[..at].rfind("\n\n").map_or(0, |p| p + 2);
            for view in [&mut v, &mut full_v] {
                view.ensure_offset(at);
                let i = view.model().item_at_offset(at);
                view.scroll_to_item(i);
            }
            let (a, b) = (shown(&mut v, &mut f), shown(&mut full_v, &mut f));
            assert_eq!(a, b, "tops {tops}, at {at}");
        }
    }
}

// ---- review of #136, round 1 ---------------------------------------------------------------

/// A separate summary inside a quote pairs with its start (the blank quote line between them
/// is not content).
#[test]
fn quoted_separate_summaries_pair() {
    let src = "> <details>\n>\n> <summary>Title</summary>\n>\n> body\n>\n> </details>\n\nafter\n";
    for m in [paged(&md(src), 1), full(&md(src))] {
        let g = &m.cross.disclosures()[0];
        assert_eq!(g.title, "Title", "{:?}", m.cross.disclosures());
        assert_eq!(g.summary, Some(src.find("<summary>").unwrap()));
    }
    // opened, the summary is the disclosure's control, not a separate block
    let doc: Arc<dyn FormatDoc> = Arc::new(md(src));
    let mut v = View::new(doc.clone(), Arc::new(full(&*doc)), Theme::light(), 700.0, 1.0);
    let i = v.model().item_at_offset(src.find("<details>").unwrap());
    v.toggle_expanded(i);
    let s = v.model().item_at_offset(src.find("<summary>").unwrap());
    assert!(v.disclosure_of(s).0, "the summary stays hidden");
    assert_eq!(v.disclosure_of(i).2, Some(("Title".to_string(), true)));
}

/// The reader's choice survives an edit that re-parses the container around an unchanged
/// disclosure (a paragraph split above it in the same quote).
#[test]
fn toggles_survive_a_reparse_of_their_container() {
    let src = "> first paragraph here\n>\n> <details>\n> <summary>T</summary>\n>\n> body\n>\n> </details>\n\nafter\n";
    let mut d = md(src);
    let doc: Arc<dyn FormatDoc> = Arc::new(d.clone());
    let mut v = View::new(doc.clone(), Arc::new(paged(&*doc, 1)), Theme::light(), 700.0, 1.0);
    let at = src.find("<details>").unwrap();
    v.ensure_offset(at);
    let i = v.model().item_at_offset(at);
    v.toggle_expanded(i);
    assert_eq!(v.disclosure_of(i).2, Some(("T".to_string(), true)));
    // split the first paragraph: the whole quote is re-parsed
    let split = src.find("paragraph").unwrap();
    let e = d.apply(&PatchSet::single_verbatim(split..split, "\n>\n> "));
    assert!(e.replaced_top.contains(&0), "{e:?}");
    let ch = v.model_mut().apply(&d, &e);
    v.apply_change(Arc::new(d.clone()), &ch);
    let at = d.text().find("<details>").unwrap();
    let i = v.model().item_at_offset(at);
    assert_eq!(v.disclosure_of(i).2, Some(("T".to_string(), true)), "still open");
}

/// A page evicted while a disclosure in it was closed measured its body as zero height;
/// reopening the disclosure drops that measurement.
#[test]
fn measured_page_heights_follow_disclosure_state() {
    let mut src = String::from("<details>\n<summary>S</summary>\n\n");
    for i in 0..30 {
        src.push_str(&format!("body paragraph {i}\n\n"));
    }
    src.push_str("</details>\n\nafter\n\n");
    for i in 0..30 {
        src.push_str(&format!("tail {i}\n\n"));
    }
    let doc: Arc<dyn FormatDoc> = Arc::new(md(&src));
    let mut v = View::new(doc.clone(), Arc::new(paged(&*doc, 4)), Theme::light(), 700.0, 1.0);
    // materialize a body page, evict it with the zero height it has while closed
    let at = src.find("body paragraph 12").unwrap();
    v.ensure_offset(at);
    let i = v.model().item_at_offset(at);
    assert!(v.disclosure_of(i).0, "hidden while closed");
    let runs = evictable(v.model(), &*doc, 4);
    let r = runs.iter().find(|r| r.contains(&i)).unwrap().clone();
    let h: f32 = r.clone().map(|j| v.height(j)).sum();
    assert_eq!(h, 0.0);
    let ch = v.model_mut().evict(&*doc, r.clone(), Some((2800, 0, h.to_bits())));
    v.apply_change(doc.clone(), &ch);
    let p = r.start;
    assert!(v.model().is_page(p) && v.height(p) == 0.0);
    // open the disclosure: the page is estimated again (not the stale 0)
    let s = v.model().item_at_offset(0);
    v.toggle_expanded(s);
    assert!(v.height(p) > 50.0, "re-estimated: {}", v.height(p));
}

/// Typing in a document with many disclosures neither derives the disclosure state again nor
/// pairs the markers nor resets heights: the work does not grow with the disclosure count.
#[test]
fn typing_does_no_document_wide_disclosure_work() {
    let mut src = String::from("Typing here.\n\n");
    for i in 0..2_000 {
        src.push_str(&format!(
            "<details>\n<summary>S{i}</summary>\n\nbody {i}\n\n</details>\n\n"
        ));
    }
    let mut d = md(&src);
    let doc: Arc<dyn FormatDoc> = Arc::new(d.clone());
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = View::new(doc.clone(), Arc::new(paged(&*doc, 16)), Theme::light(), 700.0, 1.0);
    v.frame(800.0, 300.0, &mut f, &NoImages);
    let pairings = v.model().cross.pairings;
    let mut derived_by_edits = 0;
    let mut at = 6;
    for k in 0..40 {
        // letters, and now and then an Enter (the item count changes)
        let text = if k % 10 == 9 { "\n\n" } else { "x" };
        let e = d.apply(&PatchSet::single_verbatim(at..at, text));
        at += text.len();
        let ch = v.model_mut().apply(&d, &e);
        assert!(!ch.disclosures);
        let before = v.disclosure_derivations();
        v.apply_change(Arc::new(d.clone()), &ch);
        derived_by_edits += v.disclosure_derivations() - before;
        // (a frame may materialize or evict a page: that derives, once per page)
        v.frame(800.0, 300.0, &mut f, &NoImages);
    }
    assert_eq!(derived_by_edits, 0, "no derivation while typing");
    assert_eq!(v.model().cross.pairings, pairings, "no pairing while typing");
    // the state followed the shifts
    v.model().check_against_full(&d).unwrap();
    let doc2: Arc<dyn FormatDoc> = Arc::new(d.clone());
    let full_v = View::new(doc2.clone(), Arc::new(full(&*doc2)), Theme::light(), 700.0, 1.0);
    same_states(&v, &full_v).unwrap();
}

/// Scrolled into a closed disclosure, the frame lays out no placeholder (guard for the frame's
/// anchor step, which materializes a page it lands on; the placeholder layouts the bench saw
/// came from caret hit testing walking into pages, which now stops at them).
#[test]
fn the_item_after_a_closed_disclosure_is_materialized_before_layout() {
    let mut src = String::from("start\n\n<details>\n<summary>S</summary>\n\n");
    for i in 0..20 {
        src.push_str(&format!("hidden {i}\n\n"));
    }
    src.push_str("</details>\n\n");
    for i in 0..60 {
        src.push_str(&format!("after {i}\n\n"));
    }
    let doc: Arc<dyn FormatDoc> = Arc::new(md(&src));
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = View::new(doc.clone(), Arc::new(paged(&*doc, 2)), Theme::light(), 700.0, 1.0);
    // the anchor on the closed disclosure's (hidden) content
    let at = src.find("hidden 3").unwrap();
    v.ensure_offset(at);
    let i = v.model().item_at_offset(at);
    assert!(v.disclosure_of(i).0);
    v.scroll_to_item(i);
    // scroll_to_item reveals; close it again with the anchor inside
    let s = v.model().item_at_offset(src.find("<details>").unwrap());
    v.toggle_expanded(s);
    let vis = v.frame(800.0, 300.0, &mut f, &NoImages);
    assert!(vis.iter().all(|x| !v.model().is_page(x.idx)));
    assert_eq!(v.paging_stats().placeholder_layouts, 0);
}

/// Keystroke cost (model update, view update, frame) in a document with 10,000 disclosures
/// stays within 0.1 ms of that with 100 (median, release): ordinary typing does no work per
/// disclosure (review of #136: it was 0.29 → 4.42 ms).
#[test]
#[ignore = "heavy: timing on large inputs; cargo xtask heavy"]
fn heavy_suite_typing_cost_does_not_grow_with_disclosures() {
    fn per_keystroke(n: usize) -> f64 {
        let mut src = String::from("Typing here.\n\n");
        for i in 0..n {
            src.push_str(&format!(
                "<details>\n<summary>S{i}</summary>\n\nbody {i}\n\n</details>\n\n"
            ));
        }
        let mut d = md(&src);
        let doc: Arc<dyn FormatDoc> = Arc::new(d.clone());
        let mut fonts = testing::fonts();
        let mut f = fonts.with_pixels_per_point(1.0);
        let mut v = View::new(doc.clone(), Arc::new(paged(&*doc, 128)), Theme::light(), 700.0, 1.0);
        v.frame(800.0, 300.0, &mut f, &NoImages);
        let mut at = 6;
        let mut times = Vec::new();
        let mut keep: Vec<Arc<dyn FormatDoc>> = Vec::new();
        for k in 0..300 {
            let text = if k % 30 == 29 { "\n\n" } else { "x" };
            let e = d.apply(&PatchSet::single_verbatim(at..at, text));
            // (the test copies the document for the view; the editor edits it in place)
            let shared: Arc<dyn FormatDoc> = Arc::new(d.clone());
            // (and drops the old copy outside the measurement)
            keep.push(v.doc().clone());
            let t = std::time::Instant::now();
            let ch = v.model_mut().apply(&d, &e);
            v.apply_change(shared, &ch);
            v.frame(800.0, 300.0, &mut f, &NoImages);
            times.push(t.elapsed().as_secs_f64() * 1e3);
            at += text.len();
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        times[times.len() / 2]
    }
    let small = per_keystroke(100);
    let large = per_keystroke(10_000);
    eprintln!("typing p50: 100 disclosures {small:.3} ms, 10,000 disclosures {large:.3} ms");
    // (what remains is the larger document: more pages, deeper chunk lists)
    assert!(large < small + 0.1, "100: {small:.3} ms, 10,000: {large:.3} ms");
}

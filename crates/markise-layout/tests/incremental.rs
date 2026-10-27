//! The view model on the document seam (ADR 0006 §1.4, §2.3, §7.2.6, §7.4):
//! - flattening (containers, markers, gaps, outline) through `FormatDoc::flatten`;
//! - `DocModel::apply` (incremental, from the edit's `EditSummary`) equals a full rebuild after
//!   random edits — inside lists, quotes, callouts, code, tables, with definitions, headings,
//!   `[toc]`, front matter and CRLF documents — for Markdown and the toy format;
//! - item hashes and cached layouts are offset-independent (C-18): text inserted above an
//!   unchanged block keeps its cache entry;
//! - dependency-scoped invalidation: a definition re-keys only the items that may use it;
//! - item-relative source maps round-trip.

use std::sync::Arc;

use markise_doc::{FormatDoc, LineEnding, PatchSet, PieceKind};
use markise_layout::block::{NoImages, Prim};
use markise_layout::{Container, DocModel, Gap, ItemKind, Marker, View, testing};
use markise_theme::Theme;
use proptest::prelude::*;

fn md(src: &str) -> markise_md::Document {
    markise_md::parse(src)
}

fn model(src: &str) -> DocModel {
    DocModel::build_owned(&md(src), None)
}

#[test]
fn flattens_containers_into_leaf_items() {
    let m = model("# T\n\n- a\n- b\n\n  more\n\n> q1\n>\n> q2\n");
    let kinds: Vec<_> = m.items.iter().map(|i| i.kind.clone()).collect();
    assert_eq!(
        kinds,
        vec![
            ItemKind::Heading { level: 1 },
            ItemKind::Paragraph,
            ItemKind::Paragraph,
            ItemKind::Paragraph,
            ItemKind::Paragraph,
            ItemKind::Paragraph,
        ]
    );
    let marker = |i: usize| match &m.items[i].containers[0] {
        Container::ListItem { marker, .. } => marker.clone(),
        _ => panic!(),
    };
    assert_eq!(marker(1), Some(Marker::Bullet { depth: 0 }));
    assert_eq!(marker(2), Some(Marker::Bullet { depth: 0 }));
    assert_eq!(marker(3), None);
    assert!(matches!(
        m.items[4].containers[0],
        Container::Quote {
            callout: None,
            first: true
        }
    ));
    assert!(matches!(
        m.items[5].containers[0],
        Container::Quote {
            callout: None,
            first: false
        }
    ));
    assert_eq!(m.items[0].gap, Gap::None);
}

#[test]
fn task_and_ordered_markers() {
    let m = model("- [ ] open\n- [x] done\n\n\n3. three\n4. four\n");
    let markers: Vec<_> = m
        .items
        .iter()
        .map(|i| match &i.containers[0] {
            Container::ListItem { marker, .. } => marker.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(
        markers,
        vec![
            Some(Marker::Task { checked: false }),
            Some(Marker::Task { checked: true }),
            Some(Marker::Number("3.".into())),
            Some(Marker::Number("4.".into())),
        ]
    );
    assert_eq!(m.items[1].gap, Gap::Tight);
}

#[test]
fn outline_extraction_levels_text_anchors() {
    let m = model("---\ntitle: x\n---\n# Intro *one*\n\ntext\n\n## Details\n\n> ### Quoted\n\n## Details\n\n# Write\n");
    let o: Vec<_> = m
        .outline
        .iter()
        .map(|e| (e.level, e.text.as_str(), e.anchor.as_str()))
        .collect();
    assert_eq!(
        o,
        vec![
            (1, "Intro one", "intro-one"),
            (2, "Details", "details"),
            (3, "Quoted", "quoted"),
            (2, "Details", "details-2"), // spec 01 R7.2
            (1, "Write", "write-1"),     // R7.3
        ]
    );
    for e in &m.outline {
        assert!(matches!(m.items[e.item].kind, ItemKind::Heading { .. }));
    }
    assert_eq!(m.current_heading(0), None);
    assert_eq!(m.current_heading(m.outline[0].item + 1), Some(0));
    assert_eq!(m.heading_by_anchor("#details-2").map(|e| e.level), Some(2));
}

#[test]
fn item_hash_depends_on_content_and_context() {
    let a = model("hello\n\n> hello\n");
    assert_ne!(a.items[0].hash, a.items[1].hash, "same text, different container");
    let b = model("hello\n\n> hello\n");
    assert_eq!(a.items[0].hash, b.items[0].hash, "stable within a run");
}

// ---- incremental == full ---------------------------------------------------------------

/// Applies one patch to `doc` and the resulting summary to `m`; checks against a full build.
fn step(doc: &mut dyn FormatDoc, m: &mut DocModel, ps: &PatchSet) -> Result<(), String> {
    let e = doc.apply(ps);
    m.apply(doc, &e);
    let full = DocModel::build_owned(doc, None);
    m.diff(&full)
        .map_err(|d| format!("after {ps:?} -> {:?}\n{e:?}\n{d}", doc.text()))
}

const SNIPPETS: &[&str] = &[
    "para text\n",
    "inline $x^2$ math\n",
    "\n\n",
    "# Heading\n",
    "## Intro\n",
    "## Intro\n\ntext\n",
    "- a\n- b\n",
    "  - nested\n",
    "1. one\n2. two\n",
    "- [ ] task\n",
    "> quote\n> more\n",
    "> [!NOTE]\n> callout\n",
    "```rust\nfn x() {}\n```\n",
    "```\n",
    "    indented code\n",
    "| a | b |\n|---|--:|\n| 1 | 2 |\n",
    "| x |\n",
    "[label]: http://x \"t\"\n",
    "see [label] and [other][label]\n",
    "[^n]: footnote\n",
    "ref [^n]\n",
    "[toc]\n",
    "$$\nx\n$$\n",
    "<div>\nhtml\n</div>\n",
    "---\n",
    "***strong*** `code` ![img](p.png)\n",
    "\r\n",
    "x",
    "#",
    "-",
    ">",
    "|",
    "`",
    "[",
    "]",
    " ",
];

const BASES: &[&str] = &[
    "# Title\n\nIntro paragraph with [label].\n\n- item one\n- item two\n  continued\n\n> quote line\n> second\n\n```js\nlet a = 1;\nlet b = 2;\n```\n\n| h1 | h2 |\n|----|----|\n| c1 | c2 |\n| c3 | c4 |\n\n[label]: http://example.org\n\n## Intro\n\n## Intro\n\n[toc]\n\nFinal [^n].\n\n[^n]: note\n",
    "---\ntypora-root-url: /assets\n---\n\n![a](/x.png)\n\n1. a\n2. b\n\n> [!WARNING]\n> careful\n>\n> - nested\n\ntail\n",
    "# CRLF\r\n\r\n- a\r\n- b\r\n\r\n> q\r\n\r\n```\r\ncode\r\n```\r\n\r\n| a |\r\n|---|\r\n| 1 |\r\n",
    "",
];

fn char_floor(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn edits() -> impl Strategy<Value = Vec<(usize, usize, usize, bool)>> {
    proptest::collection::vec(
        (0usize..10_000, 0usize..12, 0usize..SNIPPETS.len(), any::<bool>()),
        1..12,
    )
}

fn run_edits(doc: &mut dyn FormatDoc, ops: &[(usize, usize, usize, bool)]) -> Result<(), String> {
    let mut m = DocModel::build_owned(doc, None);
    for &(pos, del, snip, insert) in ops {
        let t = doc.text();
        let a = char_floor(t, if t.is_empty() { 0 } else { pos % (t.len() + 1) });
        let b = char_floor(t, a + del).max(a);
        let ins = if insert { SNIPPETS[snip] } else { "" };
        if a == b && ins.is_empty() {
            continue;
        }
        step(doc, &mut m, &PatchSet::single_verbatim(a..b, ins))?;
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(
        std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(256)
    ))]

    #[test]
    fn prop_apply_equals_full_rebuild_markdown(base in 0..BASES.len(), ops in edits()) {
        let mut doc = md(BASES[base]);
        if let Err(e) = run_edits(&mut doc, &ops) {
            return Err(TestCaseError::fail(e));
        }
    }

    #[test]
    fn prop_apply_equals_full_rebuild_testfmt(ops in edits()) {
        let mut doc = markise_testfmt::TestDoc::parse("= Title\n\npara *b*\nmore\n\n* item\n", LineEnding::Lf);
        if let Err(e) = run_edits(&mut doc, &ops) {
            return Err(TestCaseError::fail(e));
        }
    }
}

#[test]
fn apply_equals_full_on_a_large_document_with_many_chunks() {
    // > 256 top-level blocks and items: edits cross chunk boundaries of both relative lists
    let text = markise_corpus::generate(300 << 10, 7);
    let mut doc = md(&text);
    let mut m = DocModel::build_owned(&doc, None);
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    for k in 0..60 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let t = doc.text();
        let a = char_floor(t, (seed >> 20) as usize % t.len());
        let b = char_floor(t, a + (seed >> 50) as usize % 40).max(a);
        let ins = SNIPPETS[k % SNIPPETS.len()];
        step(&mut doc, &mut m, &PatchSet::single_verbatim(a..b, ins)).unwrap();
    }
}

#[test]
fn apply_with_hints_reports_combined_change() {
    // the keystroke completing ``` on a fresh line with its hint: one summary covers both
    use markise_doc::{HintOrigin, HintSet, LiteralHint};
    let mut doc = md("a\n\n``\n\nb\n\nc\n");
    let mut m = DocModel::build_owned(&doc, None);
    let hints = HintSet::from_hints([LiteralHint::new(3, HintOrigin::EnterPending)]);
    let e = doc.apply_with_hints(&PatchSet::single_verbatim(5..5, "`"), Some(&hints));
    m.apply(&doc, &e);
    m.diff(&DocModel::build_owned(&doc, None)).unwrap();
    // removing the hint alone re-projects the line as a fence that swallows the rest
    let e = doc.apply_with_hints(&PatchSet::empty(), Some(&HintSet::new()));
    m.apply(&doc, &e);
    m.diff(&DocModel::build_owned(&doc, None)).unwrap();
    assert_eq!(m.items.len(), 2, "para + fence");
}

// ---- dependencies (§7.4) ---------------------------------------------------------------

#[test]
fn adding_a_definition_rekeys_only_items_that_may_use_it() {
    let mut doc = md("see [foo] here\n\nplain text\n\n[bar] other\n\nlink [foo](http://inline) only\n\nend\n");
    let mut m = DocModel::build_owned(&doc, None);
    let mut v = View::new(Arc::new(doc.clone()), Arc::new(m.clone()), Theme::light(), 600.0, 1.0);
    let keys = |v: &View| (0..v.len()).map(|i| v.cache_key(i)).collect::<Vec<_>>();
    let before = keys(&v);
    let end = doc.text().len();
    let e = doc.apply(&PatchSet::single_verbatim(end..end, "\n[foo]: http://x\n"));
    assert!(!e.deps.labels.is_empty(), "the new definition is reported");
    let ch = m.apply(&doc, &e);
    m.diff(&DocModel::build_owned(&doc, None)).unwrap();
    assert_eq!(ch.labels, e.deps.labels);
    v.apply(Arc::new(doc.clone()), Arc::new(m.clone()), &ch);
    let after = keys(&v);
    assert_ne!(after[0], before[0], "unresolved [foo] now resolves: new key");
    assert_eq!(after[1], before[1], "no brackets: same key");
    assert_eq!(after[2], before[2], "[bar] does not depend on foo");
    assert_eq!(after[3], before[3], "a certain inline link looks nothing up");
    // a fresh view of the new document agrees (keys are a function of content + definitions)
    let fresh = View::new(Arc::new(doc.clone()), Arc::new(m.clone()), Theme::light(), 600.0, 1.0);
    assert_eq!(keys(&fresh)[..4], after[..4]);
}

#[test]
fn heading_anchor_renumbering_through_apply() {
    let mut doc = md("# Intro\n\n# Intro\n\n# Intro\n");
    let mut m = DocModel::build_owned(&doc, None);
    let anchors = |m: &DocModel| m.outline.iter().map(|o| o.anchor.clone()).collect::<Vec<_>>();
    assert_eq!(anchors(&m), ["intro", "intro-2", "intro-3"]);
    // rename the first: later ids shift down
    let e = doc.apply(&PatchSet::single_verbatim(2..7, "Other"));
    m.apply(&doc, &e);
    m.diff(&DocModel::build_owned(&doc, None)).unwrap();
    assert_eq!(anchors(&m), ["other", "intro", "intro-2"]);
    // a definition used inside a heading changes its text and id
    let mut doc = md("# [x]\n\ntext\n");
    let mut m = DocModel::build_owned(&doc, None);
    assert_eq!(m.outline[0].text, "[x]");
    let end = doc.text().len();
    let e = doc.apply(&PatchSet::single_verbatim(end..end, "\n[x]: http://y\n"));
    m.apply(&doc, &e);
    m.diff(&DocModel::build_owned(&doc, None)).unwrap();
    assert_eq!(m.outline[0].text, "x");
}

#[test]
fn resource_root_change_is_a_dependency() {
    let mut doc = md("---\ntypora-root-url: old\n---\n\n![x](/p.png)\n\ntext\n");
    let mut m = DocModel::build_owned(&doc, None);
    let h = m.ctx_hash;
    let at = doc.text().find("old").unwrap();
    let e = doc.apply(&PatchSet::single_verbatim(at..at + 3, "new"));
    assert!(e.deps.resources);
    m.apply(&doc, &e);
    assert_ne!(m.ctx_hash, h);
    m.diff(&DocModel::build_owned(&doc, None)).unwrap();
}

// ---- cache keys (C-18) -----------------------------------------------------------------

#[test]
fn text_inserted_above_keeps_cached_layouts_of_unchanged_blocks() {
    // a segmented code block (C-18) and plain items below an edit
    let code: String = (0..400).map(|i| format!("line {i}\n")).collect();
    let src = format!("first paragraph\n\n```\n{code}```\n\nafter the code\n\n| a | b |\n|---|---|\n| 1 | 2 |\n");
    let mut doc = md(&src);
    let mut m = DocModel::build_owned(&doc, None);
    let n = m.items.len();
    assert!(n > 5, "code is segmented");
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = View::new(Arc::new(doc.clone()), Arc::new(m.clone()), Theme::light(), 600.0, 1.0);
    for i in 0..n {
        v.layout(i, &mut f, &NoImages);
    }
    let hashes: Vec<u64> = m.items.iter().map(|i| i.hash).collect();
    // insert text into the first paragraph (above everything else)
    let e = doc.apply(&PatchSet::single_verbatim(5..5, " inserted words"));
    m.apply(&doc, &e);
    m.diff(&DocModel::build_owned(&doc, None)).unwrap();
    for (i, h) in hashes.iter().enumerate().skip(1) {
        assert_eq!(m.items[i].hash, *h, "item {i}: offset-independent hash");
    }
    v.set_model(Arc::new(doc.clone()), Arc::new(m.clone()));
    v.laid_out.clear();
    for i in 0..n {
        v.layout(i, &mut f, &NoImages);
    }
    assert_eq!(v.laid_out, vec![0], "only the edited item is laid out again");
}

#[test]
fn view_apply_matches_a_fresh_view() {
    let text = markise_corpus::generate(64 << 10, 3);
    let mut doc = md(&text);
    let mut m = DocModel::build_owned(&doc, None);
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = View::new(Arc::new(doc.clone()), Arc::new(m.clone()), Theme::light(), 600.0, 1.0);
    v.scroll_to_item(m.items.len() / 2);
    v.frame(800.0, 200.0, &mut f, &NoImages);
    let mut seed = 0x1234_5678u64;
    for k in 0..30 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let t = doc.text();
        let a = char_floor(t, (seed >> 20) as usize % t.len());
        let b = char_floor(t, a + (seed >> 52) as usize % 30).max(a);
        let e = doc.apply(&PatchSet::single_verbatim(a..b, SNIPPETS[k % SNIPPETS.len()]));
        let ch = m.apply(&doc, &e);
        v.apply(Arc::new(doc.clone()), Arc::new(m.clone()), &ch);
        assert_eq!(v.len(), m.items.len());
        v.frame(800.0, 200.0, &mut f, &NoImages);
        // every height equals what a fresh view (same cache) would assume
        let mut fresh = View::new(Arc::new(doc.clone()), Arc::new(m.clone()), Theme::light(), 600.0, 1.0);
        fresh.cache = std::mem::take(&mut v.cache);
        fresh.set_model(Arc::new(doc.clone()), Arc::new(m.clone()));
        for i in 0..m.items.len() {
            assert_eq!(v.height(i), fresh.height(i), "step {k}: height of item {i}");
        }
        // keys (with reference fingerprints) equal those of a view of a freshly parsed copy:
        // the definitions context is never stale after an edit
        let reparsed = md(doc.text());
        let rm = DocModel::build(&reparsed, None);
        let other = View::new(Arc::new(reparsed), rm, Theme::light(), 600.0, 1.0);
        for i in 0..m.items.len() {
            assert_eq!(v.cache_key(i), other.cache_key(i), "step {k}: key of item {i}");
        }
        v.cache = std::mem::take(&mut fresh.cache);
    }
}

// ---- source maps (§2.3) ----------------------------------------------------------------

fn text_prims(v: &mut View, i: usize) -> Vec<(markise_layout::FragmentId, Arc<dyn markise_layout::TextBlock>)> {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    v.layout(i, &mut f, &NoImages)
        .prims
        .iter()
        .filter_map(|p| match p {
            Prim::Text { frag, text, .. } => Some((*frag, text.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn source_maps_round_trip_and_are_item_relative() {
    let src = "intro\n\n> a **bold** [link](http://x) &amp; `co de` x\\*y\n> second line\n\n| h | `c\\|d` |\n|---|---|\n| **v** | w |\n\n```\nfn a()\n  b\n```\n";
    let doc = md(src);
    let shown = testing::Shown::new(Arc::new(doc));
    let mut v = shown.view(Theme::light(), 800.0, 1.0);
    let mut checked = 0;
    for i in 0..shown.items.len() {
        let start = shown.item_start(i);
        for (frag, tb) in text_prims(&mut v, i) {
            let Some(map) = tb.source_map() else { continue };
            let rendered = tb.text();
            for p in &map.pieces {
                assert!(p.text.end as usize <= rendered.len(), "{frag:?} {p:?}");
                match p.kind {
                    PieceKind::Visible => {
                        // rendered bytes are the source bytes (item-relative offsets)
                        let s = &src[start + p.source.start as usize..start + p.source.end as usize];
                        assert_eq!(&rendered[p.text.start as usize..p.text.end as usize], s);
                        for t in p.text.start..=p.text.end {
                            let so = map.to_source(t).unwrap();
                            assert_eq!(map.to_text(so), Some(t), "round trip at {t} in {rendered:?}");
                        }
                        checked += 1;
                    }
                    PieceKind::Atom => {
                        assert_eq!(map.to_source(p.text.start), Some(p.source.start));
                        assert_eq!(map.to_text(p.source.start), Some(p.text.start));
                    }
                    _ => {}
                }
            }
        }
    }
    assert!(checked > 10, "visible pieces checked: {checked}");
    // a hidden marker maps to the content boundary after it
    let i = (0..shown.items.len())
        .find(|&i| shown.item_span(i).slice(src).contains("**bold**"))
        .unwrap();
    let start = shown.item_start(i);
    let (_, tb) = text_prims(&mut v, i).into_iter().next().unwrap();
    let map = tb.source_map().unwrap();
    let star = (src[start..].find("**bold").unwrap()) as u32;
    let t = map.to_text(star).unwrap() as usize;
    assert!(tb.text()[t..].starts_with("bold"), "{:?}", &tb.text()[t..]);
}

#[test]
fn item_layout_is_identical_wherever_the_item_is() {
    // same block, different offsets: equal fragment ids and source maps (cacheable by content)
    let a = testing::Shown::new(Arc::new(md("x\n\n> a *b* c\n")));
    let b = testing::Shown::new(Arc::new(md("much longer text above\n\nmore\n\n> a *b* c\n")));
    let (ia, ib) = (1, 2);
    assert_eq!(a.items[ia].hash, b.items[ib].hash);
    let mut va = a.view(Theme::light(), 600.0, 1.0);
    let mut vb = b.view(Theme::light(), 600.0, 1.0);
    let pa = text_prims(&mut va, ia);
    let pb = text_prims(&mut vb, ib);
    assert_eq!(pa.len(), pb.len());
    for ((fa, ta), (fb, tb)) in pa.iter().zip(&pb) {
        assert_eq!(fa, fb);
        assert_eq!(ta.source_map(), tb.source_map());
    }
}

// ---- Codex review of PR #13 -----------------------------------------------------------

/// Applies `ps` to `doc`, the model and the view; checks model == full build and the view's
/// heights/keys == a fresh view's, and that a frame works.
fn step_view(
    doc: &mut markise_md::Document,
    m: &mut DocModel,
    v: &mut View,
    ps: &PatchSet,
) -> markise_layout::ModelChange {
    let e = doc.apply(ps);
    let ch = m.apply(&*doc, &e);
    m.diff(&DocModel::build_owned(&*doc, None)).unwrap();
    v.apply(Arc::new(doc.clone()), Arc::new(m.clone()), &ch);
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    v.scroll_to_item(m.items.len().saturating_sub(1));
    v.frame(800.0, 200.0, &mut f, &NoImages);
    let mut fresh = View::new(Arc::new(doc.clone()), Arc::new(m.clone()), Theme::light(), 600.0, 1.0);
    fresh.cache = std::mem::take(&mut v.cache);
    fresh.set_model(Arc::new(doc.clone()), Arc::new(m.clone()));
    assert_eq!(v.len(), fresh.len());
    for i in 0..m.items.len() {
        assert_eq!(v.height(i), fresh.height(i), "height of item {i}");
        assert_eq!(v.cache_key(i), fresh.cache_key(i), "key of item {i}");
    }
    v.cache = std::mem::take(&mut fresh.cache);
    ch
}

#[test]
fn toc_resegmentation_across_the_threshold_keeps_the_view_consistent() {
    // 128 headings: one TOC item; the 129th splits the TOC into three segments, and back
    let mut src = String::from("[toc]\n\n");
    for i in 0..128 {
        src.push_str(&format!("## Heading {i}\n\n"));
    }
    src.push_str("tail\n");
    let mut doc = md(&src);
    let mut m = DocModel::build_owned(&doc, None);
    let tocs = |m: &DocModel| m.items.iter().filter(|i| i.kind == ItemKind::Toc).count();
    assert_eq!(tocs(&m), 1);
    let mut v = View::new(Arc::new(doc.clone()), Arc::new(m.clone()), Theme::light(), 600.0, 1.0);
    let end = doc.text().len();
    let ch = step_view(
        &mut doc,
        &mut m,
        &mut v,
        &PatchSet::single_verbatim(end..end, "\n## Heading 128\n"),
    );
    assert!(tocs(&m) > 1, "segmented");
    assert!(ch.full, "a TOC changing its item count resets the view");
    let at = doc.text().rfind("\n## Heading 128\n").unwrap();
    step_view(&mut doc, &mut m, &mut v, &PatchSet::single_verbatim(at..at + 16, ""));
    assert_eq!(tocs(&m), 1, "unsegmented again");
    // an outline change that keeps the TOC's size is a plain re-key
    let at = doc.text().find("Heading 5").unwrap() + 8;
    let ch = step_view(&mut doc, &mut m, &mut v, &PatchSet::single_verbatim(at..at, "x"));
    assert!(!ch.full);
}

#[test]
fn footnote_reference_followed_by_parens_is_a_dependency() {
    // `[^n](url)` is tried as a footnote first: adding `[^n]: …` changes its meaning
    let mut doc = md("# Head [^n](u)\n\nbody [^n](http://x) text\n\nend\n");
    let mut m = DocModel::build_owned(&doc, None);
    let mut v = View::new(Arc::new(doc.clone()), Arc::new(m.clone()), Theme::light(), 600.0, 1.0);
    let before: Vec<_> = (0..v.len()).map(|i| v.cache_key(i)).collect();
    let text_before = m.outline[0].text.clone();
    let end = doc.text().len();
    step_view(
        &mut doc,
        &mut m,
        &mut v,
        &PatchSet::single_verbatim(end..end, "\n[^n]: note\n"),
    );
    assert_ne!(v.cache_key(0), before[0], "heading re-keyed");
    assert_ne!(v.cache_key(1), before[1], "paragraph re-keyed");
    assert_eq!(v.cache_key(2), before[2], "unrelated paragraph keeps its key");
    assert_ne!(
        m.outline[0].text, text_before,
        "heading text refreshed: {:?}",
        m.outline[0].text
    );
    // and back (positive to negative)
    let at = doc.text().find("\n[^n]: note\n").unwrap();
    step_view(&mut doc, &mut m, &mut v, &PatchSet::single_verbatim(at..at + 12, ""));
    assert_eq!(m.outline[0].text, text_before);
    assert_eq!(v.cache_key(1), before[1]);
}

#[test]
fn escaped_pipe_labels_in_table_cells_are_dependencies() {
    // the cell `[a\|b]` is looked up as `a|b` (GFM: `\|` → `|` before inline parsing)
    let mut doc = md("| h | x |\n|---|---|\n| [a\\|b] | y |\n\nend\n");
    let mut m = DocModel::build_owned(&doc, None);
    let mut v = View::new(Arc::new(doc.clone()), Arc::new(m.clone()), Theme::light(), 600.0, 1.0);
    let k0 = v.cache_key(0);
    let end = doc.text().len();
    // negative → positive
    step_view(
        &mut doc,
        &mut m,
        &mut v,
        &PatchSet::single_verbatim(end..end, "\n[a|b]: http://x\n"),
    );
    let k1 = v.cache_key(0);
    assert_ne!(k1, k0, "the new definition re-keys the table");
    // definition change
    let at = doc.text().find("http://x").unwrap() + 7;
    step_view(&mut doc, &mut m, &mut v, &PatchSet::single_verbatim(at..at + 1, "y"));
    assert_ne!(v.cache_key(0), k1, "the changed definition re-keys the table");
}

#[test]
fn segmented_table_keys_cover_row_offsets() {
    // lengthening row 70 shifts the source positions of every later row: later segments get
    // new keys and their cached cells carry the new offsets
    let mut src = String::from("| a | b |\n|---|---|\n");
    for i in 0..200 {
        src.push_str(&format!("| r{i} | {i} |\n"));
    }
    let mut doc = md(&src);
    let mut m = DocModel::build_owned(&doc, None);
    assert!(m.items.len() >= 3, "segmented: {}", m.items.len());
    let mut v = View::new(Arc::new(doc.clone()), Arc::new(m.clone()), Theme::light(), 600.0, 1.0);
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    for i in 0..m.items.len() {
        v.layout(i, &mut f, &NoImages);
    }
    let before: Vec<u64> = m.items.iter().map(|i| i.hash).collect();
    let at = doc.text().find("| r70 |").unwrap() + 5;
    step_view(&mut doc, &mut m, &mut v, &PatchSet::single_verbatim(at..at, " longer"));
    let last = m.items.len() - 1;
    assert_ne!(m.items[last].hash, before[last], "later segment re-keyed");
    // every cell fragment of the last segment points at its row's real offset
    let start = m.item_start(last);
    let rows = doc.table(start).unwrap().rows;
    let l = v.layout(last, &mut f, &NoImages);
    let mut checked = 0;
    for p in &l.prims {
        if let Prim::Text { frag, text, .. } = p
            && let markise_layout::FragmentKind::Cell { row, .. } = frag.kind
        {
            assert_eq!(frag.block as usize, rows[row as usize].start - start, "row {row}");
            if let Some(map) = text.source_map()
                && let Some(p0) = map.pieces.first()
                && p0.kind == PieceKind::Visible
            {
                let s = &doc.text()[start + p0.source.start as usize..start + p0.source.end as usize];
                assert_eq!(&text.text()[p0.text.start as usize..p0.text.end as usize], s);
            }
            checked += 1;
        }
    }
    assert!(checked > 10);
}

// ---- Codex re-review of PR #13 --------------------------------------------------------

#[test]
fn deleting_the_first_block_before_a_segmented_toc() {
    // the TOC becomes the first block: its items must be rebuilt as a whole (segments)
    let mut src = String::from("intro\n\n[toc]\n\n");
    for i in 0..129 {
        src.push_str(&format!("## Heading {i}\n\n"));
    }
    let mut doc = md(&src);
    let mut m = DocModel::build_owned(&doc, None);
    let tocs = |m: &DocModel| m.items.iter().filter(|i| i.kind == ItemKind::Toc).count();
    let segments = tocs(&m);
    assert!(segments > 1, "segmented TOC");
    let mut v = View::new(Arc::new(doc.clone()), Arc::new(m.clone()), Theme::light(), 600.0, 1.0);
    // delete only `intro`, keep the blank lines
    step_view(&mut doc, &mut m, &mut v, &PatchSet::single_verbatim(0..5, ""));
    assert_eq!(tocs(&m), segments, "same segmentation, no duplicated entries");
    // and insert it again
    step_view(&mut doc, &mut m, &mut v, &PatchSet::single_verbatim(0..0, "intro"));
    assert_eq!(tocs(&m), segments);
}

#[test]
fn definition_change_after_memo_pruning_demotes_dependent_heights() {
    // an off-screen item using [foo] was laid out (exact height); thousands of edits elsewhere
    // make the reference memo prune; a later change of `foo` must still demote its height
    let filler: String = (0..40).map(|i| format!("filler paragraph {i}\n\n")).collect();
    // the definition is far from its user, so the edit re-parses only the definition
    let src = format!("[foo]: http://a\n\ntyped\n\n{filler}uses [foo] here\n\n{filler}");
    let mut doc = md(&src);
    let mut m = DocModel::build_owned(&doc, None);
    let mut v = View::new(Arc::new(doc.clone()), Arc::new(m.clone()), Theme::light(), 600.0, 1.0);
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let user = (0..m.items.len())
        .find(|&i| m.item_span(i).slice(doc.text()).contains("[foo] here"))
        .unwrap();
    v.layout(user, &mut f, &NoImages);
    assert!(v.is_exact(user));
    let at = doc.text().find("typed").unwrap() + 2;
    for k in 0..4300 {
        let e = doc.apply(&PatchSet::single_verbatim(at..at, "x"));
        let ch = m.apply(&doc, &e);
        v.apply(Arc::new(doc.clone()), Arc::new(m.clone()), &ch);
        // the edited (visible) item's key: one new memo entry per edit
        let _ = v.cache_key(1);
        let _ = k;
    }
    let user = (0..m.items.len())
        .find(|&i| m.item_span(i).slice(doc.text()).contains("[foo] here"))
        .unwrap();
    assert!(v.is_exact(user), "untouched off-screen item keeps its exact height");
    let at = doc.text().find("http://a").unwrap() + 7;
    let e = doc.apply(&PatchSet::single_verbatim(at..at + 1, "b"));
    let ch = m.apply(&doc, &e);
    v.apply(Arc::new(doc.clone()), Arc::new(m.clone()), &ch);
    assert!(
        !v.is_exact(user),
        "the changed definition demoted the dependent item's height"
    );
    assert_eq!(v.height(user), v.estimate(user));
}

// ---- Codex round 4 --------------------------------------------------------------------

/// Image sizes for the view tests: every image ready at 400 × 300.
struct Ready;
impl markise_layout::Images for Ready {
    fn state(&self, _src: &str) -> markise_layout::ImageState {
        markise_layout::ImageState::Ready { size: [400, 300] }
    }
}

#[test]
fn resource_root_change_demotes_image_heights_in_the_view() {
    // an interior image laid out (exact height), then `typora-root-url` changes incrementally:
    // its key changes, so its height must go back to an estimate until it is laid out again
    let filler: String = (0..30).map(|i| format!("filler paragraph {i}\n\n")).collect();
    let src = format!("---\ntypora-root-url: old\n---\n\n{filler}![pic](/p.png)\n\n{filler}");
    let mut doc = md(&src);
    let mut m = DocModel::build_owned(&doc, None);
    let mut v = View::new(Arc::new(doc.clone()), Arc::new(m.clone()), Theme::light(), 600.0, 1.0);
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let img = (0..m.items.len()).find(|&i| m.items[i].has_image).unwrap();
    v.layout(img, &mut f, &Ready);
    assert!(v.is_exact(img));
    assert_ne!(
        v.height(img),
        v.estimate(img),
        "a laid-out image differs from its estimate"
    );
    let key = v.cache_key(img);
    let at = doc.text().find("old").unwrap();
    let e = doc.apply(&PatchSet::single_verbatim(at..at + 3, "new"));
    assert!(e.deps.resources);
    let ch = m.apply(&doc, &e);
    m.diff(&DocModel::build_owned(&doc, None)).unwrap();
    v.apply(Arc::new(doc.clone()), Arc::new(m.clone()), &ch);
    assert_ne!(v.cache_key(img), key);
    assert!(!v.is_exact(img), "stale exact image height demoted");
    assert_eq!(v.height(img), v.estimate(img));
    assert!(ch.rehashed.contains(&img), "{ch:?}");
    assert!(
        ch.rehashed.iter().all(|&i| m.items[i].has_image),
        "only image items reported"
    );
}

// ---- G2: nested restarts (ADR 0006 §7.2.3) --------------------------------------------

fn container_docs() -> Vec<String> {
    let mut v = Vec::new();
    let mut s = String::from("# Title\n\n");
    for i in 0..80 {
        s.push_str(&format!("- item {i} words [ref]\n"));
        if i % 9 == 4 {
            s.push_str("  - nested a\n  - nested b\n    1. deep\n");
        }
        if i % 13 == 6 {
            s.push_str("\n  second para\n");
        }
        if i % 17 == 3 {
            s.push_str("  ## heading in item\n");
        }
    }
    s.push_str("\n[ref]: /x\n\n[toc]\n");
    v.push(s);
    let mut s = String::new();
    for i in 0..60 {
        s.push_str(&format!("> para {i} words\n>\n"));
        if i % 8 == 2 {
            s.push_str("> - a\n>   - b\n> - c\n>\n");
        }
        if i % 11 == 5 {
            s.push_str("> ```\n> code\n> ```\n>\n");
        }
    }
    v.push(s);
    let mut s = String::from("1. one\n");
    for i in 0..60 {
        s.push_str(&format!("{}. ordered {i} words\n\n   > quoted {i}\n\n", i + 2));
    }
    v.push(s);
    // empty containers (no item of their own: decorations stay unspent) and neighbour-dependent
    // blocks (adjacent HTML, setext, lazy lines, definitions, tables, adjacent lists)
    let mut s = String::from("# Doc\n\n");
    for i in 0..30 {
        s.push_str(&format!(
            "- >\n\n  para {i}\n  more\n- > -\n- -\n-\n- a\n\n  - >\n\n  tail {i}\n"
        ));
        s.push_str(&format!(
            "- <!--\n  c{i}\n  -->\n  ---\n  <!--\n  d\n  -->\n- text\n  ===\n- [r{i}]: /x\n  t | u\n  --|--\n"
        ));
    }
    s.push('\n');
    for i in 0..20 {
        s.push_str(&format!(
            "> >\n>\n> para {i}\n> more\n> - >\n>\n>   p\n> lazy\ncont\n> * m\n> + n\n>\n"
        ));
    }
    s.push_str("\n[^e]:\n\n[^f]: >\n\n    para\n    more\n");
    v.push(s);
    v
}

const CSNIPS: &[&str] = &[
    "x",
    " ",
    "\n",
    "\n\n",
    "\n- ",
    "\n  - ",
    "\n> ",
    "- ",
    "  ",
    "1. ",
    "```",
    "> ",
    "## h",
    "\n\n\n",
    "[ref]",
    ">\n",
    "- >",
    "> -",
    "\n-\n",
    "<!--\n",
    "-->",
    "===",
    "---",
    "[ref]: /y\n",
    "|--|",
    "+ ",
    "lazy\n",
];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(
        std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(256)
    ))]

    #[test]
    fn prop_apply_equals_full_rebuild_in_containers(
        doc in 0usize..4,
        ops in proptest::collection::vec((0usize..10_000, 0usize..5, 0usize..CSNIPS.len(), any::<bool>()), 1..10),
    ) {
        let docs = container_docs();
        let mut d = md(&docs[doc]);
        let mut m = DocModel::build_owned(&d, None);
        for (pos, del, snip, ins) in ops {
            let t = d.text();
            let a = char_floor(t, pos % (t.len() + 1));
            let b = char_floor(t, a + del).max(a);
            let s = if ins { CSNIPS[snip] } else { "" };
            if a == b && s.is_empty() {
                continue;
            }
            if let Err(e) = step(&mut d, &mut m, &PatchSet::single_verbatim(a..b, s)) {
                return Err(TestCaseError::fail(e));
            }
        }
    }
}

#[test]
fn nested_restarts_update_only_the_region() {
    let mut s = String::new();
    for i in 0..3000 {
        s.push_str(&format!("- item {i} words\n"));
    }
    let mut d = md(&s);
    let mut m = DocModel::build_owned(&d, None);
    let at = s.find("item 1500 words").unwrap() + 15;
    let e = d.apply(&PatchSet::single_verbatim(at..at, "\n- new"));
    assert!(e.nested_from.is_some(), "{e:?}");
    let ch = m.apply(&d, &e);
    m.diff(&DocModel::build_owned(&d, None)).unwrap();
    assert!(ch.replaced.len() <= 3 && ch.inserted.len() <= 4, "{ch:?}");
}

/// Enter inside a huge block quote and inside a nested list of a huge top-level item: the
/// nested restart is taken and the model update stays local (and equals a full build).
#[test]
fn nested_restart_in_quote_and_nested_list_is_local() {
    let mut q = String::from("# Doc\n\nintro\n\n");
    for i in 0..5000 {
        q.push_str(&format!("> quoted paragraph {i} with some words\n>\n"));
    }
    q.push_str("\nafter\n");
    let mut n = String::from("# Doc\n\n");
    for i in 0..4000 {
        if i % 1000 == 0 {
            n.push_str(&format!("- top item {i} words\n"));
        } else {
            n.push_str(&format!("  - nested item {i} with some words\n"));
        }
    }
    for (src, needle, enter) in [
        (&q, "paragraph 2500 with some words", "\n> "),
        (&n, "nested item 1200 with some words", "\n  - "),
        (&n, "nested item 3999 with some words", "\n  - "),
    ] {
        let mut d = md(src);
        let mut m = DocModel::build_owned(&d, None);
        let at = src.find(needle).unwrap() + 20;
        let e = d.apply(&PatchSet::single_verbatim(at..at, enter));
        assert!(e.nested_from.is_some(), "{needle}: {e:?}");
        let ch = m.apply(&d, &e);
        m.diff(&DocModel::build_owned(&d, None)).unwrap();
        assert!(ch.replaced.len() <= 3 && ch.inserted.len() <= 4, "{needle}: {ch:?}");
    }
}

/// Codex review of PR #17 (P2): a nested-region flatten must not assume the enclosing
/// containers already spent their decorations when nothing before the region produced an item
/// (an empty quote emits none): the bullet / quote start / list-start gap stay with the first
/// item of the region. Each case: a document and an insertion that splits a paragraph.
#[test]
fn nested_region_keeps_unspent_decorations() {
    let cases: &[(&str, &str, &str)] = &[
        ("- >\n\n  para\n  more\n", "  more", "\n"),
        ("- >\n  >\n\n  para\n  more\n- next\n", "  more", "\n"),
        ("> >\n>\n> para\n> more\n", "> more", ">\n"),
        ("> - >\n>\n>   para\n>   more\n", ">   more", ">\n"),
        ("- a\n\n  - >\n\n  para\n  more\n", "  more", "\n"),
        ("1. >\n\n   para\n   more\n2. x\n", "   more", "\n"),
        ("[^n]: >\n\n    para\n    more\n", "    more", "\n"),
        ("- -\n\n  para\n  more\n", "  more", "\n"),
    ];
    for &(doc, needle, ins) in cases {
        let at = doc.find(needle).unwrap();
        let mut d = md(doc);
        let mut m = DocModel::build_owned(&d, None);
        if let Err(e) = step(&mut d, &mut m, &PatchSet::single_verbatim(at..at, ins)) {
            panic!("{doc:?} insert {ins:?} at {at}: {e}");
        }
    }
}

/// Review round 6 (PR #15): an edit inside a segment of a large paragraph (the in-place
/// segment refresh) must recompute `has_math`, or a formula typed there keeps a stale key and
/// is never re-laid out when its bitmap arrives.
#[test]
fn segment_refresh_recomputes_has_math() {
    let mut src = String::new();
    for i in 0..200 {
        src.push_str(&format!("line {i} of a long paragraph with plain words\n"));
    }
    let mut doc = md(&src);
    let mut m = DocModel::build_owned(&doc, None);
    let segs: Vec<usize> = (0..m.items.len()).filter(|&i| m.items[i].segment.is_some()).collect();
    assert!(segs.len() > 1, "the paragraph is segmented");
    assert!(segs.iter().all(|&i| !m.items[i].has_math));
    // mid-line insertion in line 150 (a later segment)
    let at = src.find("line 150 of").unwrap() + "line 150 of".len();
    step(&mut doc, &mut m, &PatchSet::single_verbatim(at..at, " $x$")).unwrap();
    let with_math: Vec<usize> = (0..m.items.len()).filter(|&i| m.items[i].has_math).collect();
    assert_eq!(with_math.len(), 1, "exactly the edited segment may show math");
    // and back: removing it clears the flag again
    step(&mut doc, &mut m, &PatchSet::single_verbatim(at..at + 4, "")).unwrap();
    assert!((0..m.items.len()).all(|i| !m.items[i].has_math));
}

/// Found by the property test on CI (PR #21): deleting the first block when the next block
/// has no items (an empty quote): the first block *with* items becomes the document's first
/// item and loses its gap.
#[test]
fn first_item_gap_skips_blocks_without_items() {
    let mut doc = md(BASES[2]);
    let ops = vec![
        (5180, 10, 0, false),
        (253, 7, 27, true),
        (702, 3, 0, false),
        (464, 2, 16, true),
        (2714, 3, 3, false),
        (0, 1, 0, false),
        (750, 7, 0, false),
    ];
    run_edits(&mut doc, &ops).unwrap();
}

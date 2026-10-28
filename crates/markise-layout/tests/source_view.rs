//! The source view (spec 02 §12.1, ADR 0006 §5.5): after random edits — line breaks (LF and
//! CRLF), over-long lines, multi-byte text, Markdown constructs, definitions — the incrementally
//! updated view equals a full rebuild: the same physical lines, line ↔ offset mapping, and the
//! same shaped text and styles for every line (no stale tokens). Tabs are left out: their
//! stops restart at each piece of an over-long line, and piece boundaries may differ.

use std::sync::Arc;

use markise_doc::{FormatDoc, PatchSet};
use markise_layout::highlight::NoHighlights;
use markise_layout::source::{SourceCtx, SourceParams, SourceView};
use markise_layout::testing;
use markise_theme::Theme;
use proptest::prelude::*;

/// Per physical line: the shaped text with one style signature per byte (colour, italics,
/// background) — pieces of an over-long line are joined (piece boundaries may differ between
/// an incrementally updated view and a rebuild; that is not a difference users see in content).
fn rendered(v: &mut SourceView, doc: &Arc<dyn FormatDoc>) -> Vec<(String, Vec<String>)> {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let ctx = SourceCtx {
        doc,
        model: None,
        highlights: &NoHighlights,
    };
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for i in 0..v.len() {
        let l = v.layout(i, &ctx, &mut f);
        for ln in &l.lines {
            let g = ln.text.galley().expect("epaint galley");
            let mut sig = vec![String::new(); g.job.text.len()];
            for sec in &g.job.sections {
                let s = format!(
                    "{:?}{}{}",
                    sec.format.color,
                    if sec.format.italics { "i" } else { "" },
                    if sec.format.background != epaint::Color32::TRANSPARENT {
                        "b"
                    } else {
                        ""
                    }
                );
                for x in &mut sig[sec.byte_range.start.0..sec.byte_range.end.0] {
                    *x = s.clone();
                }
            }
            match out.last_mut() {
                Some(last) if !ln.first => {
                    last.0.push_str(&g.job.text);
                    last.1.extend(sig);
                }
                _ => out.push((g.job.text.clone(), sig)),
            }
        }
    }
    out
}

fn check_equal(v: &mut SourceView, doc: &markise_md::Document) -> Result<(), TestCaseError> {
    let text = doc.text();
    v.check(text).map_err(TestCaseError::fail)?;
    let mut full = SourceView::new(text, v.params(), &Theme::light(), v.width(), 1.0);
    prop_assert_eq!(v.lines_debug(text), full.lines_debug(text));
    prop_assert_eq!(v.line_count(), full.line_count());
    // line ↔ offset against a naive split
    let starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    prop_assert_eq!(starts.len(), v.line_count());
    for (n, &s) in starts.iter().enumerate().step_by(3) {
        prop_assert_eq!(v.line_start(text, n), s);
        prop_assert_eq!(v.line_of(text, s), n);
    }
    let a: Arc<dyn FormatDoc> = Arc::new(doc.clone());
    let (ra, rb) = (rendered(v, &a), rendered(&mut full, &a));
    prop_assert_eq!(ra.len(), rb.len());
    for (k, (x, y)) in ra.iter().zip(&rb).enumerate() {
        if x != y {
            let d = x.1.iter().zip(&y.1).position(|(p, q)| p != q);
            return Err(TestCaseError::fail(format!(
                "line {k}: text equal {} / first style difference at byte {d:?}: {:?} vs {:?}; text {:?}",
                x.0 == y.0,
                d.map(|d| &x.1[d]),
                d.map(|d| &y.1[d]),
                x.0.chars().take(80).collect::<String>()
            )));
        }
    }
    Ok(())
}

fn piece() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("\n".to_string()),
        Just("\r\n".to_string()),
        Just("# ".to_string()),
        Just("*em* ".to_string()),
        Just("**".to_string()),
        Just("`".to_string()),
        Just("- item\n".to_string()),
        Just("> ".to_string()),
        Just("```\n".to_string()),
        Just("[x]".to_string()),
        Just("\n[x]: http://e.com\n".to_string()),
        Just("é漢".to_string()),
        Just("word ".to_string()),
        Just("y".repeat(5000)),
        Just("\n\n".to_string()),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn incremental_source_view_equals_rebuild(
        start in prop::collection::vec(piece(), 0..40),
        edits in prop::collection::vec((0.0f64..1.0, 0usize..40, prop::option::of(piece())), 1..25),
        wrap in any::<bool>(),
    ) {
        let src: String = start.concat();
        let mut doc = markise_md::parse(&src);
        let params = SourceParams { wrap, tab_width: 4 };
        let mut v = SourceView::new(&src, params, &Theme::light(), 300.0, 1.0);
        // lay everything out once so stale layouts would show
        {
            let a: Arc<dyn FormatDoc> = Arc::new(doc.clone());
            rendered(&mut v, &a);
        }
        for (at, del, ins) in edits {
            let text = doc.text().to_string();
            let mut a = ((text.len() as f64) * at) as usize;
            while !text.is_char_boundary(a) {
                a -= 1;
            }
            let mut b = (a + del).min(text.len());
            while !text.is_char_boundary(b) {
                b += 1;
            }
            let ins = ins.unwrap_or_default();
            let e = doc.apply(&PatchSet::single_verbatim(a..b, ins));
            v.apply(doc.text(), &e);
            v.check(doc.text()).map_err(TestCaseError::fail)?;
        }
        check_equal(&mut v, &doc)?;
    }
}

/// A 10 MB document: built, edited at the start, middle and end, scrolled to the end — all
/// without touching more than a few items (the edits stay local).
#[test]
fn large_document_edits_stay_local() {
    let mut src = String::with_capacity(10 << 20);
    let mut n = 0;
    while src.len() < 10 << 20 {
        n += 1;
        src.push_str(&format!("Line {n} of a long *document* with `code` and words.\n"));
    }
    let mut doc = markise_md::parse(&src);
    let t = std::time::Instant::now();
    let mut v = SourceView::new(&src, SourceParams::default(), &Theme::light(), 700.0, 1.0);
    eprintln!("build 10 MB: {:?} ({} items)", t.elapsed(), v.len());
    let items = v.len();
    for at in [0, src.len() / 2, src.len()] {
        let e = doc.apply(&PatchSet::single_verbatim(at..at, "x\ny"));
        v.apply(doc.text(), &e);
        assert!(v.len().abs_diff(items) <= 2);
    }
    v.check(doc.text()).unwrap();
    let a: Arc<dyn FormatDoc> = Arc::new(doc.clone());
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let ctx = SourceCtx {
        doc: &a,
        model: None,
        highlights: &NoHighlights,
    };
    v.scroll_to_y(f64::MAX);
    let vis = v.frame(900.0, 450.0, &ctx, &mut f);
    assert!(vis.last().is_some_and(|l| l.idx + 1 == v.len()), "the end is shown");
    assert!(v.laid_out.len() < 10, "{} items laid out", v.laid_out.len());
    assert_eq!(v.line_count(), n + 4, "three inserted breaks and the final empty line");
}

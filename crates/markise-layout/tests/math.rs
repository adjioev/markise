//! Rendered math in layout (ADR 0008): placement of inline and display formulas, fallbacks,
//! and the math epoch in cache keys.

use std::sync::Arc;

use markise_layout::block::{ImageState, Images, NoImages, Prim};
use markise_layout::{MathBox, MathQuery, MathState, View, testing};
use markise_theme::Theme;

fn view(src: &str, width: f32) -> View {
    testing::Shown::new(Arc::new(markise_md::parse(src))).view(Theme::light(), width, 1.0)
}

/// Every formula renders to a box of 40 × (20 + 10) px with 2 px padding; `\bad` fails.
struct FakeMath;

impl Images for FakeMath {
    fn state(&self, _src: &str) -> ImageState {
        ImageState::Pending
    }

    fn math(&self, q: &MathQuery) -> MathState {
        match self.natural(q) {
            // like the renderer: a formula wider than `max_width` is rendered scaled down
            MathState::Ready(b) => match q.max_width {
                Some(mw) if b.width > mw => {
                    let k = mw / b.width;
                    MathState::Ready(MathBox {
                        key: b.key ^ (mw as u64) << 32,
                        size: [
                            (b.size[0] as f32 * k).ceil() as u32,
                            (b.size[1] as f32 * k).ceil() as u32,
                        ],
                        baseline: b.baseline * k,
                        pad: b.pad,
                        width: mw,
                        ascent: b.ascent * k,
                        descent: b.descent * k,
                    })
                }
                _ => MathState::Ready(b),
            },
            other => other,
        }
    }
}

impl FakeMath {
    fn natural(&self, q: &MathQuery) -> MathState {
        if q.tex.contains("wide") {
            return MathState::Ready(MathBox {
                key: 10,
                size: [2004, 34],
                baseline: 22.0,
                pad: 2.0,
                width: 2000.0,
                ascent: 20.0,
                descent: 10.0,
            });
        }
        if q.tex.contains("\\bad") {
            return MathState::Failed("Undefined control sequence".into());
        }
        if q.tex.trim().is_empty() {
            return MathState::Empty;
        }
        if q.tex.contains("small") {
            return MathState::Ready(MathBox {
                key: 8,
                size: [24, 17],
                baseline: 12.0,
                pad: 2.0,
                width: 20.0,
                ascent: 10.0,
                descent: 3.0,
            });
        }
        if q.tex.contains("tall") {
            // tall but shallow (`\rule{1em}{10em}`): 80 px above the baseline, nothing below
            return MathState::Ready(MathBox {
                key: 9,
                size: [24, 84],
                baseline: 82.0,
                pad: 2.0,
                width: 20.0,
                ascent: 80.0,
                descent: 0.0,
            });
        }
        if q.tex.contains("deep") {
            // a tall inline matrix: 50 px above and 45 px below the baseline
            return MathState::Ready(MathBox {
                key: 7,
                size: [64, 99],
                baseline: 52.0,
                pad: 2.0,
                width: 60.0,
                ascent: 50.0,
                descent: 45.0,
            });
        }
        MathState::Ready(MathBox {
            key: q.tex.len() as u64 + if q.display { 1000 } else { 0 },
            size: [44, 34],
            baseline: 22.0,
            pad: 2.0,
            width: 40.0,
            ascent: 20.0,
            descent: 10.0,
        })
    }
}

fn math_prims(prims: &[Prim]) -> Vec<(epaint::Rect, u64, bool)> {
    prims
        .iter()
        .filter_map(|p| match p {
            Prim::Math { rect, key, query, .. } => Some((*rect, *key, query.display)),
            _ => None,
        })
        .collect()
}

#[test]
fn inline_math_reserves_its_width_on_the_text_baseline() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = view("before $x^2$ after\n", 700.0);
    let vis = v.frame(600.0, 0.0, &mut f, &FakeMath);
    let l = &vis[0].layout;
    let m = math_prims(&l.prims);
    assert_eq!(m.len(), 1, "{:?}", l.prims);
    let (rect, _, display) = m[0];
    assert!(!display);
    assert_eq!(rect.size(), epaint::vec2(44.0, 34.0));
    // the text block: "before " … " after" must not overlap the formula's ink box
    let Some(Prim::Text { pos, text, .. }) = l.prims.iter().find(|p| matches!(p, Prim::Text { .. })) else {
        panic!("no text")
    };
    let s = text.text();
    let before = s.find("before").unwrap();
    let after = s.find("after").unwrap();
    let b = text.ink_spans(&(before..before + 6)).pop().unwrap();
    let a = text.ink_spans(&(after..after + 5)).pop().unwrap();
    let ink_x0 = rect.min.x + 2.0;
    let ink_x1 = rect.max.x - 2.0;
    assert!(pos.x + b.x1 <= ink_x0 + 0.5, "formula overlaps the text before it");
    assert!(pos.x + a.x0 >= ink_x1 - 0.5, "formula overlaps the text after it");
    // baseline: bitmap baseline (22 px from its top) on the text baseline
    let text_base = pos.y + a.baseline;
    assert!(
        (rect.min.y + 22.0 - text_base).abs() < 1.0,
        "{} vs {text_base}",
        rect.min.y + 22.0
    );
    // the source map treats the formula as one atom (caret skips it)
    assert!(text.source_map().is_some());
}

#[test]
fn display_math_is_a_centred_bitmap() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = view("para\n\n$$\n\\frac{a}{b}\n$$\n\n```math\ny\n```\n", 600.0);
    let vis = v.frame(600.0, 0.0, &mut f, &FakeMath);
    for item in &vis[1..3] {
        let m = math_prims(&item.layout.prims);
        assert_eq!(m.len(), 1, "{:?}", item.layout.prims);
        let (rect, _, display) = m[0];
        assert!(display);
        assert!((rect.center().x - 300.0).abs() < 1.0, "{rect:?}");
        assert!(item.layout.height >= 34.0);
    }
}

#[test]
fn pending_and_failed_math_show_source() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let src = "a $x$ b\n\n$$\nx\n$$\n\nc $\\bad$ d\n\n$$\n\\bad\n$$\n";
    let mut v = view(src, 600.0);
    let vis = v.frame(600.0, 0.0, &mut f, &NoImages);
    assert!(vis.iter().all(|it| math_prims(&it.layout.prims).is_empty()));
    let mut v = view(src, 600.0);
    let vis = v.frame(600.0, 0.0, &mut f, &FakeMath);
    let n: Vec<usize> = vis.iter().map(|it| math_prims(&it.layout.prims).len()).collect();
    assert_eq!(n, vec![1, 1, 0, 0]);
    // the failed inline formula is shown as its source text
    let Some(Prim::Text { text, .. }) = vis[2].layout.prims.iter().find(|p| matches!(p, Prim::Text { .. })) else {
        panic!()
    };
    assert!(text.text().contains("\\bad"));
}

#[test]
fn math_epoch_relayouts_only_math_items() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = view("text\n\n$$\nx\n$$\n\nmore $y$\n\nplain\n", 700.0);
    let vis = v.frame(600.0, 0.0, &mut f, &NoImages);
    assert!(vis.iter().all(|it| math_prims(&it.layout.prims).is_empty()));
    v.math_changed();
    let vis = v.frame(600.0, 0.0, &mut f, &FakeMath);
    assert_eq!(v.laid_out, vec![1, 2]);
    assert_eq!(math_prims(&vis[1].layout.prims).len(), 1);
    assert_eq!(math_prims(&vis[2].layout.prims).len(), 1);
}

#[test]
fn math_query_is_sized_from_text_and_ppp() {
    use std::sync::Mutex;
    struct Rec(Mutex<Vec<MathQuery>>);
    impl Images for Rec {
        fn state(&self, _src: &str) -> ImageState {
            ImageState::Pending
        }
        fn math(&self, q: &MathQuery) -> MathState {
            self.0.lock().unwrap().push(q.clone());
            MathState::Pending
        }
    }
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(2.0);
    let theme = Theme::light();
    let size = theme.font_size;
    let mut v = testing::Shown::new(Arc::new(markise_md::parse("x $a+b$\n\n$$\nc\n$$\n"))).view(theme, 600.0, 2.0);
    let rec = Rec(Mutex::new(Vec::new()));
    v.frame(600.0, 0.0, &mut f, &rec);
    let qs = rec.0.lock().unwrap();
    let inline = qs.iter().find(|q| !q.display).expect("inline query");
    assert_eq!(&*inline.tex, "a+b");
    assert!((inline.px_per_em - size * markise_layout::MATH_SCALE * 2.0).abs() < 0.01);
    let block = qs.iter().find(|q| q.display).expect("block query");
    assert_eq!(&*block.tex, "c");
    // the column width (600 pt at 2 px/pt) limits the shown size
    assert_eq!(block.max_width, Some(1200.0));
    assert!(inline.max_width.is_some_and(|w| w <= 1200.0));
}

/// Review P2 #2: a deep inline formula must not overlap the rows above and below it, and must
/// stay on the text baseline.
#[test]
fn deep_inline_math_reserves_ascent_and_descent() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = view("first line here  \nbefore $deep$ after  \nlast line here\n", 700.0);
    let vis = v.frame(600.0, 0.0, &mut f, &FakeMath);
    let l = &vis[0].layout;
    let m = math_prims(&l.prims);
    assert_eq!(m.len(), 1);
    let (rect, _, _) = m[0];
    let (ink_top, ink_bottom) = (rect.min.y + 2.0, rect.max.y - 2.0);
    let Some(Prim::Text { pos, text, .. }) = l.prims.iter().find(|p| matches!(p, Prim::Text { .. })) else {
        panic!("no text")
    };
    let s = text.text();
    let span = |w: &str| {
        let i = s.find(w).unwrap();
        text.ink_spans(&(i..i + w.len())).pop().unwrap()
    };
    let (first, after, last) = (span("first"), span("after"), span("last"));
    // baseline alignment kept
    assert!((rect.min.y + 52.0 - (pos.y + after.baseline)).abs() < 1.0);
    // the row above ends above the formula, the row below starts below it
    assert!(
        pos.y + first.baseline + first.descent <= ink_top + 0.5,
        "overlaps the row above"
    );
    assert!(
        pos.y + last.baseline - last.ascent >= ink_bottom - 0.5,
        "overlaps the row below"
    );
    // and the item is tall enough to hold everything
    assert!(l.height >= ink_bottom);
}

#[test]
fn shallow_inline_math_keeps_rows_bottom_aligned() {
    // without a deep formula the job is untouched: same layout as plain text of equal width
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = view("one line  \nother line\n", 700.0);
    let plain = v.frame(600.0, 0.0, &mut f, &FakeMath)[0].layout.height;
    let mut v = view("one line  \nother $small$\n", 700.0);
    let with = v.frame(600.0, 0.0, &mut f, &FakeMath)[0].layout.height;
    assert!((with - plain).abs() < 1.0, "{with} vs {plain}");
}

/// Review round 2: a tall-but-shallow formula and a deep one in the same paragraph — the
/// paragraph is centred, so the tall formula's strut must be sized for centring too.
#[test]
fn mixed_tall_and_deep_formulas_overlap_no_row() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let src = "top words here  \nrow with $tall$ formula  \nmiddle words  \nrow with $deep$ matrix  \nbottom words\n";
    let mut v = view(src, 700.0);
    let vis = v.frame(600.0, 0.0, &mut f, &FakeMath);
    let l = &vis[0].layout;
    let Some(Prim::Text { pos, text, .. }) = l.prims.iter().find(|p| matches!(p, Prim::Text { .. })) else {
        panic!("no text")
    };
    let s = text.text();
    let span = |w: &str| {
        let i = s.find(w).unwrap();
        text.ink_spans(&(i..i + w.len())).pop().unwrap()
    };
    let rows: Vec<_> = ["top", "middle", "bottom"].iter().map(|w| span(w)).collect();
    let rects: Vec<_> = math_prims(&l.prims).into_iter().map(|(r, ..)| r).collect();
    assert_eq!(rects.len(), 2);
    for r in &rects {
        let (top, bottom) = (r.min.y + 2.0, r.max.y - 2.0);
        for (name, sp) in ["top", "middle", "bottom"].iter().zip(&rows) {
            let (t0, t1) = (pos.y + sp.baseline - sp.ascent, pos.y + sp.baseline + sp.descent);
            assert!(
                t1 <= top + 0.5 || t0 >= bottom - 0.5,
                "formula {r:?} overlaps row {name} ({t0}..{t1})"
            );
        }
    }
    // both formulas stay on their rows' baselines
    let (tall_row, deep_row) = (span("formula"), span("matrix"));
    assert!((rects[0].min.y + 82.0 - (pos.y + tall_row.baseline)).abs() < 1.0);
    assert!((rects[1].min.y + 52.0 - (pos.y + deep_row.baseline)).abs() < 1.0);
}

/// Records every query and answers like `FakeMath`.
struct Recording(std::sync::Mutex<Vec<MathQuery>>);

impl Images for Recording {
    fn state(&self, _src: &str) -> ImageState {
        ImageState::Pending
    }
    fn math(&self, q: &MathQuery) -> MathState {
        self.0.lock().unwrap().push(q.clone());
        FakeMath.math(q)
    }
}

/// Review P2 #3: a formula wider than the column is shown at most column-wide; the query
/// carries the column width (the provider buckets it and renders only that size).
#[test]
fn wide_formula_is_limited_to_the_column() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    for col in [600.0, 601.0, 615.0] {
        let rec = Recording(std::sync::Mutex::new(Vec::new()));
        let mut v = view("$$\nwide\n$$\n", col);
        let vis = v.frame(col, 0.0, &mut f, &rec);
        let m = math_prims(&vis[0].layout.prims);
        assert_eq!(m.len(), 1);
        assert!(m[0].0.width() <= col + 4.5, "{:?} in a {col} pt column", m[0].0);
        let qs = rec.0.lock().unwrap();
        assert!(
            qs.iter().all(|q| q.max_width == Some(col)),
            "{:?}",
            qs.iter().map(|q| q.max_width).collect::<Vec<_>>()
        );
        assert_eq!(qs.len(), 1, "one query per formula and layout");
    }
}

/// Review round 2: formulas in table cells are sized to their cell and clipped to it.
#[test]
fn table_formulas_fit_and_clip_to_their_cells() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = view("| a | b |\n|---|---|\n| $wide$ | $wide$ |\n", 600.0);
    let vis = v.frame(600.0, 0.0, &mut f, &FakeMath);
    let l = &vis[0].layout;
    let cells: Vec<_> = l
        .prims
        .iter()
        .filter_map(|p| match p {
            Prim::Math { rect, clip, .. } => Some((*rect, clip.expect("cell formulas are clipped"))),
            _ => None,
        })
        .collect();
    assert_eq!(cells.len(), 2, "{:?}", l.prims);
    for (rect, clip) in &cells {
        // the ink (bitmap minus padding) fits its cell
        assert!(rect.width() - 4.0 <= clip.width() + 0.5, "{rect:?} in {clip:?}");
        assert!(clip.width() < 400.0);
    }
    let (a, b) = (cells[0].1, cells[1].1);
    assert!(a.max.x <= b.min.x + 0.5, "cells overlap");
    assert!(
        cells[0].0.max.x - 2.0 <= b.min.x + 0.5,
        "the first formula paints into the second cell"
    );
}

/// Review round 3: with the real renderer and a small bitmap cache, huge formulas in table cells
/// (natural bitmaps far beyond the cache) settle: after they are drawn, further frames queue no
/// work and lay nothing out again.
#[test]
fn real_renderer_settles_under_cache_pressure() {
    use markise_blocks::math::{MathImageLookup, MathMode, MathRequest, MathService};
    struct Real(MathService);
    impl Images for Real {
        fn state(&self, _src: &str) -> ImageState {
            ImageState::Pending
        }
        fn math(&self, q: &MathQuery) -> MathState {
            let req = MathRequest {
                tex: q.tex.to_string(),
                mode: if q.display { MathMode::Display } else { MathMode::Inline },
                px_per_em: q.px_per_em,
                color: q.color.to_srgba_unmultiplied(),
                max_width: q.max_width,
            };
            if q.measure {
                return match self.0.measure(&req) {
                    markise_blocks::math::MathMeasure::Ready { width, ascent, descent } => MathState::Ready(MathBox {
                        key: 0,
                        size: [0, 0],
                        baseline: 0.0,
                        pad: 0.0,
                        width,
                        ascent,
                        descent,
                    }),
                    markise_blocks::math::MathMeasure::Empty => MathState::Empty,
                    markise_blocks::math::MathMeasure::Failed(e) => MathState::Failed(e.to_string()),
                    markise_blocks::math::MathMeasure::Pending => MathState::Pending,
                };
            }
            match self.0.get(&req) {
                MathImageLookup::Ready(i) => MathState::Ready(MathBox {
                    key: i.key,
                    size: i.size,
                    baseline: i.baseline,
                    pad: i.pad as f32,
                    width: i.width,
                    ascent: i.ascent,
                    descent: i.descent,
                }),
                MathImageLookup::Empty => MathState::Empty,
                MathImageLookup::Failed(e) => MathState::Failed(e.to_string()),
                MathImageLookup::Pending => MathState::Pending,
            }
        }
    }
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let d = done.clone();
    let real = Real(
        MathService::with_limits(
            Arc::new(move || d.store(true, std::sync::atomic::Ordering::Release)),
            1,
            4 << 20,
        )
        .unwrap(),
    );
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let src =
        "| a | b | c |\n|---|---|---|\n| $\\rule{200em}{200em}$ | $\\rule{201em}{200em}$ | $\\rule{202em}{200em}$ |\n";
    let mut v = view(src, 600.0);
    let t = std::time::Instant::now();
    let mut drawn = 0;
    while drawn < 3 {
        if done.swap(false, std::sync::atomic::Ordering::AcqRel) {
            v.math_changed();
        }
        let vis = v.frame(600.0, 0.0, &mut f, &real);
        drawn = vis.iter().map(|it| math_prims(&it.layout.prims).len()).sum();
        assert!(t.elapsed() < std::time::Duration::from_secs(30), "never settled");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    while real.0.busy() {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    if done.swap(false, std::sync::atomic::Ordering::AcqRel) {
        v.math_changed();
    }
    let _ = v.frame(600.0, 0.0, &mut f, &real);
    let before = real.0.stats();
    for _ in 0..20 {
        assert!(
            !done.swap(false, std::sync::atomic::Ordering::AcqRel),
            "no wake: nothing rendered"
        );
        let vis = v.frame(600.0, 0.0, &mut f, &real);
        assert!(v.laid_out.is_empty(), "nothing laid out again");
        assert_eq!(
            vis.iter().map(|it| math_prims(&it.layout.prims).len()).sum::<usize>(),
            3
        );
    }
    let after = real.0.stats();
    assert_eq!(after.requests, before.requests, "no new work queued");
    assert_eq!(after.evicted, 0);
}

/// Review round 5: inline display math (`$$…$$` inside a paragraph) asks for display style but
/// stays in the text flow.
#[test]
fn inline_display_math_keeps_display_style_in_the_text_flow() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let rec = Recording(std::sync::Mutex::new(Vec::new()));
    let mut v = view("before $$\\sum_i x_i$$ after and $y$\n", 600.0);
    let vis = v.frame(600.0, 0.0, &mut f, &rec);
    let qs = rec.0.lock().unwrap();
    let sum = qs.iter().find(|q| q.tex.contains("sum")).expect("query");
    assert!(sum.display, "display style requested");
    assert!(!qs.iter().find(|q| &*q.tex == "y").unwrap().display);
    // placed inline: between "before" and "after", on the text row (not a centred block)
    let l = &vis[0].layout;
    let m = math_prims(&l.prims);
    assert_eq!(m.len(), 2);
    let Some(Prim::Text { pos, text, .. }) = l.prims.iter().find(|p| matches!(p, Prim::Text { .. })) else {
        panic!()
    };
    let s = text.text();
    let b = text.ink_spans(&(0..6)).pop().unwrap();
    let i = s.find("after").unwrap();
    let a = text.ink_spans(&(i..i + 5)).pop().unwrap();
    let r = m.iter().find(|(_, _, d)| *d).unwrap().0;
    assert!(
        r.min.x + 2.0 >= pos.x + b.x1 - 0.5 && r.max.x - 2.0 <= pos.x + a.x0 + 0.5,
        "{r:?}"
    );
}

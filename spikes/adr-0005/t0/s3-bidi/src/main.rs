//! Spike S3 (ADR 0005 §6): bidi line "Hello مرحبا بالعالم 123 world" with LTR and RTL base,
//! through markise-text (parley). Checks, against an independent UAX #9 reference
//! (`unicode-bidi` levels + L2 reordering of grapheme clusters):
//!   1. visual Right/Left arrow sequences visit the visual caret slots in order;
//!   2. selection rects for every logical range are non-overlapping and gap-free (equal to the
//!      merged visual extents of the selected clusters);
//!   3. hit tests just inside both edges of every visual run land on the expected slot, and every
//!      cluster boundary round-trips under the §4.3 affinity rule.
//! Run: `cargo run --release -p spike-s3-bidi` (system fonts: needs an Arabic face).

use markise_text::{BaseDir, Family, FontStore, LocalPos, Rect, StyledText, TextEngine, TextLayout, TextStyle};
use unicode_bidi::{BidiInfo, Level};
use unicode_segmentation::UnicodeSegmentation;

const TEXT: &str = "Hello مرحبا بالعالم 123 world";
const EPS: f32 = 0.05;

/// Visual order of grapheme clusters by the reference UAX #9 implementation.
fn reference_visual(text: &str, rtl: bool) -> (Vec<(usize, usize, bool)>, bool) {
    let info = BidiInfo::new(text, Some(if rtl { Level::rtl() } else { Level::ltr() }));
    let para = &info.paragraphs[0];
    let (levels, runs) = info.visual_runs(para, para.range.clone());
    let mut out = Vec::new();
    for run in runs {
        let lvl = levels[run.start];
        let mut gs: Vec<(usize, usize, bool)> = text[run.clone()]
            .grapheme_indices(true)
            .map(|(i, g)| (run.start + i, run.start + i + g.len(), lvl.is_rtl()))
            .collect();
        if lvl.is_rtl() {
            gs.reverse();
        }
        out.extend(gs);
    }
    (out, para.level.is_rtl())
}

/// Positions that denote visual slot `k` (between visual clusters k-1 and k): the right edge
/// of cluster k-1 and the left edge of cluster k, each with the affinity that attaches the
/// caret to that cluster.
fn slot_positions(vis: &[(usize, usize, bool)], k: usize) -> Vec<LocalPos> {
    let mut v = Vec::new();
    if k > 0 {
        let (s, e, r) = vis[k - 1];
        v.push(if r { LocalPos::down(s) } else { LocalPos::up(e) });
    }
    if k < vis.len() {
        let (s, e, r) = vis[k];
        v.push(if r { LocalPos::up(e) } else { LocalPos::down(s) });
    }
    v
}

fn slot_offsets(vis: &[(usize, usize, bool)], k: usize) -> Vec<usize> {
    slot_positions(vis, k).iter().map(|p| p.byte).collect()
}

fn merge(mut iv: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    iv.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f32, f32)> = Vec::new();
    for (a, b) in iv {
        match out.last_mut() {
            Some(l) if a <= l.1 + EPS => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

fn run_case(eng: &mut TextEngine, rtl: bool) -> (usize, usize) {
    let dir = if rtl { BaseDir::Rtl } else { BaseDir::Ltr };
    let style = TextStyle::new(vec![Family::named("Georgia"), Family::parse("serif")], 20.0);
    let t = StyledText::plain(TEXT, style).with_base_dir(dir);
    let l: TextLayout = eng.layout(&t, None, 1.0);
    let (vis, ref_rtl) = reference_visual(TEXT, rtl);
    let mut fails = 0usize;
    let mut checks = 0usize;
    let mut fail = |what: String| {
        fails += 1;
        if fails <= 12 {
            println!("    FAIL {what}");
        }
    };
    println!(
        "== base {} (layout rtl={}, reference rtl={ref_rtl})",
        if rtl { "RTL" } else { "LTR" },
        l.is_rtl()
    );
    let notdef = l
        .glyph_runs()
        .iter()
        .flat_map(|r| r.glyphs.iter())
        .filter(|g| g.id == 0)
        .count();
    println!("   glyph runs: {}, .notdef glyphs: {notdef}", l.glyph_runs().len());
    let n = vis.len();
    // slot x positions from the layout (caret of either offset of the slot)
    let slot_x: Vec<f32> = (0..=n)
        .map(|k| {
            let ps = slot_positions(&vis, k);
            let xs: Vec<f32> = ps.iter().map(|p| l.caret_rect(*p).x0).collect();
            if xs.len() == 2 && (xs[0] - xs[1]).abs() > EPS {
                println!("    note: slot {k} positions {ps:?} draw at {xs:?}");
            }
            xs[0]
        })
        .collect();
    for w in slot_x.windows(2) {
        checks += 1;
        if w[1] < w[0] - EPS {
            fail(format!("slot x not increasing: {slot_x:?}"));
            break;
        }
    }

    // 1. arrows: from the leftmost slot press Right n times, then Left n times
    let mut p = slot_positions(&vis, 0)[0];
    let mut seq = vec![(p.byte, l.x_on_line(p))];
    for k in 1..=n {
        let q = l.move_right(p);
        checks += 1;
        let want = slot_offsets(&vis, k);
        let x = l.x_on_line(q);
        if !want.contains(&q.byte) || (x - slot_x[k]).abs() > EPS {
            fail(format!(
                "Right #{k}: got byte {} x {x:.1}, want {want:?} x {:.1}",
                q.byte, slot_x[k]
            ));
        }
        seq.push((q.byte, x));
        p = q;
    }
    println!(
        "   Right sequence (byte, x): {:?}",
        seq.iter()
            .map(|(b, x)| (*b, (x * 10.0).round() / 10.0))
            .collect::<Vec<_>>()
    );
    for k in (0..n).rev() {
        let q = l.move_left(p);
        checks += 1;
        let want = slot_offsets(&vis, k);
        let x = l.x_on_line(q);
        if !want.contains(&q.byte) || (x - slot_x[k]).abs() > EPS {
            fail(format!(
                "Left to slot {k}: got byte {} x {x:.1}, want {want:?} x {:.1}",
                q.byte, slot_x[k]
            ));
        }
        p = q;
    }

    // 2. selection rects for every logical range of cluster boundaries
    let bounds = l.grapheme_boundaries().to_vec();
    let extent = |s: usize, e: usize| -> (f32, f32) {
        // visual extent of one cluster from the reference visual index and slot xs
        let k = vis.iter().position(|v| v.0 == s && v.1 == e).expect("cluster");
        (slot_x[k], slot_x[k + 1])
    };
    let mut pairs = 0;
    for i in 0..bounds.len() {
        for j in i + 1..bounds.len() {
            pairs += 1;
            checks += 1;
            let rects: Vec<Rect> = l.selection_rects(LocalPos::down(bounds[i]), LocalPos::down(bounds[j]));
            let mut sorted: Vec<(f32, f32)> = rects.iter().map(|r| (r.x0, r.x1)).collect();
            sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
            let overlap = sorted.windows(2).any(|w| w[0].1 > w[1].0 + EPS);
            let want = merge(bounds[i..=j].windows(2).map(|w| extent(w[0], w[1])).collect());
            let got = merge(sorted.clone());
            let same = want.len() == got.len()
                && want
                    .iter()
                    .zip(&got)
                    .all(|(a, b)| (a.0 - b.0).abs() < 0.5 && (a.1 - b.1).abs() < 0.5);
            if overlap || !same {
                fail(format!(
                    "selection {}..{}: rects {sorted:?} want {want:?}",
                    bounds[i], bounds[j]
                ));
            }
        }
    }
    println!("   selection ranges checked: {pairs}");

    // 3. hits at both edges of each visual run, and caret round trips
    let mut edges = Vec::new();
    for k in 1..n {
        if vis[k - 1].2 != vis[k].2 {
            edges.push(k);
        }
    }
    edges.push(0);
    edges.push(n);
    for &k in &edges {
        for dx in [-0.3f32, 0.3] {
            let x = slot_x[k] + dx;
            let (_, y) = l.caret_rect(LocalPos::down(0)).center();
            let q = l.hit(x, y);
            checks += 1;
            // nearest slot by x
            let near = (0..=n)
                .min_by(|&a, &b| (slot_x[a] - x).abs().total_cmp(&(slot_x[b] - x).abs()))
                .unwrap();
            if !slot_offsets(&vis, near).contains(&q.byte) || (l.x_on_line(q) - slot_x[near]).abs() > EPS {
                fail(format!(
                    "hit x={x:.1} → {q:?} (x {:.1}), want slot {near} {:?}",
                    l.x_on_line(q),
                    slot_offsets(&vis, near)
                ));
            }
        }
    }
    let mut ambiguous = 0;
    for &b in &bounds {
        let up = l.caret_rect(LocalPos::up(b));
        let down = l.caret_rect(LocalPos::down(b));
        if (up.x0 - down.x0).abs() > EPS {
            ambiguous += 1;
        }
        for (p, r) in [(LocalPos::up(b), up), (LocalPos::down(b), down)] {
            checks += 1;
            let q = l.hit(r.x0, (r.y0 + r.y1) / 2.0);
            if (l.caret_rect(q).x0 - r.x0).abs() > EPS {
                fail(format!("round trip {p:?} at {:.1} → {q:?}", r.x0));
            }
        }
    }
    println!(
        "   run edges hit-tested: {}; boundaries with two visual carets: {ambiguous}",
        edges.len()
    );
    println!("   checks {checks}, failures {fails}");
    (checks, fails)
}

fn main() {
    let mut store = FontStore::new();
    store.start_system_discovery();
    store.wait_for_system(std::time::Duration::from_secs(60));
    let mut eng = TextEngine::new(store);
    let (c1, f1) = run_case(&mut eng, false);
    let (c2, f2) = run_case(&mut eng, true);
    // the original spike's observation (README: "6 → 36"): Right from byte 6 with Downstream
    let t = StyledText::plain(TEXT, TextStyle::new(vec![Family::parse("serif")], 20.0));
    let l = eng.layout(&t, None, 1.0);
    let mut p = LocalPos::down(6);
    let mut v = Vec::new();
    for _ in 0..8 {
        p = l.move_right(p);
        v.push(p.byte);
    }
    println!("== old spike check: Right from byte 6 (auto dir): {v:?}");
    println!("S3 TOTAL checks {} failures {}", c1 + c2, f1 + f2);
}

//! Spike S5 (ADR 0005 §4.2, §6): a 200 KiB single line mixing Arabic, Hebrew, Latin, digits,
//! brackets and nested isolates, cut into pieces, compared with an unchunked reference.
//!
//! Reference: UAX #9 levels of the whole line (`unicode-bidi`), cluster visual order per piece
//! derived from those levels (L2), mirroring from levels, glyph ids from harfrust shaping of the
//! whole Arabic word that straddles a cut.
//!
//! Options (ADR 0005 §4.2):
//!   degraded  — each piece laid out alone by parley with the paragraph's base direction;
//!   (b)       — each piece laid out by parley with directional overrides inserted so that
//!               every character gets exactly its reference level (nesting rebuilt with
//!               RLE/LRE steps and a final RLO/LRO); controls are invisible, offsets remapped;
//!   (c)       — harfrust shaping of each piece with pre/post context (our own line assembly
//!               would place it; only the shaping-context half is measured here);
//!   (a)       — upstream parley API for precomputed levels/context: not available in 0.12
//!               (nothing to run).
//! Cut sets: "normal" (first space after 4 KiB outside isolates/brackets, else forced at 8 KiB)
//! and "forced" (every 4 KiB inside an Arabic word, inside isolates/brackets where possible).
//! Also: whole-line re-analysis time (bidi + graphemes + line breaks + scripts) for 200 KiB.
//! Run: `cargo run --release -p spike-s5-chunk-context` (needs a system font with Arabic and
//! Hebrew, e.g. Arial on macOS).

use std::time::Instant;

use parley::{FontContext, Layout, LayoutContext, StyleProperty};
use unicode_bidi::{BidiClass, BidiInfo, Level, bidi_class};
use unicode_segmentation::UnicodeSegmentation;

const TARGET: usize = 200 * 1024;

fn generate() -> String {
    let arabic = ["مرحبا", "بالعالم", "كتاب", "السلام", "عليكم", "مدينة", "جميلة", "تعلم"];
    let hebrew = ["שלום", "עולם", "ספר", "בית", "מילה", "ירושלים"];
    let latin = ["hello", "world", "text", "layout", "Markise", "editor"];
    let mut s = String::new();
    let mut x: u64 = 0x2545F4914F6CDD1D;
    let mut rnd = |n: usize| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x % n as u64) as usize
    };
    while s.len() < TARGET {
        match rnd(10) {
            0..=3 => s.push_str(arabic[rnd(arabic.len())]),
            4..=5 => s.push_str(hebrew[rnd(hebrew.len())]),
            6 => s.push_str(latin[rnd(latin.len())]),
            7 => s.push_str(&format!("{}", rnd(100000))),
            8 => s.push_str(&format!("({} {})", arabic[rnd(arabic.len())], latin[rnd(latin.len())])),
            _ => s.push_str(&format!(
                "\u{2067}{} \u{2066}{} {}\u{2069} {}\u{2069}",
                hebrew[rnd(hebrew.len())],
                latin[rnd(latin.len())],
                rnd(999),
                arabic[rnd(arabic.len())]
            )),
        }
        s.push(' ');
    }
    s
}

fn is_arabic(c: char) -> bool {
    ('\u{0600}'..='\u{06FF}').contains(&c)
}

/// Cut positions. `normal`: first space after 4 KiB with isolate depth and bracket depth 0,
/// forced at 8 KiB. `forced`: every 4 KiB at a grapheme boundary inside an Arabic word.
fn cuts(text: &str, normal: bool) -> Vec<usize> {
    let mut out = Vec::new();
    let mut last = 0;
    let mut iso = 0i32;
    let mut br = 0i32;
    let bounds: Vec<usize> = text.grapheme_indices(true).map(|(i, _)| i).collect();
    let mut gi = 0;
    for (i, c) in text.char_indices() {
        while gi < bounds.len() && bounds[gi] < i {
            gi += 1;
        }
        let at_g = gi < bounds.len() && bounds[gi] == i;
        match c {
            '\u{2066}'..='\u{2068}' => iso += 1,
            '\u{2069}' => iso -= 1,
            '(' => br += 1,
            ')' => br -= 1,
            _ => {}
        }
        let since = i - last;
        if normal {
            if since >= 4096 && c == ' ' && iso == 0 && br == 0 {
                out.push(i + 1);
                last = i + 1;
            } else if since >= 8192 && at_g {
                out.push(i);
                last = i;
            }
        } else if since >= 4096 && at_g && is_arabic(c) && i > 0 {
            let prev = text[..i].chars().next_back().unwrap();
            if is_arabic(prev) {
                out.push(i);
                last = i;
            }
        }
    }
    out
}

fn mirror(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        _ => c,
    }
}

/// parley clusters of a piece in visual order: (original start, original end, glyph ids).
fn visual_clusters(
    lcx: &mut LayoutContext<()>,
    fcx: &mut FontContext,
    text: &str,
    map: &[Option<usize>],
    rtl_base: bool,
) -> Vec<(usize, usize, Vec<u32>)> {
    let mut b = lcx.ranged_builder(fcx, text, 1.0, true);
    b.push_default(StyleProperty::FontFamily("Arial, sans-serif".into()));
    b.push_default(StyleProperty::FontSize(16.0));
    b.set_base_direction(if rtl_base {
        parley::BaseDirection::Rtl
    } else {
        parley::BaseDirection::Ltr
    });
    let mut l: Layout<()> = b.build(text);
    l.break_all_lines(None);
    let mut out = Vec::new();
    for line in l.lines() {
        for run in line.runs() {
            for c in run.visual_clusters() {
                let r = c.text_range();
                if let (Some(Some(s)), Some(&e)) = (map.get(r.start), map.get(r.end).or(map.last())) {
                    let e = e.unwrap_or(*s + (r.end - r.start));
                    let glyphs = c.glyphs().map(|g| g.id).collect();
                    // explicit formatting characters (and their ZWSP placeholders) are invisible and
                    // their visual position is implementation-defined (UAX #9 X9): not compared
                    if !text[r.clone()].chars().all(|ch| matches!(ch, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200B}' | '\u{200E}' | '\u{200F}')) {
                        out.push((*s, e, glyphs));
                    }
                }
            }
        }
    }
    out
}

/// Expected visual order (L2) of the given logical clusters with reference levels.
fn l2(clusters: &[(usize, usize)], level: impl Fn(usize) -> u8) -> Vec<(usize, usize)> {
    let mut v: Vec<((usize, usize), u8)> = clusters.iter().map(|&c| (c, level(c.0))).collect();
    let max = v.iter().map(|x| x.1).max().unwrap_or(0);
    let min_odd = v.iter().map(|x| x.1).filter(|l| l % 2 == 1).min().unwrap_or(max + 1);
    let mut lvl = max;
    while lvl >= min_odd && lvl > 0 {
        let mut i = 0;
        while i < v.len() {
            if v[i].1 >= lvl {
                let s = i;
                while i < v.len() && v[i].1 >= lvl {
                    i += 1;
                }
                v[s..i].reverse();
            } else {
                i += 1;
            }
        }
        lvl -= 1;
    }
    v.into_iter().map(|x| x.0).collect()
}

/// Option (b): encode reference levels with explicit embeddings/overrides. Runs above the
/// base level: RLE/LRE steps up to level L-1, then RLO/LRO to L (every character becomes strong
/// at exactly L); base-level runs are bracketed by marks of the base direction (LRM/RLM) so
/// neutrals and digits resolve to the base level. Source isolate controls are replaced by ZWSP
/// (BN, same byte length) because their effect is already encoded.
/// Returns (encoded text, map encoded byte → original byte, one extra entry for the end).
fn encode_levels(piece: &str, base: u8, level: impl Fn(usize) -> u8, off: usize) -> (String, Vec<Option<usize>>) {
    fn push(s: &mut String, map: &mut Vec<Option<usize>>, c: char, orig: Option<usize>) {
        s.push(c);
        for k in 0..c.len_utf8() {
            map.push(orig.map(|o| o + k));
        }
    }
    let mark = if base % 2 == 1 { '\u{200F}' } else { '\u{200E}' };
    let mut s = String::new();
    let mut map: Vec<Option<usize>> = Vec::new();
    let mut cur: Option<u8> = None;
    let close = |s: &mut String, map: &mut Vec<Option<usize>>, cur: Option<u8>| {
        if let Some(lv) = cur {
            if lv == base {
                push(s, map, mark, None);
            } else {
                for _ in base..lv {
                    push(s, map, '\u{202C}', None);
                }
            }
        }
    };
    for (i, c) in piece.char_indices() {
        let lv = level(off + i);
        if cur != Some(lv) {
            close(&mut s, &mut map, cur);
            if lv == base {
                push(&mut s, &mut map, mark, None);
            } else {
                let mut l = base;
                while l < lv {
                    let next = l + 1;
                    let ctl = match (next == lv, next % 2 == 1) {
                        (true, true) => '\u{202E}',   // RLO
                        (true, false) => '\u{202D}',  // LRO
                        (false, true) => '\u{202B}',  // RLE
                        (false, false) => '\u{202A}', // LRE
                    };
                    push(&mut s, &mut map, ctl, None);
                    l = next;
                }
            }
            cur = Some(lv);
        }
        if matches!(c, '\u{2066}'..='\u{2069}' | '\u{202A}'..='\u{202E}') {
            push(&mut s, &mut map, '\u{200B}', Some(off + i));
            continue;
        }
        push(&mut s, &mut map, c, Some(off + i));
    }
    // no closing mark at the line end: a strong mark there would stop UAX #9 L1 from
    // resetting the piece's trailing whitespace (embedding closers are removed by X9)
    if cur != Some(base) {
        close(&mut s, &mut map, cur);
    }
    map.push(Some(off + piece.len()));
    (s, map)
}

fn shape(font: &harfrust::FontRef, data: &harfrust::ShaperData, text: &str, pre: &str, post: &str) -> Vec<u32> {
    let shaper = data.shaper(font).build();
    let mut buf = harfrust::UnicodeBuffer::new();
    buf.push_str(text);
    buf.set_pre_context(pre);
    buf.set_post_context(post);
    buf.set_direction(harfrust::Direction::RightToLeft);
    buf.guess_segment_properties();
    let out = shaper.shape(buf, harfrust::ShapeOptions::new());
    let mut v: Vec<u32> = out.glyph_infos().iter().map(|g| g.glyph_id).collect();
    v.reverse(); // logical order
    v
}

fn main() {
    let text = generate();
    println!("line: {} bytes, {} chars", text.len(), text.chars().count());

    // --- whole-line re-analysis cost (ADR: ≤ 2 ms for 200 KiB)
    let mut times = Vec::new();
    for _ in 0..10 {
        let t = Instant::now();
        let info = BidiInfo::new(&text, None);
        let t_bidi = t.elapsed();
        let g = text.grapheme_indices(true).count();
        let t_g = t.elapsed() - t_bidi;
        let seg = icu_segmenter::LineSegmenter::new_auto(Default::default());
        let lb = seg.segment_str(&text).count();
        let t_lb = t.elapsed() - t_bidi - t_g;
        let sc = icu_properties::CodePointMapData::<icu_properties::props::Script>::new();
        let mut scripts = 0u32;
        let mut prev = None;
        for c in text.chars() {
            let s = sc.get(c);
            if prev != Some(s) {
                scripts += 1;
                prev = Some(s);
            }
        }
        let t_sc = t.elapsed() - t_bidi - t_g - t_lb;
        std::hint::black_box((info.levels.len(), g, lb, scripts));
        times.push((t.elapsed(), t_bidi, t_g, t_lb, t_sc));
    }
    times.sort_by_key(|t| t.0);
    let m = times[5];
    println!(
        "re-analysis 200 KiB (median of 10): total {:.2} ms = bidi {:.2} + graphemes {:.2} + line breaks {:.2} + scripts {:.2}",
        m.0.as_secs_f64() * 1e3,
        m.1.as_secs_f64() * 1e3,
        m.2.as_secs_f64() * 1e3,
        m.3.as_secs_f64() * 1e3,
        m.4.as_secs_f64() * 1e3
    );

    // --- reference levels
    let info = BidiInfo::new(&text, None);
    let para = &info.paragraphs[0];
    let base = para.level.number();
    let rtl_base = para.level.is_rtl();
    println!("paragraph base level {base} ({})", if rtl_base { "RTL" } else { "LTR" });
    let level = |i: usize| info.levels[i].number();

    let mut fcx = FontContext::new();
    let mut lcx: LayoutContext<()> = LayoutContext::new();

    // reference unchunked parley layout (timing only; 200 KiB, no wrapping)
    let t = Instant::now();
    let mut b = lcx.ranged_builder(&mut fcx, &text, 1.0, true);
    b.push_default(StyleProperty::FontFamily("Arial, sans-serif".into()));
    let mut l: Layout<()> = b.build(&text);
    l.break_all_lines(None);
    println!(
        "unchunked parley layout of the 200 KiB line: {:.1} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    // font bytes of the Arabic run for harfrust
    let mut arabic_font = None;
    'f: for line in l.lines() {
        for run in line.runs() {
            if run.is_rtl() && is_arabic(text[run.text_range()].chars().next().unwrap_or(' ')) {
                arabic_font = Some(run.font().clone());
                break 'f;
            }
        }
    }
    let arabic_font = arabic_font.expect("an Arabic font");
    let fref = harfrust::FontRef::from_index(arabic_font.data.data(), arabic_font.index).unwrap();
    let sdata = harfrust::ShaperData::new(&fref);
    let mirror_ok = |gid: u32, c: char, odd: bool| -> bool {
        use skrifa::MetadataProvider;
        let f = skrifa::FontRef::from_index(arabic_font.data.data(), arabic_font.index).unwrap();
        let want = if odd { mirror(c) } else { c };
        f.charmap().map(want).map(|g| g.to_u32()) == Some(gid)
    };

    // reference glyph ids per cluster start, from the unchunked parley layout of the whole line
    let mut ref_glyphs: std::collections::HashMap<usize, Vec<u32>> = Default::default();
    for line in l.lines() {
        for run in line.runs() {
            for c in run.clusters() {
                ref_glyphs.insert(c.text_range().start, c.glyphs().map(|g| g.id).collect());
            }
        }
    }
    let invisible = |g: &str| {
        g.chars().all(
            |ch| matches!(ch, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200B}' | '\u{200E}' | '\u{200F}'),
        )
    };

    for normal in [true, false] {
        let cs = cuts(&text, normal);
        let mut edges = vec![0];
        edges.extend(cs.iter().copied());
        edges.push(text.len());
        let name = if normal { "normal" } else { "forced" };
        println!(
            "== {name} cuts: {} pieces (avg {} B)",
            edges.len() - 1,
            text.len() / (edges.len() - 1)
        );
        let mut option_glyphs: Vec<std::collections::HashMap<usize, Vec<u32>>> = Vec::new();
        for opt in ["degraded", "(b) overrides"] {
            let mut order_bad = 0;
            let mut order_bad_near_cut = 0;
            let mut level_bad = 0usize;
            let mut mirror_bad = 0;
            let mut brackets = 0;
            let mut expected_total = 0;
            let mut missing = 0;
            let mut glyphs_by_start: std::collections::HashMap<usize, Vec<u32>> = Default::default();
            for w in edges.windows(2) {
                let (s, e) = (w[0], w[1]);
                let piece = &text[s..e];
                // reference levels for this piece as a visual line: paragraph levels + L1 reset
                // of trailing whitespace / isolate controls at the line end (UAX #9 L1)
                let line_levels = info.reordered_levels(para, s..e);
                let lvl = |i: usize| line_levels[i].number();
                let (enc, map) = if opt == "degraded" {
                    let mut m: Vec<Option<usize>> = (0..piece.len()).map(|k| Some(s + k)).collect();
                    m.push(Some(e));
                    (piece.to_string(), m)
                } else {
                    // encode the *line* levels (after L1): the adapter knows each piece is a
                    // visual line, and parley's own L1 then has nothing left to change
                    encode_levels(piece, base, lvl, s)
                };
                if opt == "degraded" {
                    let pi = BidiInfo::new(piece, Some(Level::new(base).unwrap()));
                    for (k, c) in piece.char_indices() {
                        if !matches!(bidi_class(c), BidiClass::BN) && pi.levels[k].number() != level(s + k) {
                            level_bad += 1;
                        }
                    }
                } else {
                    let ei = BidiInfo::new(&enc, Some(Level::new(base).unwrap()));
                    let el = ei.reordered_levels(&ei.paragraphs[0], 0..enc.len());
                    for (k, c) in enc.char_indices() {
                        if let Some(Some(o)) = map.get(k)
                            && !matches!(bidi_class(c), BidiClass::BN)
                            && el[k].number() != lvl(*o)
                        {
                            level_bad += 1;
                        }
                    }
                }
                let vis = visual_clusters(&mut lcx, &mut fcx, &enc, &map, rtl_base);
                // independent inventory: visible graphemes of the *source* piece
                let expected: Vec<(usize, usize)> = piece
                    .grapheme_indices(true)
                    .filter(|(_, g)| !invisible(g))
                    .map(|(i, g)| (s + i, s + i + g.len()))
                    .collect();
                expected_total += expected.len();
                let want = l2(&expected, lvl);
                // visual position of every source byte from the layout under test
                let mut pos: std::collections::HashMap<usize, usize> = Default::default();
                for (k, c) in vis.iter().enumerate() {
                    for b in c.0..c.1 {
                        pos.entry(b).or_insert(k);
                    }
                    glyphs_by_start.insert(c.0, c.2.clone());
                }
                let mut got: Vec<(usize, usize)> = Vec::new();
                for g in &expected {
                    match pos.get(&g.0) {
                        Some(_) => got.push(*g),
                        None => missing += 1,
                    }
                }
                got.sort_by_key(|g| {
                    let p = pos[&g.0];
                    let tie = if lvl(g.0) % 2 == 1 { usize::MAX - g.0 } else { g.0 };
                    (p, tie)
                });
                for (k, (g, wv)) in got.iter().zip(&want).enumerate() {
                    if g != wv && order_bad == 0 && std::env::var_os("S5_DEBUG").is_some() {
                        let lo = k.saturating_sub(4);
                        let hi = (k + 6).min(got.len());
                        println!(
                            "      {opt} first mismatch piece {s}..{e} visual #{k}/{}:\n        got  {:?}\n        want {:?}",
                            got.len(),
                            &got[lo..hi],
                            &want[lo..hi]
                        );
                        let a = want[lo..hi].iter().chain(&got[lo..hi]).map(|c| c.0).min().unwrap();
                        let b = want[lo..hi].iter().chain(&got[lo..hi]).map(|c| c.1).max().unwrap();
                        println!(
                            "        text {a}..{b} {:?} line-levels {:?} para-levels {:?}",
                            &text[a..b],
                            text[a..b].char_indices().map(|(i, _)| lvl(a + i)).collect::<Vec<_>>(),
                            text[a..b].char_indices().map(|(i, _)| level(a + i)).collect::<Vec<_>>()
                        );
                    }
                    if g != wv {
                        order_bad += 1;
                        if g.0 < s + 256 || g.0 + 256 > e {
                            order_bad_near_cut += 1;
                        }
                    }
                }
                for c in &vis {
                    let ch = text[c.0..].chars().next().unwrap();
                    if matches!(ch, '(' | ')') && c.2.len() == 1 {
                        brackets += 1;
                        if !mirror_ok(c.2[0], ch, lvl(c.0) % 2 == 1) {
                            mirror_bad += 1;
                        }
                    }
                }
            }
            println!(
                "   {opt:14}: source graphemes {expected_total}, missing from layout {missing}; visual-order mismatches vs L1+L2 reference {order_bad} ({order_bad_near_cut} within 256 B of a cut); brackets {brackets}, mirroring mismatches {mirror_bad}; {}",
                if opt == "degraded" {
                    format!("char levels (pre-L1) differing from reference {level_bad}")
                } else {
                    format!(
                        "unicode-bidi line levels (L1) of the encoded pieces differing from the reference line levels {level_bad}"
                    )
                }
            );
            option_glyphs.push(glyphs_by_start);
        }
        // glyph ids for Arabic words straddling cuts: each option's own layout vs the unchunked
        // parley layout (reference), plus harfrust shaping with/without context vs harfrust on
        // the whole word
        let mut straddle = 0;
        let mut same = [0usize; 2];
        let mut hb_deg = 0;
        let mut hb_ctx = 0;
        for &c in &cs {
            let ws = text[..c]
                .rfind(|ch: char| !is_arabic(ch))
                .map_or(0, |i| i + text[i..].chars().next().unwrap().len_utf8());
            let we = text[c..].find(|ch: char| !is_arabic(ch)).map_or(text.len(), |i| c + i);
            if ws >= c || we <= c {
                continue;
            }
            straddle += 1;
            let starts: Vec<usize> = text[ws..we].char_indices().map(|(i, _)| ws + i).collect();
            let seq = |m: &std::collections::HashMap<usize, Vec<u32>>| -> Vec<u32> {
                starts
                    .iter()
                    .flat_map(|b| m.get(b).cloned().unwrap_or_default())
                    .collect()
            };
            let reference = seq(&ref_glyphs);
            for (k, m) in option_glyphs.iter().enumerate() {
                same[k] += usize::from(seq(m) == reference);
            }
            let hb_ref = shape(&fref, &sdata, &text[ws..we], "", "");
            let mut deg = shape(&fref, &sdata, &text[ws..c], "", "");
            deg.extend(shape(&fref, &sdata, &text[c..we], "", ""));
            let mut ctx = shape(&fref, &sdata, &text[ws..c], "", &text[c..we]);
            ctx.extend(shape(&fref, &sdata, &text[c..we], &text[ws..c], ""));
            hb_deg += usize::from(deg == hb_ref);
            hb_ctx += usize::from(ctx == hb_ref);
        }
        println!(
            "   Arabic words straddling a cut: {straddle}; parley glyph ids identical to the unchunked parley layout: degraded {}/{straddle}, (b) {}/{straddle}; harfrust vs harfrust whole word: no context {hb_deg}/{straddle}, (c) pre/post context {hb_ctx}/{straddle}",
            same[0], same[1]
        );
    }
}

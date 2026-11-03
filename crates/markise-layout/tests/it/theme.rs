//! The layout honors the theme model filled by the CSS importer (ADR 0004), and a theme
//! change invalidates cached layouts by fingerprint.

use markise_layout::block::{BlockLayout, NoImages, Prim, color};
use markise_layout::testing;
use markise_theme::{Rgba, TextAlign, Theme};
use std::sync::Arc;

const DOC: &str = "# Title\n\n## Pill\n\nText with `code`, ~~gone~~ and a [link](https://x.y).\n\n> quoted\n\n- a\n- b\n\n3. three\n4. four\n";

fn layouts(theme: &Theme) -> Vec<std::sync::Arc<BlockLayout>> {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = markise_layout::testing::Shown::new(Arc::new(markise_md::parse(DOC))).view(theme.clone(), 600.0, 1.0);
    let _ = v.frame(2000.0, 0.0, &mut f, &NoImages);
    (0..v.len()).map(|i| v.layout(i, &mut f, &NoImages)).collect()
}

fn rects(l: &BlockLayout) -> Vec<epaint::Color32> {
    l.prims
        .iter()
        .filter_map(|p| match p {
            Prim::Rect { fill, .. } => Some(*fill),
            _ => None,
        })
        .collect()
}

fn texts(l: &BlockLayout) -> Vec<String> {
    l.prims
        .iter()
        .filter_map(|p| match p {
            Prim::Text { text, .. } => Some(text.text().to_string()),
            _ => None,
        })
        .collect()
}

fn themed() -> Theme {
    let mut t = Theme::light();
    t.heading_decor[1].background = Some(Rgba::hex(0x4870ac));
    t.heading_decor[1].fit = true;
    t.heading_decor[1].padding = [12.0, 2.0];
    t.heading_decor[1].radius = 4.0;
    t.heading_decor[0].prefix = Some("§ ".into());
    t.heading_decor[0].align = TextAlign::Center;
    t.quote_bg = Rgba::hex(0xfdefee);
    t.link_underline = true;
    t.list_styles.bullets = vec!["none".into()];
    t.list_styles.numbers = vec!["lower-roman".into()];
    t
}

#[test]
fn heading_quote_link_and_list_styles_reach_the_layout() {
    let t = themed();
    let ls = layouts(&t);
    // items: h1, h2, paragraph, quote, li a, li b, li 3, li 4
    assert!(texts(&ls[0])[0].starts_with("§ Title"), "{:?}", texts(&ls[0]));
    let pill = rects(&ls[1]);
    assert!(pill.contains(&color(Rgba::hex(0x4870ac))), "h2 background");
    // the pill hugs the text: narrower than the column
    let w = ls[1]
        .prims
        .iter()
        .find_map(|p| match p {
            Prim::Rect { rect, .. } => Some(rect.width()),
            _ => None,
        })
        .unwrap();
    assert!(w < 200.0, "fit-content pill width {w}");
    // inline code box + strike line + link underline
    let para = &ls[2];
    assert!(
        rects(para).contains(&color(t.inline_code.background)),
        "inline code box"
    );
    let lines = para.prims.iter().filter(|p| matches!(p, Prim::Line { .. })).count();
    assert_eq!(lines, 2, "one strike line and one link underline");
    assert!(rects(&ls[3]).contains(&color(Rgba::hex(0xfdefee))), "quote background");
    // `list-style-type: none` bullets draw nothing; numbers in lower-roman
    assert!(!ls[4].prims.iter().any(|p| matches!(p, Prim::Circle { .. })));
    assert!(texts(&ls[6]).iter().any(|t| t == "iii."), "{:?}", texts(&ls[6]));
    assert!(texts(&ls[7]).iter().any(|t| t == "iv."));
}

#[test]
fn a_theme_change_invalidates_cached_layouts_and_switching_back_reuses_them() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let a = Theme::light();
    let b = themed();
    let mut v = markise_layout::testing::Shown::new(Arc::new(markise_md::parse(DOC))).view(a.clone(), 600.0, 1.0);
    let _ = v.frame(2000.0, 0.0, &mut f, &NoImages);
    assert!(!v.laid_out.is_empty());
    let _ = v.frame(2000.0, 0.0, &mut f, &NoImages);
    assert!(v.laid_out.is_empty(), "cached");
    // only decoration fields differ: still a new fingerprint, everything is laid out again
    v.set_params(600.0, &b, 1.0);
    let _ = v.frame(2000.0, 0.0, &mut f, &NoImages);
    assert_eq!(v.laid_out.len(), v.len(), "every item laid out for the new theme");
    // back to the first theme: its layouts are still in the cache
    v.set_params(600.0, &a, 1.0);
    let _ = v.frame(2000.0, 0.0, &mut f, &NoImages);
    assert!(v.laid_out.is_empty(), "old layouts reused, laid out {:?}", v.laid_out);
    // a single color change is enough to invalidate
    let mut c = a.clone();
    c.link = Rgba::hex(0x123456);
    assert_ne!(c.fingerprint(), a.fingerprint());
    v.set_params(600.0, &c, 1.0);
    let _ = v.frame(2000.0, 0.0, &mut f, &NoImages);
    assert_eq!(v.laid_out.len(), v.len());
}

/// Positions (item-local) of the glyphs spelling `word` in the item's text primitives.
fn glyph_xs(l: &BlockLayout, word: &str) -> Option<(f32, f32, f32)> {
    for p in &l.prims {
        let Prim::Text { pos, text, .. } = p else { continue };
        let galley = text.galley().unwrap();
        for prow in &galley.rows {
            let chars: Vec<char> = prow.row.glyphs.iter().map(|g| g.chr).collect();
            let w: Vec<char> = word.chars().collect();
            if let Some(i) = chars.windows(w.len()).position(|s| s == w.as_slice()) {
                let g0 = &prow.row.glyphs[i];
                let g1 = &prow.row.glyphs[i + w.len() - 1];
                let y = pos.y + prow.pos.y + g0.pos.y;
                return Some((
                    pos.x + prow.pos.x + g0.pos.x,
                    pos.x + prow.pos.x + g1.pos.x + g1.advance_width,
                    y,
                ));
            }
        }
    }
    None
}

#[test]
fn decorations_align_with_their_glyphs_across_wrapped_rows() {
    // long runs before the decorated words force several wrapped rows; letter spacing and
    // emphasis change glyph metrics before them
    let src = "Words *with emphasis* and **strong text** that wrap across the narrow column \
               again and again, then `inline code`, ~~struck~~ words and a [link](https://x.y) \
               after many more words that keep wrapping `late code` end.\n";
    for (letter, mono_body) in [(0.0, false), (1.5, false), (0.0, true)] {
        let mut t = Theme::light();
        t.link_underline = true;
        t.inline_code.padding = 3.0;
        if mono_body {
            t.fonts.body = vec!["monospace".into()];
        }
        t.heading_decor[0].letter_spacing = letter;
        let mut fonts = testing::fonts();
        let mut f = fonts.with_pixels_per_point(1.0);
        let mut v = markise_layout::testing::Shown::new(Arc::new(markise_md::parse(src))).view(t.clone(), 260.0, 1.0);
        let _ = v.frame(4000.0, 0.0, &mut f, &NoImages);
        let l = v.layout(0, &mut f, &NoImages);
        let boxes: Vec<epaint::Rect> = l
            .prims
            .iter()
            .filter_map(|p| match p {
                Prim::Rect { rect, fill, .. } if *fill == color(t.inline_code.background) => Some(*rect),
                _ => None,
            })
            .collect();
        let lines: Vec<(f32, f32, f32)> = l
            .prims
            .iter()
            .filter_map(|p| match p {
                Prim::Line { a, b, .. } => Some((a.x, b.x, a.y)),
                _ => None,
            })
            .collect();
        for word in ["inline code", "late code"] {
            let (x0, x1, _) = glyph_xs(&l, word).unwrap_or_else(|| panic!("{word} on one row"));
            // the padding surrounds the glyphs; at the column edge the box is clamped to it
            let left_ok = |r: &epaint::Rect| (r.min.x + 3.0 - x0).abs() < 0.5 || (r.min.x.abs() < 0.5 && x0 < 3.5);
            let right_ok = |r: &epaint::Rect| (r.max.x - 3.0 - x1).abs() < 0.5 || (r.max.x - 260.0).abs() < 0.5;
            assert!(
                boxes
                    .iter()
                    .any(|r| left_ok(r) && right_ok(r) && r.min.x <= x0 && r.max.x >= x1),
                "box around {word} [{x0}, {x1}] not found in {boxes:?}"
            );
        }
        for word in ["struck", "link"] {
            let (x0, x1, y) = glyph_xs(&l, word).unwrap();
            assert!(
                lines
                    .iter()
                    .any(|&(a, b, ly)| (a - x0).abs() < 0.5 && (b - x1).abs() < 0.5 && (ly - y).abs() < 20.0),
                "line under/through {word} [{x0}, {x1}] not found in {lines:?}"
            );
        }
    }
}

/// A shaper whose fonts lack `missing`.
struct Lacking<'a, 'b> {
    inner: &'a mut epaint::text::FontsView<'b>,
    missing: char,
}

impl markise_layout::TextShaper for Lacking<'_, '_> {
    fn shape(
        &mut self,
        text: &markise_layout::StyledText,
        width: Option<f32>,
    ) -> std::sync::Arc<dyn markise_layout::TextBlock> {
        self.inner.shape(text, width)
    }
    fn has_glyphs(&mut self, style: &markise_layout::TextStyle, s: &str) -> bool {
        !s.contains(self.missing) && markise_layout::TextShaper::has_glyphs(self.inner, style, s)
    }
}

#[test]
fn generated_content_without_glyphs_or_emoji_is_skipped() {
    let mut t = Theme::light();
    t.quote_prefix = Some("💡".into());
    t.heading_decor[0].prefix = Some("¶ ".into());
    t.heading_decor[1].prefix = Some("§ ".into());
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut sh = Lacking {
        inner: &mut f,
        missing: '¶',
    };
    let mut v = markise_layout::testing::Shown::new(Arc::new(markise_md::parse(DOC))).view(t.clone(), 600.0, 1.0);
    let _ = v.frame(2000.0, 0.0, &mut sh, &NoImages);
    let h1 = v.layout(0, &mut sh, &NoImages);
    let h2 = v.layout(1, &mut sh, &NoImages);
    let quote = v.layout(3, &mut sh, &NoImages);
    assert_eq!(texts(&h1)[0], "Title", "missing glyph: prefix skipped");
    assert!(texts(&h2)[0].starts_with("§ "), "available glyph: prefix drawn");
    assert!(!texts(&quote)[0].contains('💡'), "emoji prefix skipped");
    let skipped = markise_layout::block::undrawable_generated(&t, &mut sh);
    let at: Vec<&str> = skipped.iter().map(|(a, _)| a.as_str()).collect();
    assert_eq!(at, vec!["h1::before", "blockquote::before"]);
}

#[test]
fn ordered_markers_keep_a_gap_before_the_text() {
    for (indent, mono) in [(28.0, true), (12.0, false), (0.0, false)] {
        let mut t = Theme::light();
        t.list_indent = indent;
        if mono {
            t.fonts.body = vec!["monospace".into()];
        }
        let ls = layouts(&t);
        for l in &ls[6..8] {
            let marker = l
                .prims
                .iter()
                .find_map(|p| match p {
                    Prim::Text { pos, text, .. } if text.text().ends_with('.') => Some(pos.x + text.rect().max.x),
                    _ => None,
                })
                .unwrap();
            let text_x = l
                .prims
                .iter()
                .find_map(|p| match p {
                    Prim::Text { pos, text, .. } if !text.text().ends_with('.') => Some(pos.x + text.rect().min.x),
                    _ => None,
                })
                .unwrap();
            assert!(
                text_x - marker >= 5.0,
                "indent {indent}: marker ends {marker}, text starts {text_x}"
            );
        }
    }
}

/// All glyphs of an item's text primitives in item coordinates: (char, x0, x1, row y).
fn glyphs(l: &BlockLayout) -> Vec<(char, f32, f32, f32)> {
    let mut v = Vec::new();
    for p in &l.prims {
        let Prim::Text { pos, text, .. } = p else { continue };
        let galley = text.galley().unwrap();
        for prow in &galley.rows {
            for g in &prow.row.glyphs {
                let x0 = pos.x + prow.pos.x + g.pos.x;
                v.push((g.chr, x0, x0 + g.advance_width, pos.y + prow.pos.y));
            }
        }
    }
    v
}

fn lay(t: &Theme, src: &str, width: f32) -> std::sync::Arc<BlockLayout> {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = markise_layout::testing::Shown::new(Arc::new(markise_md::parse(src))).view(t.clone(), width, 1.0);
    let _ = v.frame(4000.0, 0.0, &mut f, &NoImages);
    v.layout(0, &mut f, &NoImages)
}

const RU: &str = "**Порядок выката.** Гейт потолка схемы (`MAX_KNOWN_SCHEMA_VERSION` в \
                  `apps/next-server/lib/content-sync/worker.ts`) поднят до `20260825120000` тем же PR. \
                  На прод next-server должен уехать **раньше** Rails: иначе первый же publish уронит \
                  синк, и ученики останутся на старом контенте при зелёной публикации.\n";

#[test]
fn justify_widens_only_spaces_and_leaves_last_and_sparse_rows_ragged() {
    let mut t = Theme::light();
    t.text_align = TextAlign::Justify;
    t.inline_code.padding = 3.0;
    let mut justified = 0;
    for w in (300..700).step_by(5) {
        let width = w as f32;
        let l = lay(&t, RU, width);
        let gs = glyphs(&l);
        let mut rows: Vec<f32> = gs.iter().map(|g| g.3).collect();
        rows.dedup();
        assert!(rows.len() >= 4, "several rows: {rows:?}");
        let normal = gs.iter().find(|g| g.0 == ' ').map(|g| g.2 - g.1).unwrap();
        for (ri, y) in rows.iter().enumerate() {
            let row: Vec<_> = gs.iter().filter(|g| g.3 == *y && g.0 != '\u{200B}').collect();
            // letters are never spaced apart: within a word each glyph starts where the
            // previous one ended (small kerning tolerance)
            for w2 in row.windows(2) {
                let (a, b) = (w2[0], w2[1]);
                if a.0.is_alphabetic() && b.0.is_alphabetic() {
                    assert!(
                        (b.1 - a.2).abs() < 1.0,
                        "width {width} row {ri}: gap {} in {:?}{:?}",
                        b.1 - a.2,
                        a.0,
                        b.0
                    );
                }
            }
            let right = row.iter().rev().find(|g| !g.0.is_whitespace()).unwrap().2;
            let is_justified = (right - width).abs() < 1.0;
            if is_justified {
                justified += 1;
                // a justified space is at most 1.5x its normal width (JUSTIFY_MAX_EXTRA = 0.5);
                // spaces next to a code span also carry its padding
                for (i, g) in row.iter().enumerate() {
                    if g.0 == ' '
                        && i + 1 < row.len()
                        && row[i + 1].0.is_alphabetic()
                        && i > 0
                        && row[i - 1].0.is_alphabetic()
                    {
                        let eff = row[i + 1].1 - g.1;
                        assert!(
                            eff <= normal * 1.5 + 1.0,
                            "width {width}: space {eff} vs normal {normal}"
                        );
                    }
                }
            }
            if ri + 1 == rows.len() {
                // the last row keeps normal spaces
                for (i, g) in row.iter().enumerate() {
                    if g.0 == ' '
                        && i + 1 < row.len()
                        && row[i + 1].0.is_alphabetic()
                        && i > 0
                        && row[i - 1].0.is_alphabetic()
                    {
                        let eff = row[i + 1].1 - g.1;
                        assert!(
                            (eff - normal).abs() < 1.0,
                            "width {width}: last-row space {eff} vs {normal}"
                        );
                    }
                }
            }
        }
    }
    assert!(justified >= 1, "some rows are justified across widths ({justified})");
}

#[test]
fn inline_code_boxes_do_not_overlap_neighbouring_glyphs() {
    let mut t = Theme::light();
    t.inline_code.padding = 4.0;
    for align in [TextAlign::Start, TextAlign::Justify] {
        t.text_align = align;
        let l = lay(&t, RU, 420.0);
        let gs = glyphs(&l);
        let boxes: Vec<epaint::Rect> = l
            .prims
            .iter()
            .filter_map(|p| match p {
                Prim::Rect { rect, fill, .. } if *fill == color(t.inline_code.background) => Some(*rect),
                _ => None,
            })
            .collect();
        assert!(boxes.len() >= 3);
        for r in &boxes {
            // glyphs on the box's row: those inside belong to the code, the rest must stay
            // clear of the box
            for g in gs
                .iter()
                .filter(|g| g.3 >= r.min.y - 20.0 && g.3 <= r.max.y && !g.0.is_whitespace())
            {
                let inside = g.1 >= r.min.x - 0.5 && g.2 <= r.max.x + 0.5;
                let clear = g.2 <= r.min.x + 0.5 || g.1 >= r.max.x - 0.5;
                assert!(
                    inside || clear,
                    "{align:?}: glyph {:?} [{}, {}] overlaps box {r:?}",
                    g.0,
                    g.1,
                    g.2
                );
            }
        }
    }
}

#[test]
fn justified_rows_with_a_format_boundary_reach_the_right_edge() {
    // the bold run starts right after a stretchable space and has spaces of its own
    let src = "alpha bravo **foxtrot golfing hotels** juliett kilometre lima mike november oscar \
               papa quebec romeo sierra tango uniform victor whiskey xray yankee zulu end.\n";
    let mut t = Theme::light();
    t.text_align = TextAlign::Justify;
    let mut checked = 0;
    for w in (240..420).step_by(7) {
        let width = w as f32;
        let l = lay(&t, src, width);
        let gs = glyphs(&l);
        let ff_row = gs.iter().find(|g| g.0 == 'f').unwrap().3; // `foxtrot` (bold)
        let row: Vec<_> = gs.iter().filter(|g| g.3 == ff_row && g.0 != '\u{200B}').collect();
        let right = row
            .iter()
            .filter(|g| !g.0.is_whitespace())
            .map(|g| g.2)
            .fold(0.0f32, f32::max);
        if (right - width).abs() > 1.0 {
            continue; // ragged at this width (expansion cap)
        }
        checked += 1;
        // word gaps on the row are equal, including the one before the bold run
        // effective width of each inter-word space (space start to next word start)
        let mut gaps = Vec::new();
        for (i, g) in row.iter().enumerate() {
            if g.0 == ' ' && i > 0 && i + 1 < row.len() && !row[i + 1].0.is_whitespace() {
                gaps.push((row[i + 1].0, row[i + 1].1 - g.1));
            }
        }
        let min = gaps.iter().map(|g| g.1).fold(f32::MAX, f32::min);
        let max = gaps.iter().map(|g| g.1).fold(0.0f32, f32::max);
        assert!(max - min < 1.1, "width {width}: unequal word gaps {gaps:?}");
    }
    assert!(checked >= 3, "some widths justify the bold row");
}

fn code_boxes(l: &BlockLayout, t: &Theme) -> Vec<epaint::Rect> {
    l.prims
        .iter()
        .filter_map(|p| match p {
            Prim::Rect { rect, fill, .. } if *fill == color(t.inline_code.background) => Some(*rect),
            _ => None,
        })
        .collect()
}

#[test]
fn trailing_inline_box_padding_stays_inside_the_column() {
    let mut t = Theme::light();
    t.inline_code.padding = 5.0;
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let srcs = [
        "Ends with `code`\n",
        "# Heading `code`\n",
        "`a` ![img](none.png) after\n",
        "Narrow `multi line code span with 中文字符 inside` and `末尾中文`\n",
        "`第一` 第二 `第三第四第五第六第七第八`\n",
    ];
    for src in srcs {
        for w in (60..260).step_by(9) {
            let width = w as f32;
            let mut v =
                markise_layout::testing::Shown::new(Arc::new(markise_md::parse(src))).view(t.clone(), width, 1.0);
            let _ = v.frame(4000.0, 0.0, &mut f, &NoImages);
            let l = v.layout(0, &mut f, &NoImages);
            for r in code_boxes(&l, &t) {
                assert!(
                    r.max.x <= width + 0.5,
                    "{src:?} at {width}: box {r:?} overflows the column"
                );
                assert!(
                    r.min.x >= -0.5,
                    "{src:?} at {width}: box {r:?} starts left of the column"
                );
            }
            // the padding is not a glyph of its own, so it can never wrap to a row by itself
            for p in &l.prims {
                if let Prim::Text { text, .. } = p {
                    let galley = text.galley().unwrap();
                    assert!(!galley.job.text.contains('\u{200B}'), "{src:?}: padding carrier glyph");
                }
            }
        }
        if src.contains("after") {
            let mut v =
                markise_layout::testing::Shown::new(Arc::new(markise_md::parse(src))).view(t.clone(), 500.0, 1.0);
            let _ = v.frame(2000.0, 0.0, &mut f, &NoImages);
            let l = v.layout(0, &mut f, &NoImages);
            // owed padding does not leak past the image into the next run
            let lead = l
                .prims
                .iter()
                .find_map(|p| match p {
                    Prim::Text { text, .. } if text.text().trim_start().starts_with("after") => {
                        text.galley().unwrap().job.sections.first().map(|s| s.leading_space)
                    }
                    _ => None,
                })
                .expect("text after the image");
            assert_eq!(lead, 0.0, "no leading padding after the image");
        }
    }
}

#[test]
fn the_justification_cap_holds_for_every_space_with_mixed_fonts_and_separators() {
    // ASCII spaces, ideographic spaces (U+3000) and spaces inside bold runs on the same rows
    let src = "alpha **bravo charlie** delta\u{3000}echo foxtrot\u{3000}golf **hotel india** juliett \
               kilo\u{3000}lima mike **november oscar** papa\u{3000}quebec romeo sierra tango \
               uniform\u{3000}victor whiskey xray yankee zulu end.\n";
    let mut t = Theme::light();
    t.text_align = TextAlign::Justify;
    let mut justified = 0;
    for w in (200..700).step_by(3) {
        let width = w as f32;
        let l = lay(&t, src, width);
        let mut gs: Vec<(char, f32, f32, f32, f32)> = Vec::new(); // chr, x0, x1, row, advance
        for p in &l.prims {
            let Prim::Text { pos, text, .. } = p else { continue };
            let galley = text.galley().unwrap();
            for prow in &galley.rows {
                for g in &prow.row.glyphs {
                    let x0 = pos.x + prow.pos.x + g.pos.x;
                    gs.push((g.chr, x0, x0 + g.advance_width, pos.y + prow.pos.y, g.advance_width));
                }
            }
        }
        let mut rows: Vec<f32> = gs.iter().map(|g| g.3).collect();
        rows.dedup();
        for y in rows {
            let row: Vec<_> = gs.iter().filter(|g| g.3 == y).collect();
            let right = row.iter().rev().find(|g| !g.0.is_whitespace()).map_or(0.0, |g| g.2);
            if (right - width).abs() > 1.0 {
                continue;
            }
            justified += 1;
            for (i, g) in row.iter().enumerate() {
                if (g.0 == ' ' || g.0 == '\u{3000}') && i > 0 && i + 1 < row.len() && !row[i + 1].0.is_whitespace() {
                    let eff = row[i + 1].1 - g.1;
                    assert!(
                        eff <= g.4 * 1.5 + 1.0,
                        "width {width}: {:?} widened to {eff}, normal {}",
                        g.0,
                        g.4
                    );
                }
            }
        }
    }
    assert!(justified >= 1, "some rows justified ({justified})");
}

#[test]
fn missing_glyphs_are_detected_for_lazy_fallback_fonts() {
    let mut fonts = testing::fonts(); // egui's bundled fonts: no CJK
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut latin = markise_layout::testing::Shown::new(Arc::new(markise_md::parse("Plain text, ünïcödé — fine.\n")))
        .view(Theme::light(), 500.0, 1.0);
    let _ = latin.frame(800.0, 0.0, &mut f, &NoImages);
    assert!(!latin.missing_glyphs(), "Latin text is covered");
    let mut cjk = markise_layout::testing::Shown::new(Arc::new(markise_md::parse("Text with 中文 inside.\n"))).view(
        Theme::light(),
        500.0,
        1.0,
    );
    let _ = cjk.frame(800.0, 0.0, &mut f, &NoImages);
    assert!(cjk.missing_glyphs(), "CJK needs a fallback font");
    // new fonts: checked again
    cjk.set_font_generation(1);
    assert!(!cjk.missing_glyphs());
}

#[test]
fn missing_glyphs_are_checked_against_each_runs_own_font_chain() {
    use epaint::text::{FontDefinitions, FontFamily, Fonts};
    use markise_layout::{
        FAMILY_BODY, FAMILY_BOLD, FAMILY_HEADING, FAMILY_HEADING_BOLD, FAMILY_MONO, FAMILY_MONO_BOLD,
    };
    // body covers more than the heading / code chains (Hack vs Ubuntu-Light only)
    let mut defs = FontDefinitions::default();
    let wide = vec!["Hack".to_string()];
    let narrow = vec!["Ubuntu-Light".to_string()];
    for (fam, list) in [
        (FAMILY_BODY, &wide),
        (FAMILY_BOLD, &wide),
        (FAMILY_HEADING, &narrow),
        (FAMILY_HEADING_BOLD, &narrow),
        (FAMILY_MONO, &narrow),
        (FAMILY_MONO_BOLD, &narrow),
    ] {
        defs.families.insert(FontFamily::Name(fam.into()), list.clone());
    }
    let mut fonts = Fonts::new(
        epaint::text::TextOptions {
            max_texture_side: 8192,
            ..Default::default()
        },
        defs,
    );
    let mut f = fonts.with_pixels_per_point(1.0);
    let body = epaint::FontId::new(14.0, FontFamily::Name(FAMILY_BODY.into()));
    let head = epaint::FontId::new(14.0, FontFamily::Name(FAMILY_HEADING.into()));
    let probe = ['─', '│', '┌', '█', '░', '▲', '►', '◆', '≈', '∑', '⌘', '⟶']
        .into_iter()
        .find(|c| {
            let s = c.to_string();
            f.has_glyphs(&body, &s) && !f.has_glyphs(&head, &s)
        })
        .expect("a character the body chain has and the heading chain lacks");
    let mut paragraph = markise_layout::testing::Shown::new(Arc::new(markise_md::parse(&format!("Text {probe}\n"))))
        .view(Theme::light(), 500.0, 1.0);
    let _ = paragraph.frame(800.0, 0.0, &mut f, &NoImages);
    assert!(!paragraph.missing_glyphs(), "covered by the body chain");
    for src in [format!("# Heading {probe}\n"), format!("Code `{probe}`\n")] {
        let mut v =
            markise_layout::testing::Shown::new(Arc::new(markise_md::parse(&src))).view(Theme::light(), 500.0, 1.0);
        let _ = v.frame(800.0, 0.0, &mut f, &NoImages);
        assert!(
            v.missing_glyphs(),
            "{src:?}: the run's own (heading / mono) chain lacks {probe:?}"
        );
    }
}

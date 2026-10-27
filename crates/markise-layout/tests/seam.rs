//! ADR 0005 T2: the `TextShaper` / `TextBlock` seam. The same geometric contract is checked on
//! the epaint backend (egui's `FontsView`) and on the parley backend (`markise-text`), both over
//! egui's bundled fonts.

use epaint::emath::Align;
use epaint::text::{FontFamily, FontId, LayoutJob, TextFormat};
use epaint::{Color32, Stroke, pos2};
use markise_layout::block::{NoImages, Prim};
use markise_layout::text::epaint_backend::to_layout_job;
use markise_layout::text::{Affinity, LocalPos, ParleyBlock};
use markise_layout::{FontRole, StyledText, TextShaper, TextStyle, testing};
use markise_theme::Theme;
use std::sync::Arc;

fn style() -> TextStyle {
    TextStyle::new(16.0, 24.0, FontRole::Body, Color32::BLACK)
}

/// Caret, hit testing, clusters, selection and wrapping on simple text.
fn simple_text_contract(sh: &mut dyn TextShaper, what: &str) {
    let text = "hello world";
    let b = sh.shape(&StyledText::single(text, style()), None);
    assert_eq!(b.text(), text, "{what}");
    assert_eq!(b.line_count(), 1, "{what}");
    let lines = b.lines();
    assert_eq!(lines[0].range, 0..text.len(), "{what}");
    assert!(!lines[0].ends_with_newline, "{what}");
    let h = b.first_line_height().unwrap();
    assert!((h - 24.0).abs() < 1.0, "{what}: line height {h}");
    assert!(
        b.size().x > 50.0 && (b.size().y - h).abs() < 1.0,
        "{what}: size {:?}",
        b.size()
    );

    // one cluster per character, increasing x, contiguous advances
    let cs = b.clusters();
    assert_eq!(cs.len(), text.len(), "{what}");
    for w in cs.windows(2) {
        assert!(w[1].byte > w[0].byte, "{what}");
        // epaint snaps glyph positions to pixels
        assert!((w[0].x + w[0].advance - w[1].x).abs() <= 1.0, "{what}: {w:?}");
    }
    assert_eq!(cs[5].ch, ' ', "{what}");

    // caret rects: zero width, row height, monotonic; hit inverts them
    let mut last = f32::NEG_INFINITY;
    for i in 0..=text.len() {
        let r = b.caret_rect(LocalPos::down(i));
        assert_eq!(r.width(), 0.0, "{what}");
        assert!(r.height() > 10.0, "{what}");
        assert!(r.min.x > last, "{what}: caret {i} at {} after {last}", r.min.x);
        last = r.min.x;
        let back = b.hit(pos2(r.min.x + 0.1, r.center().y));
        assert_eq!(back.byte, i, "{what}: hit at caret {i}");
    }
    // the character under the pointer (link hit testing)
    let w = &cs[6];
    assert_eq!(b.char_at(pos2(w.x + w.advance * 0.5, h * 0.5)), Some(6), "{what}");
    assert_eq!(b.char_at(pos2(-50.0, h * 0.5)), None, "{what}: outside");

    // selection of "hello": one rect from caret 0 to caret 5, either order
    let (a, e) = (LocalPos::down(0), LocalPos::new(5, Affinity::Upstream));
    for sel in [b.selection_rects(a, e), b.selection_rects(e, a)] {
        assert_eq!(sel.len(), 1, "{what}: {sel:?}");
        let c0 = b.caret_rect(a).min.x;
        let c5 = b.caret_rect(LocalPos::down(5)).min.x;
        assert!(
            (sel[0].min.x - c0).abs() <= 1.0 && (sel[0].max.x - c5).abs() <= 1.0,
            "{what}: {sel:?}"
        );
    }

    // ink spans of "world" for decorations
    let sp = b.ink_spans(&(6..11));
    assert_eq!(sp.len(), 1, "{what}");
    assert!((sp[0].x0 - cs[6].x).abs() <= 1.0 && (sp[0].x1 - (cs[10].x + cs[10].advance)).abs() <= 1.0);
    assert!(
        sp[0].ascent > 5.0 && sp[0].baseline > sp[0].ascent * 0.5,
        "{what}: {sp:?}"
    );

    // wrapping: rows partition the text, stacked downwards
    let long = "the quick brown fox jumps over the lazy dog ".repeat(4);
    let b = sh.shape(&StyledText::single(long.as_str(), style()), Some(150.0));
    let lines = b.lines();
    assert!(lines.len() > 3, "{what}: {} rows", lines.len());
    let mut end = 0;
    for (i, l) in lines.iter().enumerate() {
        assert!(l.range.start >= end, "{what}");
        end = l.range.end;
        // parley counts a trailing space in the row advance
        assert!(l.size.x <= 160.0, "{what}: row {i} {:?}", l.size);
        if i > 0 {
            assert!(l.origin.y > lines[i - 1].origin.y, "{what}");
        }
    }
    assert!(end >= long.trim_end().len(), "{what}");
    // a caret on the second row hits back on the second row
    let r = b.caret_rect(LocalPos::down(lines[1].range.start + 2));
    assert!(r.min.y >= lines[1].origin.y - 0.5, "{what}");
    assert_eq!(
        b.hit(pos2(r.min.x + 0.1, r.center().y)).byte,
        lines[1].range.start + 2,
        "{what}"
    );
}

#[test]
fn epaint_block_carets_hits_and_selection_t2() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    simple_text_contract(&mut f, "epaint");
}

#[test]
fn parley_block_carets_hits_and_selection_t2() {
    let mut p = testing::parley();
    simple_text_contract(&mut p, "parley");
    // the scale changes pixels, not points
    p.set_scale(2.0);
    let b = p.shape(&StyledText::single("hello world", style()), None);
    let pb = b.as_any().downcast_ref::<ParleyBlock>().unwrap();
    assert_eq!(pb.scale(), 2.0);
    assert!((b.first_line_height().unwrap() - 24.0).abs() < 1.0);
    assert!(!pb.glyph_runs().is_empty() && !pb.has_notdef());
}

/// Alignment follows epaint's anchor convention on both backends.
#[test]
fn alignment_anchor_convention_on_both_backends_t2() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut p = testing::parley();
    let shapers: [(&mut dyn TextShaper, &str); 2] = [(&mut f, "epaint"), (&mut p, "parley")];
    for (sh, what) in shapers {
        for (a, check) in [(Align::Min, 0.0f32), (Align::Center, 0.5), (Align::Max, 1.0)] {
            let mut t = StyledText::single("centred line", style());
            t.halign = a;
            let b = sh.shape(&t, Some(400.0));
            let r = b.rect();
            assert!((r.min.x + check * r.width()).abs() < 1.0, "{what} {a:?}: rect {r:?}");
        }
    }
}

/// Leading space (inline-box padding) moves the run on both backends.
#[test]
fn run_lead_adds_space_on_both_backends_t2() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut p = testing::parley();
    let shapers: [(&mut dyn TextShaper, &str); 2] = [(&mut f, "epaint"), (&mut p, "parley")];
    for (sh, what) in shapers {
        let x_of_b = |sh: &mut dyn TextShaper, lead: f32| {
            let mut t = StyledText::default();
            t.append("a", 0.0, style());
            t.append("b", lead, style());
            sh.shape(&t, None).clusters()[1].x
        };
        let d = x_of_b(sh, 10.0) - x_of_b(sh, 0.0);
        assert!((d - 10.0).abs() <= 1.0, "{what}: lead moved by {d}");
    }
}

/// The epaint conversion is exact: the job equals the one M1 built by hand (pixel identity).
#[test]
fn epaint_job_conversion_is_exact_t2() {
    let mut st = style();
    st.italic = true;
    st.letter_spacing = 0.5;
    let mut bold = style();
    bold.role = FontRole::Bold;
    bold.strike = true;
    let mut t = StyledText::default();
    t.append("one ", 0.0, st.clone());
    t.append("two", 0.0, st.clone()); // merged
    t.append(" three", 2.0, bold.clone());
    t.halign = Align::Center;
    let job = to_layout_job(&t, Some(120.0));

    let fmt = |s: &TextStyle, fam: &str| TextFormat {
        font_id: FontId::new(s.size, FontFamily::Name(fam.into())),
        extra_letter_spacing: s.letter_spacing,
        line_height: Some(s.line_height),
        color: s.color,
        background: Color32::TRANSPARENT,
        expand_bg: 0.0,
        italics: s.italic,
        underline: Stroke::NONE,
        strikethrough: if s.strike {
            Stroke::new(1.0, s.color)
        } else {
            Stroke::NONE
        },
        valign: Align::BOTTOM,
        coords: Default::default(),
    };
    let mut want = LayoutJob::default();
    want.append("one ", 0.0, fmt(&st, markise_layout::FAMILY_BODY));
    want.append("two", 0.0, fmt(&st, markise_layout::FAMILY_BODY));
    want.append(" three", 2.0, fmt(&bold, markise_layout::FAMILY_BOLD));
    want.wrap.max_width = 120.0;
    want.halign = Align::Center;
    assert_eq!(job, want);
    assert_eq!(
        to_layout_job(&StyledText::single("x", style()), None).wrap.max_width,
        f32::INFINITY
    );
}

const DOC: &str = "# Title\n\nSee [the docs](https://example.com) and **bold** `code`.\n\n\
| a | b |\n|---|---|\n| 1 | 2 |\n\n```\nfn main() {}\n```\n\n- item\n";

/// A whole document laid out through the parley backend: every text primitive is a parley
/// block, heights are sane and link hits go through `TextBlock`.
#[test]
fn document_layout_on_the_parley_backend_t2() {
    let mut p = testing::parley();
    let mut v = markise_layout::testing::Shown::new(Arc::new(markise_md::parse(DOC))).view(Theme::light(), 600.0, 1.0);
    let vis = v.frame(2000.0, 0.0, &mut p, &NoImages);
    assert!(vis.len() >= 5);
    let mut texts = 0;
    for vi in &vis {
        assert!(
            vi.layout.height > 10.0 && vi.layout.height < 300.0,
            "{}",
            vi.layout.height
        );
        for prim in &vi.layout.prims {
            if let Prim::Text { text, .. } = prim {
                assert!(text.as_any().is::<ParleyBlock>());
                texts += 1;
            }
        }
    }
    assert!(texts >= 7, "{texts}");
    let para = &vis[1].layout;
    assert_eq!(para.links.len(), 1);
    let Prim::Text { pos, text, .. } = &para.prims[para.links[0].prim] else {
        panic!()
    };
    let c = text.caret_rect(LocalPos::down(6));
    assert_eq!(
        para.link_at(*pos + epaint::vec2(c.min.x + 2.0, c.center().y)),
        Some("https://example.com")
    );
    assert_eq!(para.link_at(*pos + epaint::vec2(1.0, c.center().y)), None);
    // no cache sharing between backends: the generation salts the keys
    assert_ne!(TextShaper::generation(&p), 0);
}

/// Fragment ids are item-relative and say which piece of the item a text block shows.
#[test]
fn text_prims_carry_fragment_ids_t2() {
    use markise_layout::FragmentKind;
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = markise_layout::testing::Shown::new(Arc::new(markise_md::parse(DOC))).view(Theme::light(), 600.0, 1.0);
    let vis = v.frame(2000.0, 0.0, &mut f, &NoImages);
    let kinds: Vec<FragmentKind> = vis
        .iter()
        .flat_map(|vi| vi.layout.prims.iter())
        .filter_map(|p| match p {
            Prim::Text { frag, .. } => Some(frag.kind),
            _ => None,
        })
        .collect();
    assert!(kinds.contains(&FragmentKind::Para));
    assert!(kinds.contains(&FragmentKind::Cell { row: 2, col: 1 }));
    assert!(kinds.contains(&FragmentKind::CodeChunk(0)));
    assert!(kinds.iter().all(|k| !matches!(k, FragmentKind::Run(_))));
}

// ---- review round 1: soft-wrap affinity, bidi, ligatures, control characters, inline boxes,
// ---- multiline alignment, collection faces

use markise_layout::text::RoleFamilies;
use markise_text::testing::TestFont;
use markise_text::{Family, FontStore, TextEngine};

/// A parley shaper whose every role uses one synthetic face.
fn synthetic(font: &TestFont) -> markise_layout::ParleyShaper {
    let mut store = FontStore::new();
    store.register_system_family(&font.family, font.build(), 100.0, 400.0);
    let fam = vec![Family::named(&font.family)];
    let roles = RoleFamilies {
        roles: std::array::from_fn(|_| fam.clone()),
    };
    markise_layout::ParleyShaper::new(TextEngine::new(store), roles)
}

/// Latin, Hebrew and space glyphs (Hebrew letters wide, so rows break predictably).
fn bidi_font() -> TestFont {
    TestFont::new("Seam Bidi")
        .with_range('a', 'z', 500)
        .with_range('\u{5d0}', '\u{5ea}', 600)
        .with_chars(" .", 600)
}

/// The same byte offset at a soft wrap has two carets: the end of the upper row (upstream) and
/// the start of the lower row (downstream); hit testing returns the one that was clicked.
fn soft_wrap_affinity(sh: &mut dyn TextShaper, what: &str) {
    let long = "the quick brown fox jumps over the lazy dog ".repeat(3);
    let b = sh.shape(&StyledText::single(long.as_str(), style()), Some(150.0));
    let lines = b.lines();
    assert!(lines.len() > 2, "{what}");
    let at = lines[1].range.start;
    let up = b.caret_rect(LocalPos::up(at));
    let down = b.caret_rect(LocalPos::down(at));
    let row0 = lines[0].origin.y..lines[0].origin.y + lines[0].size.y;
    let row1 = lines[1].origin.y..lines[1].origin.y + lines[1].size.y;
    assert!(
        row0.contains(&up.center().y),
        "{what}: upstream caret {up:?} not on row 0"
    );
    assert!(
        row1.contains(&down.center().y),
        "{what}: downstream caret {down:?} not on row 1"
    );
    assert!((down.min.x - lines[1].origin.x).abs() <= 1.0, "{what}: {down:?}");
    assert!(up.min.x > 100.0, "{what}: upstream caret at the row end, {up:?}");
    // clicks: start of row 1 → downstream; beyond the end of row 0 → upstream
    assert_eq!(
        b.hit(pos2(down.min.x + 0.1, down.center().y)),
        LocalPos::down(at),
        "{what}"
    );
    assert_eq!(b.hit(pos2(149.0, up.center().y)), LocalPos::up(at), "{what}");
    // a selection ending upstream at the wrap stays on row 0, one starting downstream on row 1
    let sel = b.selection_rects(LocalPos::down(0), LocalPos::up(at));
    assert!(sel.iter().all(|r| r.max.y <= row0.end + 0.5), "{what}: {sel:?}");
    let sel = b.selection_rects(LocalPos::down(at), LocalPos::up(lines[1].range.end));
    assert!(sel.iter().all(|r| r.min.y >= row1.start - 0.5), "{what}: {sel:?}");
}

#[test]
fn soft_wrap_affinity_epaint_t2() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    soft_wrap_affinity(&mut f, "epaint");
}

#[test]
fn soft_wrap_affinity_parley_t2() {
    let mut p = testing::parley();
    soft_wrap_affinity(&mut p, "parley");
}

/// Justified RTL and mixed-direction paragraphs (clusters arrive in visual order) lay out
/// without panicking and do get justified rows.
#[test]
fn justified_rtl_and_mixed_bidi_paragraphs_t2() {
    let mut p = synthetic(&bidi_font());
    let rtl = "אב גד הו זח טי כל מנ סע פצ קר שת ".repeat(6);
    let mixed = "אב gd הו zh טי kl מנ so פצ qr שת ab ".repeat(6);
    for (src, what) in [(rtl, "rtl"), (mixed, "mixed")] {
        let mut t = Theme::light();
        t.text_align = markise_theme::TextAlign::Justify;
        let mut justified = false;
        for width in [180.0, 210.0, 240.0, 270.0, 300.0, 330.0] {
            let mut v =
                markise_layout::testing::Shown::new(Arc::new(markise_md::parse(&src))).view(t.clone(), width, 1.0);
            let l = v.layout(0, &mut p, &NoImages);
            let texts: Vec<_> = l
                .prims
                .iter()
                .filter_map(|p| match p {
                    Prim::Text { text, frag, .. } => Some((text.clone(), *frag)),
                    _ => None,
                })
                .collect();
            // the rows (or the unjustified block) cover the paragraph's words in order
            let words: usize = texts.iter().map(|(t, _)| t.text().split_whitespace().count()).sum();
            assert_eq!(words, src.split_whitespace().count(), "{what} at {width}");
            if texts.len() > 1 {
                justified = true;
                let mut offsets: Vec<u32> = texts.iter().map(|(_, f)| f.offset).collect();
                let sorted = {
                    let mut s = offsets.clone();
                    s.sort_unstable();
                    s
                };
                assert_eq!(offsets, sorted, "{what}: row fragments in order");
                offsets.dedup();
                assert_eq!(offsets.len(), texts.len(), "{what}");
            }
        }
        assert!(justified, "{what}: some width justifies");
    }
}

/// Bidi line on parley: carets of every grapheme boundary round-trip through hit testing with
/// their affinity, and selections over the direction change cover the selected clusters.
#[test]
fn bidi_carets_and_selection_parley_t2() {
    let mut p = synthetic(&bidi_font());
    let text = "abc אבג def";
    let b = p.shape(&StyledText::single(text, style()), None);
    let cs = b.clusters();
    assert_eq!(cs.len(), text.chars().count());
    // visual order: the Hebrew run is reversed
    let heb: Vec<usize> = cs.iter().filter(|c| !c.ch.is_ascii()).map(|c| c.byte).collect();
    assert!(heb.windows(2).all(|w| w[0] > w[1]), "{heb:?}");
    for (i, _) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        for aff in [Affinity::Upstream, Affinity::Downstream] {
            let r = b.caret_rect(LocalPos::new(i, aff));
            let back = b.hit(pos2(r.min.x, r.center().y));
            // one x can host two offsets at a direction change: the hit must be a position
            // whose caret is drawn at that x
            let rb = b.caret_rect(back);
            assert!(
                (rb.min.x - r.min.x).abs() <= 1.0,
                "caret {i} {aff:?} {r:?} → {back:?} {rb:?}"
            );
        }
    }
    // selecting "c אבג d" covers exactly those clusters
    let (s, e) = (2, text.find("def").unwrap() + 1);
    let rects = b.selection_rects(LocalPos::down(s), LocalPos::down(e));
    let covered: f32 = rects.iter().map(|r| r.width()).sum();
    let expect: f32 = cs.iter().filter(|c| c.byte >= s && c.byte < e).map(|c| c.advance).sum();
    assert!((covered - expect).abs() <= 1.0, "{rects:?} vs {expect}");
}

/// Ligatures: one glyph, but a caret stop per character; the components share the advance.
#[test]
fn ligature_clusters_and_carets_parley_t2() {
    let font = TestFont::new("Seam Liga")
        .with_chars("fix ", 500)
        .with_ligature("fi", 700);
    let mut p = synthetic(&font);
    let b = p.shape(&StyledText::single("fix", style()), None);
    let pb = b.as_any().downcast_ref::<ParleyBlock>().unwrap();
    let glyphs: Vec<u32> = pb
        .glyph_runs()
        .iter()
        .flat_map(|r| r.glyphs.iter().map(|g| g.id))
        .collect();
    assert_eq!(glyphs, vec![font.glyph_of_ligature("fi"), font.glyph_of('x')]);
    let cs = b.clusters();
    assert_eq!(cs.len(), 3);
    assert!((cs[0].advance + cs[1].advance - 11.2).abs() < 0.01, "{cs:?}");
    let mut last = f32::NEG_INFINITY;
    for i in 0..=3 {
        let r = b.caret_rect(LocalPos::down(i));
        assert!(r.min.x > last, "caret {i}");
        last = r.min.x;
        assert_eq!(b.hit(pos2(r.min.x + 0.05, r.center().y)).byte, i);
    }
    let sp = b.ink_spans(&(0..2));
    assert_eq!(sp.len(), 1);
    assert!((sp[0].x1 - sp[0].x0 - 11.2).abs() < 0.01, "{sp:?}");
}

/// Control characters get no decoration ink on either backend.
#[test]
fn control_characters_have_no_ink_t2() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut p = testing::parley();
    let shapers: [(&mut dyn TextShaper, &str); 2] = [(&mut f, "epaint"), (&mut p, "parley")];
    for (sh, what) in shapers {
        let b = sh.shape(&StyledText::single("ab\tcd", style()), None);
        let sp = b.ink_spans(&(0..5));
        assert_eq!(sp.len(), 2, "{what}: {sp:?}");
        assert!(sp[1].x0 > sp[0].x1, "{what}: {sp:?}");
        let tab = b.caret_rect(LocalPos::down(3)).min.x - b.caret_rect(LocalPos::down(2)).min.x;
        assert!(tab > 0.0, "{what}: the tab advances");
    }
}

/// An inline box (run lead) sits between the runs: the caret after it includes its width and
/// hit testing on either side of the box stays on the right characters.
#[test]
fn inline_box_lead_carets_t2() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut p = testing::parley();
    let shapers: [(&mut dyn TextShaper, &str); 2] = [(&mut f, "epaint"), (&mut p, "parley")];
    for (sh, what) in shapers {
        let mut t = StyledText::default();
        t.append("ab", 0.0, style());
        t.append("cd", 12.0, style());
        let b = sh.shape(&t, None);
        let cs = b.clusters();
        let gap = cs[2].x - (cs[1].x + cs[1].advance);
        assert!((gap - 12.0).abs() <= 1.0, "{what}: box width {gap}");
        assert!(b.size().x >= cs[3].x + cs[3].advance - 0.5, "{what}");
        let y = b.first_line_height().unwrap() * 0.5;
        assert_eq!(b.char_at(pos2(cs[2].x + 1.0, y)), Some(2), "{what}");
        assert_eq!(b.char_at(pos2(cs[1].x + 1.0, y)), Some(1), "{what}");
        assert_eq!(b.hit(pos2(cs[3].x + cs[3].advance + 0.5, y)).byte, 4, "{what}");
    }
}

/// Every row of a wrapped, centred / right-aligned block follows the anchor convention.
#[test]
fn multiline_alignment_on_both_backends_t2() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut p = testing::parley();
    let shapers: [(&mut dyn TextShaper, &str); 2] = [(&mut f, "epaint"), (&mut p, "parley")];
    for (sh, what) in shapers {
        for (a, k) in [(Align::Center, 0.5f32), (Align::Max, 1.0)] {
            let mut t = StyledText::single("a short line and then a much longer second line of text", style());
            t.halign = a;
            let b = sh.shape(&t, Some(160.0));
            let lines = b.lines();
            assert!(lines.len() >= 2, "{what}");
            for l in &lines {
                assert!((l.origin.x + k * l.size.x).abs() <= 1.0, "{what} {a:?}: {l:?}");
            }
            // hit testing inside a centred row lands in that row
            let l = &lines[1];
            let hit = b.hit(pos2(l.origin.x + l.size.x * 0.5, l.origin.y + l.size.y * 0.5));
            assert!(
                l.range.contains(&hit.byte),
                "{what} {a:?}: {hit:?} not in {:?}",
                l.range
            );
        }
    }
}

/// A TrueType collection from standalone faces (table offsets rebased).
fn ttc(faces: &[Vec<u8>]) -> Vec<u8> {
    let header = 12 + 4 * faces.len();
    let mut out = Vec::new();
    out.extend_from_slice(b"ttcf");
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&(faces.len() as u32).to_be_bytes());
    let mut bases = Vec::new();
    let mut at = header;
    for f in faces {
        at = (at + 3) & !3;
        bases.push(at);
        at += f.len();
    }
    for b in &bases {
        out.extend_from_slice(&(*b as u32).to_be_bytes());
    }
    for (f, &base) in faces.iter().zip(&bases) {
        out.resize(base, 0);
        let mut f = f.clone();
        let n = u16::from_be_bytes([f[4], f[5]]) as usize;
        for i in 0..n {
            let o = 12 + 16 * i + 8;
            let off = u32::from_be_bytes([f[o], f[o + 1], f[o + 2], f[o + 3]]) + base as u32;
            f[o..o + 4].copy_from_slice(&off.to_be_bytes());
        }
        out.extend_from_slice(&f);
    }
    out
}

/// Registering face 1 of a collection uses that face, not another member of the file.
#[test]
fn collection_face_index_is_kept_parley_t2() {
    use epaint::text::{FontData, FontDefinitions};
    let a = TestFont::new("Seam TTC A").with_chars("ab", 500).build();
    let b = TestFont::new("Seam TTC B").with_chars("ab", 900).build();
    let file: std::sync::Arc<dyn AsRef<[u8]> + Send + Sync> = std::sync::Arc::new(ttc(&[a, b]));
    for (index, advance) in [(0u32, 5.0f32), (1, 9.0)] {
        let mut defs = FontDefinitions::empty();
        defs.font_data.insert(
            "ttc".into(),
            std::sync::Arc::new(FontData::from_blob(file.clone(), index)),
        );
        defs.families
            .insert(FontFamily::Name(markise_layout::FAMILY_BODY.into()), vec!["ttc".into()]);
        let mut p = markise_layout::ParleyShaper::from_font_definitions(&defs);
        let s = TextStyle::new(10.0, 12.0, FontRole::Body, Color32::BLACK);
        let blk = p.shape(&StyledText::single("ab", s), None);
        let pb = blk.as_any().downcast_ref::<ParleyBlock>().unwrap();
        let runs = pb.glyph_runs();
        assert_eq!(runs.len(), 1);
        // the selected face, isolated: a standalone font (index 0), not the collection
        assert_eq!(runs[0].font.index(), 0, "face {index}");
        assert_ne!(runs[0].font.data().len(), (*file).as_ref().len(), "face {index}");
        assert!(
            runs[0].glyphs.iter().all(|g| (g.advance - advance).abs() < 0.01),
            "face {index}: {:?}",
            runs[0].glyphs
        );
    }
}

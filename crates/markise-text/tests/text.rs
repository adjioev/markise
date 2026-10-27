//! markise-text T1 tests (ADR 0005 §5 T1 and the §4.1/§4.3 tests that apply to T1).
//! Synthetic fonts (`markise_text::testing`) make every test deterministic and independent of
//! installed fonts; tests that need real scripts use system fonts and are skipped (printed)
//! when the font is missing.

// `unicode_range([a..=b])` is a list of CSS ranges, not a range to expand.
#![allow(clippy::single_range_in_vec_init, clippy::reversed_empty_ranges)]

use std::io::Write;
use std::sync::Arc;

use markise_text::testing::{TestFont, woff1, woff2};
use markise_text::*;

fn bytes(f: &TestFont) -> FaceSource {
    FaceSource::Bytes(Arc::new(f.build()))
}

fn engine_with(rules: Vec<FaceRule>) -> (TextEngine, Vec<FaceId>) {
    let mut store = FontStore::new();
    let ids = rules
        .into_iter()
        .map(|r| store.add_face(r).expect("face loads"))
        .collect();
    (TextEngine::new(store), ids)
}

fn style(fam: &str) -> TextStyle {
    TextStyle::new(Family::parse_list(fam), 10.0)
}

/// (blob id, glyph ids) per glyph run.
fn runs(l: &TextLayout) -> Vec<(u64, Vec<u32>)> {
    l.glyph_runs()
        .iter()
        .map(|r| (r.font.id(), r.glyphs.iter().map(|g| g.id).collect()))
        .collect()
}

fn latin() -> TestFont {
    TestFont::new("Latin")
        .with_range(' ', '~', 500)
        .with_ligature("fi", 700)
}

// ---------------------------------------------------------------- loading (R8)

#[test]
fn woff1_and_woff2_sources_decode() {
    let sfnt = latin().build();
    let w1 = woff1(&sfnt, |d| {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        e.write_all(d).unwrap();
        e.finish().unwrap()
    });
    let w2 = woff2(&sfnt, |d| {
        let mut out = Vec::new();
        let params = brotli::enc::BrotliEncoderParams::default();
        brotli::BrotliCompress(&mut &d[..], &mut out, &params).unwrap();
        out
    });
    for (name, data) in [("woff", w1), ("woff2", w2)] {
        let mut store = FontStore::new();
        let id = store
            .add_face(FaceRule::new("W", vec![FaceSource::Bytes(Arc::new(data))]))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut eng = TextEngine::new(store);
        let l = eng.layout(&StyledText::plain("ab", style("W")), None, 1.0);
        let r = runs(&l);
        assert_eq!(r[0].0, eng.fonts.face_blob_id(id), "{name}");
        assert_eq!(r[0].1, vec![latin().glyph_of('a'), latin().glyph_of('b')], "{name}");
    }
}

#[test]
fn src_list_falls_through_and_reports_failures() {
    // `src: local(missing), <bytes>` → second source wins (ADR 0005 §4.1 T4 test, T1 part)
    let mut store = FontStore::new();
    store.set_name_index(FaceNameIndex::default());
    let id = store
        .add_face(FaceRule::new(
            "X",
            vec![FaceSource::Local("No Such Face".into()), bytes(&latin())],
        ))
        .unwrap();
    assert_eq!(id, 0);
    let err = store
        .add_face(FaceRule::new(
            "Y",
            vec![
                FaceSource::Local("No Such Face".into()),
                FaceSource::Bytes(Arc::new(b"not a font".to_vec())),
            ],
        ))
        .unwrap_err();
    assert_eq!(err.failures.len(), 2);
    assert_eq!(err.failures[0].1, SourceError::LocalNotFound);
    assert!(matches!(err.failures[1].1, SourceError::Invalid(_)));
}

#[test]
fn file_sources_are_confined_to_the_root() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("ok.ttf"), latin().build()).unwrap();
    std::fs::write(outside.path().join("evil.ttf"), latin().build()).unwrap();
    let mut store = FontStore::new();
    store.set_policy(LoadPolicy {
        root: Some(root.path().to_path_buf()),
        max_bytes: 1 << 20,
    });
    assert!(
        store
            .add_face(FaceRule::new("A", vec![FaceSource::File("ok.ttf".into())]))
            .is_ok()
    );
    let e = store
        .add_face(FaceRule::new(
            "B",
            vec![FaceSource::File(outside.path().join("evil.ttf"))],
        ))
        .unwrap_err();
    assert_eq!(e.failures[0].1, SourceError::OutsideRoot);
    let e = store
        .add_face(FaceRule::new("C", vec![FaceSource::File("../x/evil.ttf".into())]))
        .unwrap_err();
    assert!(matches!(e.failures[0].1, SourceError::OutsideRoot | SourceError::Io(_)));
    store.set_policy(LoadPolicy {
        root: Some(root.path().to_path_buf()),
        max_bytes: 100,
    });
    let e = store
        .add_face(FaceRule::new("D", vec![FaceSource::File("ok.ttf".into())]))
        .unwrap_err();
    assert_eq!(e.failures[0].1, SourceError::TooLarge);
}

#[test]
fn local_by_full_and_postscript_name_including_non_regular_faces() {
    // ADR 0005 §4.1: local() success by full name and by PostScript name, incl. a bold face
    let dir = tempfile::tempdir().unwrap();
    let regular = TestFont::new("Loco Sans").with_range('a', 'z', 500);
    let bold = TestFont::new("Loco Sans").style(700, false).with_range('a', 'z', 650);
    std::fs::write(dir.path().join("r.ttf"), regular.build()).unwrap();
    std::fs::write(dir.path().join("b.otf"), bold.build()).unwrap();
    let mut store = FontStore::new();
    store.start_discovery_in(vec![dir.path().to_path_buf()], false);
    assert!(store.wait_for_system(std::time::Duration::from_secs(20)));
    let idx = store.name_index().unwrap();
    assert_eq!(idx.files(), 2);
    assert!(idx.find("Loco Sans Bold").is_some());
    assert!(idx.find("LocoSans-Bold").is_some());
    assert!(idx.find("locosans-regular").is_some());
    for name in ["Loco Sans Bold", "LocoSans-Bold"] {
        let id = store
            .add_face(FaceRule::new("T", vec![FaceSource::Local(name.into())]))
            .unwrap();
        let mut eng = TextEngine::new(std::mem::take(&mut store));
        let l = eng.layout(&StyledText::plain("a", style("T")), None, 1.0);
        let g = &l.glyph_runs()[0];
        assert_eq!(g.font.id(), eng.fonts.face_blob_id(id));
        // the bold face's advance (650 units at 10 px = 6.5)
        assert!((g.glyphs[0].advance - 6.5).abs() < 0.01, "{name}");
        store = std::mem::take(&mut eng.fonts);
        store.clear_faces();
    }
}

// ---------------------------------------------------------------- matching (§4.1)

#[test]
fn regular_and_bold_faces_bold_uses_bold_face_without_synthesis() {
    let r = TestFont::new("Fam").with_range('a', 'z', 500);
    let b = TestFont::new("Fam").style(700, false).with_range('a', 'z', 600);
    let (mut eng, ids) = engine_with(vec![
        FaceRule::new("Fam", vec![bytes(&r)]),
        FaceRule::new("Fam", vec![bytes(&b)]).weight(700.0, 700.0),
    ]);
    let mut t = StyledText::plain("ab", style("Fam"));
    t.push_run(1..2, style("Fam").weight(700.0));
    let l = eng.layout(&t, None, 1.0);
    let gr = l.glyph_runs();
    assert_eq!(gr.len(), 2);
    assert_eq!(gr[0].font.id(), eng.fonts.face_blob_id(ids[0]));
    assert_eq!(gr[1].font.id(), eng.fonts.face_blob_id(ids[1]));
    assert!(!gr[1].embolden, "real bold face: no synthesis");
    // bold requested from a family with only a regular face → synthetic bold
    let (mut eng, _) = engine_with(vec![FaceRule::new("Fam", vec![bytes(&r)])]);
    let l = eng.layout(&StyledText::plain("a", style("Fam").weight(700.0)), None, 1.0);
    assert!(l.glyph_runs()[0].embolden);
}

#[test]
fn overlapping_unicode_ranges_last_declared_wins() {
    let a = TestFont::new("A").with_range('a', 'z', 500);
    let b = TestFont::new("B").with_range('a', 'z', 600);
    let (mut eng, ids) = engine_with(vec![
        FaceRule::new("U", vec![bytes(&a)]).unicode_range([0x00..=0x7F]),
        FaceRule::new("U", vec![bytes(&b)]).unicode_range([0x61..=0x7A]),
    ]);
    let l = eng.layout(&StyledText::plain("ab", style("U")), None, 1.0);
    assert_eq!(runs(&l)[0].0, eng.fonts.face_blob_id(ids[1]));
}

#[test]
fn disjoint_unicode_ranges_base_and_mark_from_different_faces_one_caret_stop() {
    // ADR 0005 §4.1 rule 4b test: face A covers only the base range, face B only the mark range
    let a = TestFont::new("A").with_range(' ', '~', 500);
    let b = TestFont::new("B").with_chars("\u{301}", 0);
    let (mut eng, ids) = engine_with(vec![
        FaceRule::new("T", vec![bytes(&a)]).unicode_range([0x0000..=0x007F]),
        FaceRule::new("T", vec![bytes(&b)]).unicode_range([0x0300..=0x036F]),
    ]);
    let text = "xe\u{301}y";
    let l = eng.layout(&StyledText::plain(text, style("T")), None, 1.0);
    let r = runs(&l);
    let a_id = eng.fonts.face_blob_id(ids[0]);
    let b_id = eng.fonts.face_blob_id(ids[1]);
    assert!(
        r.iter().any(|(f, g)| *f == a_id && g.contains(&a.glyph_of('e'))),
        "{r:?}"
    );
    assert!(
        r.iter().any(|(f, g)| *f == b_id && g == &vec![b.glyph_of('\u{301}')]),
        "{r:?}"
    );
    assert!(r.iter().all(|(_, g)| !g.contains(&0)), "no .notdef: {r:?}");
    // caret stops: 0 x 1 é 4 y 5 — never inside the cluster
    assert_eq!(l.grapheme_boundaries(), &[0, 1, 4, 5]);
    let p = l.move_right(LocalPos::down(1));
    assert_eq!(p.byte, 4);
    assert_eq!(l.move_left(LocalPos::down(4)).byte, 1);
    // hit testing anywhere over the cluster lands on 1 or 4
    for i in 0..40 {
        let x = i as f32 * 0.5;
        let p = l.hit(x, 5.0);
        assert!(l.is_caret_stop(p.byte), "x={x} → {p:?}");
    }
}

#[test]
fn decomposed_sequence_supported_via_nfc() {
    // `e` + U+0301 in a face that only has `é` → the face supports the cluster (rule 4a NFC)
    let comp = TestFont::new("C").with_chars("é", 520);
    let other = TestFont::new("O").with_range(' ', '~', 500).with_chars("\u{301}", 0);
    let (mut eng, ids) = engine_with(vec![
        FaceRule::new("Composed", vec![bytes(&comp)]),
        FaceRule::new("Other", vec![bytes(&other)]),
    ]);
    let l = eng.layout(&StyledText::plain("e\u{301}", style("Composed, Other")), None, 1.0);
    let r = runs(&l);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0].0, eng.fonts.face_blob_id(ids[0]));
    assert_eq!(r[0].1, vec![comp.glyph_of('é')], "shaper composes to the é glyph");
}

#[test]
fn vs16_prefers_colour_face_vs15_text_face() {
    let text_face = TestFont::new("Txt").with_chars("\u{2764}a", 500);
    let color_face = TestFont::new("Clr").with_chars("\u{2764}", 900).colored();
    let (mut eng, ids) = engine_with(vec![
        FaceRule::new("Txt", vec![bytes(&text_face)]),
        FaceRule::new("Clr", vec![bytes(&color_face)]),
    ]);
    let st = style("Txt, Clr");
    let l = eng.layout(&StyledText::plain("\u{2764}\u{FE0F}", st.clone()), None, 1.0);
    assert_eq!(runs(&l)[0].0, eng.fonts.face_blob_id(ids[1]), "VS16 → colour");
    let l = eng.layout(&StyledText::plain("\u{2764}\u{FE0E}", st.clone()), None, 1.0);
    assert_eq!(runs(&l)[0].0, eng.fonts.face_blob_id(ids[0]), "VS15 → text");
    let st2 = style("Clr, Txt");
    let l = eng.layout(&StyledText::plain("\u{2764}\u{FE0E}", st2), None, 1.0);
    assert_eq!(
        runs(&l)[0].0,
        eng.fonts.face_blob_id(ids[0]),
        "VS15 → text, order ignored"
    );
    // one caret stop for the sequence
    assert_eq!(l.grapheme_boundaries(), &[0, 6]);
}

#[test]
fn cmap14_variation_sequence_preferred_then_base_alone() {
    let plain = TestFont::new("P").with_chars("\u{8FBB}", 1000);
    let ivs = TestFont::new("I")
        .with_chars("\u{8FBB}", 1000)
        .with_variation_sequence('\u{8FBB}', '\u{E0100}', 1000);
    let (mut eng, ids) = engine_with(vec![
        FaceRule::new("P", vec![bytes(&plain)]),
        FaceRule::new("I", vec![bytes(&ivs)]),
    ]);
    let l = eng.layout(&StyledText::plain("\u{8FBB}\u{E0100}", style("P, I")), None, 1.0);
    let r = runs(&l);
    assert_eq!(r[0].0, eng.fonts.face_blob_id(ids[1]), "face mapping the sequence wins");
    assert_eq!(r[0].1[0], ivs.glyph_of_sequence('\u{8FBB}', '\u{E0100}'));
    // no face maps the sequence → the base alone matches in stack order
    let (mut eng, ids) = engine_with(vec![FaceRule::new("P", vec![bytes(&plain)])]);
    let l = eng.layout(&StyledText::plain("\u{8FBB}\u{E0100}", style("P")), None, 1.0);
    assert_eq!(runs(&l)[0].0, eng.fonts.face_blob_id(ids[0]));
}

// ---------------------------------------------------------------- carets (R1, R2, §4.3)

fn emoji_font() -> TestFont {
    TestFont::new("E")
        .with_range(' ', '~', 500)
        .with_chars("\u{1F469}\u{1F467}\u{200D}\u{1F1E9}\u{1F1EA}", 1000)
}

#[test]
fn grapheme_stops_on_zwj_emoji_and_flags() {
    let (mut eng, _) = engine_with(vec![FaceRule::new("E", vec![bytes(&emoji_font())])]);
    let fam = "\u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    let flag = "\u{1F1E9}\u{1F1EA}";
    let text = format!("a{fam}b{flag}c");
    let l = eng.layout(&StyledText::plain(&text, style("E")), None, 1.0);
    let b1 = 1 + fam.len();
    let b2 = b1 + 1 + flag.len();
    assert_eq!(l.grapheme_boundaries(), &[0, 1, b1, b1 + 1, b2, b2 + 1]);
    // walking right visits exactly the grapheme boundaries
    let mut p = LocalPos::down(0);
    let mut seen = vec![0];
    for _ in 0..10 {
        let q = l.move_right(p);
        if q.byte == p.byte {
            break;
        }
        seen.push(q.byte);
        p = q;
    }
    assert_eq!(seen, l.grapheme_boundaries());
    // logical moves too; Backspace extent over the family is the whole cluster
    assert_eq!(l.next_logical(LocalPos::down(1)).byte, b1);
    assert_eq!(l.prev_logical(LocalPos::down(b1)).byte, 1);
}

fn bidi_font() -> TestFont {
    TestFont::new("Bidi")
        .with_range(' ', '~', 500)
        .with_range('\u{5D0}', '\u{5EA}', 600)
}

#[test]
fn bidi_boundary_has_two_carets_disambiguated_by_affinity() {
    let (mut eng, _) = engine_with(vec![FaceRule::new("Bidi", vec![bytes(&bidi_font())])]);
    // "abc אבג def": the boundary after the Hebrew run (byte 10) is ambiguous
    let text = "abc \u{5D0}\u{5D1}\u{5D2} def";
    let l = eng.layout(&StyledText::plain(text, style("Bidi")), None, 1.0);
    assert!(!l.is_rtl());
    let heb_end = 4 + 6;
    let up = l.caret_rect(LocalPos::up(heb_end)).x0;
    let down = l.caret_rect(LocalPos::down(heb_end)).x0;
    assert!((up - down).abs() > 1.0, "two visual positions: {up} vs {down}");
    // round trip: hit at each caret x returns a position drawn at the same x (pair assertion)
    for &b in l.grapheme_boundaries() {
        for p in [LocalPos::up(b), LocalPos::down(b)] {
            let r = l.caret_rect(p);
            let q = l.hit(r.x0, (r.y0 + r.y1) / 2.0);
            let rq = l.caret_rect(q);
            assert!((rq.x0 - r.x0).abs() < 0.01, "{p:?} at {} → {q:?} at {}", r.x0, rq.x0);
        }
    }
}

#[test]
fn visual_arrows_follow_uax9_reordering() {
    // logical: a b c ␠ א ב ג ␠ d e f (LTR paragraph). Visual: abc␠גבא␠def.
    let (mut eng, _) = engine_with(vec![FaceRule::new("Bidi", vec![bytes(&bidi_font())])]);
    let text = "abc \u{5D0}\u{5D1}\u{5D2} def";
    let l = eng.layout(&StyledText::plain(text, style("Bidi")), None, 1.0);
    let mut p = LocalPos::down(0);
    let mut xs = vec![l.x_on_line(p)];
    let mut bytes = vec![0];
    for _ in 0..20 {
        let q = l.move_right(p);
        if q == p {
            break;
        }
        bytes.push(q.byte);
        xs.push(l.x_on_line(q));
        p = q;
    }
    // x strictly increases with every Right press except at the run boundaries, where the
    // caret may stay put once while switching affinity (two positions at one x)
    for w in xs.windows(2) {
        assert!(w[1] >= w[0] - 0.01, "Right never moves left: {xs:?} {bytes:?}");
    }
    assert_eq!(*bytes.last().unwrap(), text.len());
    // inside the RTL run the logical offsets decrease while moving right
    let heb: Vec<usize> = bytes.iter().copied().filter(|b| (5..10).contains(b)).collect();
    assert!(heb.windows(2).all(|w| w[1] < w[0]), "{bytes:?}");
}

#[test]
fn selection_rects_split_at_direction_change_without_overlap_or_gap() {
    let (mut eng, _) = engine_with(vec![FaceRule::new("Bidi", vec![bytes(&bidi_font())])]);
    let text = "abc \u{5D0}\u{5D1}\u{5D2} def";
    let l = eng.layout(&StyledText::plain(text, style("Bidi")), None, 1.0);
    // select "c אב" (bytes 2..8): visually two pieces
    let mut rects = l.selection_rects(LocalPos::down(2), LocalPos::down(8));
    rects.sort_by(|a, b| a.x0.total_cmp(&b.x0));
    assert!(rects.len() >= 2, "{rects:?}");
    for w in rects.windows(2) {
        assert!(w[0].x1 <= w[1].x0 + 0.01, "overlap {rects:?}");
    }
    let total: f32 = rects.iter().map(Rect::width).sum();
    // c (5) + space (5) + two Hebrew letters (6 + 6)
    assert!((total - 22.0).abs() < 0.05, "{total} {rects:?}");
    // whole-line selection is one gap-free strip
    let mut all = l.selection_rects(LocalPos::down(0), LocalPos::down(text.len()));
    all.sort_by(|a, b| a.x0.total_cmp(&b.x0));
    for w in all.windows(2) {
        assert!((w[0].x1 - w[1].x0).abs() < 0.01, "gap/overlap {all:?}");
    }
}

#[test]
fn rtl_paragraph_resolves_rtl_and_starts_at_the_right() {
    let (mut eng, _) = engine_with(vec![FaceRule::new("Bidi", vec![bytes(&bidi_font())])]);
    let text = "\u{5D0}\u{5D1} ab";
    let l = eng.layout(&StyledText::plain(text, style("Bidi")), Some(200.0), 1.0);
    assert!(l.is_rtl());
    let start = l.caret_rect(LocalPos::down(0)).x0;
    let end = l.caret_rect(LocalPos::up(text.len())).x0;
    assert!(start > end, "RTL: logical start is on the right ({start} vs {end})");
    let l = eng.layout(
        &StyledText::plain(text, style("Bidi")).with_base_dir(BaseDir::Ltr),
        Some(200.0),
        1.0,
    );
    assert!(!l.is_rtl());
}

// ---------------------------------------------------------------- lines (R3, R7)

#[test]
fn breaks_at_fixed_widths_and_vertical_moves_keep_goal_x() {
    let (mut eng, _) = engine_with(vec![FaceRule::new("Latin", vec![bytes(&latin())])]);
    // every char 5 px at size 10; "aaaa " = 25 px
    let text = "aaaa bbbb cccc dddd";
    let l = eng.layout(&StyledText::plain(text, style("Latin")), Some(50.0), 1.0);
    let lines = l.lines();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0].text_range, (0, 10));
    assert_eq!(lines[1].text_range, (10, 19));
    assert!(lines[1].top >= lines[0].bottom - 0.01);
    // Down from byte 2 (x = 10) lands on byte 12
    let (p, gx) = l.move_lines(LocalPos::down(2), 1, None).unwrap();
    assert_eq!(p.byte, 12);
    assert!((gx - 10.0).abs() < 0.01);
    assert!(
        l.move_lines(p, 1, Some(gx)).is_none(),
        "past the last line → caller's job"
    );
    let (q, _) = l.move_lines(p, -1, Some(gx)).unwrap();
    assert_eq!(q.byte, 2);
    // soft-wrap end vs next line start: same offset, two carets
    let up = l.caret_rect(LocalPos::up(10));
    let down = l.caret_rect(LocalPos::down(10));
    assert!(down.y0 > up.y0, "wrap end (upstream) on line 0, downstream on line 1");
    assert_eq!(l.line_at(LocalPos::down(10)), 1);
    assert_eq!(l.line_start(LocalPos::down(13)).byte, 10);
    assert_eq!(l.line_end(LocalPos::down(2)).byte, 10);
    let at = |x: f32| l.hit_on_line(1, x).unwrap().byte;
    assert_eq!(at(-5.0), 10);
    assert_eq!(at(500.0), 19);
}

#[test]
fn ligatures_on_by_default_off_by_feature_caret_inside_ligature() {
    let f = latin();
    let (mut eng, _) = engine_with(vec![FaceRule::new("Latin", vec![bytes(&f)])]);
    let l = eng.layout(&StyledText::plain("fix", style("Latin")), None, 1.0);
    let g = &runs(&l)[0].1;
    assert_eq!(g, &vec![f.glyph_of_ligature("fi"), f.glyph_of('x')]);
    // two grapheme stops inside the ligature glyph: the caret at 1 sits within it
    let x1 = l.caret_rect(LocalPos::down(1)).x0;
    assert!(x1 > 0.5 && x1 < 6.5, "caret inside the 7 px ligature: {x1}");
    let l = eng.layout(&StyledText::plain("fix", style("Latin").no_ligatures()), None, 1.0);
    assert_eq!(runs(&l)[0].1, vec![f.glyph_of('f'), f.glyph_of('i'), f.glyph_of('x')]);
}

#[test]
fn inline_object_reserves_space() {
    let (mut eng, _) = engine_with(vec![FaceRule::new("Latin", vec![bytes(&latin())])]);
    let mut t = StyledText::plain("ab", style("Latin"));
    t.inline_objects.push(InlineObject {
        id: 7,
        index: 1,
        width: 30.0,
        height: 8.0,
    });
    let l = eng.layout(&t, None, 1.0);
    assert!((l.size().0 - 40.0).abs() < 0.01, "{:?}", l.size());
}

#[test]
fn scale_multiplies_geometry() {
    let (mut eng, _) = engine_with(vec![FaceRule::new("Latin", vec![bytes(&latin())])]);
    let t = StyledText::plain("abcd", style("Latin"));
    let a = eng.layout(&t, None, 1.0).size();
    let b = eng.layout(&t, None, 2.0).size();
    assert!((b.0 - 2.0 * a.0).abs() < 0.01);
    assert!((eng.layout(&t, None, 2.0).glyph_runs()[0].size - 20.0).abs() < 0.01);
}

// ---------------------------------------------------------------- property test (§4.3)

mod prop {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        /// hit↔caret round trip with the affinity rule of §4.3: for every caret stop and both
        /// affinities, hitting the caret's own x/y returns a position drawn at the same place.
        #[test]
        fn hit_caret_round_trip(words in proptest::collection::vec(
            prop_oneof!["[a-z]{1,6}", "[\u{5D0}-\u{5EA}]{1,5}", "[0-9]{1,3}"], 1..12),
            width in 20.0f32..120.0)
        {
            let text = words.join(" ");
            let (mut eng, _) = engine_with(vec![FaceRule::new("Bidi", vec![bytes(&bidi_font())])]);
            let l = eng.layout(&StyledText::plain(&text, style("Bidi")), Some(width), 1.0);
            for &b in l.grapheme_boundaries() {
                for p in [LocalPos::up(b), LocalPos::down(b)] {
                    let r = l.caret_rect(p);
                    let q = l.hit(r.x0, (r.y0 + r.y1) / 2.0);
                    prop_assert!(l.is_caret_stop(q.byte));
                    let rq = l.caret_rect(q);
                    prop_assert!((rq.x0 - r.x0).abs() < 0.01 && (rq.y0 - r.y0).abs() < 0.01,
                        "{text:?} w={width} {p:?}@{r:?} -> {q:?}@{rq:?}");
                }
            }
        }
    }
}

// ---------------------------------------------------------------- system fonts

fn system_engine() -> TextEngine {
    let mut store = FontStore::new();
    store.start_system_discovery();
    assert!(store.wait_for_system(std::time::Duration::from_secs(60)));
    TextEngine::new(store)
}

#[test]
fn system_discovery_runs_off_thread_and_shapes_scripts() {
    let mut eng = system_engine();
    if eng.fonts.system_family_names().is_empty() {
        println!("SKIP: no system fonts on this machine");
        return;
    }
    eprintln!(
        "system discovery {:?}, {} names indexed",
        eng.fonts.discovery_time(),
        eng.fonts.name_index().map_or(0, FaceNameIndex::len)
    );
    let text = "Hello مرحبا 日本語 \u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    let l = eng.layout(&StyledText::plain(text, style("sans-serif")), None, 1.0);
    let notdef: usize = l
        .glyph_runs()
        .iter()
        .map(|r| r.glyphs.iter().filter(|g| g.id == 0).count())
        .sum();
    eprintln!("system mixed-script notdef glyphs: {notdef}");
    #[cfg(target_os = "macos")]
    assert_eq!(notdef, 0);
}

#[cfg(feature = "complex-scripts")]
#[test]
fn thai_breaks_inside_a_spaceless_run_with_complex_scripts() {
    let mut eng = system_engine();
    let text = "ภาษาไทยเป็นภาษาที่ไม่มีการเว้นวรรคระหว่างคำ";
    let l = eng.layout(&StyledText::plain(text, style("sans-serif")), Some(60.0), 1.0);
    let notdef = l.glyph_runs().iter().any(|r| r.glyphs.iter().any(|g| g.id == 0));
    if notdef {
        println!("SKIP: no Thai font");
        return;
    }
    let lines = l.lines();
    assert!(lines.len() > 2, "dictionary breaks expected: {lines:?}");
    for li in &lines {
        assert!(l.is_caret_stop(li.text_range.0));
    }
}

/// TrueType collection from standalone sfnts (table offsets rebased).
fn ttc(fonts: &[Vec<u8>]) -> Vec<u8> {
    let header = 12 + 4 * fonts.len();
    let mut out = Vec::new();
    out.extend(b"ttcf");
    out.extend(0x0001_0000u32.to_be_bytes());
    out.extend((fonts.len() as u32).to_be_bytes());
    let mut base = header;
    let mut bases = Vec::new();
    for f in fonts {
        out.extend((base as u32).to_be_bytes());
        bases.push(base);
        base += f.len();
        base += (4 - base % 4) % 4;
    }
    for (f, b) in fonts.iter().zip(bases) {
        while out.len() < b {
            out.push(0);
        }
        let mut f = f.clone();
        let n = u16::from_be_bytes([f[4], f[5]]) as usize;
        for i in 0..n {
            let o = 12 + 16 * i + 8;
            let off = u32::from_be_bytes([f[o], f[o + 1], f[o + 2], f[o + 3]]) + b as u32;
            f[o..o + 4].copy_from_slice(&off.to_be_bytes());
        }
        out.extend(f);
    }
    out
}

#[test]
fn local_face_inside_a_collection_is_isolated() {
    let dir = tempfile::tempdir().unwrap();
    let r = TestFont::new("Coll Sans").with_range('a', 'z', 500);
    let b = TestFont::new("Coll Sans").style(700, false).with_range('a', 'z', 700);
    std::fs::write(dir.path().join("c.ttc"), ttc(&[r.build(), b.build()])).unwrap();
    let mut idx = FaceNameIndex::default();
    idx.add_file(&dir.path().join("c.ttc"));
    assert_eq!(idx.find("CollSans-Bold").unwrap().index, 1);
    let mut store = FontStore::new();
    store.set_name_index(idx);
    let id = store
        .add_face(FaceRule::new("T", vec![FaceSource::Local("Coll Sans Bold".into())]))
        .unwrap();
    let mut eng = TextEngine::new(store);
    let l = eng.layout(&StyledText::plain("a", style("T")), None, 1.0);
    let g = &l.glyph_runs()[0];
    assert_eq!(g.font.id(), eng.fonts.face_blob_id(id));
    assert_eq!(g.font.index(), 0, "extracted into a standalone face");
    assert!((g.glyphs[0].advance - 7.0).abs() < 0.01, "the bold face (700 units)");
}

// ---------------------------------------------------------------- decode limits (review P1 #1)

fn zlib(d: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(d).unwrap();
    e.finish().unwrap()
}

fn brotli_c(d: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let params = brotli::enc::BrotliEncoderParams {
        quality: 2,
        ..Default::default()
    };
    brotli::BrotliCompress(&mut &d[..], &mut out, &params).unwrap();
    out
}

/// A font whose decoded size is ~4.6 MiB but whose WOFF files are ~0.6 MiB (a table of zeros
/// plus pseudo-random bytes, so the compression ratio stays plausible for WOFF2 decoders).
fn bomb_font() -> Vec<u8> {
    let mut t = vec![0u8; 4 << 20];
    let mut x: u32 = 0x1234_5678;
    for b in t.iter_mut().take(600 << 10) {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        *b = x as u8;
    }
    latin().with_table(b"zzzz", t).build()
}

fn load_bytes(data: Vec<u8>, max: usize) -> Result<FaceId, FaceError> {
    let mut store = FontStore::new();
    store.set_policy(LoadPolicy {
        root: None,
        max_bytes: max,
    });
    store.add_face(FaceRule::new("B", vec![FaceSource::Bytes(Arc::new(data))]))
}

fn set_be32(v: &mut [u8], o: usize, x: u32) {
    v[o..o + 4].copy_from_slice(&x.to_be_bytes());
}

#[test]
fn woff_decoding_enforces_max_bytes_even_with_lying_headers() {
    let w1 = woff1(&bomb_font(), zlib);
    let w2 = woff2(&bomb_font(), brotli_c);
    assert!(w1.len() < 1 << 20 && w2.len() < 1 << 20, "{} {}", w1.len(), w2.len());
    // honest headers: decoded size above the cap
    for (name, w) in [("woff1", &w1), ("woff2", &w2)] {
        let e = load_bytes(w.clone(), 2 << 20).unwrap_err();
        assert_eq!(e.failures[0].1, SourceError::TooLarge, "{name}");
        // the same file decodes with a sufficient cap
        let r = load_bytes(w.clone(), 8 << 20);
        assert!(r.is_ok(), "{name}: {r:?}");
    }
    // understated totalSfntSize: the directory-derived size still trips the cap
    for (name, w) in [("woff1", &w1), ("woff2", &w2)] {
        let mut w = w.clone();
        set_be32(&mut w, 16, 1000);
        let e = load_bytes(w, 2 << 20).unwrap_err();
        assert_eq!(e.failures[0].1, SourceError::TooLarge, "{name} lying header");
    }
    // WOFF1 table claiming a huge origLength (compressed data unchanged): rejected up front
    let mut w = woff1(&latin().build(), zlib);
    set_be32(&mut w, 44 + 12, 0x4000_0000);
    let e = load_bytes(w, 1 << 20).unwrap_err();
    assert_eq!(e.failures[0].1, SourceError::TooLarge);
    // WOFF1 table understating origLength: the bounded decompressor refuses the extra output
    let mut w = woff1(&bomb_font(), zlib);
    let n = u16::from_be_bytes([w[12], w[13]]) as usize;
    for i in 0..n {
        let e = 44 + 20 * i;
        if &w[e..e + 4] == b"zzzz" {
            set_be32(&mut w, e + 12, 70_000); // compLength stays below; real output is 4 MiB
        }
    }
    let e = load_bytes(w, 2 << 20).unwrap_err();
    assert!(
        matches!(e.failures[0].1, SourceError::Decode(_) | SourceError::TooLarge),
        "{e}"
    );
    // truncated / garbage containers never panic
    for cut in [10usize, 44, 60, 200] {
        let w = woff1(&latin().build(), zlib);
        assert!(load_bytes(w[..cut.min(w.len())].to_vec(), 1 << 20).is_err());
        let w = woff2(&latin().build(), brotli_c);
        assert!(load_bytes(w[..cut.min(w.len())].to_vec(), 1 << 20).is_err());
    }
}

// ---------------------------------------------------------------- snapshots (review P1 #2)

#[test]
fn theme_font_files_are_snapshotted_not_mapped() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("t.ttf");
    std::fs::write(&path, latin().build()).unwrap();
    let mut store = FontStore::new();
    store.set_policy(LoadPolicy {
        root: Some(root.path().to_path_buf()),
        max_bytes: 1 << 20,
    });
    store
        .add_face(FaceRule::new("T", vec![FaceSource::File("t.ttf".into())]))
        .unwrap();
    // truncate and rewrite the file after loading: an mmap would now fault or change
    std::fs::write(&path, b"").unwrap();
    std::fs::write(&path, b"garbage").unwrap();
    let mut eng = TextEngine::new(store);
    let l = eng.layout(&StyledText::plain("abc", style("T")), None, 1.0);
    assert_eq!(
        runs(&l)[0].1,
        vec![latin().glyph_of('a'), latin().glyph_of('b'), latin().glyph_of('c')]
    );
}

// ---------------------------------------------------------------- trailing families (review P1 #3)

#[test]
fn trailing_generic_or_unavailable_family_keeps_rule_4b_and_vs_fallback() {
    let a = TestFont::new("A").with_range(' ', '~', 500);
    let b = TestFont::new("B").with_chars("\u{301}", 0);
    for stack in ["T, sans-serif", "T, No Such Family", "T, No Such Family, serif"] {
        let (mut eng, ids) = engine_with(vec![
            FaceRule::new("T", vec![bytes(&a)]).unicode_range([0x0000..=0x007F]),
            FaceRule::new("T", vec![bytes(&b)]).unicode_range([0x0300..=0x036F]),
        ]);
        let l = eng.layout(&StyledText::plain("xe\u{301}y", style(stack)), None, 1.0);
        let r = runs(&l);
        let b_id = eng.fonts.face_blob_id(ids[1]);
        assert!(
            r.iter().any(|(f, g)| *f == b_id && g == &vec![b.glyph_of('\u{301}')]),
            "{stack}: mark from face B via rule 4b: {r:?}"
        );
        assert!(r.iter().all(|(_, g)| !g.contains(&0)), "{stack}: {r:?}");
        assert_eq!(l.grapheme_boundaries(), &[0, 1, 4, 5], "{stack}");
    }
    // selector fallback pass is still reached with a trailing family
    let plain = TestFont::new("P").with_chars("\u{8FBB}", 1000);
    let (mut eng, ids) = engine_with(vec![FaceRule::new("P", vec![bytes(&plain)])]);
    let l = eng.layout(
        &StyledText::plain("\u{8FBB}\u{E0100}", style("P, No Such Family, sans-serif")),
        None,
        1.0,
    );
    assert_eq!(runs(&l)[0].0, eng.fonts.face_blob_id(ids[0]));
}

// ---------------------------------------------------------------- long clusters (review P2 #5)

#[test]
fn arrows_cross_a_grapheme_of_100_marks_split_across_fonts() {
    let a = TestFont::new("A").with_range(' ', '~', 500);
    let b = TestFont::new("B").with_chars("\u{301}", 0);
    let (mut eng, _) = engine_with(vec![
        FaceRule::new("T", vec![bytes(&a)]).unicode_range([0x0000..=0x007F]),
        FaceRule::new("T", vec![bytes(&b)]).unicode_range([0x0300..=0x036F]),
    ]);
    let text = format!("xe{}y", "\u{301}".repeat(100));
    let end = text.len() - 1;
    let l = eng.layout(&StyledText::plain(&text, style("T")), None, 1.0);
    assert_eq!(l.grapheme_boundaries(), &[0, 1, end, text.len()]);
    assert!(l.glyph_runs().len() > 64, "rule 4b split the marks into many items");
    assert_eq!(l.move_right(LocalPos::down(1)).byte, end);
    assert_eq!(l.move_left(LocalPos::down(end)).byte, 1);
    assert_eq!(l.move_right(LocalPos::down(end)).byte, text.len());
    assert_eq!(l.move_left(LocalPos::down(1)).byte, 0);
}

// ---------------------------------------------------------------- variable instances (review P2 #7)

/// fvar with wght 100–900 (default 400), wdth 50–200 (100), slnt −20–0 (0).
fn fvar() -> Vec<u8> {
    let mut t = Vec::new();
    let axes: [(&[u8; 4], f32, f32, f32); 3] = [
        (b"wght", 100.0, 400.0, 900.0),
        (b"wdth", 50.0, 100.0, 200.0),
        (b"slnt", -20.0, 0.0, 0.0),
    ];
    for v in [1u16, 0, 16, 2, axes.len() as u16, 20, 0, 4 + 4 * axes.len() as u16] {
        t.extend(v.to_be_bytes());
    }
    for (i, (tag, lo, def, hi)) in axes.iter().enumerate() {
        t.extend(*tag);
        for v in [lo, def, hi] {
            t.extend(((v * 65536.0) as i32).to_be_bytes());
        }
        t.extend(0u16.to_be_bytes());
        t.extend((256 + i as u16).to_be_bytes());
    }
    t
}

#[test]
fn resolved_weight_stretch_and_oblique_reach_the_variation_axes() {
    let f = TestFont::new("Var")
        .with_range('a', 'z', 500)
        .with_table(b"fvar", fvar());
    let rule = FaceRule::new("V", vec![bytes(&f)])
        .weight(100.0, 900.0)
        .stretch(50.0, 200.0)
        .slope(FaceSlope::Oblique(0.0, 20.0));
    let (mut eng, _) = engine_with(vec![rule]);
    let mut st = style("V").weight(650.0).slope(Slope::Oblique(15.0));
    st.stretch = 87.5;
    let l = eng.layout(&StyledText::plain("ab", st), None, 1.0);
    let r = &l.glyph_runs()[0];
    // normalised F2Dot14: wght (650−400)/500 = 0.5; wdth (87.5−100)/50 = −0.25; slnt −15/20 = −0.75
    assert_eq!(r.coords, vec![8192, -4096, -12288], "{:?}", r.coords);
    assert!(!r.embolden && r.skew.is_none(), "instance, not synthesis");
    // a plain request lands on the defaults
    let l = eng.layout(&StyledText::plain("ab", style("V")), None, 1.0);
    assert!(
        l.glyph_runs()[0].coords.iter().all(|c| *c == 0),
        "{:?}",
        l.glyph_runs()[0].coords
    );
}

#[test]
fn cmap14_default_uvs_counts_as_support_and_null_mappings_do_not() {
    // review P2 #8: a default-UVS entry supports the sequence (base glyph used as is)
    let plain = TestFont::new("P").with_chars("\u{8FBB}", 1000);
    let dflt = TestFont::new("D")
        .with_chars("\u{8FBB}", 900)
        .with_default_sequence('\u{8FBB}', '\u{E0100}');
    let null = TestFont::new("N")
        .with_chars("\u{8FBB}", 800)
        .with_null_sequence('\u{8FBB}', '\u{E0100}');
    let (mut eng, ids) = engine_with(vec![
        FaceRule::new("P", vec![bytes(&plain)]),
        FaceRule::new("D", vec![bytes(&dflt)]),
        FaceRule::new("N", vec![bytes(&null)]),
    ]);
    let seq = "\u{8FBB}\u{E0100}";
    let l = eng.layout(&StyledText::plain(seq, style("P, D")), None, 1.0);
    assert_eq!(
        runs(&l)[0].0,
        eng.fonts.face_blob_id(ids[1]),
        "default UVS beats base-only"
    );
    let l = eng.layout(&StyledText::plain(seq, style("N, D")), None, 1.0);
    assert_eq!(
        runs(&l)[0].0,
        eng.fonts.face_blob_id(ids[1]),
        "glyph-0 mapping is no support"
    );
}

// ---------------------------------------------------------------- caches (review P2 #9, #10)

#[test]
fn system_coverage_cache_distinguishes_stretch() {
    // installed family "Sys": normal width covers 'a', condensed covers only 'b'
    let normal = TestFont::new("SysN").with_chars("a", 500);
    let condensed = TestFont::new("SysC").with_chars("b", 400);
    let theme = TestFont::new("T").with_range('a', 'z', 600);
    let mut store = FontStore::new();
    store.register_system_family("Sys", normal.build(), 100.0, 400.0);
    store.register_system_family("Sys", condensed.build(), 75.0, 400.0);
    let t = store.add_face(FaceRule::new("T", vec![bytes(&theme)])).unwrap();
    let mut eng = TextEngine::new(store);
    let mut st = style("Sys, T");
    let l = eng.layout(&StyledText::plain("a", st.clone()), None, 1.0);
    assert_ne!(runs(&l)[0].0, eng.fonts.face_blob_id(t), "normal width: Sys covers 'a'");
    st.stretch = 75.0;
    let l = eng.layout(&StyledText::plain("a", st), None, 1.0);
    assert_eq!(
        runs(&l)[0].0,
        eng.fonts.face_blob_id(t),
        "condensed Sys lacks 'a' → theme"
    );
}

#[test]
fn matching_caches_stay_bounded_and_drop_removed_theme_faces() {
    let (sel_cap, sys_cap, cov_cap) = FontStore::cache_caps();
    let cjk = TestFont::new("C").with_range('\u{4E00}', '\u{9FFF}', 1000);
    let font = cjk.build();
    let mut eng = TextEngine::new(FontStore::new());
    for round in 0..3 {
        eng.fonts
            .add_face(FaceRule::new("C", vec![FaceSource::Bytes(Arc::new(font.clone()))]))
            .unwrap();
        // ~21k distinct code points, more than the coverage cap
        let text: String = ('\u{4E00}'..='\u{9FFF}').collect();
        let l = eng.layout(&StyledText::plain(text, style("C")), Some(2000.0), 1.0);
        assert!(l.line_count() > 1);
        let (s, y, c) = eng.fonts.cache_entries();
        assert!(
            s <= sel_cap && y <= sys_cap && c <= cov_cap,
            "round {round}: {s} {y} {c}"
        );
        eng.fonts.clear_faces();
        assert_eq!(eng.fonts.cache_entries().2, 0, "theme coverage dropped on clear");
    }
}

// ---------------------------------------------------------------- malformed input (review P2 #11)

#[test]
fn malformed_public_input_is_normalised_not_panicking() {
    let (mut eng, _) = engine_with(vec![FaceRule::new("Latin", vec![bytes(&latin())])]);
    // reversed and out-of-range runs, run naming a missing style
    let mut t = StyledText::plain("hello world", style("Latin"));
    t.push_run(5..2, style("Latin").weight(700.0));
    t.push_run(8..999, style("Latin"));
    t.runs.push((0..3, 42));
    let l = eng.layout(&t, Some(30.0), 1.0);
    assert!(l.line_count() >= 1);
    // non-boundary run ends inside a multi-byte char
    let mut t = StyledText::plain("aéb", style("Latin"));
    t.push_run(2..3, style("Latin"));
    eng.layout(&t, None, 1.0);
    // empty style list
    let t = StyledText {
        text: "abc".into(),
        styles: Vec::new(),
        runs: vec![(0..2, 0)],
        base_dir: BaseDir::Auto,
        inline_objects: vec![InlineObject {
            id: 1,
            index: 99,
            width: f32::NAN,
            height: -3.0,
        }],
    };
    let l = eng.layout(&t, Some(f32::NAN), f32::NAN);
    assert_eq!(l.scale(), 1.0);
    // NaN everywhere in a style
    let mut st = style("Latin");
    st.size = f32::NAN;
    st.weight = f32::NAN;
    st.stretch = f32::INFINITY;
    st.slope = Slope::Oblique(f32::NAN);
    st.letter_spacing = f32::NAN;
    st.line_height = Some(f32::NAN);
    let l = eng.layout(&StyledText::plain("abc", st), Some(-5.0), 0.0);
    assert!(l.size().0.is_finite() && l.size().1.is_finite());
    // reversed face descriptors and unicode ranges
    let mut store = FontStore::new();
    store
        .add_face(
            FaceRule::new("R", vec![bytes(&latin())])
                .weight(900.0, 100.0)
                .unicode_range([0x7A..=0x61, 0x110000..=0x120000]),
        )
        .unwrap();
    let mut eng = TextEngine::new(store);
    let l = eng.layout(&StyledText::plain("abc", style("R").weight(f32::NAN)), None, 1.0);
    assert!(l.glyph_runs().iter().all(|r| r.glyphs.iter().all(|g| g.id != 0)));
}

#[test]
fn malformed_collection_entry_does_not_shift_face_indices() {
    // review P2 #12: face 0 of the collection is garbage, face 1 valid
    let dir = tempfile::tempdir().unwrap();
    let good = TestFont::new("Late Face").style(700, false).with_range('a', 'z', 700);
    let mut bytes_ = ttc(&[latin().build(), good.build()]);
    // corrupt face 0's offset table (sfnt version) so it fails to parse
    let off0 = u32::from_be_bytes([bytes_[12], bytes_[13], bytes_[14], bytes_[15]]) as usize;
    bytes_[off0..off0 + 4].copy_from_slice(b"XXXX");
    std::fs::write(dir.path().join("m.ttc"), &bytes_).unwrap();
    let mut idx = FaceNameIndex::default();
    idx.add_file(&dir.path().join("m.ttc"));
    assert!(idx.find("Latin-Regular").is_none(), "face 0 really is unreadable");
    assert_eq!(idx.find("LateFace-Bold").map(|l| l.index), Some(1));
}

// ---------------------------------------------------------------- glyph clusters (PDF text)

/// `glyph_clusters` parallels `glyph_runs` (also when a style change splits one shaped run and
/// across line breaks), and a ligature glyph maps to all of its characters.
#[test]
fn glyph_clusters_map_glyphs_to_text() {
    let (mut e, _) = engine_with(vec![FaceRule::new("Latin", vec![bytes(&latin())])]);
    let mut t = StyledText::plain("a fine day and more words here", style("Latin"));
    let mut bold = style("Latin");
    bold.color = 0xff00_00ff;
    t.push_run(7..10, bold); // "day" in another color: same font, split glyph run
    let l = e.layout(&t, Some(60.0), 1.0);
    let runs = l.glyph_runs();
    let clusters = l.glyph_clusters();
    assert_eq!(runs.len(), clusters.len());
    assert!(l.line_count() > 1);
    let mut covered = String::new();
    for (r, c) in runs.iter().zip(&clusters) {
        assert_eq!(r.glyphs.len(), c.len());
        let mut last = None;
        for range in c {
            assert!(range.start <= range.end && range.end <= t.text.len());
            if last != Some(range.clone()) {
                covered.push_str(&t.text[range.clone()]);
                last = Some(range.clone());
            }
        }
    }
    // every character is reachable from some glyph (spaces at line ends may be dropped)
    assert_eq!(covered.replace(' ', ""), t.text.replace(' ', ""));
    // the "fi" ligature: one glyph whose cluster covers both letters
    let fi = t.text.find("fi").unwrap();
    assert!(clusters.iter().flatten().any(|r| *r == (fi..fi + 2)), "{clusters:?}");
}

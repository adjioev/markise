//! Extended callouts in the view (ADR 0025): the header shows the title or the capitalised name,
//! the tag never; colour and icon come from the theme.
use markise_layout::{
    View,
    block::{NoImages, Prim},
    testing,
};
use markise_theme::Theme;
use std::sync::Arc;

fn view_in(s: &str, theme: Theme) -> View {
    testing::Shown::new(Arc::new(markise_md::parse(s))).view(theme, 600.0, 1.0)
}

fn prims(v: &mut View) -> Vec<Prim> {
    let mut fonts = testing::fonts();
    let mut sh = fonts.with_pixels_per_point(1.0);
    v.frame(4000.0, 0.0, &mut sh, &NoImages)
        .iter()
        .flat_map(|v| v.layout.prims.clone())
        .collect()
}

fn texts(v: &mut View) -> Vec<String> {
    prims(v)
        .iter()
        .filter_map(|p| match p {
            Prim::Text { text, .. } => Some(text.text().to_owned()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_header_shows_the_title_or_the_name() {
    let t = |s: &str| texts(&mut view_in(s, Theme::light()));
    assert_eq!(t("> [!warning] Mind this\n> Body\n"), ["Mind this", "Body"]);
    assert_eq!(t("> [!warning]\n> Body\n"), ["Warning", "Body"]);
    assert_eq!(t("> [!faq]\n> Body\n"), ["Faq", "Body"]);
    assert_eq!(t("> [!my-thing]\n> Body\n"), ["My-thing", "Body"], "an unknown name");
    assert_eq!(t("> [!NOTE]\n> Body\n"), ["Note", "Body"]);
    assert_eq!(t("> [!info] Only a title\n"), ["Only a title"]);
}

#[test]
fn inline_markdown_in_a_title() {
    let shown = texts(&mut view_in("> [!tip] A **bold** [link](u)\n", Theme::light()));
    assert_eq!(shown, ["A bold link"]);
}

fn accent_of(prims: &[Prim]) -> Vec<epaint::Color32> {
    prims
        .iter()
        .filter_map(|p| match p {
            Prim::Line { stroke, .. } => Some(stroke.color),
            Prim::Rect { fill, .. } => Some(*fill),
            Prim::Path { fill, stroke, .. } => Some(if stroke.width > 0.0 { stroke.color } else { *fill }),
            Prim::Circle { fill, stroke, .. } => Some(if stroke.width > 0.0 { stroke.color } else { *fill }),
            _ => None,
        })
        .collect()
}

#[test]
fn colours_follow_the_theme() {
    for (theme, name) in [(Theme::light(), "light"), (Theme::dark(), "dark")] {
        let slot = |a: usize| {
            let c = theme.alert[a].0;
            epaint::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
        };
        for (src, want) in [
            ("> [!info] T\n", slot(0)),
            ("> [!success] T\n", slot(1)),
            ("> [!example] T\n", slot(2)),
            ("> [!question] T\n", slot(3)),
            ("> [!danger] T\n", slot(4)),
            ("> [!mine] T\n", slot(0)),
        ] {
            let colours = accent_of(&prims(&mut view_in(src, theme.clone())));
            assert!(
                !colours.is_empty() && colours.contains(&want),
                "{name} {src:?}: {colours:?} lacks {want:?}"
            );
        }
    }
}

/// A bare alert draws no icon: it looks as it did before callouts.
#[test]
fn only_extended_callouts_have_an_icon() {
    let icon_prims = |s: &str| {
        prims(&mut view_in(s, Theme::light()))
            .iter()
            .filter(|p| matches!(p, Prim::Path { .. } | Prim::Circle { .. }))
            .count()
    };
    assert_eq!(icon_prims("> [!NOTE]\n> x\n"), 0);
    assert!(icon_prims("> [!note] T\n> x\n") > 0);
    assert!(icon_prims("> [!info]\n> x\n") > 0);
}

/// Every position of a title is a caret stop, and typing there re-lays out as a title.
#[test]
fn a_title_is_text_with_source_positions() {
    let src = "> [!warning] Title\n> Body\n";
    let mut v = view_in(src, Theme::light());
    let ps = prims(&mut v);
    let map = ps
        .iter()
        .find_map(|p| match p {
            Prim::Text { text, .. } if text.text() == "Title" => text.source_map().cloned(),
            _ => None,
        })
        .expect("a mapped title");
    let at = src.find("Title").unwrap() as u32 - src.find("[!").unwrap() as u32;
    assert_eq!(
        map.to_source(0),
        Some(at),
        "the title's first byte, relative to the item"
    );
}

fn md(s: &str) -> markise_md::Document {
    markise_md::parse(s)
}

fn offset(s: &str, needle: &str) -> usize {
    s.find(needle).unwrap()
}

#[test]
fn a_fold_mark_decides_how_it_starts_and_the_view_toggles_it() {
    let src = "> [!info]- T\n> Body\n>\n> More\n\nAfter\n";
    let mut v = view_in(src, Theme::light());
    assert_eq!(texts(&mut v), ["T", "After"]);
    v.toggle_expanded(0);
    assert_eq!(texts(&mut v), ["T", "Body", "More", "After"]);
    v.toggle_expanded(0);
    assert_eq!(texts(&mut v), ["T", "After"]);
    let mut v = view_in("> [!info]+ T\n> Body\n", Theme::light());
    assert_eq!(texts(&mut v), ["T", "Body"]);
    v.toggle_expanded(0);
    assert_eq!(texts(&mut v), ["T"]);
    // no title: the name, and the first leaf's body lines go too
    let mut v = view_in("> [!tip]-\n> Body\n\nAfter\n", Theme::light());
    assert_eq!(texts(&mut v), ["Tip", "After"]);
}

#[test]
fn closed_heights_and_estimates_agree() {
    let body = "> Body line of some length.\n>\n".repeat(40);
    let src = format!("> [!info]- T\n{body}\nAfter\n");
    let mut v = view_in(&src, Theme::light());
    let before = v.total_height();
    prims(&mut v);
    let laid = v.total_height();
    assert!((before - laid).abs() < 30.0, "estimate {before}, laid out {laid}");
    assert!(laid < 200.0, "only the header and After: {laid}");
    v.toggle_expanded(0);
    let open = v.total_height();
    assert!(open > 1000.0, "the body counts again: {open}");
    prims(&mut v);
    let open_laid = v.total_height();
    v.toggle_expanded(0);
    prims(&mut v);
    assert!(
        (v.total_height() - laid).abs() < 1.0,
        "back: {} vs {laid}",
        v.total_height()
    );
    assert!(open_laid > laid);
}

#[test]
fn reveal_offset_opens_what_hides_it_and_nothing_else() {
    let src = "> [!a]- T\n> Body\n>\n> > [!b]- Inner\n> > Deep\n\nAfter\n\n> [!c]- Other\n> O\n";
    let mut v = view_in(src, Theme::light());
    prims(&mut v);
    assert!(!v.reveal_offset(offset(src, "After")));
    assert!(v.reveal_offset(offset(src, "Deep")));
    let shown = texts(&mut v);
    assert!(shown.contains(&"Deep".to_string()), "{shown:?}");
    assert!(!shown.contains(&"O".to_string()), "{shown:?}");
    // a line of the first leaf hidden on the visible item
    assert!(v.reveal_offset(offset(src, "O\n")));
    assert!(texts(&mut v).contains(&"O".to_string()));
}

#[test]
fn the_cross_index_knows_the_folds_and_their_extents() {
    let src = "> [!a]- T\n> B\n>\n> > [!b]+ I\n> > D\n\nAfter\n";
    let d = md(src);
    let m = markise_layout::DocModel::build_with(&d, None, markise_layout::Paging::Full);
    let f = m.cross.folds();
    assert_eq!(f.len(), 2);
    assert_eq!(f[0].start, offset(src, "[!a]"));
    assert_eq!(f[0].end(), offset(src, "D\n") + 1);
    assert_eq!(f[1].start, offset(src, "[!b]"));
    assert_eq!(f[1].end(), offset(src, "D\n") + 1);
    assert!(!f[0].open() && f[1].open());
}

fn paged(d: &markise_md::Document, tops: usize) -> markise_layout::DocModel {
    markise_layout::DocModel::build_with(d, None, markise_layout::Paging::Paged { tops })
}

fn folded_doc(n: usize) -> String {
    let mut s = String::new();
    for i in 0..n {
        s.push_str(&format!(
            "Paragraph {i}\n\n> [!warning]- Title {i}\n> Body {i} line\n>\n> second block {i}\n\n"
        ));
        if i % 5 == 0 {
            s.push_str(&format!("> [!info]+ Open {i}\n> Open body {i}\n\n"));
        }
    }
    s
}

/// Pages know a closed callout's body is hidden: their estimates and what the frames lay out stay
/// close, whichever pages are loaded, and the reader's choice survives an eviction.
#[test]
fn a_paged_view_estimates_closed_callouts() {
    use markise_doc::FormatDoc;
    let src = folded_doc(400);
    let d = md(&src);
    let doc: Arc<dyn FormatDoc> = Arc::new(d.clone());
    let mut v = View::new(doc.clone(), Arc::new(paged(&d, 8)), Theme::light(), 700.0, 1.0);
    let full = Arc::new(markise_layout::DocModel::build_with(
        &d,
        None,
        markise_layout::Paging::Full,
    ));
    let fv = View::new(doc.clone(), full, Theme::light(), 700.0, 1.0);
    let (a, b) = (v.total_height(), fv.total_height());
    assert!((a - b).abs() / b < 0.05, "paged {a}, full {b}");
    // much less than with every body shown
    let open = md(&src.replace("]- ", "]+ "));
    let opened: Arc<dyn FormatDoc> = Arc::new(open.clone());
    let ov = View::new(opened, Arc::new(paged(&open, 8)), Theme::light(), 700.0, 1.0);
    assert!(a < ov.total_height() * 0.8, "closed {a}, open {}", ov.total_height());

    // a closed one deep in the document, toggled through the page it lives in
    let at = offset(&src, "[!warning]- Title 300");
    v.ensure_offset(at);
    let i = v.model().item_at_offset(at);
    assert!(v.disclosure_of(i + 1).0, "its body is hidden");
    v.toggle_expanded(i);
    assert!(!v.disclosure_of(i + 1).0);
    let starts = v.model().cross.folds();
    let f = starts.iter().find(|f| f.start == at).unwrap();
    assert_eq!(f.toggled, Some(true));
    v.model().check_against_full(&d).unwrap();
}

/// Typing in a body keeps the fold's extent right without a document-wide derivation of anything
/// but the folds themselves; typing outside any callout does no work per fold at all.
#[test]
fn typing_outside_a_callout_does_no_fold_work() {
    use markise_doc::{FormatDoc, PatchSet};
    let src = format!("Typing here.\n\n{}", folded_doc(500));
    let mut d = md(&src);
    let doc: Arc<dyn FormatDoc> = Arc::new(d.clone());
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = View::new(doc, Arc::new(paged(&d, 16)), Theme::light(), 700.0, 1.0);
    v.frame(800.0, 300.0, &mut f, &NoImages);
    for at in 6..26 {
        let e = d.apply(&PatchSet::single_verbatim(at..at, "x"));
        let ch = v.model_mut().apply(&d, &e);
        assert!(!ch.disclosures);
        let before = v.disclosure_derivations();
        v.apply_change(Arc::new(d.clone()), &ch);
        assert_eq!(v.disclosure_derivations(), before);
    }
    v.model().check_against_full(&d).unwrap();
    // in a body: the extent follows
    let at = d.text().find("Body 7 line").unwrap();
    let e = d.apply(&PatchSet::single_verbatim(at..at, "typed "));
    let ch = v.model_mut().apply(&d, &e);
    v.apply_change(Arc::new(d.clone()), &ch);
    v.model().check_against_full(&d).unwrap();
}

mod props {
    use super::*;
    use markise_doc::{FormatDoc, PatchSet};
    use proptest::prelude::*;

    const SNIPS: &[&str] = &[
        "x",
        "\n",
        "\n\n",
        "> ",
        ">",
        "[!a]-",
        "[!b]+ T",
        "- ",
        "\n> ",
        "\n>\n> ",
        "]",
        "-",
        "+",
        "# ",
        "\n> > [!c]- I\n",
    ];

    fn floor(s: &str, mut i: usize) -> usize {
        i = i.min(s.len());
        while !s.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    proptest! {
        /// After any edit the folds and everything else of the model equal a fresh build's, full
        /// or paged.
        #[test]
        fn prop_folds_follow_edits(
            pages in 1usize..4,
            ops in proptest::collection::vec((0usize..4096, 0usize..6, 0usize..SNIPS.len(), any::<bool>()), 1..8),
        ) {
            let mut d = md(&folded_doc(4));
            let mut pm = paged(&d, pages);
            let mut fm = markise_layout::DocModel::build_with(&d, None, markise_layout::Paging::Full);
            for (pos, del, snip, ins) in ops {
                let t = d.text();
                let a = floor(t, pos % (t.len() + 1));
                let b = floor(t, a + del).max(a);
                let s = if ins { SNIPS[snip] } else { "" };
                if a == b && s.is_empty() {
                    continue;
                }
                let e = d.apply(&PatchSet::single_verbatim(a..b, s));
                pm.apply(&d, &e);
                fm.apply(&d, &e);
                pm.check_against_full(&d).map_err(|m| TestCaseError::fail(format!("paged after {a}..{b}={s:?}: {m}\n{:?}", d.text())))?;
                let fresh = markise_layout::DocModel::build_with(&d, None, markise_layout::Paging::Full);
                fm.diff(&fresh).map_err(|m| TestCaseError::fail(format!("full after {a}..{b}={s:?}: {m}\n{:?}", d.text())))?;
            }
        }
    }
}

/// Keystroke, frame and toggle cost with many closed callouts, against the same document with
/// plain quotes (median, release). Prints the numbers ADR 0025 reports.
#[test]
#[ignore = "heavy: timing on large inputs; cargo xtask heavy"]
fn heavy_suite_many_closed_callouts_do_not_regress() {
    use markise_doc::{FormatDoc, PatchSet};
    use std::time::Instant;
    fn median(mut v: Vec<f64>) -> f64 {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    }
    /// (build ms, typing outside p50 ms, frame while scrolling p50 ms, total height)
    fn measure(src: &str) -> (f64, f64, f64, f64) {
        let mut d = md(src);
        let t = Instant::now();
        let m = paged(&d, 128);
        let build = t.elapsed().as_secs_f64() * 1e3;
        let doc: Arc<dyn FormatDoc> = Arc::new(d.clone());
        let mut fonts = testing::fonts();
        let mut f = fonts.with_pixels_per_point(1.0);
        let mut v = View::new(doc, Arc::new(m), Theme::light(), 700.0, 1.0);
        v.frame(800.0, 300.0, &mut f, &NoImages);
        let mut at = 6;
        let mut typing = Vec::new();
        let mut keep: Vec<Arc<dyn FormatDoc>> = Vec::new();
        for k in 0..200 {
            let text = if k % 30 == 29 { "\n\n" } else { "x" };
            let e = d.apply(&PatchSet::single_verbatim(at..at, text));
            let shared: Arc<dyn FormatDoc> = Arc::new(d.clone());
            keep.push(v.doc().clone());
            let t = Instant::now();
            let ch = v.model_mut().apply(&d, &e);
            v.apply_change(shared, &ch);
            v.frame(800.0, 300.0, &mut f, &NoImages);
            typing.push(t.elapsed().as_secs_f64() * 1e3);
            at += text.len();
        }
        let mut frames = Vec::new();
        for _ in 0..300 {
            let t = Instant::now();
            v.scroll_by(240.0);
            v.frame(800.0, 300.0, &mut f, &NoImages);
            frames.push(t.elapsed().as_secs_f64() * 1e3);
        }
        (build, median(typing), median(frames), v.total_height())
    }
    for n in [1_000usize, 10_000] {
        let mut folded = String::from("Typing here.\n\n");
        for i in 0..n {
            folded.push_str(&format!(
                "> [!warning]- Title {i}\n> Body {i} of a closed callout, some words long.\n>\n> Second block {i}.\n\nAfter {i}\n\n"
            ));
        }
        // the same bytes, not a callout: `[` in the text (a document with brackets builds its
        // reference context per copy; the test copies the document, the editor does not)
        let plain = folded.replace("[!warning]- ", "[!warning]x ");
        let (b0, t0, f0, h0) = measure(&plain);
        let (b1, t1, f1, h1) = measure(&folded);
        eprintln!(
            "{n} callouts: build {b0:.1} -> {b1:.1} ms; typing p50 {t0:.3} -> {t1:.3} ms; scroll frame p50 {f0:.3} -> {f1:.3} ms; height {h0:.0} -> {h1:.0}"
        );
        assert!(t1 < t0 + 0.2, "typing {t0:.3} -> {t1:.3} ms");
        assert!(h1 < h0, "closed bodies take no height");

        // toggling one, and typing in an open callout among closed ones
        let mut d = md(&folded);
        let doc: Arc<dyn FormatDoc> = Arc::new(d.clone());
        let mut fonts = testing::fonts();
        let mut f = fonts.with_pixels_per_point(1.0);
        let mut v = View::new(doc, Arc::new(paged(&d, 128)), Theme::light(), 700.0, 1.0);
        v.frame(800.0, 300.0, &mut f, &NoImages);
        let head = v.model().item_at_offset(d.text().find("[!warning]").unwrap());
        let mut toggles = Vec::new();
        for _ in 0..41 {
            let t = Instant::now();
            v.toggle_expanded(head);
            v.frame(800.0, 300.0, &mut f, &NoImages);
            toggles.push(t.elapsed().as_secs_f64() * 1e3);
        }
        v.toggle_expanded(head);
        v.frame(800.0, 300.0, &mut f, &NoImages);
        let first = d.text().find("Body 0").unwrap();
        let mut inside = Vec::new();
        let mut keep: Vec<Arc<dyn FormatDoc>> = Vec::new();
        for at in first..first + 200 {
            let e = d.apply(&PatchSet::single_verbatim(at..at, "x"));
            let shared: Arc<dyn FormatDoc> = Arc::new(d.clone());
            keep.push(v.doc().clone());
            let t = Instant::now();
            let ch = v.model_mut().apply(&d, &e);
            v.apply_change(shared, &ch);
            v.frame(800.0, 300.0, &mut f, &NoImages);
            inside.push(t.elapsed().as_secs_f64() * 1e3);
        }
        eprintln!(
            "{n} callouts: toggle + frame p50 {:.3} ms; typing in an open callout p50 {:.3} ms",
            median(toggles),
            median(inside)
        );
    }
}

//! Layout cache invalidation and viewport virtualisation (M1).

use std::sync::Arc;

use markise_layout::block::{ImageState, Images, NoImages, Prim};
use markise_layout::{DocModel, View, testing};
use markise_theme::Theme;

fn doc(n: usize) -> String {
    let mut s = String::from("# Title\n\n");
    for i in 0..n {
        s.push_str(&format!("Paragraph {i} with some *styled* text and `code`.\n\n"));
    }
    s
}

fn view(src: &str, width: f32) -> View {
    markise_layout::testing::Shown::new(Arc::new(markise_md::parse(src))).view(Theme::light(), width, 1.0)
}

#[test]
fn only_viewport_plus_margin_is_laid_out() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = view(&doc(5000), 700.0);
    let vis = v.frame(800.0, 400.0, &mut f, &NoImages);
    assert!(!vis.is_empty());
    assert_eq!(vis[0].idx, 0);
    // ~ (800 + 400) / ~40 pt per paragraph — far fewer than 5001 items
    assert!(v.laid_out.len() < 60, "laid out {} items", v.laid_out.len());
    assert!(v.laid_out.iter().all(|&i| i < 60));
    // visible items tile the viewport top to bottom without overlap
    for w in vis.windows(2) {
        assert!(w[1].y >= w[0].y + w[0].layout.height - 0.01);
    }
    assert!(vis.last().unwrap().y < 800.0);

    // jump to the middle: only items around it are laid out
    v.scroll_to_item(2500);
    let vis = v.frame(800.0, 400.0, &mut f, &NoImages);
    assert_eq!(vis[0].idx, 2500);
    let lo = *v.laid_out.iter().min().unwrap();
    let hi = *v.laid_out.iter().max().unwrap();
    assert!(lo >= 2480 && hi <= 2540, "laid out range {lo}..={hi}");

    // second frame without changes: everything comes from the cache
    let _ = v.frame(800.0, 400.0, &mut f, &NoImages);
    assert!(v.laid_out.is_empty());
}

#[test]
fn cache_invalidates_on_width_theme_and_content_change() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let src = doc(50);
    let mut v = view(&src, 700.0);
    v.frame(600.0, 0.0, &mut f, &NoImages);
    let first = v.laid_out.clone();
    assert!(!first.is_empty());

    v.frame(600.0, 0.0, &mut f, &NoImages);
    assert!(v.laid_out.is_empty(), "unchanged: cache hit");

    v.set_params(500.0, &Theme::light(), 1.0);
    v.frame(600.0, 0.0, &mut f, &NoImages);
    assert!(!v.laid_out.is_empty(), "width change re-lays out");

    v.set_params(500.0, &Theme::dark(), 1.0);
    v.frame(600.0, 0.0, &mut f, &NoImages);
    assert!(!v.laid_out.is_empty(), "theme change re-lays out");

    // back to the earlier parameters: layouts are still cached
    v.set_params(700.0, &Theme::light(), 1.0);
    v.frame(600.0, 0.0, &mut f, &NoImages);
    assert!(v.laid_out.is_empty(), "old entries reused");

    // edit paragraph 1 only: exactly that item is laid out again
    let mut d = markise_md::parse(&src);
    let pos = src.find("Paragraph 1 ").unwrap();
    d.edit(pos..pos + 9, "Changed");
    let m = DocModel::build(&d, None);
    v.set_model(Arc::new(d), m);
    v.scroll_to_item(0);
    v.frame(600.0, 0.0, &mut f, &NoImages);
    assert_eq!(v.laid_out, vec![2], "only the edited block (item 2) is re-laid out");
}

#[test]
fn heights_refine_without_moving_the_anchor() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = view(&doc(2000), 400.0);
    v.scroll_to_item(1000);
    let vis = v.frame(600.0, 300.0, &mut f, &NoImages);
    assert_eq!(vis[0].idx, 1000);
    assert_eq!(vis[0].y, 0.0);
    // scrolling up a little shows item 999 directly above, laid out exactly (margin prefetch)
    assert!(v.is_exact(999));
    v.scroll_by(-10.0);
    let vis = v.frame(600.0, 300.0, &mut f, &NoImages);
    assert_eq!(vis[0].idx, 999);
    // on screen, item 1000 moved down by exactly 10 pt although more items above were laid
    // out (prefetch) and the absolute position changed
    let i1000 = vis.iter().find(|x| x.idx == 1000).unwrap();
    assert!((i1000.y - 10.0).abs() < 0.01, "{}", i1000.y);
}

#[test]
fn bottom_is_clamped() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = view(&doc(30), 700.0);
    v.scroll_to_y(1e9);
    let vis = v.frame(500.0, 0.0, &mut f, &NoImages);
    let last = vis.last().unwrap();
    assert_eq!(last.idx, v.len() - 1);
    let bottom = last.y + last.layout.height;
    assert!(
        bottom <= 500.0 + 0.5,
        "document end is at or above the viewport bottom ({bottom})"
    );
}

struct Sized;
impl Images for Sized {
    fn state(&self, _src: &str) -> ImageState {
        ImageState::Ready { size: [200, 100] }
    }
}

#[test]
fn image_epoch_relayouts_only_image_items() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = view("text\n\n![alt](pic.png)\n\nmore\n", 700.0);
    let vis = v.frame(600.0, 0.0, &mut f, &NoImages);
    let ph = vis[1].layout.height;
    v.images_changed();
    let vis = v.frame(600.0, 0.0, &mut f, &Sized);
    assert_eq!(v.laid_out, vec![1]);
    assert!(vis[1].layout.height != ph);
    assert!(
        vis[1]
            .layout
            .prims
            .iter()
            .any(|p| matches!(p, Prim::Image { rect, .. } if rect.width() == 200.0))
    );
}

#[test]
fn renders_every_block_kind_without_panicking() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../markise-md/tests/corpus/edge.md"
    ))
    .unwrap();
    let mut all = src.clone();
    for f in [
        "nested.md",
        "tables.md",
        "math.md",
        "footnotes.md",
        "frontmatter.md",
        "cjk.md",
        "crlf.md",
    ] {
        all.push_str("\n\n");
        all.push_str(
            &std::fs::read_to_string(format!("{}/../markise-md/tests/corpus/{f}", env!("CARGO_MANIFEST_DIR"))).unwrap(),
        );
    }
    all.push_str("\n\n[toc]\n\n> [!WARNING]\n> careful\n\n- [ ] a\n- [x] b\n\n1. x\n2. y\n\n---\n\n| a | b |\n|:-|-:|\n| 1 | 2 |\n");
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(2.0);
    let model = testing::Shown::new(Arc::new(markise_md::parse(&all)));
    let n = model.items.len();
    let mut v = model.view(Theme::dark(), 640.0, 2.0);
    for i in 0..n {
        let l = v.layout(i, &mut f, &NoImages);
        assert!(l.height >= 0.0 && l.height.is_finite());
    }
}

#[test]
fn links_are_hit_testable() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = view("see [the docs](https://example.com) now\n", 700.0);
    let vis = v.frame(400.0, 0.0, &mut f, &NoImages);
    let l = &vis[0].layout;
    assert_eq!(l.links.len(), 1);
    let Prim::Text { pos, text, .. } = &l.prims[l.links[0].prim] else {
        panic!()
    };
    let galley = text.galley().unwrap();
    // the middle of the galley's first row is inside "the docs"? find x of byte 6
    let r = galley.pos_from_cursor(epaint::text::cursor::CCursor::new(6));
    let hit = *pos + epaint::vec2(r.min.x + 2.0, r.center().y);
    assert_eq!(l.link_at(hit), Some("https://example.com"));
    let miss = *pos + epaint::vec2(1.0, r.center().y);
    assert_eq!(l.link_at(miss), None);
}

fn text_bytes(l: &markise_layout::BlockLayout) -> usize {
    l.prims
        .iter()
        .map(|p| match p {
            Prim::Text { text, .. } => text.text().len(),
            _ => 0,
        })
        .sum()
}

/// Review item 8: a single huge block must not be shaped whole on the first frame.
fn assert_bounded_first_frame(src: &str, what: &str) {
    assert_bounded(src, what, 12)
}

fn assert_bounded(src: &str, what: &str, max_items: usize) {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = view(src, 700.0);
    assert!(v.len() > 10, "{what}: split into segments ({} items)", v.len());
    let vis = v.frame(800.0, 400.0, &mut f, &NoImages);
    assert!(!vis.is_empty());
    let shaped: usize = v
        .laid_out
        .clone()
        .into_iter()
        .map(|i| text_bytes(&v.layout(i, &mut f, &NoImages)))
        .sum();
    assert!(
        shaped < 200 * 1024,
        "{what}: first frame shaped {shaped} bytes of a {} byte block",
        src.len()
    );
    // jumping into the middle lays out only segments around it
    v.scroll_to_item(v.len() / 2);
    v.frame(800.0, 400.0, &mut f, &NoImages);
    assert!(
        v.laid_out.len() <= max_items,
        "{what}: {} items laid out",
        v.laid_out.len()
    );
}

#[test]
fn huge_code_block_is_virtualised() {
    let mut s = String::from("```rust\n");
    for i in 0..60_000 {
        s.push_str(&format!("let x{i} = {i}; // some code on line {i}\n"));
    }
    s.push_str("```\n");
    assert_bounded_first_frame(&s, "code block");
}

#[test]
fn huge_paragraph_and_single_long_line_are_virtualised() {
    let mut s = String::new();
    for i in 0..40_000 {
        s.push_str(&format!("word{i} and some more words on soft line {i}\n"));
    }
    assert_bounded_first_frame(&s, "multi-line paragraph");
    let one_line = "lorem ipsum dolor sit amet ".repeat(40_000); // ~1 MB, no newline
    assert_bounded_first_frame(&one_line, "single-line paragraph");
}

#[test]
fn huge_table_is_virtualised_with_stable_columns() {
    let mut s = String::from("| name | value | note |\n|---|--:|:-:|\n");
    for i in 0..20_000 {
        s.push_str(&format!("| row {i} | {} | n{} |\n", i * 7, i % 13));
    }
    assert_bounded_first_frame(&s, "table");
    // column separators are at the same x in the first and a later segment
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = view(&s, 700.0);
    let xs = |l: &markise_layout::BlockLayout| -> Vec<i32> {
        l.prims
            .iter()
            .filter_map(|p| match p {
                Prim::Line { a, b, .. } if a.x == b.x => Some(a.x.round() as i32),
                _ => None,
            })
            .collect()
    };
    let a = v.layout(0, &mut f, &NoImages);
    let b = v.layout(v.len() - 1, &mut f, &NoImages);
    assert_eq!(xs(&a), xs(&b));
    assert!(
        a.prims.len() < 2_000,
        "table segment primitives bounded: {}",
        a.prims.len()
    );
}

#[test]
fn huge_toc_is_segmented_and_outline_indices_stay_correct() {
    let mut s = String::from("[toc]\n\n");
    for i in 0..3_000 {
        s.push_str(&format!("## Heading {i}\n\ntext {i}\n\n"));
    }
    let m = DocModel::build(&markise_md::parse(&s), None);
    assert!(
        m.items
            .iter()
            .filter(|i| i.kind == markise_layout::ItemKind::Toc)
            .count()
            > 10
    );
    for e in &m.outline {
        assert!(matches!(m.items[e.item].kind, markise_layout::ItemKind::Heading { .. }));
    }
    // the middle of this document is ordinary headings/paragraphs (one screen ≈ 40 items)
    assert_bounded(&s, "toc", 60);
}

#[test]
fn image_resolution_context_is_part_of_the_cache_key() {
    // review item 12: same paragraph, different typora-root-url → different layout key
    let a = DocModel::build(
        &markise_md::parse("---\ntypora-root-url: old\n---\n![x](/p.png)\n"),
        None,
    );
    let b = DocModel::build(
        &markise_md::parse("---\ntypora-root-url: new\n---\n![x](/p.png)\n"),
        None,
    );
    assert_ne!(a.ctx_hash, b.ctx_hash);
    let c = DocModel::build(
        &markise_md::parse("![x](p.png)\n"),
        Some(std::path::Path::new("/a/doc.md")),
    );
    let d = DocModel::build(
        &markise_md::parse("![x](p.png)\n"),
        Some(std::path::Path::new("/b/doc.md")),
    );
    assert_ne!(c.ctx_hash, d.ctx_hash);
}

/// Records the image sources the view asks for.
struct Recorder(std::cell::RefCell<Vec<String>>);
impl Images for Recorder {
    fn state(&self, src: &str) -> ImageState {
        self.0.borrow_mut().push(src.to_string());
        ImageState::Ready { size: [10, 10] }
    }
}

fn requested_src(dir: &std::path::Path, md: &str) -> String {
    let doc: Arc<dyn markise_doc::FormatDoc> = Arc::new(markise_md::parse(md));
    let model = DocModel::build(&*doc, Some(&dir.join("doc.md")));
    let mut v = View::new(doc, model, Theme::light(), 700.0, 1.0);
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let rec = Recorder(Default::default());
    v.frame(600.0, 0.0, &mut f, &rec);
    rec.0.borrow().first().cloned().unwrap_or_default()
}

// Codex round 10: the viewer prefers a literal `%` file name; decoding is the fallback.
#[test]
fn viewer_resolves_literal_percent_names_first() {
    let d = std::env::temp_dir().join(format!("markise-view-pct-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("photo final.png"), b"x").unwrap();
    let src = requested_src(&d, "![x](photo%20final.png)\n");
    assert_eq!(
        std::path::Path::new(&src),
        d.join("photo final.png"),
        "only the decoded name exists"
    );
    std::fs::write(d.join("photo%20final.png"), b"x").unwrap();
    let src = requested_src(&d, "![x](photo%20final.png)\n");
    assert_eq!(
        std::path::Path::new(&src),
        d.join("photo%20final.png"),
        "literal wins when both exist"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// Link targets for the wikilink tests: only `Here` exists.
#[derive(Debug)]
struct OnlyHere;

impl markise_layout::LinkTargets for OnlyHere {
    fn generation(&self) -> u64 {
        7
    }
    fn resolves(&self, target: &str) -> bool {
        target == "Here"
    }
}

/// Spec 06 §2.2: a wikilink shows its label in the link color, an unresolved one a muted
/// variant; both are links (`[[target]]`); the source is revealed when the caret is in it.
#[test]
fn wikilinks_show_labels_styled_by_resolution() {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let src = "go [[Here|there]] and [[Gone#Part]]\n";
    let mut v = view(src, 700.0);
    let theme = Theme::light();
    v.set_links(Arc::new(OnlyHere));
    let l = v.layout(0, &mut f, &NoImages);
    let targets: Vec<&str> = l.links.iter().map(|h| h.target.as_str()).collect();
    assert_eq!(targets, ["[[Here]]", "[[Gone#Part]]"]);
    let Prim::Text { text, .. } = &l.prims[l.links[0].prim] else {
        panic!()
    };
    let shown = text.text().to_string();
    assert_eq!(shown, "go there and Gone › Part");
    let galley = text.galley().unwrap();
    let color_at = |needle: &str| {
        let at = shown.find(needle).unwrap();
        galley
            .job
            .sections
            .iter()
            .find(|s| s.byte_range.start.0 <= at && at < s.byte_range.end.0)
            .unwrap()
            .format
            .color
    };
    let link = markise_layout::block::color(theme.link);
    assert_eq!(color_at("there"), link);
    assert_ne!(color_at("Gone"), link, "unresolved links are muted");
    // the caret inside the first link reveals its source
    let focus = markise_layout::ItemFocus {
        reveal: vec![3..17],
        ..Default::default()
    };
    v.set_focus(Some((0, focus)));
    let l = v.layout(0, &mut f, &NoImages);
    let Prim::Text { text, .. } = &l.prims[l.links[0].prim] else {
        panic!()
    };
    assert_eq!(text.text(), "go [[Here|there]] and Gone › Part");
}

//! Native views (ADR 0021): layout through `BlockViews`, with a stand-in for what knows about
//! blocks. No package is involved: this crate has none.

use std::ops::Range;
use std::sync::Arc;

use markise_doc::FormatDoc;
use markise_layout::block::{NoImages, Prim};
use markise_layout::native::accent_color;
use markise_layout::{
    BlockLook, BlockViewMode, BlockViews, ItemKind, NativeBlock, NativeKind, NativeRole, NativeView, Paging, View,
    testing,
};
use markise_theme::Theme;

struct Stand {
    id: u64,
    kind: NativeKind,
    accent: Option<&'static str>,
}

fn role_ranges(source: &str) -> Vec<(Range<u32>, NativeRole)> {
    let mut out = Vec::new();
    let mut at = 0;
    for line in source.split_inclusive('\n') {
        let r = at as u32..(at + line.len()) as u32;
        if line.starts_with('#') {
            out.push((r, NativeRole::Title));
        } else if line.starts_with("- [") {
            out.push((r, NativeRole::Answer));
        } else if !line.starts_with(":::") && !line.trim().is_empty() {
            out.push((r, NativeRole::Text));
        }
        at += line.len();
    }
    out
}

impl BlockViews for Stand {
    fn id(&self) -> u64 {
        self.id
    }
    fn has_view(&self, name: &str) -> bool {
        name == "shown"
    }
    fn look(&self, _name: &str, args: &str, source: &str) -> BlockLook {
        if args == "misfit" {
            return BlockLook::Generic;
        }
        BlockLook::Native(NativeBlock {
            view: Arc::new(NativeView {
                kind: self.kind,
                accent: self.accent.map(Into::into),
                label: "Shown label".into(),
                single_answer: args != "many",
            }),
            parts: role_ranges(source),
        })
    }
}

fn stand(kind: NativeKind) -> Arc<dyn BlockViews> {
    Arc::new(Stand {
        id: 1,
        kind,
        accent: Some("green"),
    })
}

fn view_of(src: &str) -> View {
    testing::Shown::full(Arc::new(markise_md::parse(src))).view(Theme::light(), 700.0, 1.0)
}

fn texts(v: &mut View) -> Vec<String> {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut out = Vec::new();
    for s in v.frame(4000.0, 0.0, &mut f, &NoImages) {
        for p in &s.layout.prims {
            if let Prim::Text { text, .. } = p {
                out.push(text.text().to_string());
            }
        }
    }
    out
}

const BLOCK: &str = ":::shown\n\n## The title\n\nThe body.\n\n- [ ] One {#a1}\n- [x] Two {#a2}\n\n:::\n";

#[test]
fn with_no_views_nothing_changes() {
    let mut v = view_of(BLOCK);
    let plain = texts(&mut v);
    assert!(plain.contains(&"shown".to_string()), "{plain:?}");
    assert!(v.native_block(9).is_none());
    assert!(v.native_item(1).is_none());
}

#[test]
fn a_block_with_a_view_shows_its_label_not_its_name() {
    let mut v = view_of(BLOCK);
    v.set_block_views(Some(stand(NativeKind::Callout)));
    let shown = texts(&mut v);
    assert!(shown.contains(&"Shown label".to_string()), "{shown:?}");
    assert!(!shown.contains(&"shown".to_string()), "{shown:?}");
    assert!(shown.contains(&"One".to_string()), "the id stays hidden: {shown:?}");
    let n = v.native_block(BLOCK.find("The body").unwrap()).expect("native");
    assert_eq!(n.range, 0..BLOCK.len() - 1);
    assert_eq!(n.role_at(BLOCK.find("The body").unwrap()), Some(NativeRole::Text));
    assert_eq!(n.role_at(BLOCK.find("The title").unwrap()), Some(NativeRole::Title));
}

#[test]
fn a_block_that_does_not_fit_and_a_name_without_a_view_stay_generic() {
    let src = ":::shown misfit\n\nBody.\n\n:::\n\n:::other\n\nBody.\n\n:::\n";
    let mut v = view_of(src);
    v.set_block_views(Some(stand(NativeKind::Callout)));
    let shown = texts(&mut v);
    assert!(shown.contains(&"shown misfit".to_string()), "{shown:?}");
    assert!(shown.contains(&"other".to_string()), "{shown:?}");
    assert!(v.native_block(src.find("Body").unwrap()).is_none());
}

#[test]
fn the_mode_decides_and_comes_back() {
    let mut v = view_of(BLOCK);
    v.set_block_views(Some(stand(NativeKind::Callout)));
    let native = texts(&mut v);
    v.set_block_view_mode(BlockViewMode::Markdown);
    let md = texts(&mut v);
    assert!(md.contains(&"shown".to_string()), "{md:?}");
    assert!(v.native_block(9).is_none());
    v.set_block_view_mode(BlockViewMode::Markdown.next());
    assert_eq!(texts(&mut v), native);
}

fn item_of(v: &View, at: usize) -> usize {
    v.model().item_at_offset(at)
}

#[test]
fn only_a_view_change_gets_a_new_key_and_a_field_edit_keeps_the_others() {
    let mut v = view_of(BLOCK);
    let i = item_of(&v, BLOCK.find("The body").unwrap());
    let plain = v.cache_key(i);
    v.set_block_views(Some(stand(NativeKind::Callout)));
    let callout = v.cache_key(i);
    assert_ne!(plain, callout);
    // the same words elsewhere in the block mean the same to its other items
    let other = BLOCK.replace("The body.", "Another body.");
    let mut w = view_of(&other);
    w.set_block_views(Some(stand(NativeKind::Callout)));
    let j = item_of(&v, BLOCK.find("The title").unwrap());
    assert_eq!(v.cache_key(j), w.cache_key(j), "another item's edit is not this item's");
}

#[test]
fn accents_differ_by_theme_and_an_unknown_name_is_the_themes() {
    let (l, d) = (Theme::light(), Theme::dark());
    assert_ne!(accent_color(Some("purple"), &l), accent_color(Some("purple"), &d));
    assert_eq!(accent_color(Some("PURPLE"), &l), accent_color(Some("purple"), &l));
    let theme_accent = markise_layout::block::color(l.chrome.accent);
    assert_eq!(accent_color(Some("no-such-colour"), &l), theme_accent);
    assert_eq!(accent_color(None, &l), theme_accent);
}

#[test]
fn every_kind_keeps_the_same_heights_and_estimates_hold() {
    let mut heights = Vec::new();
    for kind in [
        NativeKind::Callout,
        NativeKind::Card,
        NativeKind::Choices,
        NativeKind::Media,
    ] {
        let mut v = view_of(BLOCK);
        v.set_block_views(Some(stand(kind)));
        let mut fonts = testing::fonts();
        let mut f = fonts.with_pixels_per_point(1.0);
        let i = item_of(&v, BLOCK.find("The title").unwrap());
        let est = v.estimate(i);
        let vis = v.frame(4000.0, 0.0, &mut f, &NoImages);
        let real: Vec<f32> = vis.iter().map(|s| s.layout.height).collect();
        // the title's estimate is its layout, to the pixel a shaper may round to
        assert!(
            (est - real[i]).abs() < 2.0,
            "{kind:?}: estimated {est}, laid out {}",
            real[i]
        );
        // the opener's height is the generic card's
        let mut g = view_of(BLOCK);
        let plain = g.frame(4000.0, 0.0, &mut f, &NoImages);
        assert_eq!(plain[0].layout.height, real[0], "{kind:?}");
        heights.push(real);
    }
    assert!(heights.windows(2).all(|w| w[0] == w[1]), "{heights:?}");
}

#[test]
fn zoom_scales_a_native_block_with_the_theme() {
    let mut small = view_of(BLOCK);
    small.set_block_views(Some(stand(NativeKind::Callout)));
    let big_theme = Theme {
        font_size: Theme::light().font_size * 2.0,
        ..Theme::light()
    };
    let mut big = testing::Shown::full(Arc::new(markise_md::parse(BLOCK))).view(big_theme, 700.0, 1.0);
    big.set_block_views(Some(stand(NativeKind::Callout)));
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let title = item_of(&small, BLOCK.find("The title").unwrap());
    let a = small.frame(4000.0, 0.0, &mut f, &NoImages)[title].layout.height;
    let b = big.frame(4000.0, 0.0, &mut f, &NoImages)[title].layout.height;
    assert!(b > a * 1.5, "{a} {b}");
}

#[test]
fn a_block_over_the_limit_is_not_read() {
    struct Never;
    impl BlockViews for Never {
        fn id(&self) -> u64 {
            2
        }
        fn has_view(&self, _: &str) -> bool {
            true
        }
        fn look(&self, _: &str, _: &str, _: &str) -> BlockLook {
            panic!("read a block over the limit");
        }
    }
    let src = format!(":::x\n\n{}\n\n:::\n", "word ".repeat(20 * 1024));
    let mut v = view_of(&src);
    v.set_block_views(Some(Arc::new(Never)));
    assert!(v.native_block(8).is_none());
    assert!(texts(&mut v).contains(&"x".to_string()));
}

#[test]
fn a_page_estimate_follows_the_names_with_views() {
    // many blocks with a large heading, cut into pages
    let block = ":::shown\n\n# A very large title\n\nBody.\n\n:::\n\n";
    let src = block.repeat(600);
    let doc: Arc<dyn FormatDoc> = Arc::new(markise_md::parse(&src));
    let model = Arc::new(markise_layout::DocModel::build_with(
        &*doc,
        None,
        Paging::Paged { tops: 32 },
    ));
    let page = (1..model.items.len())
        .find(|&i| model.is_page(i))
        .expect("a placeholder");
    let plain = View::new(doc.clone(), model.clone(), Theme::light(), 700.0, 1.0);
    let before = plain.estimate(page);
    let mut native = View::new(doc.clone(), model.clone(), Theme::light(), 700.0, 1.0);
    native.set_block_views(Some(stand(NativeKind::Callout)));
    let after = native.estimate(page);
    assert!(
        after < before,
        "a title is smaller than a level one heading: {after} {before}"
    );
    // one block's own items, estimated one by one, add up to the placeholder's share
    let full = testing::Shown::full(doc.clone());
    let mut items = View::new(doc.clone(), full.model.clone(), Theme::light(), 700.0, 1.0);
    items.set_block_views(Some(stand(NativeKind::Callout)));
    let of_one: f32 = (0..full.model.items.len())
        .filter(|&i| !matches!(full.model.items[i].kind, ItemKind::Hidden))
        .skip(4) // past the column's padding above the first item
        .take(4)
        .map(|i| items.estimate(i))
        .sum();
    let in_page = model.page_items(&*doc, page).len() as f32 / 4.0;
    assert!(
        ((after / in_page) - of_one).abs() < 8.0,
        "{} per block, {of_one} by items",
        after / in_page
    );
}

#[test]
fn a_label_is_one_row_whatever_the_package_wrote() {
    struct Labelled(String);
    impl BlockViews for Labelled {
        fn id(&self) -> u64 {
            5
        }
        fn has_view(&self, _: &str) -> bool {
            true
        }
        fn look(&self, _: &str, _: &str, source: &str) -> BlockLook {
            BlockLook::Native(NativeBlock {
                view: Arc::new(NativeView {
                    kind: NativeKind::Callout,
                    accent: None,
                    label: self.0.as_str().into(),
                    single_answer: true,
                }),
                parts: role_ranges(source),
            })
        }
    }
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut plain = view_of(BLOCK);
    let h0 = plain.frame(4000.0, 0.0, &mut f, &NoImages)[0].layout.height;
    for label in ["Tip\n".repeat(1024), "word ".repeat(1000)] {
        let mut v = view_of(BLOCK);
        v.set_block_views(Some(Arc::new(Labelled(label.clone()))));
        let vis = v.frame(4000.0, 0.0, &mut f, &NoImages);
        assert_eq!(vis[0].layout.height, h0, "the opener keeps its height");
        let shown: Vec<_> = (vis[0].layout.prims.iter())
            .filter_map(|p| match p {
                Prim::Text { text, pos, .. } => Some((text.text().to_string(), pos.x + text.size().x)),
                _ => None,
            })
            .collect();
        assert_eq!(shown.len(), 1, "{shown:?}");
        assert!(!shown[0].0.contains('\n'), "{:?}", shown[0].0);
        assert!(shown[0].1 < 700.0, "inside the card: {shown:?}");
    }
}

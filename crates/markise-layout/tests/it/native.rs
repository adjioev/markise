//! Native views (ADR 0021): layout through `BlockViews`, with a stand-in for what knows about
//! blocks. No package is involved: this crate has none.

use std::ops::Range;
use std::sync::Arc;

use markise_doc::FormatDoc;
use markise_layout::block::{Band, NoImages, Prim, merged_bands};
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
    icons: std::sync::Mutex<Vec<String>>,
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
    fn icon_ready(&self, icon: &str) -> bool {
        self.icons.lock().unwrap().iter().any(|i| i == icon)
    }
    fn look(&self, _name: &str, args: &str, source: &str) -> BlockLook {
        if args == "misfit" {
            return BlockLook::Outside {
                reasons: vec!["First reason".to_string(), "Second reason".to_string()],
                native: None,
            };
        }
        if args == "plain" {
            return BlockLook::Generic;
        }
        BlockLook::Native(NativeBlock {
            view: Arc::new(NativeView {
                kind: self.kind,
                accent: self.accent.map(Into::into),
                icon: Some("assets/shown.svg".into()),
                label: "Shown label".into(),
                single_answer: args != "many",
                fill: true,
                label_row: true,
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
        icons: Default::default(),
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
    assert!(shown.contains(&"⚠ First reason (+1)".to_string()), "{shown:?}");
    assert_eq!(v.block_warnings(0).unwrap().len(), 2);
    assert!(v.block_warnings(1).is_none(), "the opener has them");
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
fn a_warning_is_cut_to_its_row_and_keeps_the_height() {
    let long = "A reason much too long for the row it is on ".repeat(10);
    struct Long(String);
    impl BlockViews for Long {
        fn id(&self) -> u64 {
            3
        }
        fn has_view(&self, _: &str) -> bool {
            false
        }
        fn has_schema(&self, _: &str) -> bool {
            true
        }
        fn look(&self, _: &str, _: &str, _: &str) -> BlockLook {
            BlockLook::Outside {
                reasons: vec![self.0.clone()],
                native: None,
            }
        }
    }
    let src = ":::any\n\nBody.\n\n:::\n";
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut plain = view_of(src);
    let h0 = plain.frame(4000.0, 0.0, &mut f, &NoImages)[0].layout.height;
    let mut v = view_of(src);
    v.set_block_views(Some(Arc::new(Long(long))));
    let vis = v.frame(4000.0, 0.0, &mut f, &NoImages);
    assert_eq!(vis[0].layout.height, h0);
    let r = vis[0].layout.warning.expect("shown");
    assert!(r.max.x <= 700.0, "{r:?}");
}

#[test]
fn an_icon_takes_its_place_when_the_host_has_it_and_only_then() {
    let shown = Arc::new(Stand {
        id: 4,
        kind: NativeKind::Callout,
        accent: None,
        icons: Default::default(),
    });
    let mut v = view_of(BLOCK);
    v.set_block_views(Some(shown.clone()));
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let h = v.frame(4000.0, 0.0, &mut f, &NoImages)[0].layout.height;
    let icons = |l: &markise_layout::BlockLayout| l.prims.iter().filter(|p| matches!(p, Prim::Icon { .. })).count();
    assert_eq!(
        icons(&v.frame(4000.0, 0.0, &mut f, &NoImages)[0].layout),
        0,
        "no picture yet: no room for one"
    );
    let before = v.cache_key(0);
    shown.icons.lock().unwrap().push("assets/shown.svg".to_string());
    assert_ne!(v.cache_key(0), before, "the opener is laid out again");
    assert_eq!(v.cache_key(1), {
        let mut w = view_of(BLOCK);
        w.set_block_views(Some(shown.clone()));
        w.cache_key(1)
    });
    assert_eq!(icons(&v.frame(4000.0, 0.0, &mut f, &NoImages)[0].layout), 1);
    assert_eq!(
        v.frame(4000.0, 0.0, &mut f, &NoImages)[0].layout.height,
        h,
        "the same height"
    );
}

#[test]
fn a_block_can_differ_from_the_document_and_every_state_comes_round() {
    let src = format!("{BLOCK}\n{BLOCK}");
    let second = src.rfind(":::shown").unwrap();
    let mut v = view_of(&src);
    v.set_block_views(Some(stand(NativeKind::Callout)));
    assert!(v.native_block(second + 12).is_some());
    v.set_block_modes(BlockViewMode::Native, vec![(second, BlockViewMode::Markdown)]);
    assert!(v.native_block(5).is_some(), "the other one");
    assert!(v.native_block(second + 12).is_none());
    assert_eq!(v.block_mode_of(second), BlockViewMode::Markdown);
    let shown = texts(&mut v);
    assert!(
        shown.contains(&"shown".to_string()) && shown.contains(&"Shown label".to_string()),
        "{shown:?}"
    );
    v.set_block_modes(BlockViewMode::Markdown, vec![(second, BlockViewMode::Native)]);
    assert!(v.native_block(5).is_none());
    assert!(v.native_block(second + 12).is_some());
    // however many states there are, going on from one comes back to it
    for start in [BlockViewMode::Native, BlockViewMode::Markdown] {
        let mut m = start.next();
        let mut steps = 1;
        while m != start {
            m = m.next();
            steps += 1;
            assert!(steps < 8, "a cycle");
        }
    }
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
                    icon: None,
                    label: self.0.as_str().into(),
                    single_answer: true,
                    fill: true,
                    label_row: true,
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

#[test]
fn a_long_name_leaves_room_for_the_warning_and_an_icon_for_the_label() {
    struct Long;
    impl BlockViews for Long {
        fn id(&self) -> u64 {
            6
        }
        fn has_view(&self, name: &str) -> bool {
            name == "shown"
        }
        fn has_schema(&self, _: &str) -> bool {
            true
        }
        fn icon_ready(&self, _: &str) -> bool {
            true
        }
        fn look(&self, name: &str, _: &str, source: &str) -> BlockLook {
            if name != "shown" {
                return BlockLook::Outside {
                    reasons: vec!["The reason".to_string()],
                    native: None,
                };
            }
            BlockLook::Native(NativeBlock {
                view: Arc::new(NativeView {
                    kind: NativeKind::Callout,
                    accent: None,
                    icon: Some("assets/a.svg".into()),
                    label: "word ".repeat(1000).into(),
                    single_answer: true,
                    fill: true,
                    label_row: true,
                }),
                parts: role_ranges(source),
            })
        }
    }
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    // a name far wider than the card: the warning is still there, with its symbol
    let name = "n".repeat(400);
    let src = format!(":::{name} and some more arguments\n\nBody.\n\n:::\n");
    let mut v = view_of(&src);
    v.set_block_views(Some(Arc::new(Long)));
    let vis = v.frame(4000.0, 0.0, &mut f, &NoImages);
    let opener = &vis[0].layout;
    let r = opener.warning.expect("the warning stays");
    assert!(r.max.x <= 700.0, "{r:?}");
    assert!(texts(&mut v).iter().any(|t| t.starts_with('⚠')), "the symbol shows");
    assert!(v.block_warnings(0).is_some(), "and the reasons for the tooltip");

    // an icon's width comes out of the label's
    let mut v = view_of(BLOCK);
    v.set_block_views(Some(Arc::new(Long)));
    let vis = v.frame(4000.0, 0.0, &mut f, &NoImages);
    let right = (vis[0].layout.prims.iter())
        .filter_map(|p| match p {
            Prim::Text { text, pos, .. } => Some(pos.x + text.size().x),
            _ => None,
        })
        .fold(0.0, f32::max);
    assert!(right <= 700.0 - 34.0, "{right}");
}

/// A view of every block named `shown`, with the options of `editor/views.json`.
struct Options {
    kind: NativeKind,
    accent: &'static str,
    fill: bool,
    label_row: bool,
}

impl BlockViews for Options {
    fn id(&self) -> u64 {
        u64::from(self.fill) + 2 * u64::from(self.label_row) + 100
    }
    fn has_view(&self, name: &str) -> bool {
        name == "shown"
    }
    fn label_row(&self, name: &str) -> bool {
        name != "shown" || self.label_row
    }
    fn look(&self, _name: &str, args: &str, source: &str) -> BlockLook {
        let block = NativeBlock {
            view: Arc::new(NativeView {
                kind: self.kind,
                accent: Some(self.accent.into()),
                icon: None,
                label: "Shown label".into(),
                single_answer: true,
                fill: self.fill,
                label_row: self.label_row,
            }),
            parts: role_ranges(source),
        };
        if args == "unfilled" {
            BlockLook::Unfilled {
                reasons: vec!["Title: Required".to_string()],
                block,
            }
        } else {
            BlockLook::Native(block)
        }
    }
}

fn options(kind: NativeKind, accent: &'static str, fill: bool, label_row: bool) -> Arc<dyn BlockViews> {
    Arc::new(Options {
        kind,
        accent,
        fill,
        label_row,
    })
}

/// Everything a view paints of its own, in one item at a time: bands, then lines.
fn chrome(views: Arc<dyn BlockViews>, theme: Theme) -> (Vec<Band>, Vec<Prim>) {
    let mut v = testing::Shown::full(Arc::new(markise_md::parse(BLOCK))).view(theme, 700.0, 1.0);
    v.set_block_views(Some(views));
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let (mut bands, mut prims) = (Vec::new(), Vec::new());
    for s in v.frame(4000.0, 0.0, &mut f, &NoImages) {
        bands.extend(s.layout.bands.iter().copied());
        prims.extend(s.layout.prims.iter().cloned());
    }
    (bands, prims)
}

fn lines(prims: &[Prim]) -> usize {
    prims.iter().filter(|p| matches!(p, Prim::Line { .. })).count()
}

fn rects_with_alpha(prims: &[Prim]) -> usize {
    prims
        .iter()
        .filter(|p| matches!(p, Prim::Rect { fill, .. } if fill.a() != 255 && fill.a() != 0))
        .count()
}

#[test]
fn without_a_fill_a_view_is_a_thin_bar_of_its_accent_in_every_kind() {
    for kind in [NativeKind::Callout, NativeKind::Card, NativeKind::Choices] {
        let (bands, prims) = chrome(options(kind, "blue", false, true), Theme::light());
        assert!(!bands.is_empty(), "{kind:?}");
        let blue = accent_color(Some("blue"), &Theme::light());
        for b in &bands {
            assert_eq!(b.fill, blue, "{kind:?}: no tint");
            assert_eq!(b.rect.width(), 2.0, "{kind:?}");
        }
        assert_eq!(lines(&prims), 0, "{kind:?}: no frame");
        assert_eq!(rects_with_alpha(&prims), 0, "{kind:?}: a marked answer has no tint");
    }
    // what it has with a fill, as today
    let (bands, _) = chrome(options(NativeKind::Callout, "blue", true, true), Theme::light());
    assert!(bands.iter().any(|b| b.fill.a() < 255 && b.rect.width() > 100.0));
    assert!(bands.iter().any(|b| b.rect.width() == 3.0));
    let (bands, prims) = chrome(options(NativeKind::Card, "blue", true, true), Theme::light());
    assert!(bands.is_empty());
    assert!(lines(&prims) > 0);
}

#[test]
fn no_accent_is_neutral_in_both_themes_with_a_thin_line() {
    for theme in [Theme::light(), Theme::dark()] {
        let rule = markise_layout::block::color(theme.rule);
        let (bands, _) = chrome(options(NativeKind::Callout, "none", false, true), theme.clone());
        assert!(bands.iter().all(|b| b.fill == rule && b.rect.width() == 2.0));
        // a very light ground of the text's own colour, and no hue of an accent
        let (bands, _) = chrome(options(NativeKind::Callout, "none", true, true), theme.clone());
        let ground = bands.iter().find(|b| b.rect.width() > 100.0).unwrap().fill;
        assert!(ground.a() <= 14, "{ground:?}");
        // the choices' frame and a card's top edge are rule, a line wide
        for kind in [NativeKind::Card, NativeKind::Choices] {
            let (_, prims) = chrome(options(kind, "none", true, true), theme.clone());
            for p in &prims {
                if let Prim::Line { stroke, .. } = p {
                    assert_eq!(stroke.color, rule, "{kind:?}");
                    assert_eq!(stroke.width, 1.0, "{kind:?}");
                }
            }
        }
        assert_ne!(accent_color(Some("none"), &theme), accent_color(None, &theme));
    }
}

#[test]
fn no_label_row_takes_the_row_away_and_the_estimates_follow() {
    let mut f_fonts = testing::fonts();
    let mut f = f_fonts.with_pixels_per_point(1.0);
    for theme in [Theme::light(), Theme::dark()] {
        let src = format!("Before.\n\n{BLOCK}");
        let mut with = testing::Shown::full(Arc::new(markise_md::parse(&src))).view(theme.clone(), 700.0, 1.0);
        with.set_block_views(Some(options(NativeKind::Callout, "blue", true, true)));
        let mut without = testing::Shown::full(Arc::new(markise_md::parse(&src))).view(theme, 700.0, 1.0);
        without.set_block_views(Some(options(NativeKind::Callout, "blue", true, false)));
        let opener = (0..without.model().items.len())
            .find(|&i| without.model().items[i].kind == (ItemKind::ContainerMarker { close: false }))
            .unwrap();
        let (est_with, est_without) = (with.estimate(opener), without.estimate(opener));
        assert!(est_without < est_with);
        let real_with = with.frame(4000.0, 0.0, &mut f, &NoImages)[opener].layout.height;
        let shown = without.frame(4000.0, 0.0, &mut f, &NoImages);
        let real_without = shown[opener].layout.height;
        assert!(real_without < real_with);
        assert!(
            (est_without - real_without).abs() < 0.01,
            "{est_without} {real_without}"
        );
        assert!(
            shown[opener].layout.card_top.is_some(),
            "the buttons still have a place"
        );
    }
    assert!(!texts_with(options(NativeKind::Callout, "blue", true, false)).contains("Shown label"));
    assert!(texts_with(options(NativeKind::Callout, "blue", true, true)).contains("Shown label"));
    // a warning needs its row, and its row holds no label
    let mut v = view_of(&BLOCK.replace(":::shown", ":::shown unfilled"));
    v.set_block_views(Some(options(NativeKind::Callout, "blue", true, false)));
    let all = texts(&mut v).join("\n");
    assert!(all.contains("Title: Required") && !all.contains("Shown label"), "{all}");
}

fn texts_with(views: Arc<dyn BlockViews>) -> String {
    let mut v = view_of(BLOCK);
    v.set_block_views(Some(views));
    texts(&mut v).join("\n")
}

#[test]
fn a_page_estimate_follows_a_view_with_no_label_row() {
    let block = ":::shown\n\n### A title\n\nBody.\n\n:::\n\n";
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
    let estimate = |label_row| {
        let mut v = View::new(doc.clone(), model.clone(), Theme::light(), 700.0, 1.0);
        v.set_block_views(Some(options(NativeKind::Callout, "blue", true, label_row)));
        v.estimate(page)
    };
    let in_page = model.page_items(&*doc, page).len() as f32 / 4.0;
    // the row, as laid out
    let row = {
        let mut fonts = testing::fonts();
        let mut f = fonts.with_pixels_per_point(1.0);
        let mut height = |label_row| {
            let mut v = view_of(BLOCK);
            v.set_block_views(Some(options(NativeKind::Callout, "blue", true, label_row)));
            v.frame(4000.0, 0.0, &mut f, &NoImages)[0].layout.height
        };
        height(true) - height(false)
    };
    let less = estimate(true) - estimate(false);
    assert!(
        (less - in_page * row).abs() < 1.0,
        "{less} less for {in_page} blocks of {row}"
    );

    // and the page's estimate is what its items lay out to, a block at a time
    let full = testing::Shown::full(doc.clone());
    let mut items = View::new(doc.clone(), full.model.clone(), Theme::light(), 700.0, 1.0);
    items.set_block_views(Some(options(NativeKind::Callout, "blue", true, false)));
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let shown = items.frame(1200.0, 0.0, &mut f, &NoImages);
    let opener = |at: usize| full.model.items[shown[at].idx].kind == (ItemKind::ContainerMarker { close: false });
    let first = (0..shown.len()).find(|&i| opener(i)).unwrap();
    let second = (first + 1..shown.len()).find(|&i| opener(i)).unwrap();
    let laid: f32 = shown[second..second + 4].iter().map(|s| s.layout.height).sum();
    assert!(
        (estimate(false) / in_page - laid).abs() < 8.0,
        "{} per block, {laid} laid out",
        estimate(false) / in_page
    );
}

#[test]
fn a_blocks_bands_are_whole_wherever_its_items_are_moved_to_a_pixel() {
    let (bands, prims) = chrome(options(NativeKind::Callout, "green", true, true), Theme::light());
    assert_eq!(rects_with_alpha(&prims), 1, "only a marked answer's row: {prims:?}");
    let items = bands.len() / 2;
    assert!(items >= 4, "{items}");
    // each item's part, with heights no pixel divides, tops rounded on their own as a host does
    let mut parts = Vec::new();
    let mut y = 0.0_f32;
    for pair in bands.chunks(2) {
        let height = 17.3;
        let mut own: Vec<Band> = pair.to_vec();
        for b in &mut own {
            b.rect = egui_rect(b.rect.min.x, b.rect.max.x, 0.0, height);
        }
        parts.push((epaint::pos2(0.0, y.round()), own));
        y += height;
    }
    let bottom = parts.last().unwrap().0.y + 17.3;
    let runs = merged_bands(parts.iter().map(|(o, b)| (*o, b.as_slice())));
    assert_eq!(runs.len(), 2, "a ground and a bar: {runs:?}");
    for (r, _) in &runs {
        assert_eq!((r.min.y, r.max.y), (0.0, bottom));
    }
    assert!(runs[0].1.a() < 255, "the ground first, below the bar");
}

fn egui_rect(l: f32, r: f32, t: f32, b: f32) -> epaint::Rect {
    epaint::Rect::from_min_max(epaint::pos2(l, t), epaint::pos2(r, b))
}

#[test]
fn another_block_is_another_band_and_so_is_what_a_gap_of_items_lets_through() {
    let band = |opens| Band {
        rect: egui_rect(0.0, 100.0, 0.0, 10.0),
        fill: epaint::Color32::from_rgba_unmultiplied(0, 128, 0, 26),
        opens,
    };
    let at = |y| epaint::pos2(0.0, y);
    // two blocks one after the other: an opener starts anew
    let (a, b, c) = ([band(true)], [band(false)], [band(true)]);
    let runs = merged_bands([(at(0.0), &a[..]), (at(10.0), &b[..]), (at(20.0), &c[..])]);
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].0.max.y, 20.0);
    // a nested block's items hold none of the outer one's: it does not paint over them
    let none: [Band; 0] = [];
    let runs = merged_bands([(at(0.0), &a[..]), (at(10.0), &none[..]), (at(20.0), &b[..])]);
    assert_eq!(runs.len(), 2);
}

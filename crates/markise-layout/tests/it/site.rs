//! Blocks shown as pictures (ADR 0023), with a stand-in for the host that holds them. The view knows
//! nothing of how they are drawn.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use markise_doc::DocPos;
use markise_layout::block::{NoImages, Prim};
use markise_layout::caret::Geometry;
use markise_layout::{
    BlockLook, BlockViewMode, BlockViews, DocModel, ItemKind, Paging, SitePicture, SiteQuery, SiteShot, View, testing,
};
use markise_theme::Theme;

/// Pictures by block name, as a host would have them.
#[derive(Default)]
struct Host {
    pictures: Mutex<HashMap<String, f32>>,
    failed: Mutex<Vec<String>>,
    seen: Mutex<Vec<u64>>,
    asked: Mutex<Vec<String>>,
    older: Mutex<HashMap<usize, SitePicture>>,
    off: AtomicBool,
}

impl BlockViews for Host {
    fn id(&self) -> u64 {
        7
    }
    fn has_view(&self, name: &str) -> bool {
        name == "viewed"
    }
    fn has_schema(&self, name: &str) -> bool {
        name != "unknown"
    }
    fn has_site(&self) -> bool {
        !self.off.load(Ordering::SeqCst)
    }
    fn site_shot(&self, q: &SiteQuery<'_>) -> (u64, SiteShot) {
        self.asked.lock().unwrap().push(q.source.to_string());
        let key = q.hash;
        if self.failed.lock().unwrap().iter().any(|n| n == q.name) {
            return (key, SiteShot::Failed("It threw.".into()));
        }
        match self.pictures.lock().unwrap().get(q.name) {
            Some(&height) => {
                let picture = SitePicture {
                    id: key,
                    width: q.width,
                    height,
                };
                self.older.lock().unwrap().insert(q.opener, picture);
                (key, SiteShot::Ready(picture))
            }
            None => (
                key,
                SiteShot::Pending(self.older.lock().unwrap().get(&q.opener).copied()),
            ),
        }
    }
    fn site_seen(&self, key: u64) {
        self.seen.lock().unwrap().push(key);
    }
    fn look(&self, _name: &str, args: &str, _source: &str) -> BlockLook {
        if args == "misfit" {
            return BlockLook::Outside {
                reasons: vec!["No.".to_string()],
                native: None,
            };
        }
        BlockLook::Generic
    }
}

fn set_up(src: &str, width: f32) -> (View, Arc<Host>) {
    let mut v = testing::Shown::full(Arc::new(markise_md::parse(src))).view(Theme::light(), width, 2.0);
    let host = Arc::new(Host::default());
    v.set_block_views(Some(host.clone()));
    (v, host)
}

fn frame(v: &mut View) -> Vec<(usize, f32)> {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    v.frame(500.0, 0.0, &mut f, &NoImages)
        .iter()
        .map(|s| (s.idx, s.layout.height))
        .collect()
}

fn picture_at(v: &mut View, i: usize) -> Option<(f32, f32)> {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let l = v.layout(i, &mut f, &NoImages);
    l.prims.iter().find_map(|p| match p {
        Prim::Picture { rect, .. } => Some((rect.width(), rect.height())),
        _ => None,
    })
}

fn picture_of(v: &mut View, needle: &str) -> Option<(f32, f32)> {
    let at = item_of(v, needle);
    picture_at(v, at)
}

const DOC: &str = "Before.\n\n:::scene\n\n## Inside\n\nText inside.\n\n:::\n\nBetween.\n\n:::viewed\n\nHas a view.\n\n:::\n\nAfter.\n";

fn item_of(v: &View, needle: &str) -> usize {
    let at = v.doc().text().find(needle).unwrap();
    v.model().item_at_offset(at)
}

#[test]
fn a_block_with_no_view_shows_its_picture_and_nothing_else_of_it() {
    let (mut v, host) = set_up(DOC, 700.0);
    frame(&mut v);
    let (opener, inside, closer) = (
        item_of(&v, ":::scene"),
        item_of(&v, "## Inside"),
        item_of(&v, "Text inside"),
    );
    assert!(v.height(inside) > 0.0, "a card until it comes");
    host.pictures.lock().unwrap().insert("scene".into(), 300.0);
    v.site_changed();
    frame(&mut v);
    assert_eq!(picture_at(&mut v, opener), Some((700.0, 300.0)));
    assert_eq!(v.height(inside), 0.0);
    assert_eq!(v.height(closer), 0.0);
    assert_eq!(v.height(closer + 1), 0.0, "the closer holds no height");
    assert!((v.height(opener) - (300.0 + v.theme().block_spacing)).abs() < 0.01);
    assert!(
        picture_of(&mut v, ":::viewed").is_none(),
        "a block with a view is not one"
    );
    assert_eq!(
        v.estimate(opener),
        v.height(opener),
        "the estimate is what it will be: nothing moves when it is laid out"
    );
}

#[test]
fn only_what_is_on_screen_is_said_to_be() {
    let (mut v, host) = set_up(DOC, 700.0);
    host.seen.lock().unwrap().clear();
    frame(&mut v);
    assert_eq!(host.seen.lock().unwrap().len(), 1, "one block wants a picture");
    // and the source is read once per version of a block
    let asked = host.asked.lock().unwrap().len();
    frame(&mut v);
    frame(&mut v);
    assert_eq!(host.asked.lock().unwrap().len(), asked);
}

#[test]
fn a_document_with_nothing_to_draw_with_is_as_ever() {
    let (mut v, host) = set_up(DOC, 700.0);
    host.off.store(true, Ordering::SeqCst);
    host.asked.lock().unwrap().clear();
    host.pictures.lock().unwrap().insert("scene".into(), 300.0);
    v.site_changed();
    frame(&mut v);
    assert!(picture_of(&mut v, ":::scene").is_none());
    assert!(host.asked.lock().unwrap().is_empty(), "nothing asked");
    assert!(v.height(item_of(&v, "## Inside")) > 0.0);
}

#[test]
fn what_does_not_fit_or_is_not_known_or_failed_stays_a_card() {
    let src = ":::scene misfit\n\nA.\n\n:::\n\n:::unknown\n\nB.\n\n:::\n\n:::other\n\nC.\n\n:::\n";
    let (mut v, host) = set_up(src, 700.0);
    host.pictures.lock().unwrap().insert("scene".into(), 300.0);
    host.pictures.lock().unwrap().insert("unknown".into(), 300.0);
    host.failed.lock().unwrap().push("other".into());
    v.site_changed();
    frame(&mut v);
    for needle in [":::scene", ":::unknown", ":::other"] {
        assert!(picture_of(&mut v, needle).is_none(), "{needle}");
    }
    let other = item_of(&v, ":::other");
    assert_eq!(v.block_note(other).as_deref(), Some("It threw."));
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    assert!(v.layout(other, &mut f, &NoImages).note.is_some());
}

#[test]
fn markdown_state_means_no_picture() {
    let (mut v, host) = set_up(DOC, 700.0);
    host.pictures.lock().unwrap().insert("scene".into(), 300.0);
    v.site_changed();
    frame(&mut v);
    let at = item_of(&v, ":::scene");
    assert!(picture_at(&mut v, at).is_some());
    let opener = v.doc().text().find(":::scene").unwrap();
    v.set_block_modes(BlockViewMode::Native, vec![(opener, BlockViewMode::Markdown)]);
    frame(&mut v);
    assert!(picture_at(&mut v, at).is_none());
    assert!(v.height(item_of(&v, "## Inside")) > 0.0);
}

#[test]
fn an_older_picture_stands_in_until_the_new_one_comes() {
    let (mut v, host) = set_up(DOC, 700.0);
    host.pictures.lock().unwrap().insert("scene".into(), 300.0);
    v.site_changed();
    frame(&mut v);
    let at = item_of(&v, ":::scene");
    // gone from the host's hands, as during a new render: the older one is at the same place
    host.pictures.lock().unwrap().clear();
    v.site_changed();
    frame(&mut v);
    assert_eq!(picture_at(&mut v, at), Some((700.0, 300.0)));
}

#[test]
fn an_older_picture_at_another_width_scales_with_its_height() {
    let (mut v, host) = set_up(DOC, 700.0);
    host.older.lock().unwrap().insert(
        DOC.find(":::scene").unwrap(),
        SitePicture {
            id: 1,
            width: 350.0,
            height: 100.0,
        },
    );
    v.site_changed();
    frame(&mut v);
    assert_eq!(picture_of(&mut v, ":::scene"), Some((700.0, 200.0)));
    assert!((v.estimate(item_of(&v, ":::scene")) - (200.0 + v.theme().block_spacing)).abs() < 0.01);
}

#[test]
fn a_selection_inside_shows_what_the_block_holds_and_leaving_brings_the_picture_back() {
    let (mut v, host) = set_up(DOC, 700.0);
    host.pictures.lock().unwrap().insert("scene".into(), 300.0);
    v.site_changed();
    frame(&mut v);
    let at = item_of(&v, ":::scene");
    let inside = DOC.find("Text inside").unwrap();
    v.set_selection_for_site(&[inside]);
    frame(&mut v);
    assert!(picture_at(&mut v, at).is_none());
    assert!(v.height(item_of(&v, "Text inside")) > 0.0);
    v.set_selection_for_site(&[DOC.find("Between").unwrap()]);
    frame(&mut v);
    assert!(picture_at(&mut v, at).is_some());
    // holding the block whole is not being in it
    v.set_selection_for_site(&[0, DOC.find("Between").unwrap()]);
    frame(&mut v);
    assert!(picture_at(&mut v, at).is_some());
}

#[test]
fn caret_keys_go_past_a_picture_and_clicks_land_before_or_after_it() {
    let (mut v, host) = set_up(DOC, 700.0);
    host.pictures.lock().unwrap().insert("scene".into(), 300.0);
    v.site_changed();
    frame(&mut v);
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let before = DOC.find("Before.").unwrap() + 7;
    let between = DOC.find("Between").unwrap();
    let closer_end = DOC.find("\n\nBetween").unwrap();
    let opener = item_of(&v, ":::scene");
    let mut g = Geometry {
        view: &mut v,
        shaper: &mut f,
        images: &NoImages,
    };
    // a right arrow from the end of the text before: the gap before, or past the picture
    let mut pos = DocPos::new(before);
    let mut stops = Vec::new();
    for _ in 0..4 {
        pos = g.horizontal(pos, true, false);
        stops.push(pos);
    }
    assert!(
        stops
            .iter()
            .all(|p| p.offset <= DOC.find(":::scene").unwrap() || p.offset >= closer_end)
    );
    assert!(stops.iter().any(|p| p.offset >= between));
    // down and back up, the same
    let (down, _) = g.vertical(DocPos::new(before), None, true);
    let inside = DOC.find(":::scene").unwrap()..closer_end;
    assert!(!inside.contains(&down.offset), "{down:?}");
    // a click on the upper half lands before it, on the lower half after it
    let up = g.hit(opener, egui_pos(10.0, 40.0)).unwrap();
    assert!(up.offset <= DOC.find(":::scene").unwrap(), "{up:?}");
    let low = g.hit(opener, egui_pos(10.0, 280.0)).unwrap();
    assert!(low.offset >= closer_end, "{low:?}");
}

fn egui_pos(x: f32, y: f32) -> epaint::Pos2 {
    epaint::pos2(x, y)
}

#[test]
fn a_picture_coming_above_the_viewport_does_not_move_what_is_on_screen() {
    let mut src = String::from("Top.\n\n:::scene\n\n");
    for i in 0..30 {
        src.push_str(&format!("Line {i} of it.\n\n"));
    }
    src.push_str(":::\n\n");
    for i in 0..60 {
        src.push_str(&format!("Paragraph {i}.\n\n"));
    }
    let (mut v, host) = set_up(&src, 700.0);
    frame(&mut v);
    let far = item_of(&v, "Paragraph 20.");
    v.scroll_to_item(far);
    frame(&mut v);
    assert_eq!(v.anchor().0, far);
    host.pictures.lock().unwrap().insert("scene".into(), 100.0);
    v.site_changed();
    frame(&mut v);
    assert_eq!(v.anchor(), (far, 0.0), "the item at the top stays at the top");
}

#[test]
fn a_viewport_starting_inside_a_block_stays_at_its_place_in_the_picture() {
    let mut src = String::from(":::scene\n\n");
    for i in 0..100 {
        src.push_str(&format!("Line {i} of it.\n\n"));
    }
    src.push_str(":::\n\nAfter.\n");
    let (mut v, host) = set_up(&src, 700.0);
    frame(&mut v);
    let mid = item_of(&v, "Line 50 of it.");
    v.scroll_to_item(mid);
    frame(&mut v);
    host.pictures.lock().unwrap().insert("scene".into(), 1000.0);
    v.site_changed();
    frame(&mut v);
    let opener = item_of(&v, ":::scene");
    let (at, off) = v.anchor();
    assert_eq!(at, opener);
    assert!((300.0..800.0).contains(&off), "about half way: {off}");
}

#[test]
fn a_jump_to_a_heading_in_a_picture_goes_to_the_picture() {
    let (mut v, host) = set_up(DOC, 700.0);
    host.pictures.lock().unwrap().insert("scene".into(), 300.0);
    v.site_changed();
    frame(&mut v);
    v.scroll_to_item(item_of(&v, "## Inside"));
    frame(&mut v);
    assert_eq!(v.anchor().0, item_of(&v, ":::scene"));
}

#[test]
fn a_selection_holding_the_block_paints_over_the_picture() {
    let (mut v, host) = set_up(DOC, 700.0);
    host.pictures.lock().unwrap().insert("scene".into(), 300.0);
    v.site_changed();
    frame(&mut v);
    let opener = item_of(&v, ":::scene");
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut g = Geometry {
        view: &mut v,
        shaper: &mut f,
        images: &NoImages,
    };
    let rects = g.selection_rects(opener, 0..DOC.len());
    assert_eq!(rects.len(), 1);
    assert!((rects[0].height() - 300.0).abs() < 0.01);
}

#[test]
fn pages_keep_the_picture_heights_and_the_invariants() {
    let mut src = String::new();
    for i in 0..400 {
        src.push_str(&format!(":::scene\n\nBlock {i}.\n\n:::\n\nProse {i}.\n\n"));
    }
    let doc: Arc<dyn markise_doc::FormatDoc> = Arc::new(markise_md::parse(&src));
    let model = Arc::new(DocModel::build_with(&*doc, None, Paging::Paged { tops: 16 }));
    let mut v = View::new(doc, model, Theme::light(), 700.0, 2.0);
    let host = Arc::new(Host::default());
    host.pictures.lock().unwrap().insert("scene".into(), 40.0);
    v.set_block_views(Some(host.clone()));
    frame(&mut v);
    v.site_changed();
    frame(&mut v);
    let at_top = v.total_height();
    // far down and back: everything evicted comes back the same
    v.scroll_to_y(v.total_height() * 0.8);
    frame(&mut v);
    frame(&mut v);
    v.scroll_to_y(0.0);
    frame(&mut v);
    frame(&mut v);
    assert!(v.model().items.iter().any(|it| it.is_page()), "a paged document");
    assert!(
        (v.total_height() - at_top).abs() < at_top * 0.5,
        "{} {at_top}",
        v.total_height()
    );
    let opener = v
        .model()
        .items
        .iter()
        .position(|it| it.kind == ItemKind::ContainerMarker { close: false });
    assert!(picture_at(&mut v, opener.unwrap()).is_some());
    assert_eq!(v.height(opener.unwrap() + 1), 0.0);
}

#[test]
fn a_nested_block_in_a_picture_is_inside_it() {
    let src = "::::scene\n\n:::inner\n\nDeep.\n\n:::\n\n::::\n\nAfter.\n";
    let (mut v, host) = set_up(src, 700.0);
    host.pictures.lock().unwrap().insert("scene".into(), 300.0);
    host.pictures.lock().unwrap().insert("inner".into(), 50.0);
    v.site_changed();
    frame(&mut v);
    assert_eq!(v.height(item_of(&v, ":::inner")), 0.0);
    assert_eq!(v.height(item_of(&v, "Deep.")), 0.0);
    assert!(picture_of(&mut v, "::::scene").is_some());
}

#[test]
fn the_switch_goes_through_what_a_block_can_show() {
    use BlockViewMode::{Markdown, Native, Site};
    // a view and something to draw with: all three, in order
    assert_eq!(Native.next_for(true, true), Site);
    assert_eq!(Site.next_for(true, true), Markdown);
    assert_eq!(Markdown.next_for(true, true), Native);
    // no view: Native is the site's picture, and is skipped
    assert_eq!(Native.next_for(false, true), Markdown);
    assert_eq!(Site.next_for(false, true), Markdown);
    assert_eq!(Markdown.next_for(false, true), Site);
    // nothing to draw with: as before this phase
    assert_eq!(Native.next_for(true, false), Markdown);
    assert_eq!(
        Site.next_for(true, false),
        Native,
        "shown as Markdown: the helper went away"
    );
    assert_eq!(Markdown.next_for(true, false), Native);
    // neither: one state
    assert_eq!(Markdown.next_for(false, false), Markdown);
    assert_eq!(Native.next_for(false, false), Markdown);
}

#[test]
fn the_site_state_gives_a_block_with_a_view_its_picture_too() {
    let (mut v, host) = set_up(DOC, 700.0);
    host.pictures.lock().unwrap().insert("viewed".into(), 200.0);
    host.pictures.lock().unwrap().insert("scene".into(), 100.0);
    v.site_changed();
    frame(&mut v);
    assert!(picture_of(&mut v, ":::viewed").is_none(), "its own view by default");
    v.set_block_view_mode(BlockViewMode::Site);
    frame(&mut v);
    assert_eq!(picture_of(&mut v, ":::viewed"), Some((700.0, 200.0)));
    assert_eq!(picture_of(&mut v, ":::scene"), Some((700.0, 100.0)));
    // one block alone
    let opener = DOC.find(":::viewed").unwrap();
    v.set_block_modes(BlockViewMode::Native, vec![(opener, BlockViewMode::Site)]);
    frame(&mut v);
    assert_eq!(picture_of(&mut v, ":::viewed"), Some((700.0, 200.0)));
}

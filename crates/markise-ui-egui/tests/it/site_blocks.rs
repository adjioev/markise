//! Blocks drawn by their site's code, on the real app (ADR 0023), with a helper played over pipes: no
//! web engine, no window. What shows, when a helper starts, what comes around a picture.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::Event;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_doc::Selection;
use markise_layout::Prim;
use markise_ui_egui::MarkiseApp;
use markise_ui_egui::viewer::HelperSource;
use markise_web_render::client::{Spawned, Spawner};
use markise_web_render::proto::{self, FailKind, FromHelper, PROTOCOL, RenderRequest, Rendered, ToHelper};

use crate::block_views::{Fixture, caret, shown, text};
use crate::common;

type H = Harness<'static, MarkiseApp>;

/// A helper played here: it draws a plain picture of the height asked for, or fails, or holds on.
#[derive(Default)]
struct Played {
    spawned: AtomicUsize,
    renders: Mutex<Vec<RenderRequest>>,
    fail: Mutex<Option<String>>,
    hold: AtomicBool,
    height: AtomicU32,
}

impl Played {
    fn source(self: &Arc<Self>) -> HelperSource {
        let me = self.clone();
        Arc::new(move || Some(Box::new(Spawn(me.clone())) as Box<dyn Spawner>))
    }

    fn count(&self) -> usize {
        self.renders.lock().unwrap().len()
    }
}

struct Spawn(Arc<Played>);

impl Spawner for Spawn {
    fn spawn(&mut self) -> std::io::Result<Spawned> {
        self.0.spawned.fetch_add(1, Ordering::SeqCst);
        let (mut from_client, to_helper) = std::io::pipe()?;
        let (from_helper, mut to_client) = std::io::pipe()?;
        let played = self.0.clone();
        std::thread::spawn(move || {
            let hello = FromHelper::Hello {
                protocol: PROTOCOL,
                available: true,
            };
            let _ = proto::write_frame(&mut to_client, &hello.encode(), &[]);
            while let Ok((m, _)) = proto::read_to_helper(&mut from_client) {
                let ToHelper::Render(r) = m else { continue };
                played.renders.lock().unwrap().push(r.clone());
                while played.hold.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                if let Some(reason) = played.fail.lock().unwrap().clone() {
                    let failed = FromHelper::Failed {
                        id: r.id,
                        kind: FailKind::Script,
                        reason,
                    };
                    let _ = proto::write_frame(&mut to_client, &failed.encode(), &[]);
                    continue;
                }
                let css = played.height.load(Ordering::SeqCst).max(1);
                let (w, h) = (
                    (r.width as f64 * r.scale).ceil() as u32,
                    (css as f64 * r.scale).ceil() as u32,
                );
                let rendered = Rendered {
                    id: r.id,
                    css_height: css as f64,
                    px_width: w,
                    px_height: h,
                    ms: 1.0,
                };
                let bytes = vec![200u8; (w * h * 4) as usize];
                let _ = proto::write_frame(&mut to_client, &FromHelper::Rendered(rendered).encode(), &bytes);
            }
        });
        Ok(Spawned {
            to: Box::new(to_helper),
            from: Box::new(from_helper),
            kill: Box::new(|| {}),
        })
    }
}

const DOC: &str =
    "# Doc\n\nBefore.\n\n:::plain\n\nDrawn by its site.\n\n:::\n\nBetween.\n\n:::tip\n\nHas a view.\n\n:::\n\nAfter.\n";

fn at(needle: &str) -> usize {
    DOC.find(needle).unwrap()
}

fn played(height: u32) -> Arc<Played> {
    let p = Arc::new(Played::default());
    p.height.store(height, Ordering::SeqCst);
    p
}

/// The picture on screen, if any: its rectangle in the item.
fn picture(h: &mut H, offset: usize) -> Option<egui::Rect> {
    let v = h.state_mut().viewer().unwrap();
    let i = v.ed.view.model().item_at_offset(offset);
    let l = v.ed.view.cache_peek(i)?;
    l.prims.iter().find_map(|p| match p {
        Prim::Picture { rect, .. } => Some(*rect),
        _ => None,
    })
}

fn wait_for(h: &mut H, what: &str, mut f: impl FnMut(&mut H) -> bool) {
    let t = Instant::now();
    while !f(h) {
        assert!(t.elapsed() < common::HANG_GUARD, "{what}");
        h.step();
        std::thread::sleep(Duration::from_millis(2));
    }
    h.step();
}

#[test]
fn a_block_with_no_view_shows_its_picture_and_the_file_stays() {
    let f = Fixture::new(DOC);
    let p = played(120);
    let mut h = f.ready_with(Some(p.source()));
    wait_for(&mut h, "its picture", |h| picture(h, at(":::plain")).is_some());
    let r = picture(&mut h, at(":::plain")).unwrap();
    assert_eq!(r.height(), 120.0);
    let s = shown(&mut h);
    assert!(!s.contains("Drawn by its site"), "a picture instead: {s}");
    assert!(s.contains("Has a view"), "a native block is as it was: {s}");
    assert_eq!(text(&h), DOC);
    assert!(!h.state().is_dirty());
    let renders = p.renders.lock().unwrap();
    assert_eq!(renders.len(), 1, "only the block with no view");
    let q = &renders[0];
    assert!(q.html.contains(r#"data-component="plain""#) && q.html.contains("Drawn by its site."));
    assert!(q.html.contains(r#"data-field="body""#));
    assert!(q.vars.iter().any(|(k, _)| k == "--markise-text"), "{:?}", q.vars);
}

#[test]
fn no_helper_starts_without_a_package_accepted_or_a_block_that_needs_one() {
    let f = Fixture::new(DOC);
    let p = played(100);
    let mut h = f.opened_with(Some(p.source()));
    common::run(&mut h);
    assert_eq!(p.spawned.load(Ordering::SeqCst), 0, "before consent");
    drop(h);

    let only_view = ":::tip\n\nHas a view.\n\n:::\n";
    let f = Fixture::new(only_view);
    let p = played(100);
    let mut h = f.ready_with(Some(p.source()));
    common::run(&mut h);
    assert_eq!(p.spawned.load(Ordering::SeqCst), 0, "no block needs it");
    assert!(shown(&mut h).contains("Has a view"));
}

#[test]
fn with_no_helper_at_all_a_block_stays_its_card_with_no_error() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    let s = shown(&mut h);
    assert!(s.contains("plain") && s.contains("Drawn by its site"), "{s}");
    assert!(h.state().banner_text().is_none());
}

#[test]
fn while_a_picture_is_pending_the_card_shows() {
    let f = Fixture::new(DOC);
    let p = played(120);
    p.hold.store(true, Ordering::SeqCst);
    let mut h = f.opened_with(Some(p.source()));
    h.get_by_label("Use Package").click();
    wait_for(&mut h, "the render starts", |_| p.count() == 1);
    assert!(shown(&mut h).contains("Drawn by its site"), "the card meanwhile");
    p.hold.store(false, Ordering::SeqCst);
    wait_for(&mut h, "its picture", |h| picture(h, at(":::plain")).is_some());
}

#[test]
fn typing_elsewhere_redraws_nothing() {
    let f = Fixture::new(DOC);
    let p = played(120);
    let mut h = f.ready_with(Some(p.source()));
    wait_for(&mut h, "its picture", |h| picture(h, at(":::plain")).is_some());
    let before = p.count();
    caret(&mut h, at("Between."));
    for t in ["a", "b", "c"] {
        h.event(Event::Text(t.into()));
        common::run(&mut h);
    }
    assert!(text(&h).contains("abcBetween."));
    // and text above moves it, which is no change to the block
    caret(&mut h, at("Before."));
    h.event(Event::Text("x".into()));
    common::run(&mut h);
    assert_eq!(p.count(), before);
    let opener = text(&h).find(":::plain").unwrap();
    assert!(picture(&mut h, opener).is_some(), "it moved with its block");
}

#[test]
fn saving_the_form_refreshes_the_picture_and_the_old_one_stays_meanwhile() {
    let f = Fixture::new(DOC);
    let p = played(120);
    let mut h = f.ready_with(Some(p.source()));
    wait_for(&mut h, "its picture", |h| picture(h, at(":::plain")).is_some());
    h.get_all_by_label("Edit Block: plain").next().unwrap().click();
    common::run(&mut h);
    assert!(h.state().block_form().is_some(), "the pencil stays on a picture");
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("!");
    common::run(&mut h);
    p.hold.store(true, Ordering::SeqCst);
    p.height.store(200, Ordering::SeqCst);
    let before = p.count();
    let ctx = h.ctx.clone();
    h.state_mut().block_form_save(&ctx);
    wait_for(&mut h, "the new render", |_| p.count() == before + 1);
    assert!(text(&h).contains("Drawn by its site.!"));
    // the old one, with no flicker (the panel going away changes the width, and it scales with it)
    assert!(picture(&mut h, at(":::plain")).is_some());
    assert!(!shown(&mut h).contains("Drawn by its site"));
    p.hold.store(false, Ordering::SeqCst);
    wait_for(&mut h, "the new one", |h| {
        picture(h, at(":::plain")).is_some_and(|r| r.height() == 200.0)
    });
    assert!(p.renders.lock().unwrap().last().unwrap().html.contains("site.!"));
}

#[test]
fn a_failure_keeps_the_card_with_a_quiet_reason_and_is_not_tried_again() {
    let f = Fixture::new(DOC);
    let p = played(120);
    *p.fail.lock().unwrap() = Some("The block threw.".to_string());
    let mut h = f.ready_with(Some(p.source()));
    let label = "Not drawn by its site's code: The block threw.";
    wait_for(&mut h, "its reason", |h| h.query_by_label(label).is_some());
    assert!(shown(&mut h).contains("Drawn by its site"), "the card");
    assert!(h.state().banner_text().is_none(), "nothing louder");
    common::run(&mut h);
    let renders = p.count();
    caret(&mut h, at("Between."));
    h.event(Event::Text("a".into()));
    common::run(&mut h);
    assert_eq!(p.count(), renders);
}

#[test]
fn the_caret_in_a_block_shows_what_it_holds_and_undo_comes_back_to_the_picture() {
    let f = Fixture::new(DOC);
    let p = played(120);
    let mut h = f.ready_with(Some(p.source()));
    wait_for(&mut h, "its picture", |h| picture(h, at(":::plain")).is_some());
    // what a search or undo does: the selection goes in
    caret(&mut h, at("Drawn"));
    assert!(picture(&mut h, at(":::plain")).is_none());
    assert!(shown(&mut h).contains("Drawn by its site"));
    h.event(Event::Text("q".into()));
    common::run(&mut h);
    assert!(text(&h).contains("qDrawn"));
    let between = text(&h).find("Between").unwrap();
    caret(&mut h, between);
    wait_for(&mut h, "its picture", |h| {
        let opener = text(h).find(":::plain").unwrap();
        picture(h, opener).is_some()
    });
    assert!(p.renders.lock().unwrap().last().unwrap().html.contains("qDrawn"));
    let v = h.state_mut().viewer().unwrap();
    v.ed.edit(|s| s.undo());
    common::run(&mut h);
    assert_eq!(text(&h), DOC);
    caret(&mut h, at("Between"));
    wait_for(&mut h, "the picture of the text as it was", |h| {
        picture(h, at(":::plain")).is_some()
    });
}

#[test]
fn a_click_on_a_picture_lands_before_or_after_it_and_keys_go_past_it() {
    let f = Fixture::new(DOC);
    let p = played(300);
    let mut h = f.ready_with(Some(p.source()));
    wait_for(&mut h, "its picture", |h| picture(h, at(":::plain")).is_some());
    let r = h.state().viewer_item_rect(at(":::plain")).expect("on screen");
    let click = |h: &mut H, y: f32| {
        let pos = egui::pos2(r.left() + 40.0, y);
        h.hover_at(pos);
        h.drag_at(pos);
        h.drop_at(pos);
        common::run(h);
        h.state_mut().viewer().unwrap().ed.session.selection().primary().focus
    };
    let above = click(&mut h, r.top() + 60.0);
    assert!(above.offset <= at(":::plain"), "{above:?}");
    let below = click(&mut h, r.bottom() - 40.0);
    assert!(below.offset >= at("\n\nBetween"), "{below:?}");
    assert!(picture(&mut h, at(":::plain")).is_some(), "still a picture");
    assert_eq!(text(&h), DOC);
    // from below, up past it and into what is above
    caret(&mut h, at("Between"));
    let inside = at(":::plain") + 1..at("\n\nBetween");
    for _ in 0..3 {
        h.key_press(egui::Key::ArrowUp);
        common::run(&mut h);
        let now = h.state_mut().viewer().unwrap().ed.session.selection().primary().focus;
        assert!(!inside.contains(&now.offset), "{now:?}");
    }
    assert!(picture(&mut h, at(":::plain")).is_some());
}

#[test]
fn a_selection_over_a_picture_holds_the_block_whole_and_can_delete_it() {
    let f = Fixture::new(DOC);
    let p = played(120);
    let mut h = f.ready_with(Some(p.source()));
    wait_for(&mut h, "its picture", |h| picture(h, at(":::plain")).is_some());
    let v = h.state_mut().viewer().unwrap();
    v.ed.set_selection(Selection::range(at("Before."), at("Between")));
    v.ed.refresh_focus();
    v.request_refocus();
    common::run(&mut h);
    assert!(picture(&mut h, at(":::plain")).is_some());
    h.key_press(egui::Key::Backspace);
    common::run(&mut h);
    assert!(!text(&h).contains(":::plain"), "{}", text(&h));
    let v = h.state_mut().viewer().unwrap();
    v.ed.edit(|s| s.undo());
    common::run(&mut h);
    assert_eq!(text(&h), DOC);
}

#[test]
fn the_markdown_state_is_a_card_and_no_helper_time() {
    let f = Fixture::new(DOC);
    let p = played(120);
    let mut h = f.ready_with(Some(p.source()));
    wait_for(&mut h, "its picture", |h| picture(h, at(":::plain")).is_some());
    let before = p.count();
    h.state_mut().viewer().unwrap().ed.switch_block_view(at(":::plain"));
    common::run(&mut h);
    assert!(picture(&mut h, at(":::plain")).is_none());
    assert!(shown(&mut h).contains("Drawn by its site"));
    assert_eq!(p.count(), before);
}

#[test]
fn another_theme_or_zoom_is_another_picture() {
    let f = Fixture::new(DOC);
    let p = played(120);
    let mut h = f.ready_with(Some(p.source()));
    wait_for(&mut h, "its picture", |h| picture(h, at(":::plain")).is_some());
    let before = p.count();
    let first = p.renders.lock().unwrap().last().unwrap().clone();
    h.state_mut().set_zoom(1.5);
    wait_for(&mut h, "a new render for the scale", |_| p.count() > before);
    common::run(&mut h);
    let second = p.renders.lock().unwrap().last().unwrap().clone();
    assert!(second.scale > first.scale, "{} {}", second.scale, first.scale);
    assert!(picture(&mut h, at(":::plain")).is_some());
}

#[test]
fn a_document_at_rest_asks_for_no_frames() {
    let f = Fixture::new(DOC);
    let p = played(120);
    let mut h = f.ready_with(Some(p.source()));
    wait_for(&mut h, "its picture", |h| picture(h, at(":::plain")).is_some());
    common::run(&mut h);
    let renders = p.count();
    h.step();
    assert!(!h.ctx.has_requested_repaint(), "no frame asked for");
    assert_eq!(p.count(), renders);
}

#[test]
fn stopping_the_package_goes_back_to_cards() {
    let f = Fixture::new(DOC);
    let p = played(120);
    let mut h = f.ready_with(Some(p.source()));
    wait_for(&mut h, "its picture", |h| picture(h, at(":::plain")).is_some());
    h.state_mut()
        .block_package_action(markise_ui_egui::package_ui::PackageAction::Forget);
    common::run(&mut h);
    assert!(h.state().block_package().is_none());
    assert!(picture(&mut h, at(":::plain")).is_none());
    assert!(shown(&mut h).contains("Drawn by its site"));
}

fn button_says(h: &H, what: &str) -> bool {
    h.query_by_label(what).is_some()
}

#[test]
fn a_picture_shows_as_picked_up_on_a_press_and_moves() {
    use egui::{Modifiers, PointerButton, pos2};
    let f = Fixture::new(DOC);
    let p = played(120);
    let mut h = f.ready_with(Some(p.source()));
    wait_for(&mut h, "the picture", |h| picture(h, at(":::plain")).is_some());
    let item = h.state().viewer_item_rect(at(":::plain")).unwrap();
    let from = pos2(item.left() + 40.0, item.center().y);
    h.hover_at(from);
    h.step();
    h.step();
    let grip = h.get_by_label("Move Block").rect().center();
    for i in 1..=10 {
        h.hover_at(from + (grip - from) * (i as f32 / 10.0));
        h.step();
    }
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.event(button(grip, true));
    h.step();
    h.step();
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Grabbing);
    let s = h.state_mut().viewer().unwrap().block_drag_shown().cloned().unwrap();
    let outline = s.outline.unwrap();
    assert!(outline.height() >= 120.0 && outline.top() <= grip.y, "{outline:?}");
    assert!(!s.moves);
    let to = h.state().viewer_item_rect(at("After.")).unwrap().left_top();
    let to = pos2(grip.x, to.y);
    for i in 1..=8 {
        h.hover_at(grip + (to - grip) * (i as f32 / 8.0));
        h.step();
    }
    assert!(h.state_mut().viewer().unwrap().block_drag_shown().unwrap().moves);
    h.event(button(to, false));
    h.step();
    h.step();
    common::run(&mut h);
    let t = text(&h);
    assert!(t.find(":::tip").unwrap() < t.find(":::plain").unwrap(), "{t}");
}

#[test]
fn a_block_with_a_view_goes_native_site_markdown_and_back() {
    let f = Fixture::new(DOC);
    let p = played(120);
    let mut h = f.ready_with(Some(p.source()));
    common::run(&mut h);
    let tip = at(":::tip");
    let hello = at("Has a view");
    let is_native = |h: &mut H| h.state_mut().viewer().unwrap().ed.view.native_block(hello).is_some();
    assert!(is_native(&mut h));
    assert!(button_says(&h, "Show as on the Site: tip"));
    h.get_by_label("Show as on the Site: tip").click();
    wait_for(&mut h, "its picture", |h| picture(h, tip).is_some());
    assert!(!is_native(&mut h));
    assert!(!shown(&mut h).contains("Has a view"));
    // over a picture the buttons are for the pointer; the command needs none
    let v = h.state_mut().viewer().unwrap();
    assert_eq!(v.ed.next_block_view(tip), markise_layout::BlockViewMode::Markdown);
    v.ed.switch_block_view(tip);
    common::run(&mut h);
    assert!(picture(&mut h, tip).is_none());
    assert!(!is_native(&mut h));
    assert!(shown(&mut h).contains("Has a view"), "the card");
    assert!(button_says(&h, "Show Native View: tip"));
    h.get_by_label("Show Native View: tip").click();
    common::run(&mut h);
    assert!(is_native(&mut h));
    assert_eq!(text(&h), DOC);
}

#[test]
fn a_block_with_no_view_skips_native() {
    let f = Fixture::new(DOC);
    let p = played(120);
    let mut h = f.ready_with(Some(p.source()));
    wait_for(&mut h, "its picture", |h| picture(h, at(":::plain")).is_some());
    let v = h.state_mut().viewer().unwrap();
    assert_eq!(
        v.ed.next_block_view(at(":::plain")),
        markise_layout::BlockViewMode::Markdown
    );
    v.ed.switch_block_view(at(":::plain"));
    common::run(&mut h);
    assert!(picture(&mut h, at(":::plain")).is_none());
    assert!(button_says(&h, "Show as on the Site: plain"));
    h.get_by_label("Show as on the Site: plain").click();
    wait_for(&mut h, "its picture again", |h| picture(h, at(":::plain")).is_some());
    let (mode, own) = h.state_mut().viewer().unwrap().ed.block_view_state();
    assert_eq!(mode, markise_layout::BlockViewMode::Native);
    assert!(own.is_empty(), "as the document shows it: no state of its own {own:?}");
}

#[test]
fn with_nothing_to_draw_with_the_switch_has_no_site_state() {
    let f = Fixture::new(DOC);
    let mut h = f.ready();
    assert!(button_says(&h, "Show Markdown: tip"));
    assert!(!button_says(&h, "Show as on the Site: tip"));
    assert!(
        !button_says(&h, "Show as on the Site: plain"),
        "no view and no helper: no switch"
    );
    assert!(!button_says(&h, "Show Markdown: plain"));
    let v = h.state_mut().viewer().unwrap();
    v.ed.switch_all_block_views();
    assert_eq!(v.ed.block_view_state().0, markise_layout::BlockViewMode::Markdown);
}

#[test]
fn the_whole_document_goes_through_the_states_and_a_block_on_its_own_follows() {
    let f = Fixture::new(DOC);
    let p = played(120);
    let mut h = f.ready_with(Some(p.source()));
    wait_for(&mut h, "its picture", |h| picture(h, at(":::plain")).is_some());
    let v = h.state_mut().viewer().unwrap();
    v.ed.switch_block_view(at(":::plain")); // Markdown on its own
    v.ed.switch_all_block_views();
    assert_eq!(
        v.ed.block_view_state(),
        (markise_layout::BlockViewMode::Site, Vec::new())
    );
    wait_for(&mut h, "both pictures", |h| {
        picture(h, at(":::plain")).is_some() && picture(h, at(":::tip")).is_some()
    });
    let v = h.state_mut().viewer().unwrap();
    v.ed.switch_all_block_views();
    common::run(&mut h);
    assert!(picture(&mut h, at(":::plain")).is_none() && picture(&mut h, at(":::tip")).is_none());
    let v = h.state_mut().viewer().unwrap();
    v.ed.switch_all_block_views();
    common::run(&mut h);
    assert!(
        picture(&mut h, at(":::plain")).is_some(),
        "its picture is its native state"
    );
    let v = h.state_mut().viewer().unwrap();
    assert!(v.ed.view.native_block(at("Has a view")).is_some());
}

#[test]
fn asking_for_a_picture_from_inside_a_block_leaves_it() {
    let f = Fixture::new(DOC);
    let p = played(120);
    let mut h = f.ready_with(Some(p.source()));
    common::run(&mut h);
    caret(&mut h, at("Has a view") + 3);
    let v = h.state_mut().viewer().unwrap();
    v.ed.switch_block_view(at(":::tip"));
    let focus = v.ed.session.selection().primary().focus;
    assert!(focus.offset >= at("\n\nAfter"), "{focus:?}");
    wait_for(&mut h, "its picture", |h| picture(h, at(":::tip")).is_some());
    assert_eq!(text(&h), DOC, "a caret moved, nothing else");
}

#[test]
fn the_choice_is_remembered_for_the_document() {
    let f = Fixture::new(DOC);
    let p = played(120);
    let mut h = f.ready_with(Some(p.source()));
    common::run(&mut h);
    h.get_by_label("Show as on the Site: tip").click();
    wait_for(&mut h, "its picture", |h| picture(h, at(":::tip")).is_some());
    let other = f.doc.with_file_name("other.md");
    std::fs::write(&other, "Other.\n").unwrap();
    h.state_mut().open(other);
    common::wait_until(&mut h, "the other", |a| !a.is_loading());
    common::run(&mut h);
    h.state_mut().open(f.doc.clone());
    common::wait_until(&mut h, "back", |a| a.document_path() == Some(&f.doc) && !a.is_loading());
    common::run(&mut h);
    wait_for(&mut h, "its picture again", |h| picture(h, at(":::tip")).is_some());
    let mode = h.state_mut().viewer().unwrap().ed.view.block_mode_of(at(":::tip"));
    assert_eq!(mode, markise_layout::BlockViewMode::Site);
}

#[test]
fn another_block_at_the_same_place_does_not_show_the_first_ones_picture() {
    let f = Fixture::new(DOC);
    let p = played(120);
    let mut h = f.ready_with(Some(p.source()));
    wait_for(&mut h, "its picture", |h| picture(h, at(":::plain")).is_some());
    p.hold.store(true, Ordering::SeqCst);
    let before = p.count();
    // the whole of it goes and another comes, at the same offset
    let (start, end) = (at(":::plain"), at("\n\nBetween"));
    let v = h.state_mut().viewer().unwrap();
    v.ed.set_selection(Selection::range(start, end));
    v.ed.edit(|s| s.insert_text(":::plain\n\nSomething else entirely.\n\n:::"))
        .unwrap();
    v.ed.set_selection(Selection::caret(0));
    v.ed.refresh_focus();
    wait_for(&mut h, "its render", |_| p.count() > before);
    assert!(
        picture(&mut h, start).is_none(),
        "the card, not the other block's picture"
    );
    assert!(shown(&mut h).contains("Something else entirely"));
    p.hold.store(false, Ordering::SeqCst);
    wait_for(&mut h, "its own picture", |h| picture(h, start).is_some());
}

#[test]
fn text_above_a_block_being_redrawn_does_not_lose_its_picture() {
    let f = Fixture::new(DOC);
    let p = played(120);
    let mut h = f.ready_with(Some(p.source()));
    wait_for(&mut h, "its picture", |h| picture(h, at(":::plain")).is_some());
    p.hold.store(true, Ordering::SeqCst);
    let before = p.count();
    // an edit in the block and one above it, with no frame between: the block moved and changed
    let inner = at("Drawn");
    let v = h.state_mut().viewer().unwrap();
    v.ed.set_selection(Selection::caret(inner));
    v.ed.edit(|s| s.insert_text("z")).unwrap();
    v.ed.set_selection(Selection::caret(0));
    v.ed.edit(|s| s.insert_text("ab")).unwrap();
    v.ed.refresh_focus();
    wait_for(&mut h, "its render", |_| p.count() > before);
    assert!(
        picture(&mut h, at(":::plain") + 2).is_some(),
        "the older picture, with no flicker"
    );
    p.hold.store(false, Ordering::SeqCst);
    common::run(&mut h);
}

#[test]
fn a_picture_over_the_graphics_limit_is_a_card_with_its_reason() {
    let f = Fixture::new(DOC);
    let p = played(3000);
    let mut h = f.opened_with(Some(p.source()));
    h.input_mut().max_texture_side = Some(2048);
    h.get_by_label("Use Package").click();
    h.step();
    common::wait_until(&mut h, "the install", |a| a.block_package().is_some());
    wait_for(&mut h, "its reason", |h| {
        h.input_mut().max_texture_side = Some(2048);
        h.query_by_label_contains("larger than").is_some()
    });
    assert!(picture(&mut h, at(":::plain")).is_none());
    assert!(shown(&mut h).contains("Drawn by its site"));
}

#[test]
fn switching_one_block_leaves_a_caret_in_another_where_it_is() {
    let f = Fixture::new(DOC);
    let p = played(120);
    let mut h = f.ready_with(Some(p.source()));
    common::run(&mut h);
    let inside = at("Drawn") + 3;
    caret(&mut h, inside);
    let v = h.state_mut().viewer().unwrap();
    v.ed.switch_block_view(at(":::tip"));
    assert_eq!(
        v.ed.session.selection().primary().focus.offset,
        inside,
        "that block is not this one"
    );
    common::run(&mut h);
    assert!(picture(&mut h, at(":::tip")).is_some());
    assert_eq!(
        h.state_mut()
            .viewer()
            .unwrap()
            .ed
            .session
            .selection()
            .primary()
            .focus
            .offset,
        inside
    );
}

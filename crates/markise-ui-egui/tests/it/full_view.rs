//! The full-window view of diagrams and images (issue #174): opening by button and double
//! click, a plain click unchanged, closing and focus, zoom and pan, modal command routing, the
//! document untouched, full-resolution images, an untouched view idle.

use crate::common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Vec2, pos2, vec2};
use egui_kittest::Harness;
use markise_ui_egui::menu::{MenuAction, Trigger};
use markise_ui_egui::{AppOptions, MarkiseApp};

type H = Harness<'static, MarkiseApp>;

/// A flowchart wider than the column (scaled down in the document).
const WIDE: &str = "# T\n\nbefore\n\n```mermaid\nflowchart LR\nA[Some rather long first label] --> B[Some rather long second label]\nB --> C[Some rather long third label]\nC --> D[Some rather long fourth label]\nD --> E[Some rather long fifth label]\n```\n\nafter\n";

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-fullview-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn app_of(text: &str, name: &str) -> MarkiseApp {
    let p = dir(name).join("doc.md");
    std::fs::write(&p, text).unwrap();
    MarkiseApp::new(AppOptions {
        open: vec![p],
        ..common::no_watch()
    })
}

fn build(app: MarkiseApp) -> H {
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        // short simulated frames: a double click is two presses within the double-click time
        .with_step_dt(0.02)
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app);
    h.input_mut().max_texture_side = Some(8192);
    let t = Instant::now();
    while !(h.state().model().is_some() && !h.state().is_loading()) {
        assert!(t.elapsed() < common::HANG_GUARD, "timed out loading");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    common::settle(&mut h);
    h.run_steps(3);
    h
}

fn harness(text: &str, name: &str) -> H {
    build(app_of(text, name))
}

/// A document showing `img` (a PNG of `size`) and the harness, the image loaded.
fn image_harness(md: &str, size: [u32; 2], name: &str) -> H {
    let d = dir(name);
    image::RgbaImage::from_pixel(size[0], size[1], image::Rgba([200, 30, 40, 255]))
        .save(d.join("pic.png"))
        .unwrap();
    let p = d.join("doc.md");
    std::fs::write(&p, md).unwrap();
    let mut h = build(MarkiseApp::new(AppOptions {
        open: vec![p],
        ..common::no_watch()
    }));
    common::settle(&mut h);
    h.run_steps(3);
    h
}

fn text(h: &H) -> String {
    h.state().document_text().unwrap()
}

fn key(h: &mut H, k: Key) {
    h.event(Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    h.step();
    h.step();
}

fn button(h: &mut H, pos: Pos2, pressed: bool) {
    h.event(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    });
}

fn click(h: &mut H, pos: Pos2) {
    h.hover_at(pos);
    h.step();
    button(h, pos, true);
    h.step();
    button(h, pos, false);
    h.step();
    h.step();
}

fn double_click(h: &mut H, pos: Pos2) {
    h.hover_at(pos);
    h.step();
    for _ in 0..2 {
        button(h, pos, true);
        h.step();
        button(h, pos, false);
        h.step();
    }
    h.step();
    h.step();
}

fn drag(h: &mut H, from: Pos2, by: Vec2) {
    h.hover_at(from);
    h.step();
    button(h, from, true);
    h.step();
    let to = from + by;
    h.hover_at(to);
    h.step();
    button(h, to, false);
    h.step();
    h.step();
}

fn wheel(h: &mut H, at: Pos2, lines: f32) {
    h.hover_at(at);
    h.step();
    h.event(Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: vec2(0.0, lines),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
        modifiers: Modifiers::NONE,
    });
    h.step();
    h.step();
}

fn item_rect(h: &H, what: &str) -> egui::Rect {
    let at = h.state().document_text().unwrap().find(what).unwrap();
    h.state().viewer_item_rect(at).expect("item visible")
}

/// Hovers the diagram until it is rendered and its controls show.
fn diagram_tools(h: &mut H) -> markise_ui_egui::viewer::ShownTools {
    let t = Instant::now();
    loop {
        let r = item_rect(h, "```mermaid");
        h.hover_at(r.center());
        h.step();
        h.step();
        if let Some(s) = h
            .state_mut()
            .viewer()
            .unwrap()
            .code_tools_shown()
            .iter()
            .find(|s| s.expand.is_some())
        {
            return *s;
        }
        assert!(t.elapsed() < common::HANG_GUARD, "the diagram never rendered");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn open_by_button(h: &mut H) {
    let tools = diagram_tools(h);
    click(h, tools.expand.unwrap().center());
    assert!(h.state().full_view().is_some(), "the button opens the view");
}

fn surface_focused(h: &mut H) -> bool {
    let id = h.state_mut().viewer().unwrap().surface_id().unwrap();
    h.ctx.memory(|m| m.has_focus(id))
}

#[test]
fn the_button_opens_the_view_fitted_to_the_window() {
    let mut h = harness(WIDE, "button");
    open_by_button(&mut h);
    let t = h.state().full_view().unwrap();
    assert_eq!(t.scale(), t.fit());
    assert!(t.offset().x >= 0.0 && t.offset().y >= 0.0, "inside the window: {t:?}");
    let laid = h.state().full_view_layouts().unwrap();
    assert!(laid > 0, "laid out for the shown scale");
}

#[test]
fn a_double_click_opens_the_view_and_a_plain_click_does_not() {
    let mut h = harness(WIDE, "double");
    diagram_tools(&mut h);
    let center = item_rect(&h, "```mermaid").center();
    click(&mut h, center);
    assert!(h.state().full_view().is_none(), "a plain click stays in the document");
    let caret = h.state_mut().viewer().unwrap().ed.session.selection().clone();
    let after = WIDE.find("after").unwrap();
    assert!(
        caret.primary().focus.offset <= after,
        "the click placed the caret by the diagram: {caret:?}"
    );
    // (out of the previous click's multi-click time)
    h.run_steps(30);
    double_click(&mut h, center);
    assert!(h.state().full_view().is_some(), "a double click opens the view");
    assert_eq!(
        h.state_mut().viewer().unwrap().ed.session.selection().clone(),
        caret,
        "the double click selected nothing"
    );
}

/// The hover controls sit in a row under the picture, the screenshot's language label no longer
/// covers the diagram.
#[test]
fn the_controls_do_not_cover_the_diagram() {
    let mut h = harness(WIDE, "controls");
    let tools = diagram_tools(&mut h);
    let item = item_rect(&h, "```mermaid");
    let a = h.state_mut();
    let model = a.model().unwrap().clone();
    let idx = model
        .items
        .iter()
        .position(|i| matches!(i.kind, markise_layout::ItemKind::Diagram { .. }))
        .unwrap();
    let mut fonts = markise_layout::testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let v = a.viewer().unwrap();
    let l = v.ed.view.layout(idx, &mut f, &markise_layout::block::NoImages);
    let row = l.diagram_controls.expect("a row reserved under the scene");
    let top = item.top() + row.top();
    for r in [tools.copy, tools.lang, tools.expand.unwrap()] {
        assert!(r.top() >= top - 0.5, "{r:?} below {top}");
    }
    let bottom = l
        .prims
        .iter()
        .filter_map(|p| match p {
            markise_layout::block::Prim::Rect { rect, .. } => Some(rect.bottom()),
            _ => None,
        })
        .fold(0.0, f32::max);
    assert!(bottom <= row.top() + 0.5, "the picture ends above the row");
}

#[test]
fn esc_closes_and_the_document_gets_the_focus_back() {
    let mut h = harness(WIDE, "esc");
    let para = item_rect(&h, "after").center();
    click(&mut h, para);
    assert!(surface_focused(&mut h));
    open_by_button(&mut h);
    h.step();
    assert!(!surface_focused(&mut h), "the view holds the keyboard");
    key(&mut h, Key::Escape);
    assert!(h.state().full_view().is_none());
    h.run_steps(4);
    assert!(surface_focused(&mut h), "the focus came back");
}

#[test]
fn a_click_on_the_backdrop_closes_and_on_the_picture_does_not() {
    let mut h = harness(WIDE, "backdrop");
    open_by_button(&mut h);
    click(&mut h, pos2(500.0, 350.0));
    assert!(h.state().full_view().is_some(), "a click on the picture keeps it");
    click(&mut h, pos2(5.0, 5.0));
    assert!(h.state().full_view().is_none(), "a click on the backdrop closes it");
    h.run_steps(4);
    assert!(
        surface_focused(&mut h),
        "the document has the focus (nothing was focused before)"
    );
}

#[test]
fn wheel_keys_and_drag_move_the_picture_within_limits() {
    let mut h = harness(WIDE, "transform");
    open_by_button(&mut h);
    let t0 = h.state().full_view().unwrap();
    // the wheel zooms around the pointer
    let at = pos2(300.0, 350.0);
    let p0 = (at.to_vec2() - t0.offset()) / t0.scale();
    wheel(&mut h, at, 3.0);
    let t1 = h.state().full_view().unwrap();
    assert!(t1.scale() > t0.scale());
    let p1 = (at.to_vec2() - t1.offset()) / t1.scale();
    assert!(
        (p0 - p1).length() < 0.5,
        "the point under the pointer stays: {p0:?} {p1:?}"
    );
    let laid = h.state().full_view_layouts().unwrap();
    assert!(laid >= 2, "laid out again, sharp at the new scale");
    // a pinch too
    h.event(Event::Zoom(1.5));
    h.step();
    h.step();
    assert!(h.state().full_view().unwrap().scale() > t1.scale());
    // two fingers on a trackpad move it
    key(&mut h, Key::Num0);
    h.event(Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: vec2(0.0, 30.0),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Trackpad,
        modifiers: Modifiers::NONE,
    });
    h.step();
    h.step();
    let scrolled = h.state().full_view().unwrap();
    assert_eq!(scrolled.scale(), t0.scale());
    assert_eq!(scrolled.offset(), t0.offset() + vec2(0.0, 30.0));
    // keys
    key(&mut h, Key::Num0);
    assert_eq!(h.state().full_view().unwrap().scale(), t0.scale(), "0 fits again");
    key(&mut h, Key::Plus);
    assert!(h.state().full_view().unwrap().scale() > t0.scale());
    key(&mut h, Key::Minus);
    key(&mut h, Key::Minus);
    assert!(h.state().full_view().unwrap().scale() < t0.scale());
    key(&mut h, Key::Num0);
    // drag pans
    drag(&mut h, pos2(500.0, 350.0), vec2(40.0, 25.0));
    let t2 = h.state().full_view().unwrap();
    assert_eq!(t2.scale(), t0.scale());
    assert!((t2.offset() - t0.offset() - vec2(40.0, 25.0)).length() < 8.0, "{t2:?}");
    // and stops with part of the picture in the window
    for _ in 0..6 {
        drag(&mut h, pos2(500.0, 100.0), vec2(450.0, 500.0));
    }
    let t3 = h.state().full_view().unwrap();
    assert!(
        t3.offset().x < 1000.0 && t3.offset().y < 700.0,
        "still in the window: {t3:?}"
    );
    assert!(h.state().full_view().is_some(), "a drag is not a backdrop click");
}

#[test]
fn input_while_open_never_reaches_the_document() {
    let mut h = harness(WIDE, "readonly");
    let para = item_rect(&h, "after").center();
    click(&mut h, para);
    open_by_button(&mut h);
    let scroll = h.state_mut().viewer().unwrap().ed.view.scroll_y();
    h.event(Event::Text("x".into()));
    h.step();
    key(&mut h, Key::Enter);
    key(&mut h, Key::Backspace);
    key(&mut h, Key::PageDown);
    wheel(&mut h, pos2(500.0, 350.0), -3.0);
    drag(&mut h, pos2(500.0, 350.0), vec2(30.0, 30.0));
    key(&mut h, Key::Escape);
    h.run_steps(3);
    assert_eq!(text(&h), WIDE);
    assert!(!h.state().is_dirty());
    assert_eq!(h.state_mut().viewer().unwrap().ed.view.scroll_y(), scroll);
    // typing works again
    h.event(Event::Text("y".into()));
    h.run_steps(2);
    assert_ne!(text(&h), WIDE, "the document has the keyboard again");
}

#[test]
fn commands_are_blocked_and_close_dismisses_the_view_first() {
    let mut h = harness(WIDE, "commands");
    open_by_button(&mut h);
    let ctx = h.ctx.clone();
    for action in [
        MenuAction::Command("block.heading1"),
        MenuAction::Command("file.new"),
        MenuAction::Command("view.sideBySide"),
        MenuAction::Appearance(markise_ui_egui::ThemeChoice::Dark),
    ] {
        assert!(
            !h.state_mut().menu_action_by(&ctx, &action, Trigger::Click),
            "{action:?}"
        );
        assert!(h.state().full_view().is_some());
    }
    h.step();
    assert_eq!(text(&h), WIDE);
    assert!(
        h.state_mut()
            .menu_action_by(&ctx, &MenuAction::Command("file.close"), Trigger::Click)
    );
    assert!(h.state().full_view().is_none(), "Close dismisses it and goes on");
    // the window's own close button does the same
    h.step();
    open_by_button(&mut h);
    assert!(h.state_mut().request_close(), "a clean document closes");
    assert!(h.state().full_view().is_none());
}

#[test]
fn works_in_side_by_side_and_keeps_its_scene_when_the_diagrams_go() {
    let mut h = harness(WIDE, "sbs");
    h.state_mut().toggle_side_by_side();
    h.run_steps(3);
    open_by_button(&mut h);
    let t = h.state().full_view().unwrap();
    assert_eq!(t.scale(), t.fit());
    // the view holds its own scene: diagrams switched off underneath do not disturb it
    h.state_mut().set_diagrams(false);
    h.run_steps(3);
    key(&mut h, Key::Plus);
    assert!(h.state().full_view().unwrap().scale() > t.scale());
    key(&mut h, Key::Escape);
    assert!(h.state().full_view().is_none());
    assert_eq!(text(&h), WIDE);
}

/// A diagram scrolled far away under the open view (its page may be evicted) stays shown.
#[test]
fn the_view_outlives_the_page_it_came_from() {
    let mut doc = String::from(WIDE);
    doc.push_str(&markise_corpus::generate(3 << 20, 9));
    let mut h = harness(&doc, "paged");
    assert!(h.state().model().unwrap().is_paged());
    open_by_button(&mut h);
    let laid = h.state().full_view_layouts().unwrap();
    h.state_mut().viewer().unwrap().ed.view.scroll_to_y(f64::MAX);
    h.run_steps(5);
    assert!(h.state().full_view().is_some());
    key(&mut h, Key::Plus);
    assert!(
        h.state().full_view_layouts().unwrap() > laid,
        "still lays out at a new scale"
    );
    key(&mut h, Key::Escape);
    assert!(h.state().full_view().is_none());
}

// ---- images

fn image_tools(h: &mut H, pos: Pos2) -> Vec<egui::Rect> {
    h.hover_at(pos);
    h.step();
    h.step();
    h.state_mut().viewer().unwrap().image_tools_shown().to_vec()
}

fn wait_full(h: &mut H, pixels: [usize; 2]) {
    let t = Instant::now();
    while h.state().full_view_image_pixels() != Some(pixels) {
        assert!(
            t.elapsed() < common::HANG_GUARD,
            "the whole image never arrived: {:?}",
            h.state().full_view_image_pixels()
        );
        h.step();
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn an_image_opens_by_button_and_double_click_and_closes_the_same_way() {
    let md = "para\n\n![pic](pic.png)\n\nafter\n";
    let mut h = image_harness(md, [300, 200], "image");
    let r = item_rect(&h, "![pic]");
    let center = r.center();
    let tools = image_tools(&mut h, center);
    assert_eq!(tools.len(), 1, "the expand button shows on hover");
    // a plain click: the caret, no view
    click(&mut h, center);
    assert!(h.state().full_view().is_none());
    h.run_steps(30);
    double_click(&mut h, center);
    let t = h.state().full_view().expect("a double click opens it");
    assert_eq!(t.scale(), t.fit());
    assert_eq!(
        h.state().full_view_image_pixels(),
        Some([300, 200]),
        "the document's texture is whole"
    );
    // the same keys and closing rules
    key(&mut h, Key::Plus);
    assert!(h.state().full_view().unwrap().scale() > t.scale());
    key(&mut h, Key::Num0);
    click(&mut h, pos2(500.0, 350.0));
    assert!(h.state().full_view().is_some(), "a click on the image keeps it");
    click(&mut h, pos2(5.0, 5.0));
    assert!(h.state().full_view().is_none(), "a click on the backdrop closes it");
    h.run_steps(30);
    click(&mut h, tools[0].center());
    assert!(h.state().full_view().is_some(), "the button opens it");
    key(&mut h, Key::Escape);
    assert!(h.state().full_view().is_none());
    assert_eq!(text(&h), md);
}

#[test]
fn a_downscaled_image_is_loaded_whole_and_zooms_past_its_pixels() {
    let md = "![pic](pic.png)\n";
    let mut h = image_harness(md, [3000, 1000], "big");
    let center = item_rect(&h, "![pic]").center();
    let tools = image_tools(&mut h, center);
    click(&mut h, tools[0].center());
    assert!(h.state().full_view().is_some());
    wait_full(&mut h, [3000, 1000]);
    let t = h.state().full_view().unwrap();
    assert_eq!(t.scale(), t.fit(), "fitted");
    assert!(t.fit() < 1.0);
    for _ in 0..40 {
        h.event(Event::Zoom(1.5));
        h.step();
    }
    h.step();
    let t = h.state().full_view().unwrap();
    assert!(t.scale() > 1.0, "beyond its pixels: {t:?}");
    key(&mut h, Key::Escape);
    assert_eq!(text(&h), md);
}

/// Codex review of #175: opening and dismissing a large image repeatedly, then opening it once
/// more, still brings the whole image.
#[test]
fn reopening_a_large_image_still_loads_it_whole() {
    let md = "![pic](pic.png)\n";
    let mut h = image_harness(md, [3000, 1000], "reopen");
    let center = item_rect(&h, "![pic]").center();
    let tools = image_tools(&mut h, center);
    for _ in 0..4 {
        click(&mut h, tools[0].center());
        assert!(h.state().full_view().is_some());
        key(&mut h, Key::Escape);
        assert!(h.state().full_view().is_none());
        h.hover_at(center);
        h.run_steps(2);
    }
    click(&mut h, tools[0].center());
    wait_full(&mut h, [3000, 1000]);
    common::settle(&mut h);
}

#[test]
fn an_image_that_failed_to_load_offers_no_view() {
    let md = "![gone](missing.png)\n\nafter\n";
    let mut h = harness(md, "failed");
    let center = item_rect(&h, "![gone]").center();
    assert!(image_tools(&mut h, center).is_empty(), "no button");
    double_click(&mut h, center);
    assert!(h.state().full_view().is_none(), "no double click");
}

#[test]
fn a_linked_image_follows_its_link_on_command_click() {
    let md = "# Top\n\n[![pic](pic.png)](not-there.md)\n\nafter\n";
    let mut h = image_harness(md, [300, 200], "linked");
    let center = item_rect(&h, "[![pic]").center();
    let cmd = Modifiers {
        command: true,
        mac_cmd: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        ..Default::default()
    };
    h.hover_at(center);
    h.event(Event::ModifiersChanged(cmd));
    h.step();
    let selection = h.state_mut().viewer().unwrap().ed.session.selection().clone();
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: cmd,
        });
        h.step();
    }
    h.run_steps(2);
    assert!(h.state().full_view().is_none(), "the link's, not the view's");
    let banner = h.state().banner_text().unwrap_or_default();
    assert!(banner.contains("not-there.md"), "the link was followed: {banner}");
    assert_eq!(
        h.state_mut().viewer().unwrap().ed.session.selection().clone(),
        selection,
        "a link click places no caret"
    );
    assert_eq!(text(&h), md);
}

/// Without the modifier a linked image opens like any image, and a plain click follows nothing.
#[test]
fn a_linked_image_opens_on_a_plain_double_click() {
    let md = "# Top\n\n[![pic](pic.png)](not-there.md)\n\nafter\n";
    let mut h = image_harness(md, [300, 200], "linked2");
    let center = item_rect(&h, "[![pic]").center();
    click(&mut h, center);
    assert!(h.state().banner_text().is_none(), "a plain click follows nothing");
    assert!(h.state().full_view().is_none());
    h.run_steps(30);
    double_click(&mut h, center);
    assert!(h.state().full_view().is_some());
}

// ---- idle (NFR-10)

fn input(events: Vec<Event>) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 780.0))),
        events,
        ..Default::default()
    }
}

fn frame(ctx: &egui::Context, app: &mut MarkiseApp, events: Vec<Event>) -> Duration {
    let mut out = ctx.run_ui(input(events), |ui| app.ui(ui));
    out.textures_delta.clear();
    let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
    out.viewport_output[&egui::ViewportId::ROOT].repaint_delay
}

#[test]
fn an_untouched_view_schedules_no_repaint() {
    let ctx = egui::Context::default();
    let mut app = app_of(WIDE, "idle");
    let _ = common::idle_delays(&mut app, |a| frame(&ctx, a, Vec::new()), 3);
    let at = WIDE.find("```mermaid").unwrap();
    let t = Instant::now();
    let expand = loop {
        let c = app.viewer_item_rect(at).expect("visible").center();
        frame(&ctx, &mut app, vec![Event::PointerMoved(c)]);
        frame(&ctx, &mut app, Vec::new());
        if let Some(r) = app.viewer().unwrap().code_tools_shown().iter().find_map(|s| s.expand) {
            break r.center();
        }
        assert!(t.elapsed() < common::HANG_GUARD, "the diagram never rendered");
        std::thread::sleep(Duration::from_millis(5));
    };
    frame(&ctx, &mut app, vec![Event::PointerMoved(expand)]);
    for pressed in [true, false] {
        frame(
            &ctx,
            &mut app,
            vec![Event::PointerButton {
                pos: expand,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }],
        );
    }
    assert!(app.full_view().is_some());
    // move off the close button's and the document's hover effects
    frame(&ctx, &mut app, vec![Event::PointerMoved(pos2(550.0, 390.0))]);
    let delays = common::idle_delays(&mut app, |a| frame(&ctx, a, Vec::new()), 6);
    assert_eq!(*delays.last().unwrap(), Duration::MAX, "idle: {delays:?}");
    let laid = app.full_view_layouts().unwrap();
    for _ in 0..3 {
        frame(&ctx, &mut app, Vec::new());
    }
    assert_eq!(
        app.full_view_layouts(),
        Some(laid),
        "the picture is kept between frames"
    );
}

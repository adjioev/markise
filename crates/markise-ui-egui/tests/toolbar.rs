//! Formatting toolbar (spec 04 §3.2): controls run the registry commands exactly like the
//! menus (saved bytes), reflect the selection, are disabled where the command is unavailable,
//! never take the text focus, drop-downs are keyboard-navigable, View → Toolbar (on by default)
//! and View → Toolbar Position are persisted, the bar never covers the document at the top or
//! the bottom, narrow windows collapse groups into More, and an idle window with the bar stays
//! quiet (NFR-10).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers, RawInput, ViewportId};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_doc::{DocPos, Selection};
use markise_engine::settings::ToolbarPosition;
use markise_ui_egui::menu::{Check, MenuAction};
use markise_ui_egui::{AppOptions, ConfigDir, MarkiseApp, ViewMode};

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-toolbar-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn harness_with(open: Vec<PathBuf>, config: ConfigDir, width: f32) -> Harness<'static, MarkiseApp> {
    let app = MarkiseApp::new(AppOptions {
        open,
        config,
        ..Default::default()
    });
    Harness::builder()
        .with_size(egui::vec2(width, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app)
}

fn wait_until(h: &mut Harness<'static, MarkiseApp>, what: &str, f: impl Fn(&MarkiseApp) -> bool) {
    let t = Instant::now();
    while !f(h.state()) {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    h.step();
}

fn cmd() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers {
            mac_cmd: true,
            command: true,
            ..Default::default()
        }
    } else {
        Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        }
    }
}

/// Opens `src` with the toolbar shown, focuses the document and selects `sel` (byte range).
fn open(name: &str, src: &str, width: f32) -> (Harness<'static, MarkiseApp>, PathBuf) {
    let dir = tmpdir(name);
    let p = dir.join("t.md");
    std::fs::write(&p, src).unwrap();
    let mut h = harness_with(vec![p.clone()], ConfigDir::None, width);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    h.state_mut().set_show_toolbar(true);
    // click into the document body (below the bar) for the keyboard focus
    let at = egui::pos2(width * 0.6, 300.0);
    h.hover_at(at);
    h.drag_at(at);
    h.step();
    h.drop_at(at);
    for _ in 0..3 {
        h.step();
    }
    (h, p)
}

fn select(h: &mut Harness<'static, MarkiseApp>, from: usize, to: usize) {
    h.state_mut()
        .viewer()
        .unwrap()
        .ed
        .session
        .set_selection(Selection::single(DocPos::new(from), DocPos::new(to)));
    h.step();
    h.step();
}

fn text(h: &Harness<'static, MarkiseApp>) -> String {
    h.state().document_text().unwrap()
}

/// The document surface itself (not just some widget) has the keyboard focus.
fn doc_focused(h: &mut Harness<'static, MarkiseApp>) -> bool {
    let surface = h.state_mut().viewer().unwrap().surface_id();
    surface.is_some() && h.ctx.memory(|m| m.focused()) == surface
}

fn control(h: &Harness<'static, MarkiseApp>, id: &str) -> markise_ui_egui::toolbar::Control {
    h.state()
        .toolbar_state()
        .expect("toolbar shown")
        .get(id)
        .cloned()
        .expect(id)
}

fn click(h: &mut Harness<'static, MarkiseApp>, label: &str) {
    h.get_by_label(label).click();
    h.step();
    h.step();
}

fn save(h: &mut Harness<'static, MarkiseApp>, p: &Path) -> String {
    h.key_press_modifiers(cmd(), Key::S);
    wait_until(h, "save", |a| !a.is_busy() && !a.is_dirty());
    std::fs::read_to_string(p).unwrap()
}

#[test]
fn bold_list_and_heading_controls_edit_the_document() {
    let (mut h, p) = open("edit", "Hello world\n\nSecond line\n", 1000.0);
    assert!(doc_focused(&mut h), "document has the focus");
    select(&mut h, 0, 5);
    assert!(!control(&h, "format.strong").active);
    let tip = control(&h, "format.strong").tooltip();
    assert!(tip.starts_with("Strong (") && tip.contains('B'), "{tip}");

    // Bold: wraps the selection, the control turns active, focus and selection stay
    click(&mut h, "Strong");
    assert_eq!(text(&h), "**Hello** world\n\nSecond line\n");
    assert!(control(&h, "format.strong").active, "state follows the selection");
    assert!(doc_focused(&mut h), "a click never takes the text focus");
    // the same command again removes it (toggle), one undo step each
    click(&mut h, "Strong");
    assert_eq!(text(&h), "Hello world\n\nSecond line\n");
    h.key_press_modifiers(cmd(), Key::Z);
    h.step();
    assert_eq!(text(&h), "**Hello** world\n\nSecond line\n", "undo like the menu item");
    h.key_press_modifiers(cmd(), Key::Z);
    h.step();
    assert_eq!(text(&h), "Hello world\n\nSecond line\n");

    // Block style drop-down → Heading 2
    select(&mut h, 2, 2);
    assert_eq!(h.state().toolbar_state().unwrap().block_label(), "Paragraph");
    click(&mut h, "Block Style");
    assert!(h.state().toolbar().popup_open());
    click(&mut h, "Heading 2");
    assert!(!h.state().toolbar().popup_open());
    assert_eq!(text(&h), "## Hello world\n\nSecond line\n");
    assert_eq!(h.state().toolbar_state().unwrap().block_label(), "Heading 2");
    assert!(doc_focused(&mut h), "focus back in the document after the drop-down");

    // List drop-down → bullet list on the second paragraph; typing still reaches the document
    let second = text(&h).find("Second").unwrap();
    select(&mut h, second, second);
    click(&mut h, "List");
    click(&mut h, "Unordered List");
    assert_eq!(text(&h), "## Hello world\n\n- Second line\n");
    assert!(control(&h, "block.bulletList").active);
    h.event(Event::Text("X".into()));
    h.step();
    assert_eq!(
        text(&h),
        "## Hello world\n\n- XSecond line\n",
        "caret kept in the document"
    );
    assert_eq!(save(&mut h, &p), "## Hello world\n\n- XSecond line\n");
}

#[test]
fn drop_downs_are_keyboard_navigable() {
    let (mut h, p) = open("keys", "Hello world\n", 1000.0);
    select(&mut h, 1, 1);
    click(&mut h, "Block Style");
    assert!(h.state().toolbar().popup_open());
    // rows: Paragraph (checked, disabled: a no-op here), Heading 1, Heading 2…; the keys
    // never reach the document
    for _ in 0..2 {
        h.key_press(Key::ArrowDown);
        h.step();
    }
    h.key_press(Key::Enter);
    h.step();
    h.step();
    assert!(!h.state().toolbar().popup_open());
    assert_eq!(text(&h), "## Hello world\n");
    assert!(doc_focused(&mut h));
    // Escape closes without acting and returns the focus
    click(&mut h, "Block Style");
    h.key_press(Key::ArrowDown);
    h.step();
    h.key_press(Key::Escape);
    h.step();
    h.step();
    assert!(!h.state().toolbar().popup_open());
    assert!(doc_focused(&mut h), "focus returned on Escape");
    assert_eq!(text(&h), "## Hello world\n");
    assert_eq!(save(&mut h, &p), "## Hello world\n");
}

#[test]
fn controls_are_disabled_where_commands_are_unavailable() {
    let (mut h, _p) = open("disabled", "Intro\n\n```\ncode here\n```\n", 1000.0);
    let inside = text(&h).find("code here").unwrap() + 2;
    select(&mut h, inside, inside);
    let m = h.state().toolbar_state().unwrap();
    assert!(
        !m.get("format.strong").unwrap().enabled,
        "no inline formats in a code block"
    );
    assert!(!m.get("block.heading2").unwrap().enabled);
    assert_eq!(m.block_label(), "—");
    // disabled controls stay visible (never hidden) and a click does nothing
    let before = text(&h);
    click(&mut h, "Strong");
    assert_eq!(text(&h), before);

    // Side by Side: the rendered pane is read-only → every control disabled
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    for _ in 0..3 {
        h.step();
    }
    let m = h.state().toolbar_state().unwrap();
    assert!(m.controls.iter().all(|c| !c.enabled));
    // Source mode: no toolbar in this version
    h.state_mut().set_view_mode(ViewMode::Source);
    h.step();
    assert!(h.state().toolbar_state().is_none());
    assert!(h.query_by_label("Strong").is_none());
}

#[test]
fn narrow_window_collapses_groups_into_more() {
    let (mut h, p) = open("narrow", "Hello world\n", 520.0);
    assert!(
        h.state().toolbar().collapsed_groups() >= 1,
        "the last group moved into More"
    );
    assert!(
        h.query_by_label("Quote").is_none(),
        "the rightmost group went into More"
    );
    select(&mut h, 0, 5);
    click(&mut h, "More");
    assert!(h.state().toolbar().popup_open());
    click(&mut h, "Quote");
    assert_eq!(text(&h), "> Hello world\n");
    assert!(doc_focused(&mut h));
    assert_eq!(save(&mut h, &p), "> Hello world\n");
}

/// Owner decision 2026-10-28 (spec 04 §3.2, spec 05): on for fresh settings; turning it off
/// is persisted and kept on the next start.
#[test]
fn view_toolbar_toggle_is_persisted_and_on_by_default() {
    let dir = tmpdir("persist");
    let p = dir.join("t.md");
    std::fs::write(&p, "Hello\n").unwrap();
    let cfg = dir.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    let settings = cfg.join("markise").join("settings.json");
    {
        let mut h = harness_with(vec![p.clone()], ConfigDir::Dir(cfg.clone()), 1000.0);
        wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
        h.step();
        assert!(h.state().show_toolbar(), "spec 05: on by default");
        assert!(h.query_by_label("Strong").is_some());
        let item = h.state().menu_model().item("view.toolbar").cloned().unwrap();
        assert_eq!(item.check, Check::Toggle(true));
        let ctx = h.ctx.clone();
        assert!(h.state_mut().menu_action(&ctx, &MenuAction::Command("view.toolbar")));
        h.step();
        h.step();
        assert!(!h.state().show_toolbar());
        assert!(h.query_by_label("Strong").is_none());
        assert_eq!(
            h.state().menu_model().item("view.toolbar").unwrap().check,
            Check::Toggle(false)
        );
        wait_until(&mut h, "settings written", |_| {
            std::fs::read_to_string(&settings).is_ok_and(|s| s.contains("\"showToolbar\": false"))
        });
    }
    // the saved choice wins over the new default
    let mut h = harness_with(vec![p], ConfigDir::Dir(cfg), 1000.0);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    wait_until(&mut h, "settings applied", |a| !a.show_toolbar());
}

/// View → Toolbar Position: a Top / Bottom radio pair in the menu model (so native menus show
/// the check marks), default Top, persisted as `toolbarPosition`.
#[test]
fn toolbar_position_is_a_persisted_radio_defaulting_to_top() {
    let dir = tmpdir("position");
    let p = dir.join("t.md");
    std::fs::write(&p, "Hello\n").unwrap();
    let cfg = dir.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    let settings = cfg.join("markise").join("settings.json");
    let checks = |h: &Harness<'static, MarkiseApp>| {
        let m = h.state().menu_model();
        (
            m.item("view.toolbarTop").unwrap().check,
            m.item("view.toolbarBottom").unwrap().check,
        )
    };
    {
        let mut h = harness_with(vec![p.clone()], ConfigDir::Dir(cfg.clone()), 1000.0);
        wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
        assert_eq!(h.state().toolbar_position(), ToolbarPosition::Top);
        assert_eq!(checks(&h), (Check::Radio(true), Check::Radio(false)));
        let top_bar = h.state().toolbar().bar_rect().unwrap();
        let ctx = h.ctx.clone();
        assert!(
            h.state_mut()
                .menu_action(&ctx, &MenuAction::Command("view.toolbarBottom"))
        );
        h.step();
        h.step();
        assert_eq!(h.state().toolbar_position(), ToolbarPosition::Bottom);
        assert_eq!(checks(&h), (Check::Radio(false), Check::Radio(true)));
        let bar = h.state().toolbar().bar_rect().unwrap();
        assert!(
            bar.top() > 500.0 && bar.top() > top_bar.bottom(),
            "moved to the bottom: {bar:?}"
        );
        assert!((bar.center().x - top_bar.center().x).abs() < 0.5, "still centred");
        wait_until(&mut h, "settings written", |_| {
            std::fs::read_to_string(&settings).is_ok_and(|s| s.contains("\"toolbarPosition\": \"bottom\""))
        });
    }
    let mut h = harness_with(vec![p], ConfigDir::Dir(cfg), 1000.0);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    wait_until(&mut h, "settings applied", |a| {
        a.toolbar_position() == ToolbarPosition::Bottom
    });
    assert_eq!(checks(&h), (Check::Radio(false), Check::Radio(true)));
}

/// Bottom position: the document area ends above the bar (Rendered and Side by Side's rendered
/// pane), so scrolled to the end the last line is fully visible, not under the bar.
#[test]
fn bottom_bar_never_covers_the_document() {
    let src: String = (1..=80).map(|i| format!("Line {i} of the document.\n\n")).collect();
    let (mut h, _p) = open("cover-bottom", &src, 1000.0);
    h.state_mut().set_toolbar_position(ToolbarPosition::Bottom);
    for _ in 0..3 {
        h.step();
    }
    let check = |h: &mut Harness<'static, MarkiseApp>, what: &str| {
        let bar = h.state().toolbar().bar_rect().unwrap();
        let surface = h.state_mut().viewer().unwrap().surface_rect().unwrap();
        assert!(
            surface.bottom() <= bar.top(),
            "{what}: surface {surface:?} above bar {bar:?}"
        );
        assert!(
            surface.x_range().intersects(bar.x_range()),
            "{what}: bar under this surface"
        );
        assert!(bar.bottom() <= 700.0, "{what}: bar inside the window");
        (bar, surface)
    };
    let (_, surface) = check(&mut h, "rendered");
    assert!(surface.top() < 60.0, "no strip kept at the top: {surface:?}");
    // the caret to the end of the document: the view scrolls so the last line shows
    if cfg!(target_os = "macos") {
        h.key_press_modifiers(cmd(), Key::ArrowDown);
    } else {
        h.key_press_modifiers(cmd(), Key::End);
    }
    for _ in 0..4 {
        h.step();
    }
    let (bar, surface) = check(&mut h, "rendered, at the end");
    let last = h
        .state_mut()
        .viewer()
        .unwrap()
        .item_screen_rect(src.len() - 5)
        .expect("last line laid out");
    assert!(
        last.bottom() <= surface.bottom() + 0.5 && last.bottom() <= bar.top(),
        "last line {last:?} visible above the bar {bar:?}"
    );
    assert!(last.top() >= surface.top(), "scrolled to the end");
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    for _ in 0..4 {
        h.step();
    }
    let (_, surface) = check(&mut h, "side by side");
    assert!(surface.left() > 400.0, "the rendered pane");
}

/// Bottom position: the drop-downs open upward and stay inside the window, also in a short
/// window where the More list has to scroll.
#[test]
fn bottom_bar_drop_downs_open_upward_inside_the_window() {
    let (mut h, _p) = open("upward", "Hello world\n", 1000.0);
    h.state_mut().set_toolbar_position(ToolbarPosition::Bottom);
    for _ in 0..3 {
        h.step();
    }
    let bar = h.state().toolbar().bar_rect().unwrap();
    click(&mut h, "Block Style");
    let popup = h.state().toolbar().popup_rect().unwrap();
    assert!(popup.bottom() <= bar.top(), "above the bar: {popup:?} / {bar:?}");
    assert!(popup.top() >= 0.0, "inside the window: {popup:?}");
    assert!(h.get_by_label("Heading 1").rect().bottom() <= bar.top());
    h.key_press(Key::Escape);
    h.step();

    let dir = tmpdir("upward-short");
    let p = dir.join("t.md");
    std::fs::write(&p, "Hello world\n").unwrap();
    let app = MarkiseApp::new(AppOptions {
        open: vec![p],
        ..Default::default()
    });
    let mut h = Harness::builder()
        .with_size(egui::vec2(480.0, 300.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    h.state_mut().set_toolbar_position(ToolbarPosition::Bottom);
    for _ in 0..3 {
        h.step();
    }
    assert!(h.state().toolbar().collapsed_groups() >= 2, "narrow collapse unchanged");
    let bar = h.state().toolbar().bar_rect().unwrap();
    click(&mut h, "More");
    let popup = h.state().toolbar().popup_rect().unwrap();
    assert!(popup.top() >= 0.0 && popup.bottom() <= bar.top(), "{popup:?} / {bar:?}");
    h.key_press(Key::End);
    for _ in 0..3 {
        h.step();
    }
    let last = h.get_by_label("Horizontal Line").rect();
    assert!(
        last.top() >= popup.top() - 0.5 && last.bottom() <= popup.bottom() + 0.5,
        "last entry {last:?} visible in {popup:?}"
    );
}

#[test]
fn idle_window_with_toolbar_requests_no_repaint() {
    idle_check("idle", ToolbarPosition::Top);
}

#[test]
fn idle_window_with_bottom_toolbar_requests_no_repaint() {
    idle_check("idle-bottom", ToolbarPosition::Bottom);
}

fn idle_check(name: &str, position: ToolbarPosition) {
    let dir = tmpdir(name);
    let p = dir.join("t.md");
    std::fs::write(&p, "# Title\n\nSome **text** here.\n").unwrap();
    let ctx = egui::Context::default();
    let mut app = MarkiseApp::new(AppOptions {
        open: vec![p],
        ..Default::default()
    });
    app.set_show_toolbar(true);
    app.set_toolbar_position(position);
    let frame = |app: &mut MarkiseApp| {
        let input = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 780.0))),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| app.ui(ui));
        out.textures_delta.clear();
        out.viewport_output[&ViewportId::ROOT].repaint_delay
    };
    let t = Instant::now();
    while (app.model().is_none() || app.is_loading()) && t.elapsed() < Duration::from_secs(20) {
        frame(&mut app);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.toolbar_state().is_some());
    // the debounced settings write of the toggle is a scheduled wake-up, not a loop
    while app.theme_manager().save_deadline().is_some() && t.elapsed() < Duration::from_secs(20) {
        frame(&mut app);
        std::thread::sleep(Duration::from_millis(20));
    }
    let delays: Vec<Duration> = (0..8).map(|_| frame(&mut app)).collect();
    assert_eq!(*delays.last().unwrap(), Duration::MAX, "NFR-10, closed: {delays:?}");
    // an open drop-down is idle too
    let at = app.toolbar().bar_rect().unwrap().left_center() + egui::vec2(30.0, 0.0);
    let press = |pressed| {
        let mut input = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 780.0))),
            ..Default::default()
        };
        input.events = vec![
            Event::PointerMoved(at),
            Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            },
        ];
        input
    };
    for p in [true, false] {
        let mut out = ctx.run_ui(press(p), |ui| app.ui(ui));
        out.textures_delta.clear();
    }
    assert!(app.toolbar().popup_open(), "block style list opened");
    let delays: Vec<Duration> = (0..8).map(|_| frame(&mut app)).collect();
    assert!(app.toolbar().popup_open());
    if *delays.last().unwrap() != Duration::MAX {
        eprintln!("causes: {:?}", ctx.repaint_causes());
    }
    assert_eq!(*delays.last().unwrap(), Duration::MAX, "NFR-10, open: {delays:?}");
}

/// The first lines are never under the bar: the document surface starts below it in the
/// rendered view and in Side by Side's rendered pane, also after scrolling (painting is
/// clipped to the surface).
#[test]
fn bar_never_covers_the_document() {
    let src: String = (1..=80).map(|i| format!("Line {i} of the document.\n\n")).collect();
    let (mut h, _p) = open("cover", &src, 1000.0);
    let check = |h: &mut Harness<'static, MarkiseApp>, what: &str| {
        let bar = h.state().toolbar().bar_rect().unwrap();
        let surface = h.state_mut().viewer().unwrap().surface_rect().unwrap();
        assert!(
            surface.top() >= bar.bottom(),
            "{what}: surface {surface:?} below bar {bar:?}"
        );
        assert!(
            surface.x_range().intersects(bar.x_range()),
            "{what}: bar over this surface"
        );
        // `None`: the first item scrolled out of the laid-out range
        let first = h.state_mut().viewer().unwrap().item_screen_rect(0);
        (first, surface)
    };
    let (first, surface) = check(&mut h, "rendered");
    assert!(first.unwrap().top() >= surface.top(), "first line visible");
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    for _ in 0..4 {
        h.step();
    }
    let (first, surface) = check(&mut h, "side by side");
    assert!(
        first.unwrap().top() >= surface.top(),
        "first line visible in the rendered pane"
    );
    // scroll the rendered pane: content moves up under the surface edge, never under the bar
    let over = surface.center();
    h.hover_at(over);
    for _ in 0..5 {
        h.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -120.0),
            modifiers: Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
        });
        h.step();
    }
    for _ in 0..3 {
        h.step();
    }
    let (first, _) = check(&mut h, "side by side, scrolled");
    let bar = h.state().toolbar().bar_rect().unwrap();
    assert!(first.is_none_or(|f| f.top() < bar.bottom()), "the pane did scroll");
    // and scrolled back to the top the first line is fully visible again
    h.state_mut().viewer().unwrap().view().scroll_to_y(0.0);
    for _ in 0..3 {
        h.step();
    }
    let (first, surface) = check(&mut h, "side by side, back at the top");
    assert!(first.unwrap().top() >= surface.top());
}

/// A long More list in a short window stays inside the window and scrolls; the entry
/// chosen with the keyboard is scrolled into view.
#[test]
fn long_drop_down_is_bounded_and_scrolls() {
    let dir = tmpdir("short");
    let p = dir.join("t.md");
    std::fs::write(&p, "Hello world\n").unwrap();
    let app = MarkiseApp::new(AppOptions {
        open: vec![p],
        ..Default::default()
    });
    let mut h = Harness::builder()
        .with_size(egui::vec2(480.0, 300.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    h.state_mut().set_show_toolbar(true);
    for _ in 0..3 {
        h.step();
    }
    assert!(h.state().toolbar().collapsed_groups() >= 2);
    click(&mut h, "More");
    let popup = h.state().toolbar().popup_rect().unwrap();
    assert!(popup.bottom() <= 300.0, "inside the window: {popup:?}");
    // End highlights the last entry and scrolls it into view
    h.key_press(Key::End);
    for _ in 0..3 {
        h.step();
    }
    let last = h.get_by_label("Horizontal Line").rect();
    assert!(
        last.top() >= popup.top() - 0.5 && last.bottom() <= popup.bottom() + 0.5,
        "last entry {last:?} visible in {popup:?}"
    );
    h.key_press(Key::Home);
    for _ in 0..3 {
        h.step();
    }
    // the first entry: Strong, from the collapsed inline group
    let first = h.get_by_label("Strong").rect();
    assert!(first.top() >= popup.top() - 0.5, "first entry {first:?} back in view");
}

//! Formatting toolbar (spec 04 §3.2): a rounded bar floating at the top centre of the editor
//! area, coloured with the sidebar tokens ([`SideLook`]). Every control names a registry
//! command; the application builds a [`Model`] (label, effective shortcut, enabled and active
//! state — the same queries as the Format and Paragraph menus) and runs the command the bar
//! returns exactly like the menu item, so effect, undo step and shortcut are identical.
//!
//! - **Focus.** Controls never take keyboard focus: a pointer press on the bar or one of its
//!   drop-downs hands the focus straight back to the widget that had it (the document), so
//!   clicking acts on the selection and leaves it where it was.
//! - **Drop-downs** (block style, list, More) opened by the mouse are keyboard-navigable:
//!   [`Toolbar::pre_input`] runs before the document surface and takes Up/Down/Home/End,
//!   Enter and Escape while one is open; closing returns the focus to the document.
//! - **Narrow windows.** Groups collapse from the right into the More menu.
//! - **Idle** (NFR-10): the bar never schedules repaints of its own.
//!
//! The icons are original line drawings (SVG path syntax, drawn as strokes by
//! [`crate::icons`]).

use std::sync::OnceLock;

use egui::{Color32, Context, CornerRadius, Id, Key, Order, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::icons::{Lines, parse_path};
use crate::sidebar::SideLook;

/// Block-style drop-down entries (group 1).
pub const BLOCK_IDS: [&str; 7] = [
    "block.paragraph",
    "block.heading1",
    "block.heading2",
    "block.heading3",
    "block.heading4",
    "block.heading5",
    "block.heading6",
];
/// Inline toggles (group 2).
pub const INLINE_IDS: [&str; 4] = ["format.strong", "format.emph", "format.code", "format.link"];
/// List drop-down entries (part of group 3).
pub const LIST_IDS: [&str; 3] = ["block.bulletList", "block.orderedList", "block.taskList"];
/// Group 3 buttons before the list drop-down.
pub const INSERT_IDS: [&str; 2] = ["format.image", "block.quote"];
/// The More menu's own entries (`None`: separator). Ids the registry does not have (yet) are
/// left out, as in the menus.
pub const MORE_IDS: &[Option<&str>] = &[
    Some("format.strike"),
    Some("format.highlight"),
    Some("format.underline"),
    Some("format.superscript"),
    Some("format.subscript"),
    Some("format.inlineMath"),
    None,
    Some("block.codeFence"),
    Some("block.mathBlock"),
    Some("insert.table"),
    Some("block.insertRule"),
    Some("insert.rule"),
    Some("markdown.insertFootnote"),
];

/// Every command id the toolbar can show.
pub fn all_ids() -> impl Iterator<Item = &'static str> {
    BLOCK_IDS
        .iter()
        .chain(INLINE_IDS.iter())
        .chain(INSERT_IDS.iter())
        .chain(LIST_IDS.iter())
        .copied()
        .chain(MORE_IDS.iter().flatten().copied())
}

/// One control's state, built by the application from the registry.
#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    pub id: &'static str,
    /// Command label (menu title).
    pub label: &'static str,
    /// Effective shortcut (`keyBinding` applied), as text.
    pub shortcut: Option<String>,
    pub enabled: bool,
    /// The selection has this format / block type (menu check mark).
    pub active: bool,
}

impl Control {
    /// Tool tip: command name and shortcut.
    pub fn tooltip(&self) -> String {
        match &self.shortcut {
            Some(s) => format!("{} ({s})", self.label),
            None => self.label.to_string(),
        }
    }
}

/// The controls of the commands present in the registry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    pub controls: Vec<Control>,
}

impl Model {
    pub fn get(&self, id: &str) -> Option<&Control> {
        self.controls.iter().find(|c| c.id == id)
    }

    fn has_any(&self, ids: &[&str]) -> bool {
        ids.iter().any(|id| self.get(id).is_some())
    }

    /// Text of the block-style drop-down: the active entry, or "—" (a block that is neither
    /// a paragraph nor a heading, e.g. a code block).
    pub fn block_label(&self) -> &'static str {
        BLOCK_IDS
            .iter()
            .filter_map(|id| self.get(id))
            .find(|c| c.active)
            .map_or("—", |c| c.label)
    }
}

/// The toolbar's groups, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Group {
    Block,
    Inline,
    Insert,
}

const GROUPS: [Group; 3] = [Group::Block, Group::Inline, Group::Insert];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Popup {
    Block,
    List,
    More,
}

/// A drop-down row (`None` in a list: separator).
#[derive(Clone, Debug)]
struct Row {
    id: &'static str,
    label: &'static str,
    shortcut: Option<String>,
    enabled: bool,
    checked: bool,
}

// ---- geometry
const BAR_H: f32 = 34.0;
const BTN: f32 = 28.0;
const PAD: f32 = 3.0;
const GAP: f32 = 2.0;
const DIVIDER: f32 = 9.0;
const BLOCK_W: f32 = 112.0;
const LIST_W: f32 = 40.0;
const MARGIN_TOP: f32 = 8.0;
/// Height the rendered view keeps free above the document for the bar (so the bar never
/// covers the first lines; the document scrolls below it).
pub const RESERVED_H: f32 = MARGIN_TOP + BAR_H + 6.0;
const ROW_H: f32 = 24.0;
const POPUP_W: f32 = 240.0;

fn group_width(g: Group) -> f32 {
    match g {
        Group::Block => BLOCK_W,
        Group::Inline => 4.0 * BTN + 3.0 * GAP,
        Group::Insert => 2.0 * BTN + LIST_W + 2.0 * GAP,
    }
}

fn group_present(g: Group, m: &Model) -> bool {
    match g {
        Group::Block => m.has_any(&BLOCK_IDS),
        Group::Inline => m.has_any(&INLINE_IDS),
        Group::Insert => m.has_any(&INSERT_IDS) || m.has_any(&LIST_IDS),
    }
}

/// Groups shown in the bar for `avail` points of width (the rest collapses into More, from the
/// right) and the bar's width.
fn fit(m: &Model, avail: f32) -> (Vec<Group>, f32) {
    let mut shown: Vec<Group> = GROUPS.iter().copied().filter(|g| group_present(*g, m)).collect();
    let width = |s: &[Group]| 2.0 * PAD + s.iter().map(|g| group_width(*g) + DIVIDER).sum::<f32>() + BTN;
    while !shown.is_empty() && width(&shown) > avail {
        shown.pop();
    }
    let w = width(&shown);
    (shown, w)
}

/// Toolbar state (per window).
#[derive(Default)]
pub struct Toolbar {
    open: Option<Popup>,
    /// Keyboard-highlighted row of the open drop-down.
    hi: Option<usize>,
    /// The widget to give the focus back to when the drop-down closes.
    return_focus: Option<Id>,
    /// The focused widget before the document surface ran this frame.
    focus_before: Option<Id>,
    /// Groups collapsed into More at the last frame.
    collapsed: Vec<Group>,
    bar_rect: Option<Rect>,
    popup_rect: Option<Rect>,
    /// The control that opened the drop-down (it is placed below it).
    anchor: Option<Rect>,
}

impl Toolbar {
    /// Whether a drop-down is open (tests).
    pub fn popup_open(&self) -> bool {
        self.open.is_some()
    }

    /// Groups collapsed into More at the last frame (tests).
    pub fn collapsed_groups(&self) -> usize {
        self.collapsed.len()
    }

    /// Closes any drop-down (toolbar hidden, mode change).
    pub fn close(&mut self) {
        self.open = None;
        self.hi = None;
        self.popup_rect = None;
    }

    fn rows(&self, popup: Popup, m: &Model) -> Vec<Option<Row>> {
        let row = |id: &str| {
            m.get(id).map(|c| {
                Some(Row {
                    id: c.id,
                    label: c.label,
                    shortcut: c.shortcut.clone(),
                    enabled: c.enabled,
                    checked: c.active,
                })
            })
        };
        let list = |ids: &[&str]| ids.iter().filter_map(|id| row(id)).collect::<Vec<_>>();
        let mut out = match popup {
            Popup::Block => list(&BLOCK_IDS),
            Popup::List => list(&LIST_IDS),
            Popup::More => {
                let mut v = Vec::new();
                for g in &self.collapsed {
                    v.extend(match g {
                        Group::Block => list(&BLOCK_IDS),
                        Group::Inline => list(&INLINE_IDS),
                        Group::Insert => {
                            let mut r = list(&INSERT_IDS);
                            r.extend(list(&LIST_IDS));
                            r
                        }
                    });
                    v.push(None);
                }
                v.extend(MORE_IDS.iter().filter_map(|e| match e {
                    None => Some(None),
                    Some(id) => row(id),
                }));
                v
            }
        };
        // no leading, trailing or repeated separators
        let mut tidy: Vec<Option<Row>> = Vec::with_capacity(out.len());
        for r in out.drain(..) {
            if r.is_some() || matches!(tidy.last(), Some(Some(_))) {
                tidy.push(r);
            }
        }
        while matches!(tidy.last(), Some(None)) {
            tidy.pop();
        }
        tidy
    }

    /// Runs before the document surface: remembers the focused widget and, while a drop-down
    /// is open, handles its keys (they never reach the document). Returns a chosen command.
    pub fn pre_input(&mut self, ctx: &Context, m: &Model) -> Option<&'static str> {
        self.focus_before = ctx.memory(|mem| mem.focused());
        let popup = self.open?;
        let rows = self.rows(popup, m);
        let selectable: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.as_ref().is_some_and(|r| r.enabled))
            .map(|(i, _)| i)
            .collect();
        let mut keys = Vec::new();
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                egui::Event::Key {
                    key: k @ (Key::ArrowUp | Key::ArrowDown | Key::Home | Key::End | Key::Enter | Key::Escape | Key::Tab),
                    pressed,
                    ..
                } => {
                    if *pressed {
                        keys.push(*k);
                    }
                    false
                }
                _ => true,
            })
        });
        let mut chosen = None;
        for k in keys {
            let pos = self.hi.and_then(|h| selectable.iter().position(|&i| i == h));
            match k {
                Key::ArrowDown => {
                    self.hi = match pos {
                        Some(p) => selectable.get(p + 1).or(selectable.first()).copied(),
                        None => selectable.first().copied(),
                    }
                }
                Key::ArrowUp => {
                    self.hi = match pos {
                        Some(0) | None => selectable.last().copied(),
                        Some(p) => selectable.get(p - 1).copied(),
                    }
                }
                Key::Home => self.hi = selectable.first().copied(),
                Key::End => self.hi = selectable.last().copied(),
                Key::Enter => {
                    if let Some(Some(r)) = self.hi.and_then(|h| rows.get(h))
                        && r.enabled
                    {
                        chosen = Some(r.id);
                    }
                    self.finish(ctx);
                    break;
                }
                // Escape (and Tab) close without acting
                _ => {
                    self.finish(ctx);
                    break;
                }
            }
        }
        ctx.request_repaint();
        chosen
    }

    /// Closes the drop-down and gives the focus back to the document.
    fn finish(&mut self, ctx: &Context) {
        self.close();
        if let Some(f) = self.return_focus.take() {
            ctx.memory_mut(|mem| mem.request_focus(f));
            self.focus_before = Some(f);
        }
    }

    fn contains(&self, p: Pos2) -> bool {
        self.bar_rect.is_some_and(|r| r.contains(p)) || self.popup_rect.is_some_and(|r| r.contains(p))
    }

    /// Shows the bar at the top centre of `area` (the editor area). `active`: the bar acts on
    /// this area (otherwise every control is shown disabled). Returns the chosen command.
    pub fn show(&mut self, ui: &mut Ui, area: Rect, m: &Model, look: &Look) -> Option<&'static str> {
        let ctx = ui.ctx().clone();
        // a press outside the bar and its drop-down closes the drop-down (the press acts there)
        let press = ctx.input(|i| i.pointer.any_pressed().then(|| i.pointer.interact_pos()).flatten());
        if let Some(p) = press
            && self.open.is_some()
            && !self.contains(p)
        {
            self.close();
            self.return_focus = None;
        }
        let avail = area.width() - 16.0;
        let (shown, width) = fit(m, avail);
        self.collapsed = GROUPS
            .iter()
            .copied()
            .filter(|g| group_present(*g, m) && !shown.contains(g))
            .collect();
        let left = (area.center().x - width / 2.0).max(area.left());
        let bar = Rect::from_min_size(
            pos2(left.round(), (area.top() + MARGIN_TOP).round()),
            vec2(width, BAR_H),
        );
        self.bar_rect = Some(bar);
        let mut chosen = None;
        let mut toggle_popup = None;
        egui::Area::new(Id::unique("markise-toolbar"))
            .order(Order::Middle)
            .fixed_pos(bar.min)
            .constrain(false)
            .fade_in(false)
            .show(&ctx, |ui| {
                let (rect, bg) = ui.allocate_exact_size(bar.size(), Sense::click());
                bg.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Toolbar, true, "Formatting Toolbar"));
                let p = ui.painter();
                let radius = CornerRadius::same(8);
                p.add(
                    egui::epaint::Shadow {
                        offset: [0, 2],
                        blur: 10,
                        spread: 0,
                        color: Color32::from_black_alpha(28),
                    }
                    .as_shape(rect, radius),
                );
                let fill = look.bar_bg;
                p.rect(rect, radius, fill, Stroke::new(1.0, look.border), StrokeKind::Inside);
                let mut x = rect.left() + PAD;
                let cy = rect.center().y;
                let slot = |x: f32, w: f32| Rect::from_center_size(pos2(x + w / 2.0, cy), vec2(w, BTN));
                for g in &shown {
                    match g {
                        Group::Block => {
                            let r = slot(x, BLOCK_W);
                            let enabled = BLOCK_IDS.iter().filter_map(|id| m.get(id)).any(|c| c.enabled);
                            let open = self.open == Some(Popup::Block);
                            let resp = drop_button(ui, r, look, enabled, open, "Block Style", |p, r, c| {
                                p.text(
                                    pos2(r.left() + 8.0, r.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    m.block_label(),
                                    look.font.clone(),
                                    c,
                                );
                            });
                            let resp = resp.on_hover_text("Paragraph and heading style");
                            if resp.clicked() && enabled {
                                toggle_popup = Some((Popup::Block, r));
                            }
                            x += BLOCK_W;
                        }
                        Group::Inline => {
                            for (k, id) in INLINE_IDS.iter().enumerate() {
                                if k > 0 {
                                    x += GAP;
                                }
                                if let Some(c) = m.get(id) {
                                    let r = slot(x, BTN);
                                    if tool_button(ui, r, look, c, icon_for(id)) {
                                        chosen = Some(c.id);
                                    }
                                }
                                x += BTN;
                            }
                        }
                        Group::Insert => {
                            for id in INSERT_IDS {
                                if let Some(c) = m.get(id) {
                                    let r = slot(x, BTN);
                                    if tool_button(ui, r, look, c, icon_for(id)) {
                                        chosen = Some(c.id);
                                    }
                                }
                                x += BTN + GAP;
                            }
                            let lists: Vec<&Control> = LIST_IDS.iter().filter_map(|id| m.get(id)).collect();
                            if !lists.is_empty() {
                                let r = slot(x, LIST_W);
                                let enabled = lists.iter().any(|c| c.enabled);
                                let active = lists.iter().find(|c| c.active);
                                let icon = icon_for(active.map_or("block.bulletList", |c| c.id));
                                let open = self.open == Some(Popup::List);
                                let resp =
                                    drop_button(ui, r, look, enabled, open || active.is_some(), "List", |p, r, c| {
                                        let ir = Rect::from_center_size(
                                            pos2(r.left() + 14.0, r.center().y),
                                            vec2(16.0, 16.0),
                                        );
                                        paint_icon(p, ir, icon, c);
                                    });
                                if resp.on_hover_text("List").clicked() && enabled {
                                    toggle_popup = Some((Popup::List, r));
                                }
                            }
                            x += LIST_W;
                        }
                    }
                    // divider
                    let dx = x + DIVIDER / 2.0;
                    ui.painter()
                        .line_segment([pos2(dx, cy - 9.0), pos2(dx, cy + 9.0)], Stroke::new(1.0, look.border));
                    x += DIVIDER;
                }
                // More
                let r = slot(x, BTN);
                let more_rows = self.rows(Popup::More, m);
                let enabled = more_rows.iter().flatten().any(|r| r.enabled);
                let open = self.open == Some(Popup::More);
                let resp = drop_button(ui, r, look, enabled, open, "More", |p, r, c| {
                    for k in -1..=1 {
                        p.circle_filled(pos2(r.center().x + k as f32 * 5.0, r.center().y), 1.6, c);
                    }
                });
                if resp.on_hover_text("More formats and insertions").clicked() && enabled {
                    toggle_popup = Some((Popup::More, r));
                }
            });

        if let Some((popup, anchor)) = toggle_popup {
            if self.open == Some(popup) {
                self.close();
                self.return_focus = None;
            } else {
                self.open = Some(popup);
                self.hi = None;
                self.anchor = Some(anchor);
                self.return_focus = self.focus_before.or(self.return_focus);
            }
        }
        if let Some(popup) = self.open {
            let rows = self.rows(popup, m);
            if let Some(id) = self.popup_ui(&ctx, popup, &rows, look) {
                chosen = Some(id);
                self.close();
            }
        } else {
            self.popup_rect = None;
        }
        if chosen.is_some() {
            self.close();
            self.return_focus = None;
        }
        // mouse use never moves the text focus: a press on the bar or a drop-down gives it back
        let on_us = ctx.input(|i| {
            (i.pointer.any_pressed() || i.pointer.any_released() || i.pointer.any_click())
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| self.contains(p) || bar.contains(p))
        });
        if (on_us || chosen.is_some())
            && let Some(f) = self.focus_before
            && ctx.memory(|mem| mem.focused()) != Some(f)
        {
            ctx.memory_mut(|mem| mem.request_focus(f));
        }
        chosen
    }

    fn popup_ui(&mut self, ctx: &Context, popup: Popup, rows: &[Option<Row>], look: &Look) -> Option<&'static str> {
        let mut chosen = None;
        let h: f32 = rows.iter().map(|r| if r.is_some() { ROW_H } else { 9.0 }).sum::<f32>() + 8.0;
        let screen = ctx.content_rect();
        let anchor = self.anchor.unwrap_or(Rect::from_min_size(Pos2::ZERO, vec2(BTN, BTN)));
        let mut pos = pos2(anchor.left(), anchor.bottom() + 6.0);
        if pos.x + POPUP_W > screen.right() - 4.0 {
            pos.x = (anchor.right() - POPUP_W).max(screen.left() + 4.0);
        }
        let rect = Rect::from_min_size(pos, vec2(POPUP_W, h));
        self.popup_rect = Some(rect);
        let label = match popup {
            Popup::Block => "Block Style Menu",
            Popup::List => "List Menu",
            Popup::More => "More Menu",
        };
        egui::Area::new(Id::unique("markise-toolbar-popup"))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .constrain(false)
            .fade_in(false)
            .show(ctx, |ui| {
                let (rect, bg) = ui.allocate_exact_size(rect.size(), Sense::click());
                bg.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Menu, true, label));
                let p = ui.painter();
                let radius = CornerRadius::same(6);
                p.add(
                    egui::epaint::Shadow {
                        offset: [0, 3],
                        blur: 12,
                        spread: 0,
                        color: Color32::from_black_alpha(36),
                    }
                    .as_shape(rect, radius),
                );
                p.rect(
                    rect,
                    radius,
                    look.bar_bg,
                    Stroke::new(1.0, look.border),
                    StrokeKind::Inside,
                );
                let mut y = rect.top() + 4.0;
                for (i, r) in rows.iter().enumerate() {
                    let Some(r) = r else {
                        let sy = y + 4.5;
                        ui.painter().line_segment(
                            [pos2(rect.left() + 8.0, sy), pos2(rect.right() - 8.0, sy)],
                            Stroke::new(1.0, look.border),
                        );
                        y += 9.0;
                        continue;
                    };
                    let rr = Rect::from_min_size(pos2(rect.left() + 4.0, y), vec2(rect.width() - 8.0, ROW_H));
                    y += ROW_H;
                    let resp = ui.interact(rr, Id::unique(("markise-toolbar-row", r.id)), Sense::click());
                    resp.widget_info(|| {
                        egui::WidgetInfo::selected(egui::Role::MenuItem, r.enabled, r.checked, r.label)
                    });
                    if resp.hovered() && r.enabled && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                        self.hi = Some(i);
                    }
                    let hi = self.hi == Some(i);
                    let p = ui.painter();
                    if hi && r.enabled {
                        p.rect_filled(rr, CornerRadius::same(4), look.hover_bg);
                    }
                    let c = if !r.enabled {
                        look.disabled
                    } else if hi {
                        look.hover_text
                    } else {
                        look.text
                    };
                    if r.checked {
                        let cr = Rect::from_center_size(pos2(rr.left() + 11.0, rr.center().y), vec2(12.0, 12.0));
                        paint_icon(p, cr, ToolIcon::Check, c);
                    }
                    p.text(
                        pos2(rr.left() + 24.0, rr.center().y),
                        egui::Align2::LEFT_CENTER,
                        r.label,
                        look.font.clone(),
                        c,
                    );
                    if let Some(s) = &r.shortcut {
                        p.text(
                            pos2(rr.right() - 8.0, rr.center().y),
                            egui::Align2::RIGHT_CENTER,
                            s,
                            look.small_font.clone(),
                            look.disabled,
                        );
                    }
                    if resp.clicked() && r.enabled {
                        chosen = Some(r.id);
                    }
                }
            });
        chosen
    }
}

/// Colours and fonts of the bar: the sidebar tokens (spec 04 §3.2).
#[derive(Clone, Debug)]
pub struct Look {
    pub font: egui::FontId,
    pub small_font: egui::FontId,
    pub text: Color32,
    pub active_text: Color32,
    pub active_bg: Color32,
    pub hover_text: Color32,
    pub hover_bg: Color32,
    pub bar_bg: Color32,
    pub border: Color32,
    pub disabled: Color32,
}

impl Look {
    pub fn new(t: &markise_theme::Theme) -> Look {
        let s = SideLook::new(t);
        let bar_bg = markise_layout::block::color(t.chrome.sidebar_bg);
        let mut small_font = s.font.clone();
        small_font.size = (s.font.size - 1.5).max(9.0);
        Look {
            text: s.text,
            active_text: s.active_text,
            active_bg: s.active_bg,
            hover_text: s.hover_text,
            hover_bg: s.hover_bg,
            border: s.text.lerp_to_gamma(bar_bg, 0.8),
            disabled: s.text.lerp_to_gamma(bar_bg, 0.6),
            font: s.font,
            small_font,
            bar_bg,
        }
    }
}

/// A toggle button; returns whether it was clicked (enabled only).
fn tool_button(ui: &mut Ui, r: Rect, look: &Look, c: &Control, icon: ToolIcon) -> bool {
    let sense = if c.enabled { Sense::click() } else { Sense::hover() };
    let resp = ui.interact(r, Id::unique(("markise-toolbar", c.id)), sense);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, c.enabled, c.active, c.label));
    let p = ui.painter();
    let radius = CornerRadius::same(5);
    let color = if !c.enabled {
        look.disabled
    } else if c.active {
        p.rect_filled(r, radius, look.active_bg);
        look.active_text
    } else if resp.hovered() {
        p.rect_filled(r, radius, look.hover_bg);
        look.hover_text
    } else {
        look.text
    };
    let ir = Rect::from_center_size(r.center(), vec2(16.0, 16.0));
    paint_icon(p, ir, icon, color);
    let tip = c.tooltip();
    resp.on_hover_text(tip).clicked() && c.enabled
}

/// A drop-down button (content painted by `paint`, a chevron at the right).
fn drop_button(
    ui: &mut Ui,
    r: Rect,
    look: &Look,
    enabled: bool,
    on: bool,
    label: &str,
    paint: impl FnOnce(&egui::Painter, Rect, Color32),
) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let resp = ui.interact(r, Id::unique(("markise-toolbar-drop", label)), sense);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::ComboBox, enabled, on, label));
    let p = ui.painter();
    let radius = CornerRadius::same(5);
    let color = if !enabled {
        look.disabled
    } else if on {
        p.rect_filled(r, radius, look.active_bg);
        look.active_text
    } else if resp.hovered() {
        p.rect_filled(r, radius, look.hover_bg);
        look.hover_text
    } else {
        look.text
    };
    paint(p, r, color);
    if label != "More" {
        let cr = Rect::from_center_size(pos2(r.right() - 9.0, r.center().y), vec2(10.0, 10.0));
        paint_icon(p, cr, ToolIcon::Chevron, color);
    }
    resp
}

// ------------------------------------------------------------------ icons

/// Toolbar icons (original drawings in a 24×24 box).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolIcon {
    Bold,
    Italic,
    Code,
    Link,
    Image,
    Quote,
    Bullet,
    Ordered,
    Task,
    Chevron,
    Check,
}

const ICONS: [(ToolIcon, &str); 11] = [
    (
        ToolIcon::Bold,
        "M7 4H13.5A4 4 0 0 1 13.5 12H7Z M7 12H14.5A4 4 0 0 1 14.5 20H7Z",
    ),
    (ToolIcon::Italic, "M10 4H18 M6 20H14 M15 4L9 20"),
    (ToolIcon::Code, "M8 6L2.5 12L8 18 M16 6L21.5 12L16 18"),
    (
        ToolIcon::Link,
        "M10.5 13.5A3.5 3.5 0 0 0 15.5 13.5L19 10A3.5 3.5 0 0 0 14 5L12.5 6.5 \
         M13.5 10.5A3.5 3.5 0 0 0 8.5 10.5L5 14A3.5 3.5 0 0 0 10 19L11.5 17.5",
    ),
    (
        ToolIcon::Image,
        "M4 5H20V19H4Z M4 16L9 11L13 15L15.5 12.5L20 17 M14 8.5A1.5 1.5 0 1 0 17 8.5A1.5 1.5 0 1 0 14 8.5",
    ),
    (
        ToolIcon::Quote,
        "M4 18C7 17 8.5 14.5 8.5 11V6H4V11H8.5 M13.5 18C16.5 17 18 14.5 18 11V6H13.5V11H18",
    ),
    (ToolIcon::Bullet, "M10 6H20 M10 12H20 M10 18H20"),
    (
        ToolIcon::Ordered,
        "M10 6H20 M10 12H20 M10 18H20 M4 4.5L5.5 3.5V9 M3.5 13A1.5 1.5 0 0 1 6.5 13C6.5 14.5 3.5 15.5 3.5 17H6.5",
    ),
    (
        ToolIcon::Task,
        "M3 4H8V9H3Z M3 15H8V20H3Z M4 6.5L5.3 7.8L7.2 5.2 M11 6.5H21 M11 17.5H21",
    ),
    (ToolIcon::Chevron, "M6 9L12 15L18 9"),
    (ToolIcon::Check, "M4 12.5L9.5 18L20 6"),
];

fn icon_lines(icon: ToolIcon) -> &'static Lines {
    static CACHE: OnceLock<Vec<(ToolIcon, Lines)>> = OnceLock::new();
    let c = CACHE.get_or_init(|| ICONS.iter().map(|(i, d)| (*i, parse_path(d))).collect());
    &c.iter().find(|(i, _)| *i == icon).expect("icon").1
}

fn icon_for(id: &str) -> ToolIcon {
    match id {
        "format.strong" => ToolIcon::Bold,
        "format.emph" => ToolIcon::Italic,
        "format.code" => ToolIcon::Code,
        "format.link" => ToolIcon::Link,
        "format.image" => ToolIcon::Image,
        "block.quote" => ToolIcon::Quote,
        "block.orderedList" => ToolIcon::Ordered,
        "block.taskList" => ToolIcon::Task,
        _ => ToolIcon::Bullet,
    }
}

/// Draws `icon` into `rect` (square).
pub fn paint_icon(p: &egui::Painter, rect: Rect, icon: ToolIcon, color: Color32) {
    let s = rect.width().min(rect.height()) / 24.0;
    let width = match icon {
        ToolIcon::Bold => 2.6,
        _ => 2.0,
    };
    let stroke = Stroke::new((width * s).max(1.0), color);
    for l in icon_lines(icon) {
        let pts: Vec<Pos2> = l.iter().map(|q| rect.min + vec2(q.x * s, q.y * s)).collect();
        p.add(egui::Shape::line(pts, stroke));
    }
    if icon == ToolIcon::Bullet {
        for y in [6.0, 12.0, 18.0] {
            p.circle_filled(rect.min + vec2(5.0 * s, y * s), 1.7 * s.max(0.6), color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(ids: &[&'static str]) -> Model {
        Model {
            controls: ids
                .iter()
                .map(|id| Control {
                    id,
                    label: id,
                    shortcut: None,
                    enabled: true,
                    active: false,
                })
                .collect(),
        }
    }

    #[test]
    fn icons_stay_inside_their_box() {
        for (icon, _) in ICONS {
            let lines = icon_lines(icon);
            assert!(!lines.is_empty(), "{icon:?}");
            for p in lines.iter().flatten() {
                assert!(
                    (0.0..=24.0).contains(&p.x) && (0.0..=24.0).contains(&p.y),
                    "{icon:?} {p:?}"
                );
            }
        }
    }

    /// Spec 04 §3.2: groups collapse from the right into the More menu.
    #[test]
    fn groups_collapse_from_the_right() {
        let m = model(&all_ids().collect::<Vec<_>>());
        let (all, full) = fit(&m, 2000.0);
        assert_eq!(all, GROUPS);
        let (two, w2) = fit(&m, full - 1.0);
        assert_eq!(two, [Group::Block, Group::Inline]);
        assert!(w2 < full);
        let (none, w0) = fit(&m, 10.0);
        assert!(none.is_empty());
        assert_eq!(w0, 2.0 * PAD + BTN);
        // collapsed groups come first in More, then More's own entries
        let tb = Toolbar {
            collapsed: vec![Group::Insert],
            ..Default::default()
        };
        let rows = tb.rows(Popup::More, &m);
        assert_eq!(rows[0].as_ref().unwrap().id, "format.image");
        assert!(rows.iter().flatten().any(|r| r.id == "block.taskList"));
        assert!(rows.iter().flatten().any(|r| r.id == "format.strike"));
        // no doubled, leading or trailing separators
        assert!(rows.first().unwrap().is_some() && rows.last().unwrap().is_some());
        assert!(rows.windows(2).all(|w| w[0].is_some() || w[1].is_some()));
    }

    #[test]
    fn missing_commands_are_left_out_and_block_label_follows_state() {
        let mut m = model(&["block.paragraph", "block.heading2", "format.strong"]);
        assert_eq!(m.block_label(), "—");
        m.controls[1].active = true;
        assert_eq!(m.block_label(), "block.heading2");
        let tb = Toolbar::default();
        assert_eq!(tb.rows(Popup::Block, &m).len(), 2);
        assert!(tb.rows(Popup::More, &m).is_empty());
        let (shown, _) = fit(&m, 2000.0);
        assert_eq!(shown, [Group::Block, Group::Inline]);
    }
}

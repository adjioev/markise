//! PROTOTYPE (branch `proto/block-packages`, not product code): the block palette — a native
//! egui list in the right sidekick panel (blocks of the bound library with presets, search),
//! one insertion mechanism behind several entries:
//!
//! - click on a preset → insert at the caret; drag it onto the page → drop between
//!   paragraphs/blocks (the viewer draws the insertion line, never inside a block);
//! - "+ Добавить блок" between items, `/` on an empty line and Paragraph › Insert Block… open
//!   the palette with its search focused and a fixed insertion point.
//!
//! Choosing never writes: the viewer opens a draft form ([`crate::block_edit`]); Save inserts.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use egui::{Color32, CursorIcon, Rect, RichText, Sense, Stroke, Ui, vec2};

use crate::block_edit::InsertTarget;
use crate::block_lib::Preset;

/// Drag payload of a palette preset.
#[derive(Clone, Debug)]
pub struct DragPreset(pub Preset);

#[derive(Default)]
struct State {
    open: bool,
    /// Opened for a fixed point ("+", `/`, menu); None: the caret.
    target: Option<InsertTarget>,
    focus_search: bool,
    query: String,
    /// Chosen preset waiting for the viewer, with its point.
    chosen: Option<(Preset, Option<InsertTarget>)>,
    cache: Option<(PathBuf, String, Vec<Preset>)>,
    /// The palette is shown because an entry opened it (closed again after the choice).
    transient: bool,
    /// Keyboard highlight (row index) and blocks whose presets are expanded.
    hl: usize,
    expanded: std::collections::HashSet<String>,
    /// Hovered row (preset markdown) and since when (preview popover after 300 ms).
    hover: Option<(String, std::time::Instant)>,
}

thread_local! {
    static ST: RefCell<State> = RefCell::new(State::default());
}

pub fn is_open() -> bool {
    ST.with(|s| s.borrow().open)
}

/// View › Block Palette.
pub fn toggle() {
    ST.with(|s| {
        let mut s = s.borrow_mut();
        s.open = !s.open;
        s.transient = false;
        s.target = None;
        s.focus_search = s.open;
    });
}

/// Opens the palette with the search focused for `target` (None: the caret).
pub fn open_for(target: Option<InsertTarget>) {
    ST.with(|s| {
        let mut s = s.borrow_mut();
        if !s.open {
            s.transient = true;
        }
        s.open = true;
        s.target = target;
        s.focus_search = true;
        s.query.clear();
    });
}

/// The insertion point the palette was opened for (scripted checks).
pub fn target() -> Option<InsertTarget> {
    ST.with(|s| s.borrow().target.clone())
}

/// Closes the palette.
pub fn close() {
    ST.with(|s| {
        let mut s = s.borrow_mut();
        s.open = false;
        s.target = None;
    });
}

/// A preset dropped on the page (the viewer calls this).
pub fn choose(p: Preset, target: Option<InsertTarget>) {
    ST.with(|s| s.borrow_mut().chosen = Some((p, target)));
}

/// The choice waiting for the viewer.
pub fn take_choice() -> Option<(Preset, Option<InsertTarget>)> {
    ST.with(|s| {
        let mut s = s.borrow_mut();
        let c = s.chosen.take();
        if c.is_some() && s.transient {
            s.open = false;
            s.transient = false;
        }
        c
    })
}

fn presets_of(root: &Path, lang: &str) -> Vec<Preset> {
    ST.with(|s| {
        let mut s = s.borrow_mut();
        if let Some((r, l, p)) = &s.cache
            && r == root
            && l == lang
        {
            return p.clone();
        }
        let p = crate::block_lib::presets(root, lang);
        s.cache = Some((root.to_path_buf(), lang.to_string(), p.clone()));
        p
    })
}

/// Group heading of a block type (prototype mapping).
fn group_of(block: &str, ru: bool) -> &'static str {
    match block {
        "question" => {
            if ru {
                "Вопросы"
            } else {
                "Questions"
            }
        }
        "scene" => {
            if ru {
                "Медиа"
            } else {
                "Media"
            }
        }
        "note" | "takeaway" | "comparison" | "goals" => {
            if ru {
                "Текст"
            } else {
                "Text"
            }
        }
        _ => {
            if ru {
                "Другое"
            } else {
                "Other"
            }
        }
    }
}

fn group_rank(block: &str) -> u8 {
    match block {
        "question" => 0,
        "note" | "takeaway" | "comparison" | "goals" => 1,
        "scene" => 2,
        _ => 3,
    }
}

/// A preset's short text: the part after "Тип: " in its label.
fn chip_text(label: &str) -> &str {
    label.split_once(": ").map_or(label, |(_, b)| b)
}

/// A small line icon for a block type (24 × 24 box), drawn with shapes.
pub fn paint_icon(p: &egui::Painter, r: Rect, block: &str, c: Color32) {
    let s = Stroke::new(1.6, c);
    let k = r.width() / 24.0;
    let at = |x: f32, y: f32| r.min + vec2(x * k, y * k);
    let line = |a: (f32, f32), b: (f32, f32)| p.line_segment([at(a.0, a.1), at(b.0, b.1)], s);
    match block {
        "question" => {
            // list with checks
            for (i, y) in [6.0, 12.0, 18.0].into_iter().enumerate() {
                if i == 1 {
                    line((3.5, y), (5.0, y + 1.5));
                    line((5.0, y + 1.5), (7.5, y - 1.5));
                } else {
                    p.circle_stroke(at(5.5, y), 1.8 * k, s);
                }
                line((10.0, y), (21.0, y));
            }
        }
        "note" => {
            // sticky note with a folded corner
            p.add(egui::Shape::line(
                vec![
                    at(4.0, 4.0),
                    at(20.0, 4.0),
                    at(20.0, 14.0),
                    at(14.0, 20.0),
                    at(4.0, 20.0),
                    at(4.0, 4.0),
                ],
                s,
            ));
            p.add(egui::Shape::line(
                vec![at(20.0, 14.0), at(14.0, 14.0), at(14.0, 20.0)],
                s,
            ));
        }
        "takeaway" => {
            // lightbulb
            p.circle_stroke(at(12.0, 10.0), 6.0 * k, s);
            line((9.5, 16.5), (9.5, 19.0));
            line((14.5, 16.5), (14.5, 19.0));
            line((9.5, 19.0), (14.5, 19.0));
            line((10.5, 21.5), (13.5, 21.5));
        }
        "goals" => {
            // target
            p.circle_stroke(at(12.0, 12.0), 8.5 * k, s);
            p.circle_stroke(at(12.0, 12.0), 5.0 * k, s);
            p.circle_filled(at(12.0, 12.0), 1.8 * k, c);
        }
        "comparison" => {
            // two columns
            p.rect_stroke(
                Rect::from_min_max(at(3.5, 5.0), at(20.5, 19.0)),
                2.5 * k,
                s,
                egui::StrokeKind::Middle,
            );
            line((12.0, 5.0), (12.0, 19.0));
            line((6.0, 9.0), (9.5, 9.0));
            line((14.5, 9.0), (18.0, 9.0));
        }
        "scene" => {
            // image
            p.rect_stroke(
                Rect::from_min_max(at(3.5, 5.0), at(20.5, 19.0)),
                2.5 * k,
                s,
                egui::StrokeKind::Middle,
            );
            p.circle_stroke(at(8.5, 9.5), 1.6 * k, s);
            p.add(egui::Shape::line(
                vec![
                    at(3.5, 17.0),
                    at(9.0, 12.5),
                    at(13.0, 15.5),
                    at(16.0, 13.0),
                    at(20.5, 16.5),
                ],
                s,
            ));
        }
        _ => {
            p.rect_stroke(
                Rect::from_min_max(at(5.0, 5.0), at(19.0, 19.0)),
                3.0 * k,
                s,
                egui::StrokeKind::Middle,
            );
        }
    }
}

/// Width (CSS px) of the hover preview render; shown scaled to [`POP_W`].
const THUMB_W: f32 = 380.0;
const POP_W: f32 = 320.0;

enum Row<'a> {
    Block(&'a str, Vec<&'a Preset>),
    Sub(&'a Preset),
}

/// Draws the palette into `rect` (Notion/Gutenberg-inserter style): header with ×, search,
/// compact rows (icon, label, description), presets as expandable sub-rows, keyboard
/// navigation, a full preview popover on hover.
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    root: Option<&Path>,
    accent: Color32,
    mut snaps: Option<&mut crate::block_snap::BlockSnaps>,
) {
    let lang = crate::block_edit::lang();
    let ru = lang == "ru";
    let inner = rect.shrink2(vec2(12.0, 12.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).id_salt("markise-block-palette"));
    let ui = &mut child;
    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
    let weak = ui.visuals().weak_text_color();
    let line = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let text_c = ui.visuals().text_color();
    // header
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(RichText::new(if ru { "Блоки" } else { "Blocks" }).size(16.0).strong())
            .on_hover_text(if ru {
                "Щелчок или Enter — вставить у курсора; перетащите строку на страницу, чтобы вставить в нужное место"
            } else {
                "Click or Enter inserts at the caret; drag a row onto the page to place it"
            });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (r, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
            if resp.hovered() {
                ui.painter()
                    .rect_filled(r, 6.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            let c = r.center();
            let st = Stroke::new(1.5, if resp.hovered() { text_c } else { weak });
            ui.painter()
                .line_segment([c + vec2(-4.5, -4.5), c + vec2(4.5, 4.5)], st);
            ui.painter()
                .line_segment([c + vec2(-4.5, 4.5), c + vec2(4.5, -4.5)], st);
            if resp.on_hover_text(if ru { "Закрыть" } else { "Close" }).clicked() {
                close();
            }
        });
    });
    let Some(root) = root else {
        ui.add_space(16.0);
        ui.label(
            RichText::new(if ru {
                "Библиотека блоков не подключена"
            } else {
                "No block library"
            })
            .strong(),
        );
        ui.label(
            RichText::new(if ru {
                "Положите blocks.config.json в папку с уроками — Markise предложит подключить библиотеку."
            } else {
                "Put a blocks.config.json into the content folder; Markise will offer to connect the library."
            })
            .size(12.5)
            .color(weak),
        );
        return;
    };
    // search
    let presets = presets_of(root, &lang);
    let (mut query, focus) = ST.with(|s| {
        let mut s = s.borrow_mut();
        (s.query.clone(), std::mem::take(&mut s.focus_search))
    });
    let search = egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(Stroke::new(1.0, line))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                ui.painter()
                    .circle_stroke(r.center() + vec2(-1.5, -1.5), 4.5, Stroke::new(1.4, weak));
                ui.painter().line_segment(
                    [r.center() + vec2(2.0, 2.0), r.center() + vec2(5.5, 5.5)],
                    Stroke::new(1.6, weak),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut query)
                        .frame(egui::Frame::NONE)
                        .hint_text(if ru {
                            "Найти блок…"
                        } else {
                            "Find a block…"
                        })
                        .desired_width(f32::INFINITY),
                )
            })
            .inner
        })
        .inner;
    if focus
        || !ui.ctx().memory(|m| m.focused().is_some())
            && ST.with(|s| s.borrow().open)
            && ui.ctx().input(|i| i.pointer.any_pressed()) == false
            && focus
    {
        search.request_focus();
    }
    if ST.with(|s| s.borrow().target.is_some()) {
        ui.label(
            RichText::new(if ru {
                "Выберите блок для отмеченного места"
            } else {
                "Choose a block for the marked point"
            })
            .size(11.5)
            .color(weak),
        );
    }
    let changed = ST.with(|s| s.borrow().query != query);
    ST.with(|s| {
        let mut s = s.borrow_mut();
        s.query = query.clone();
        if changed {
            s.hl = 0;
        }
    });
    let q = query.to_lowercase();
    // rows: blocks (grouped), presets of expanded blocks (or of all when searching)
    let mut blocks: Vec<(&str, Vec<&Preset>)> = Vec::new();
    for p in &presets {
        let hay = format!("{} {} {} {}", p.block_label, p.label, p.description, p.block).to_lowercase();
        if !q.is_empty() && !hay.contains(&q) {
            continue;
        }
        match blocks.iter_mut().find(|(b, _)| *b == p.block) {
            Some((_, v)) => v.push(p),
            None => blocks.push((p.block.as_str(), vec![p])),
        }
    }
    blocks.sort_by_key(|(b, _)| group_rank(b));
    let expanded = ST.with(|s| s.borrow().expanded.clone());
    let mut rows: Vec<Row> = Vec::new();
    for (b, ps) in &blocks {
        rows.push(Row::Block(b, ps.clone()));
        if ps.len() > 1 && (expanded.contains(*b) || !q.is_empty()) {
            rows.extend(ps.iter().map(|p| Row::Sub(p)));
        }
    }
    // keyboard: ↑/↓ highlight, Enter inserts (a block row with several presets expands),
    // → expands, ← collapses
    let (up, down, enter, esc, right, left) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::ArrowUp),
            i.key_pressed(egui::Key::ArrowDown),
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
            i.key_pressed(egui::Key::ArrowRight),
            i.key_pressed(egui::Key::ArrowLeft),
        )
    });
    let keys_ok = search.has_focus() || search.lost_focus();
    let mut hl = ST.with(|s| s.borrow().hl).min(rows.len().saturating_sub(1));
    let mut chosen: Option<Preset> = None;
    let mut toggle: Option<String> = None;
    if keys_ok && !rows.is_empty() {
        if down {
            hl = (hl + 1).min(rows.len() - 1);
        }
        if up {
            hl = hl.saturating_sub(1);
        }
        match &rows[hl] {
            Row::Block(b, ps) if ps.len() > 1 && (right || left) => {
                if right != expanded.contains(*b) {
                    toggle = Some(b.to_string());
                }
            }
            _ => {}
        }
        if enter {
            match &rows[hl] {
                Row::Block(b, ps) if ps.len() > 1 && q.is_empty() && !expanded.contains(*b) => {
                    toggle = Some(b.to_string());
                }
                Row::Block(_, ps) => chosen = Some(ps[0].clone()),
                Row::Sub(p) => chosen = Some((*p).clone()),
            }
            search.request_focus();
        }
        if up || down {
            search.request_focus();
        }
    }
    if esc && keys_ok {
        ST.with(|s| {
            let mut s = s.borrow_mut();
            s.target = None;
            if s.transient {
                s.open = false;
            }
        });
    }
    if rows.is_empty() {
        ui.add_space(8.0);
        ui.label(
            RichText::new(if ru {
                "Ничего не найдено"
            } else {
                "Nothing found"
            })
            .color(weak),
        );
    }
    let mut hovered: Option<(&Preset, Rect)> = None;
    let mut dragged_label: Option<(String, String)> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
        let mut last_group = "";
        for (ri, row) in rows.iter().enumerate() {
            let (block, preset, sub) = match row {
                Row::Block(b, ps) => (*b, ps[0], false),
                Row::Sub(p) => (p.block.as_str(), *p, true),
            };
            if !sub {
                let g = group_of(block, ru);
                if g != last_group {
                    last_group = g;
                    ui.add_space(if ri == 0 { 2.0 } else { 8.0 });
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        ui.label(RichText::new(g.to_uppercase()).size(10.5).color(weak));
                    });
                    ui.add_space(2.0);
                }
            }
            let w = ui.available_width();
            let h = if sub { 32.0 } else { 46.0 };
            let (r, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click_and_drag());
            if resp.hovered() && (resp.ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO)) {
                hl = ri;
            }
            let is_hl = ri == hl;
            let painter = ui.painter_at(r);
            if is_hl {
                painter.rect_filled(r, 8.0, accent.gamma_multiply(0.16));
            } else if resp.hovered() {
                painter.rect_filled(r, 8.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            let multi = matches!(row, Row::Block(_, ps) if ps.len() > 1);
            if sub {
                painter.circle_filled(egui::pos2(r.left() + 30.0, r.center().y), 2.0, weak);
                painter.text(
                    egui::pos2(r.left() + 42.0, r.center().y),
                    egui::Align2::LEFT_CENTER,
                    chip_text(&preset.label),
                    egui::FontId::proportional(13.5),
                    text_c,
                );
            } else {
                let ib = Rect::from_min_size(egui::pos2(r.left() + 8.0, r.center().y - 15.0), vec2(30.0, 30.0));
                painter.rect_filled(ib, 7.0, accent.gamma_multiply(if is_hl { 0.3 } else { 0.14 }));
                paint_icon(&painter, ib.shrink(4.0), block, accent);
                let label = if preset.block_label.is_empty() {
                    &preset.label
                } else {
                    &preset.block_label
                };
                painter.text(
                    egui::pos2(r.left() + 48.0, r.top() + 7.0),
                    egui::Align2::LEFT_TOP,
                    label,
                    egui::FontId::proportional(14.5),
                    text_c,
                );
                if !preset.description.is_empty() {
                    let max_w = r.width() - 48.0 - if multi { 28.0 } else { 8.0 };
                    let mut d = preset.description.clone();
                    let font = egui::FontId::proportional(12.0);
                    while d.chars().count() > 4
                        && painter.layout_no_wrap(d.clone(), font.clone(), weak).size().x > max_w
                    {
                        d.pop();
                        d = d.trim_end_matches('…').trim_end().to_string() + "…";
                        d.pop();
                        d.push('…');
                    }
                    painter.text(
                        egui::pos2(r.left() + 48.0, r.top() + 26.0),
                        egui::Align2::LEFT_TOP,
                        d,
                        font,
                        weak,
                    );
                }
                if multi {
                    // chevron
                    let c = egui::pos2(r.right() - 16.0, r.center().y);
                    let open = expanded.contains(block) || !q.is_empty();
                    let pts = if open {
                        vec![c + vec2(-4.0, -2.0), c + vec2(0.0, 2.0), c + vec2(4.0, -2.0)]
                    } else {
                        vec![c + vec2(-2.0, -4.0), c + vec2(2.0, 0.0), c + vec2(-2.0, 4.0)]
                    };
                    painter.add(egui::Shape::line(pts, Stroke::new(1.5, weak)));
                }
            }
            let resp = resp.on_hover_text(format!(":::{block}"));
            if resp.hovered() {
                ui.ctx().set_cursor_icon(if resp.dragged() {
                    CursorIcon::Grabbing
                } else {
                    CursorIcon::Grab
                });
                hovered = Some((preset, r));
            }
            if resp.drag_started() {
                egui::DragAndDrop::set_payload(ui.ctx(), DragPreset(preset.clone()));
            }
            if resp.dragged() {
                let label = if sub {
                    chip_text(&preset.label).to_string()
                } else {
                    preset.block_label.clone()
                };
                dragged_label = Some((block.to_string(), label));
            }
            if resp.clicked() {
                if multi && q.is_empty() {
                    toggle = Some(block.to_string());
                } else {
                    chosen = Some(preset.clone());
                }
            }
        }
    });
    ST.with(|s| {
        let mut s = s.borrow_mut();
        s.hl = hl;
        if let Some(b) = toggle {
            if !s.expanded.remove(&b) {
                s.expanded.insert(b);
            }
        }
    });
    if let Some(p) = chosen {
        let t = ST.with(|s| s.borrow_mut().target.take());
        choose(p, t);
    }
    // hover preview: the full preset render to the left of the panel (after 300 ms)
    let dragging = egui::DragAndDrop::payload::<DragPreset>(ui.ctx());
    let now = std::time::Instant::now();
    let show_for = ST.with(|s| {
        let mut s = s.borrow_mut();
        match (&hovered, &s.hover) {
            (Some((p, _)), Some((m, _))) if *m == p.markdown => {}
            (Some((p, _)), _) => s.hover = Some((p.markdown.clone(), now)),
            (None, _) => s.hover = None,
        }
        s.hover
            .as_ref()
            .filter(|(_, t)| t.elapsed().as_millis() >= 300)
            .map(|(m, _)| m.clone())
    });
    if hovered.is_some() && show_for.is_none() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
    }
    if dragging.is_none()
        && let (Some(md), Some((p, row))) = (show_for, hovered)
    {
        let thumb = snaps.as_deref_mut().and_then(|s| s.thumb(&md, THUMB_W));
        let screen = ui.ctx().content_rect();
        let h = thumb.map_or(80.0, |(_, sz)| sz.y * POP_W / sz.x.max(1.0));
        let top = row.top().min(screen.bottom() - h - 40.0).max(screen.top() + 8.0);
        let pos = egui::pos2(rect.left() - POP_W - 28.0, top);
        egui::Area::new(egui::Id::unique("markise-block-preview"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .interactable(false)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(POP_W);
                        ui.label(RichText::new(chip_text(&p.label)).size(12.0).color(weak));
                        match thumb {
                            Some((tex, sz)) => {
                                let size = vec2(POP_W, sz.y * POP_W / sz.x.max(1.0));
                                ui.add(egui::Image::new((tex, size)));
                            }
                            None => {
                                ui.label(
                                    RichText::new(if ru {
                                        "Рисую предпросмотр…"
                                    } else {
                                        "Rendering preview…"
                                    })
                                    .color(weak),
                                );
                                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                            }
                        }
                    });
            });
    }
    // drag ghost: a small pill with the icon and label
    if let Some(d) = dragging
        && let Some(pos) = ui.ctx().pointer_interact_pos()
    {
        let (block, label) = dragged_label.unwrap_or((d.0.block.clone(), d.0.block_label.clone()));
        let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("markise-block-drag"));
        let painter = ui.ctx().layer_painter(layer);
        let galley = painter.layout_no_wrap(label, egui::FontId::proportional(13.0), Color32::WHITE);
        let r = Rect::from_min_size(pos + vec2(14.0, 6.0), vec2(galley.size().x + 44.0, 28.0));
        painter.rect_filled(r, 14.0, accent);
        paint_icon(
            &painter,
            Rect::from_min_size(r.min + vec2(8.0, 5.0), vec2(18.0, 18.0)),
            &block,
            Color32::WHITE,
        );
        painter.galley(
            r.min + vec2(32.0, (28.0 - galley.size().y) / 2.0),
            galley,
            Color32::WHITE,
        );
    }
}

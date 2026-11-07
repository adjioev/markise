//! The outline with sections as a level above headings, and what a section's row offers (ADR 0026): a menu, and
//! a grip to drag it by. Rows are asked for by what is on screen, so a large document costs what a small one does,
//! also while one is dragged. All of it goes by the engine's moves and refusals, which the app runs; here only
//! what can be done is offered.

use egui::accesskit::Role;
use egui::{
    Button, Color32, CursorIcon, Key, Modifiers, Popup, Pos2, Rect, Response, RichText, Sense, Stroke, Ui, WidgetInfo,
    pos2, vec2,
};
use markise_engine::editor::EditorView;
use markise_layout::OutlineRow;

use crate::sidebar::{RowKind, SideLook, virtual_rows};
use crate::viewer::SectionAct;

/// What the outline came to in this frame.
#[derive(Default)]
pub struct OutlineOut {
    /// The clicked row's meaning.
    pub clicked: Option<OutlineRow>,
    /// What a section's menu asked for.
    pub act: Option<SectionAct>,
}

/// Room beside the rows for a grip, there whether or not it shows, so that no row moves when one does.
const GUTTER: f32 = 14.0;
/// How near an edge of the list the pointer has to be for it to scroll, and how fast at the very edge.
const SCROLL_EDGE: f32 = 28.0;
const SCROLL_SPEED: f32 = 700.0;
/// How far from the press the pointer has to go for the edges to count.
const MOVED: f32 = 6.0;

/// What the panel keeps between frames.
#[derive(Default)]
pub struct PanelState {
    drag: Option<SectionDrag>,
    /// A drag ended: the section to keep where it was on screen, with its headings back in the list.
    settle: Option<(usize, f32)>,
}

impl PanelState {
    /// What the drag going on shows.
    pub fn drag_shown(&self) -> Option<&SectionDragShown> {
        self.drag.as_ref()?.shown.as_ref()
    }
}

/// A section being dragged.
struct SectionDrag {
    section: usize,
    /// The document and its state when the drag started: anything else ends it, with nothing done.
    origin: (u64, u64),
    /// How far below its row's top the pointer took it, and where on screen.
    grab_y: f32,
    pressed_at: f32,
    has_moved: bool,
    /// Empty rows above the sections, so that the one picked up can stay under the pointer: with only sections
    /// in it the list is shorter than it was.
    pad: usize,
    /// What to set the list's scroll offset to in the next frame.
    scroll_to: Option<f32>,
    /// The frame it was last shown in: not shown means it is over.
    pass: u64,
    shown: Option<SectionDragShown>,
}

/// What a drag shows on screen in this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionDragShown {
    pub section: usize,
    /// It would go before this section; the section count for the end.
    pub to: usize,
    /// Where the line is.
    pub line_y: f32,
    /// The line is a place to land; otherwise it stays where it is.
    pub moves: bool,
    /// The rows the list has while it lasts: only its sections.
    pub rows: usize,
    /// The list's offset.
    pub offset: f32,
}

/// The document in the outline.
pub struct OutlineDoc<'a> {
    pub ed: &'a EditorView,
    /// Which document, for what a drag is for.
    pub id: u64,
    /// The headings to list, when not all of them.
    pub only: Option<&'a [usize]>,
    pub current: usize,
    /// It holds the first part of its file.
    pub partial: bool,
    /// What a section is called here, while sections can be changed; `None` offers nothing, as in Read.
    pub word: Option<&'a str>,
}

/// The outline.
pub fn section_outline(ui: &mut Ui, doc: OutlineDoc<'_>, look: &SideLook, state: &mut PanelState) -> OutlineOut {
    let OutlineDoc {
        ed,
        id,
        only,
        current,
        partial,
        word,
    } = doc;
    let origin = (id, ed.changes());
    let pass = ui.ctx().cumulative_pass_nr();
    // a word gone, an edit, another document, the panel not shown in between: it is over with nothing done
    state.drag = state.drag.take().filter(|d| {
        word.is_some()
            && !partial
            && d.origin == origin
            && d.pass + 1 >= pass
            && d.section < ed.view.doc().section_count()
    });
    if state.drag.is_some() {
        return dragging_list(ui, ed, look, state, pass);
    }
    let view = &ed.view;
    let settled = state.settle.take().map(|(section, y)| {
        let row = view.outline_row_of(section, None, only) as f32 * look.row_h;
        (row - y).max(0.0)
    });
    let model = view.model();
    let min_level = model.outline_min_level();
    let sections = view.doc().section_count();
    let mut out = OutlineOut::default();
    let listed = view.outline_row_count(only);
    let rows = listed + usize::from(partial);
    let mut started = None;
    let frame = egui::Frame::new().inner_margin(egui::Margin {
        left: GUTTER as i8,
        ..Default::default()
    });
    let list = virtual_rows(ui, "markise-section-outline", look.row_h, rows, settled, |ui, range| {
        frame.show(ui, |ui| {
            for i in range {
                if i >= listed {
                    ui.label(
                        RichText::new("More when the file has loaded…")
                            .font(look.font.clone())
                            .color(look.icon),
                    );
                    continue;
                }
                let row = view.outline_row(i, only);
                let r = match row {
                    OutlineRow::Section(k) => {
                        let mut name = view.section_name(k);
                        // the last section of a part goes on in what has yet to load
                        if partial && k + 1 == sections {
                            name.push('…');
                        }
                        // A menu names its section by its place, which means something only in this very state
                        // of this document: it stays with its row however the list scrolls, and any change at
                        // all, a reload too, ends it with nothing done.
                        let r = ui
                            .push_id(("markise-section-row", id, ed.changes(), k), |ui| {
                                look.row(ui, 0, RowKind::Heading { level: 1 }, &name, current == i)
                            })
                            .inner;
                        if let Some(word) = word {
                            row_menu(ui, ed, k, &r, word, look, &mut out.act);
                            started = row_grip(ui, ed, k, &r, word, look).or(started);
                        }
                        r
                    }
                    OutlineRow::Heading(h) => {
                        let Some(e) = model.outline.get(h) else { continue };
                        let level = e.level - min_level + 1;
                        let text = if e.text.is_empty() { "(empty heading)" } else { &e.text };
                        look.row(
                            ui,
                            level as usize,
                            RowKind::Heading { level: level + 1 },
                            text,
                            current == i,
                        )
                    }
                };
                if r.clicked() {
                    out.clicked = Some(row);
                }
            }
        });
    });
    if let Some((section, pointer_y, row_top)) = started {
        // with only its sections in the list, its row where it was
        let on_screen = row_top - list.inner_rect.top();
        let pad = ((on_screen / look.row_h).ceil() as isize - section as isize).max(0) as usize;
        state.drag = Some(SectionDrag {
            section,
            origin,
            grab_y: pointer_y - row_top,
            pressed_at: pointer_y,
            has_moved: false,
            pad,
            scroll_to: Some((pad + section) as f32 * look.row_h - on_screen),
            pass,
            shown: None,
        });
        ui.ctx().request_repaint();
    }
    if out.act.is_some() {
        ui.ctx().request_repaint();
    }
    out
}

/// The grip of a section's row: beside it while the pointer is on it. What it starts: the section, and where the
/// pointer and the row's top are on screen.
fn row_grip(
    ui: &mut Ui,
    ed: &EditorView,
    k: usize,
    row: &Response,
    word: &str,
    look: &SideLook,
) -> Option<(usize, f32, f32)> {
    let id = row.id.with("grip");
    let rect = Rect::from_min_max(
        pos2(row.rect.left() - GUTTER, row.rect.top()),
        pos2(row.rect.left(), row.rect.bottom()),
    );
    let ctx = ui.ctx().clone();
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let over = pointer.is_some_and(|p| {
        row.rect.union(rect).contains(p) && ui.clip_rect().contains(p) && ctx.layer_id_at(p) == Some(ui.layer_id())
    });
    // only what could go anywhere; a first section with nothing in it stays
    let goes = ed.can_move_section(k, true) || ed.can_move_section(k, false);
    if !over || !goes {
        return None;
    }
    let label = format!("Drag {word} to move");
    let resp = ui
        .interact(rect, id, Sense::drag())
        .on_hover_cursor(CursorIcon::Grab)
        .on_hover_text(&label);
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, &label));
    paint_grip(ui, rect, resp.hovered() || resp.dragged(), look);
    let y = resp.interact_pointer_pos().map(|p| p.y);
    resp.drag_started()
        .then(|| (k, y.unwrap_or(rect.center().y), row.rect.top()))
}

fn paint_grip(ui: &Ui, rect: Rect, hot: bool, look: &SideLook) {
    let p = ui.painter_at(ui.clip_rect());
    let tone = if hot { look.active_text } else { look.icon };
    for (dx, dy) in [-1.0f32, 1.0]
        .into_iter()
        .flat_map(|x| [-1.0f32, 0.0, 1.0].map(|y| (x, y)))
    {
        p.circle_filled(rect.center() + vec2(dx * 2.2, dy * 4.0), 1.0, tone);
    }
}

/// While one is dragged the list has only its sections, which makes it short and easy to go along. Nothing here
/// walks the document: the rows are those on screen and the place to land is by the pointer alone.
fn dragging_list(ui: &mut Ui, ed: &EditorView, look: &SideLook, state: &mut PanelState, pass: u64) -> OutlineOut {
    let mut out = OutlineOut::default();
    let Some(drag) = state.drag.as_mut() else { return out };
    let view = &ed.view;
    let rows = view.doc().section_count();
    let row_h = look.row_h;
    let section = drag.section;
    let cancelled = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
    let (pointer, down, dt) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down(), i.stable_dt));
    let scroll_to = drag.scroll_to.take();
    let frame = egui::Frame::new().inner_margin(egui::Margin {
        left: GUTTER as i8,
        ..Default::default()
    });
    let pad = drag.pad;
    // room below as well as above, so that the offset that keeps the row in its place can always be had
    let tail = (ui.available_height() / row_h).ceil() as usize + 1;
    let list = virtual_rows(
        ui,
        "markise-section-outline",
        row_h,
        pad + rows + tail,
        scroll_to,
        |ui, range| {
            frame.show(ui, |ui| {
                for i in range {
                    let Some(k) = i.checked_sub(pad).filter(|k| *k < rows) else {
                        ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
                        continue;
                    };
                    let name = view.section_name(k);
                    ui.push_id(("markise-section-row", k), |ui| {
                        look.row(ui, 0, RowKind::Heading { level: 1 }, &name, k == section)
                    });
                }
            });
        },
    );
    let area = list.inner_rect;
    let offset = list.state.clamped_offset().y;
    drag.pass = pass;
    let y = pointer.map_or(drag.pressed_at, |p| p.y);
    drag.has_moved |= (y - drag.pressed_at).abs() > MOVED;
    // the place nearest the pointer: between two rows
    let place = ((y - area.top() + offset) / row_h).round().max(0.0) as usize;
    let to = place.saturating_sub(pad).min(rows);
    // a press and a release move nothing
    let moves = drag.has_moved && ed.can_move_section_to(section, to);
    let top = area.top() - offset + pad as f32 * row_h;
    let line_y = (top + to as f32 * row_h).clamp(area.top() + 1.0, area.bottom() - 1.0);
    if !down {
        if moves {
            out.act = Some(SectionAct::MoveTo { index: section, to });
        }
        let ends_at = if !moves {
            section
        } else if to > section {
            to - 1
        } else {
            to
        };
        state.settle = Some((ends_at, y - area.top() - drag.grab_y));
        state.drag = None;
        ui.ctx().request_repaint();
        return out;
    }
    if cancelled {
        state.settle = Some((section, y - area.top() - drag.grab_y));
        state.drag = None;
        ui.ctx().request_repaint();
        return out;
    }
    if drag.has_moved {
        let up = SCROLL_EDGE - (y - area.top());
        let below = SCROLL_EDGE - (area.bottom() - y);
        let speed = SCROLL_SPEED / SCROLL_EDGE;
        let dy = if up > 0.0 {
            -up.min(SCROLL_EDGE) * speed * dt
        } else if below > 0.0 {
            below.min(SCROLL_EDGE) * speed * dt
        } else {
            0.0
        };
        if dy != 0.0 {
            // no further than brings the first or the last section in, and never away from them
            let least = (pad as f32 * row_h).min(offset);
            let most = ((pad + rows) as f32 * row_h - area.height()).max(offset);
            drag.scroll_to = Some((offset + dy).clamp(least, most));
        }
    }
    paint_drag(ui, area, section, top, line_y, moves, look);
    ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    drag.shown = Some(SectionDragShown {
        section,
        to,
        line_y,
        moves,
        rows,
        offset,
    });
    // the pointer may stand still at an edge, and the list has to go on
    ui.ctx().request_repaint();
    out
}

/// The row as picked up, and the line: strong where it would land, quiet where it stays.
fn paint_drag(ui: &Ui, area: Rect, section: usize, top: f32, line_y: f32, moves: bool, look: &SideLook) {
    let painter = ui.painter_at(area);
    let x0 = area.left() + GUTTER;
    let x1 = area.right() - 8.0;
    let row_top = top + section as f32 * look.row_h;
    let row = Rect::from_min_max(pos2(x0, row_top), pos2(x1, row_top + look.row_h));
    painter.rect_filled(row, look.radius, look.accent.gamma_multiply(0.14));
    if moves {
        painter.hline(x0..=x1, line_y, Stroke::new(2.0, look.accent));
        let end: Pos2 = pos2(x0, line_y);
        painter.circle_filled(end, 3.0, look.accent);
    } else {
        let weak = look.icon;
        painter.extend(egui::Shape::dashed_line(
            &[pos2(x0, line_y), pos2(x1, line_y)],
            Stroke::new(1.5, weak),
            5.0,
            4.0,
        ));
    }
}

/// A section row's menu: by a right click anywhere on it, and from a quiet `⋯` at its end, which shows under the
/// pointer, while the keyboard is on the row or on it, and while its menu is open. By the keyboard: Tab to the
/// `⋯`, or Shift+F10 on the row; the arrows go through the items, Esc goes back to the `⋯`.
fn row_menu(
    ui: &mut Ui,
    ed: &EditorView,
    k: usize,
    row: &Response,
    word: &str,
    look: &SideLook,
    act: &mut Option<SectionAct>,
) {
    let ctx = ui.ctx().clone();
    let more = row.id.with("more");
    // the id of a popup of a button with the id `more`
    let menu = more.with("popup");
    let by_key = menu.with("by-key");
    let shift_f10 = row.has_focus() && ui.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::F10));
    let open = Popup::is_id_open(&ctx, menu);
    let over = ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| {
        row.rect.contains(p) && ui.clip_rect().contains(p) && ctx.layer_id_at(p) == Some(ui.layer_id())
    });
    let focused = row.has_focus() || ctx.memory(|m| m.has_focus(more));
    if over || open || focused || shift_f10 {
        let side = (look.row_h - 6.0).max(12.0);
        let rect = Rect::from_center_size(
            pos2(row.rect.right() - side * 0.5 - 4.0, row.rect.center().y),
            vec2(side, side),
        );
        let label = format!("{word} actions");
        let dots = ui.interact(rect, more, Sense::click());
        dots.widget_info(|| WidgetInfo::labeled(Role::Button, true, &label));
        let keyed = dots.clicked() && ui.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Space));
        if shift_f10 {
            Popup::open_id(&ctx, menu);
        }
        let dots = dots.on_hover_text(&label);
        paint_dots(ui, &dots, rect, focused, look);
        if keyed || shift_f10 {
            ctx.data_mut(|d| d.insert_temp(by_key, true));
            ctx.data_mut(|d| d.insert_temp(menu.with("first"), true));
        }
        Popup::menu(&dots).show(|ui| menu_items(ui, ed, k, menu, act));
        // Esc: the keyboard goes back to where the menu was opened from
        if ctx.data(|d| d.get_temp::<bool>(by_key)).is_some() && !Popup::is_id_open(&ctx, menu) {
            ctx.data_mut(|d| d.remove::<bool>(by_key));
            if act.is_none() && ctx.memory(|m| m.focused()).is_none() {
                dots.request_focus();
            }
        }
    }
    row.context_menu(|ui| menu_items(ui, ed, k, menu, act));
}

fn paint_dots(ui: &Ui, dots: &Response, rect: Rect, focused: bool, look: &SideLook) {
    let p = ui.painter_at(ui.clip_rect());
    let hot = dots.hovered() || dots.has_focus();
    p.rect_filled(rect, look.radius, if hot { look.active_bg } else { look.hover_bg });
    if dots.has_focus() {
        p.rect_stroke(
            rect,
            look.radius,
            egui::Stroke::new(1.0, look.accent),
            egui::StrokeKind::Inside,
        );
    }
    let tone: Color32 = if hot || focused { look.active_text } else { look.icon };
    for dx in [-1.0, 0.0, 1.0] {
        p.circle_filled(rect.center() + vec2(dx * 4.0, 0.0), 1.2, tone);
    }
}

/// Offers only what can be done now, as the engine says.
fn menu_items(ui: &mut Ui, ed: &EditorView, k: usize, menu: egui::Id, act: &mut Option<SectionAct>) {
    ui.set_min_width(130.0);
    let first = ui
        .ctx()
        .data_mut(|d| d.remove_temp::<bool>(menu.with("first")))
        .is_some();
    let mut item = |ui: &mut Ui, enabled: bool, label: &str, what: SectionAct, focus: &mut bool| {
        let r = ui.add_enabled(enabled, Button::new(label).wrap_mode(egui::TextWrapMode::Extend));
        if enabled && std::mem::take(focus) {
            r.request_focus();
        }
        if r.clicked() {
            *act = Some(what);
        }
    };
    let mut focus = first;
    item(
        ui,
        ed.can_move_section(k, true),
        "Move Up",
        SectionAct::MoveUp(k),
        &mut focus,
    );
    item(
        ui,
        ed.can_move_section(k, false),
        "Move Down",
        SectionAct::MoveDown(k),
        &mut focus,
    );
    item(
        ui,
        ed.can_duplicate_section(k),
        "Duplicate",
        SectionAct::Duplicate(k),
        &mut focus,
    );
    item(
        ui,
        ed.can_insert_after_section(k),
        "Insert After…",
        SectionAct::InsertAfter(k),
        &mut focus,
    );
    ui.separator();
    item(
        ui,
        ed.can_delete_section(k),
        "Delete",
        SectionAct::Delete(k),
        &mut focus,
    );
}

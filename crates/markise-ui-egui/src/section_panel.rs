//! The outline with sections as a level above headings, and what a section's row offers (ADR 0026). Rows are
//! asked for by what is on screen, so a large document costs what a small one does. All of it goes by the
//! engine's moves and refusals, which the app runs; here only what can be done is offered.

use egui::accesskit::Role;
use egui::{Button, Color32, Key, Modifiers, Popup, Rect, Response, RichText, Sense, Ui, WidgetInfo, pos2, vec2};
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

/// What a section's row says, and what it is known by. Beside its number, with no name, the word says the rest.
pub fn section_row_name(view: &markise_layout::View, k: usize) -> (String, String) {
    let name = view.section_name(k);
    let by_word = view.section_own_name(k).is_none() && view.section_shown_number(k).is_some();
    (
        if by_word {
            view.section_word().to_string()
        } else {
            name.clone()
        },
        name,
    )
}

/// How wide the slot of a section's number is: the same on every row, so that names stand in a line.
pub(crate) fn number_slot(sections: usize) -> f32 {
    let digits = sections.max(1).ilog10() as f32 + 1.0;
    (digits * 7.5).max(18.0)
}

/// The outline. `id`: which document. `only`: the headings to list, when not all of them. `editable`: sections can be changed; otherwise a row offers nothing, as in Read.
#[allow(clippy::too_many_arguments)]
pub fn section_outline(
    ui: &mut Ui,
    ed: &EditorView,
    id: u64,
    only: Option<&[usize]>,
    current: usize,
    look: &SideLook,
    partial: bool,
    editable: bool,
) -> OutlineOut {
    let view = &ed.view;
    let model = view.model();
    // under a title that stands apart the rest go by what is left
    let titled = matches!(view.outline_row(0, only), OutlineRow::Title(_));
    let min_level = if titled { 2 } else { model.outline_min_level() };
    let sections = view.doc().section_count();
    let mut out = OutlineOut::default();
    let listed = view.outline_row_count(only);
    let slot = number_slot(sections);
    let rows = listed + usize::from(partial);
    virtual_rows(ui, "markise-section-outline", look.row_h, rows, None, |ui, range| {
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
                    let (mut shown, mut name) = section_row_name(view, k);
                    // the last section of a part goes on in what has yet to load
                    if partial && k + 1 == sections {
                        shown.push('…');
                        name.push('…');
                    }
                    let number = view.section_shown_number(k);
                    let kind = RowKind::Section {
                        number,
                        slot: slot as u16,
                    };
                    // where the ground of the row goes while what is on it holds the pointer
                    let ground = ui.painter().add(egui::Shape::Noop);
                    // A menu names its section by its place, which means something only in this very state of
                    // this document: it stays with its row however the list scrolls, and any change at all,
                    // a reload too, ends it with nothing done.
                    let r = ui
                        .push_id(("markise-section-row", id, ed.changes(), k), |ui| {
                            look.row_named(ui, 0, kind, &shown, &name, current == i)
                        })
                        .inner;
                    if editable && row_menu(ui, ed, k, &r, &name, slot, look, &mut out.act) {
                        let shape =
                            egui::Shape::rect_filled(r.rect.shrink2(vec2(1.0, 1.0)), look.radius, look.hover_bg);
                        ui.painter().set(ground, shape);
                    }
                    r
                }
                // plain: it is no section's, and offers nothing
                OutlineRow::Title(h) => {
                    let Some(e) = model.outline.get(h) else { continue };
                    look.row(ui, 0, RowKind::Heading { level: 1 }, &e.text, current == i)
                }
                OutlineRow::Heading(h) => {
                    let Some(e) = model.outline.get(h) else { continue };
                    let level = e.level.saturating_sub(min_level) + 1;
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
    if out.act.is_some() {
        ui.ctx().request_repaint();
    }
    out
}

/// A section row's menu: by a right click anywhere on it, and from a quiet `⋯` at its start, in the place of its
/// number, which shows under the pointer, while the keyboard is on the row or on it, and while its menu is open. By
/// the keyboard: Tab to the `⋯`, or Shift+F10 on the row; the arrows go through the items, Esc goes back to the
/// `⋯`. Returns whether it shows.
#[allow(clippy::too_many_arguments)]
fn row_menu(
    ui: &mut Ui,
    ed: &EditorView,
    k: usize,
    row: &Response,
    name: &str,
    slot: f32,
    look: &SideLook,
    act: &mut Option<SectionAct>,
) -> bool {
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
    let shows = over || open || focused || shift_f10;
    if shows {
        let rect = Rect::from_center_size(
            pos2(row.rect.left() + look.pad_x + slot * 0.5, row.rect.center().y),
            vec2(slot + 2.0, (look.row_h - 6.0).max(12.0)),
        );
        // the name goes to those who hear it, as a tooltip stands by the row and has to read in any language
        let label = format!("Actions: {name}");
        let dots = ui.interact(rect, more, Sense::click());
        dots.widget_info(|| WidgetInfo::labeled(Role::Button, true, &label));
        let keyed = dots.clicked() && ui.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Space));
        if shift_f10 {
            Popup::open_id(&ctx, menu);
        }
        let dots = dots.on_hover_text("Actions");
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
    shows
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

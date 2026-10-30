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

/// Draws the palette into `rect`.
pub fn show(ui: &mut Ui, rect: Rect, root: Option<&Path>, accent: Color32) {
    let lang = crate::block_edit::lang();
    let ru = lang == "ru";
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(14.0, 12.0)))
            .id_salt("markise-block-palette"),
    );
    let ui = &mut child;
    ui.horizontal(|ui| {
        ui.label(RichText::new(if ru { "Блоки" } else { "Blocks" }).size(17.0).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button(if ru { "Скрыть" } else { "Hide" })
                .on_hover_text(if ru { "Закрыть палитру" } else { "Close" })
                .clicked()
            {
                ST.with(|s| {
                    let mut s = s.borrow_mut();
                    s.open = false;
                    s.target = None;
                });
            }
        });
    });
    let target_note = ST.with(|s| s.borrow().target.is_some());
    ui.label(
        RichText::new(if target_note {
            if ru {
                "Выберите блок для отмеченного места"
            } else {
                "Choose a block for the marked point"
            }
        } else if ru {
            "Щелчок — вставить у курсора, или перетащите на страницу"
        } else {
            "Click to insert at the caret, or drag onto the page"
        })
        .size(12.0)
        .weak(),
    );
    ui.add_space(8.0);
    let Some(root) = root else {
        ui.label(if ru {
            "Для этого документа не подключена библиотека блоков (blocks.config.json)."
        } else {
            "No block library is bound to this document (blocks.config.json)."
        });
        return;
    };
    let presets = presets_of(root, &lang);
    let (mut query, focus) = ST.with(|s| {
        let mut s = s.borrow_mut();
        (s.query.clone(), std::mem::take(&mut s.focus_search))
    });
    let search = ui.add(
        egui::TextEdit::singleline(&mut query)
            .hint_text(if ru {
                "Поиск блока…"
            } else {
                "Search blocks…"
            })
            .desired_width(f32::INFINITY),
    );
    if focus {
        search.request_focus();
    }
    let esc = search.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape));
    let enter = search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    ST.with(|s| s.borrow_mut().query = query.clone());
    if esc {
        ST.with(|s| {
            let mut s = s.borrow_mut();
            s.target = None;
            if s.transient {
                s.open = false;
            }
        });
    }
    ui.add_space(8.0);
    let q = query.to_lowercase();
    let matches: Vec<&Preset> = presets
        .iter()
        .filter(|p| {
            q.is_empty()
                || format!("{} {} {}", p.label, p.description, p.block)
                    .to_lowercase()
                    .contains(&q)
        })
        .collect();
    if enter && let Some(p) = matches.first() {
        let t = ST.with(|s| s.borrow_mut().target.take());
        choose((*p).clone(), t);
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let mut last_block = String::new();
        for p in matches {
            if p.block != last_block {
                last_block = p.block.clone();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                    ui.painter().rect_filled(r, 6.0, accent.gamma_multiply(0.25));
                    let letter = p.block.chars().next().unwrap_or('?').to_uppercase().to_string();
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        letter,
                        egui::FontId::proportional(13.0),
                        accent,
                    );
                    ui.label(RichText::new(format!(":::{}", p.block)).size(12.0).monospace().weak());
                });
                if !p.description.is_empty() {
                    ui.label(RichText::new(&p.description).size(12.0).weak());
                }
            }
            // a preset row: click inserts, drag carries it to the page
            let w = ui.available_width();
            let (r, resp) = ui.allocate_exact_size(vec2(w, 30.0), Sense::click_and_drag());
            let hot = resp.hovered() || resp.dragged();
            ui.painter().rect(
                r,
                8.0,
                if hot {
                    accent.gamma_multiply(0.18)
                } else {
                    ui.visuals().faint_bg_color
                },
                Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                r.left_center() + vec2(10.0, 0.0),
                egui::Align2::LEFT_CENTER,
                &p.label,
                egui::FontId::proportional(14.0),
                ui.visuals().text_color(),
            );
            if hot {
                ui.ctx().set_cursor_icon(if resp.dragged() {
                    CursorIcon::Grabbing
                } else {
                    CursorIcon::Grab
                });
            }
            if resp.drag_started() {
                egui::DragAndDrop::set_payload(ui.ctx(), DragPreset(p.clone()));
            }
            if resp.clicked() {
                let t = ST.with(|s| s.borrow_mut().target.take());
                choose(p.clone(), t);
            }
            ui.add_space(4.0);
        }
    });
    // the dragged preset follows the pointer
    if let Some(d) = egui::DragAndDrop::payload::<DragPreset>(ui.ctx())
        && let Some(pos) = ui.ctx().pointer_interact_pos()
    {
        let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("markise-block-drag"));
        let painter = ui.ctx().layer_painter(layer);
        let r = Rect::from_min_size(pos + vec2(12.0, 6.0), vec2(200.0, 26.0));
        painter.rect_filled(r, 8.0, accent.gamma_multiply(0.9));
        painter.text(
            r.left_center() + vec2(10.0, 0.0),
            egui::Align2::LEFT_CENTER,
            &d.0.label,
            egui::FontId::proportional(13.0),
            Color32::WHITE,
        );
    }
}

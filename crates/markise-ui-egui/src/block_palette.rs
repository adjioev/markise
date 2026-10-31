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

/// A preset's chip text: the part after "Тип: " in its label.
fn chip_text(label: &str) -> &str {
    label.split_once(": ").map_or(label, |(_, b)| b)
}

/// Width (CSS px) thumbnails are rendered at.
const THUMB_W: f32 = 380.0;
/// Visible thumbnail height (points; the top of the block).
const THUMB_H: f32 = 96.0;

/// Draws the palette into `rect`: header with ×, search, then one card per block (thumbnail
/// of its first preset, label, description, preset chips when there are several).
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    root: Option<&Path>,
    accent: Color32,
    mut snaps: Option<&mut crate::block_snap::BlockSnaps>,
) {
    let lang = crate::block_edit::lang();
    let ru = lang == "ru";
    let inner = rect.shrink2(vec2(16.0, 14.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).id_salt("markise-block-palette"));
    let ui = &mut child;
    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
    let weak = ui.visuals().weak_text_color();
    let line = ui.visuals().widgets.noninteractive.bg_stroke.color;
    // header: title + close ×
    ui.horizontal(|ui| {
        ui.label(RichText::new(if ru { "Блоки" } else { "Blocks" }).size(18.0).strong())
            .on_hover_text(if ru {
                "Щелчок — вставить у курсора; перетащите карточку на страницу, чтобы вставить в нужное место"
            } else {
                "Click to insert at the caret; drag a card onto the page to place it"
            });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (r, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
            if resp.hovered() {
                ui.painter()
                    .rect_filled(r, 6.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            let c = r.center();
            let k = 5.0;
            let st = Stroke::new(
                1.6,
                if resp.hovered() {
                    ui.visuals().text_color()
                } else {
                    weak
                },
            );
            ui.painter().line_segment([c + vec2(-k, -k), c + vec2(k, k)], st);
            ui.painter().line_segment([c + vec2(-k, k), c + vec2(k, -k)], st);
            if resp.on_hover_text(if ru { "Закрыть" } else { "Close" }).clicked() {
                ST.with(|s| {
                    let mut s = s.borrow_mut();
                    s.open = false;
                    s.target = None;
                });
            }
        });
    });
    let Some(root) = root else {
        ui.add_space(24.0);
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
            .color(weak),
        );
        return;
    };
    if ST.with(|s| s.borrow().target.is_some()) {
        ui.label(
            RichText::new(if ru {
                "Выберите блок для отмеченного места"
            } else {
                "Choose a block for the marked point"
            })
            .size(12.5)
            .color(accent),
        );
    }
    // search with a magnifier
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
                    .circle_stroke(r.center() + vec2(-1.5, -1.5), 5.0, Stroke::new(1.5, weak));
                ui.painter().line_segment(
                    [r.center() + vec2(2.0, 2.0), r.center() + vec2(6.0, 6.0)],
                    Stroke::new(1.8, weak),
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
    let q = query.to_lowercase();
    // blocks in palette order with their matching presets
    let mut blocks: Vec<(String, Vec<&Preset>)> = Vec::new();
    for p in &presets {
        let hay = format!("{} {} {} {}", p.block_label, p.label, p.description, p.block).to_lowercase();
        if !q.is_empty() && !hay.contains(&q) {
            continue;
        }
        match blocks.iter_mut().find(|(b, _)| *b == p.block) {
            Some((_, v)) => v.push(p),
            None => blocks.push((p.block.clone(), vec![p])),
        }
    }
    let order = |b: &str| match group_of(b, true) {
        "Вопросы" => 0,
        "Текст" => 1,
        "Медиа" => 2,
        _ => 3,
    };
    blocks.sort_by_key(|(b, _)| order(b));
    if enter && let Some(p) = blocks.first().map(|(_, v)| v[0]) {
        let t = ST.with(|s| s.borrow_mut().target.take());
        choose(p.clone(), t);
    }
    if blocks.is_empty() {
        ui.add_space(12.0);
        ui.label(
            RichText::new(if ru {
                "Ничего не найдено"
            } else {
                "Nothing found"
            })
            .color(weak),
        );
    }
    let mut drag_thumb: Option<(egui::TextureId, egui::Vec2)> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 10.0);
        let mut last_group = "";
        for (block, ps) in &blocks {
            let g = group_of(block, ru);
            if g != last_group {
                last_group = g;
                ui.add_space(4.0);
                ui.label(RichText::new(g.to_uppercase()).size(11.0).strong().color(weak));
            }
            let first = ps[0];
            let w = ui.available_width();
            let thumb = snaps.as_deref_mut().and_then(|s| s.thumb(&first.markdown, THUMB_W));
            let chips = ps.len() > 1;
            let desc_h = if first.description.is_empty() { 0.0 } else { 18.0 };
            let card_h = 10.0 + THUMB_H + 10.0 + 20.0 + desc_h + if chips { 30.0 } else { 0.0 } + 10.0;
            let (r, resp) = ui.allocate_exact_size(vec2(w, card_h), Sense::click_and_drag());
            let hot = resp.hovered() || resp.dragged();
            let painter = ui.painter_at(r.expand(2.0));
            painter.rect(
                r,
                10.0,
                if hot {
                    accent.gamma_multiply(0.08)
                } else {
                    ui.visuals().faint_bg_color
                },
                Stroke::new(1.0, if hot { accent.gamma_multiply(0.8) } else { line }),
                egui::StrokeKind::Inside,
            );
            // thumbnail: the top of the rendered preset
            let tr = Rect::from_min_size(r.min + vec2(10.0, 10.0), vec2(w - 20.0, THUMB_H));
            painter.rect_filled(tr, 7.0, ui.visuals().extreme_bg_color);
            if let Some((tex, size)) = thumb {
                let scale = tr.width() / size.x.max(1.0);
                let shown_h = (size.y * scale).min(THUMB_H);
                let uv_h = (shown_h / (size.y * scale)).min(1.0);
                let ir = Rect::from_min_size(tr.min, vec2(tr.width(), shown_h));
                painter.image(
                    tex,
                    ir,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, uv_h)),
                    Color32::WHITE,
                );
                if hot && resp.dragged() {
                    drag_thumb = Some((tex, size));
                }
            }
            let mut y = tr.bottom() + 10.0;
            painter.text(
                egui::pos2(r.left() + 12.0, y),
                egui::Align2::LEFT_TOP,
                if first.block_label.is_empty() {
                    &first.label
                } else {
                    &first.block_label
                },
                egui::FontId::proportional(15.0),
                ui.visuals().strong_text_color(),
            );
            y += 20.0;
            if !first.description.is_empty() {
                painter.text(
                    egui::pos2(r.left() + 12.0, y),
                    egui::Align2::LEFT_TOP,
                    &first.description,
                    egui::FontId::proportional(12.5),
                    weak,
                );
                y += desc_h;
            }
            // several presets: chips (each inserts its preset); one preset: the card inserts it
            let mut chip_hit = false;
            if chips {
                let mut x = r.left() + 12.0;
                for p in ps {
                    let txt = chip_text(&p.label);
                    let galley = ui.painter().layout_no_wrap(
                        txt.to_string(),
                        egui::FontId::proportional(12.5),
                        ui.visuals().text_color(),
                    );
                    let cr = Rect::from_min_size(egui::pos2(x, y + 4.0), vec2(galley.size().x + 20.0, 22.0));
                    if cr.right() > r.right() - 8.0 {
                        break;
                    }
                    let cresp = ui.interact(cr, ui.scope_id().with(("chip", &p.label)), Sense::click());
                    let chot = cresp.hovered();
                    painter.rect(
                        cr,
                        11.0,
                        if chot {
                            accent.gamma_multiply(0.22)
                        } else {
                            ui.visuals().widgets.inactive.weak_bg_fill
                        },
                        Stroke::new(1.0, if chot { accent } else { line }),
                        egui::StrokeKind::Inside,
                    );
                    painter.galley(
                        cr.min + vec2(10.0, (22.0 - galley.size().y) / 2.0),
                        galley,
                        ui.visuals().text_color(),
                    );
                    if cresp.clicked() {
                        chip_hit = true;
                        let t = ST.with(|s| s.borrow_mut().target.take());
                        choose((*p).clone(), t);
                    }
                    x = cr.right() + 6.0;
                }
            }
            let resp = resp.on_hover_text(format!(":::{block}"));
            if hot {
                ui.ctx().set_cursor_icon(if resp.dragged() {
                    CursorIcon::Grabbing
                } else {
                    CursorIcon::Grab
                });
            }
            if resp.drag_started() {
                egui::DragAndDrop::set_payload(ui.ctx(), DragPreset(first.clone()));
            }
            if resp.clicked() && !chip_hit {
                let t = ST.with(|s| s.borrow_mut().target.take());
                choose(first.clone(), t);
            }
        }
    });
    // drag ghost: the thumbnail follows the pointer
    if let Some(d) = egui::DragAndDrop::payload::<DragPreset>(ui.ctx())
        && let Some(pos) = ui.ctx().pointer_interact_pos()
    {
        let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("markise-block-drag"));
        let painter = ui.ctx().layer_painter(layer);
        let thumb = drag_thumb.or_else(|| snaps.as_deref_mut().and_then(|s| s.thumb(&d.0.markdown, THUMB_W)));
        match thumb {
            Some((tex, size)) => {
                let w = 200.0;
                let h = (size.y * w / size.x.max(1.0)).min(120.0);
                let r = Rect::from_min_size(pos + vec2(14.0, 8.0), vec2(w, h));
                painter.rect_filled(r.expand(2.0), 9.0, accent);
                let uv_h = (h / (size.y * w / size.x.max(1.0))).min(1.0);
                painter.image(
                    tex,
                    r,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, uv_h)),
                    Color32::from_white_alpha(230),
                );
            }
            None => {
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
    }
}

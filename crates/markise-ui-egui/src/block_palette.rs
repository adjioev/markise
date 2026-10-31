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
    /// Highlighted insertable row (stable key: preset markdown) and whether the keyboard put it
    /// there (focus ring; the pointer takes over only when it moves over rows).
    hl: Option<String>,
    kbd: bool,
    /// Row under the pointer or keyboard highlight and since when (preview after 300 ms), and a
    /// preview hidden by Esc / shown at once by Space.
    hover: Option<(String, std::time::Instant)>,
    preview_hidden: bool,
    preview_forced: bool,
    /// Give the keyboard back to the editor (the palette was closed with Esc / ×).
    refocus_editor: bool,
    /// Scripted checks: keyboard-highlight the n-th insertable row.
    script_hl: Option<usize>,
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

/// The editor should take the keyboard back (the palette was closed with Esc).
pub fn take_refocus() -> bool {
    ST.with(|s| std::mem::take(&mut s.borrow_mut().refocus_editor))
}

/// Scripted checks: keyboard-highlight the n-th insertable row (its preview follows).
pub fn script_highlight(n: Option<usize>) {
    ST.with(|s| s.borrow_mut().script_hl = n);
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
        // the search field must not keep the keyboard after the palette is gone
        s.refocus_editor = true;
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

/// Colour tokens of the palette (contrast at least the approved mock's).
struct Tok {
    fg: Color32,
    muted: Color32,
    faint: Color32,
    line: Color32,
    hover: Color32,
    accent: Color32,
    accent_soft: Color32,
    field: Color32,
}

fn tokens(dark: bool) -> Tok {
    let c = |h: u32| Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8);
    if dark {
        Tok {
            fg: c(0xececee),
            muted: c(0xa8a8b0),
            faint: c(0x9a9aa3),
            line: c(0x33343a),
            hover: c(0x2b2840),
            accent: c(0xa08af0),
            accent_soft: c(0x2f2a4a),
            field: c(0x222327),
        }
    } else {
        Tok {
            fg: c(0x1d1d1f),
            muted: c(0x5f5f66),
            faint: c(0x6a6a72),
            line: c(0xe4e4e8),
            hover: c(0xf0eefb),
            accent: c(0x5d3fba),
            accent_soft: c(0xece7fb),
            field: c(0xffffff),
        }
    }
}

/// A small line icon (24 × 24 design box) for a block, drawn with shapes; the site may name
/// one in `presets.json` (`icon`), else the block name decides.
pub fn paint_icon(p: &egui::Painter, r: Rect, icon: &str, c: Color32) {
    let k = r.width() / 24.0;
    let s = Stroke::new(1.8 * k.max(0.75), c);
    let at = |x: f32, y: f32| r.min + vec2(x * k, y * k);
    let line = |pts: &[(f32, f32)]| {
        p.add(egui::Shape::line(pts.iter().map(|&(x, y)| at(x, y)).collect(), s));
    };
    let rrect = |x0: f32, y0: f32, x1: f32, y1: f32| {
        p.rect_stroke(
            Rect::from_min_max(at(x0, y0), at(x1, y1)),
            2.0 * k,
            s,
            egui::StrokeKind::Middle,
        );
    };
    match icon {
        "list-check" | "question" => {
            line(&[(9.0, 6.0), (20.0, 6.0)]);
            line(&[(9.0, 12.0), (20.0, 12.0)]);
            line(&[(9.0, 18.0), (20.0, 18.0)]);
            line(&[(3.5, 6.0), (4.7, 7.2), (7.0, 4.8)]);
            line(&[(3.5, 12.0), (4.7, 13.2), (7.0, 10.8)]);
            p.circle_stroke(at(5.0, 18.0), 1.4 * k, s);
        }
        "sticky-note" | "note" => {
            line(&[
                (5.0, 4.0),
                (19.0, 4.0),
                (19.0, 15.0),
                (14.0, 20.0),
                (5.0, 20.0),
                (5.0, 4.0),
            ]);
            line(&[(14.0, 20.0), (14.0, 15.0), (19.0, 15.0)]);
        }
        "lightbulb" | "takeaway" => {
            line(&[(9.0, 18.0), (15.0, 18.0)]);
            line(&[(10.0, 21.0), (14.0, 21.0)]);
            // bulb: arc approximated by a polyline
            let mut pts = vec![(8.5, 16.0), (8.5, 13.9)];
            for i in 0..=12 {
                let a = std::f32::consts::PI * (0.75 + 1.5 * i as f32 / 12.0);
                pts.push((12.0 + 6.0 * a.cos(), 9.0 + 6.0 * a.sin()));
            }
            pts.extend([(15.5, 13.9), (15.5, 16.0), (8.5, 16.0)]);
            line(&pts);
        }
        "flag" | "goals" => {
            line(&[(5.0, 21.0), (5.0, 4.0)]);
            line(&[(5.0, 4.0), (16.0, 4.0), (14.0, 8.0), (16.0, 12.0), (5.0, 12.0)]);
        }
        "columns" | "comparison" => {
            rrect(3.0, 4.0, 11.0, 20.0);
            rrect(13.0, 4.0, 21.0, 20.0);
        }
        "image" | "scene" => {
            rrect(3.0, 5.0, 21.0, 19.0);
            p.circle_stroke(at(9.0, 10.0), 1.6 * k, s);
            line(&[(21.0, 16.0), (16.0, 11.0), (8.0, 19.0)]);
        }
        _ => rrect(5.0, 5.0, 19.0, 19.0),
    }
}

/// Words of `q` (lower case, ё = е) all found in `hay`.
fn matches(q: &str, hay: &str) -> bool {
    let n = |t: &str| t.to_lowercase().replace('ё', "е");
    let hay = n(hay);
    n(q).split_whitespace().all(|w| hay.contains(w))
}

/// Width (CSS px) the hover preview is rendered at; shown at [`POP_W`] points.
const THUMB_W: f32 = 380.0;
const POP_W: f32 = 320.0;

/// One palette line.
enum Line<'a> {
    Group(&'a str),
    /// A block with several presets: a heading only (not insertable, not draggable).
    Head(&'a Preset),
    /// An insertable row: a single-preset block, or a preset under its heading.
    Item {
        p: &'a Preset,
        sub: bool,
        ctx: bool,
    },
}

/// Draws the palette into `rect` (approved mock): header with ×, search (focused on open),
/// compact rows with line icons, presets always visible under their block, word search with
/// context, keyboard navigation, full preview to the left after 300 ms, drag to the page.
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    root: Option<&Path>,
    _accent: Color32,
    mut snaps: Option<&mut crate::block_snap::BlockSnaps>,
) {
    let lang = crate::block_edit::lang();
    let ru = lang == "ru";
    let t = tokens(ui.visuals().dark_mode);
    let inner = rect.shrink2(vec2(10.0, 10.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).id_salt("markise-block-palette"));
    let ui = &mut child;
    ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
    // ---- header
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(
            RichText::new(if ru { "Блоки" } else { "Blocks" })
                .size(15.0)
                .strong()
                .color(t.fg),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (r, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(r, 6.0, t.hover);
            }
            let c = r.center();
            let st = Stroke::new(1.5, if resp.hovered() { t.fg } else { t.muted });
            ui.painter()
                .line_segment([c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0)], st);
            ui.painter()
                .line_segment([c + vec2(-5.0, 5.0), c + vec2(5.0, -5.0)], st);
            if resp.on_hover_text(if ru { "Закрыть" } else { "Close" }).clicked() {
                close();
                ST.with(|s| s.borrow_mut().refocus_editor = true);
            }
        });
    });
    if ST.with(|s| s.borrow().target.is_some()) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(
                RichText::new(if ru {
                    "Выберите блок для отмеченного места"
                } else {
                    "Choose a block for the marked point"
                })
                .size(12.0)
                .color(t.muted),
            );
        });
    }
    let Some(root) = root else {
        ui.add_space(16.0);
        ui.label(
            RichText::new(if ru {
                "Библиотека блоков не подключена"
            } else {
                "No block library"
            })
            .color(t.fg)
            .strong(),
        );
        ui.label(
            RichText::new(if ru {
                "Положите blocks.config.json в папку с уроками — Markise предложит подключить библиотеку."
            } else {
                "Put a blocks.config.json into the content folder; Markise will offer to connect the library."
            })
            .size(12.0)
            .color(t.muted),
        );
        return;
    };
    let presets = presets_of(root, &lang);
    // ---- search (focused on open)
    let (mut query, focus) = ST.with(|s| {
        let mut s = s.borrow_mut();
        (s.query.clone(), std::mem::take(&mut s.focus_search))
    });
    let search = egui::Frame::new()
        .fill(t.field)
        .stroke(Stroke::new(1.0, t.line))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                ui.painter()
                    .circle_stroke(r.center() + vec2(-1.0, -1.0), 5.0, Stroke::new(1.6, t.faint));
                ui.painter().line_segment(
                    [r.center() + vec2(2.8, 2.8), r.center() + vec2(6.5, 6.5)],
                    Stroke::new(1.8, t.faint),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut query)
                        .frame(egui::Frame::NONE)
                        .text_color(t.fg)
                        .hint_text(
                            RichText::new(if ru {
                                "Найти блок…"
                            } else {
                                "Find a block…"
                            })
                            .color(t.faint),
                        )
                        .desired_width(f32::INFINITY),
                )
            })
            .inner
        });
    let search_resp = search.inner;
    if search_resp.has_focus() {
        ui.painter().rect_stroke(
            search.response.rect,
            8.0,
            Stroke::new(1.5, t.accent),
            egui::StrokeKind::Inside,
        );
    }
    if focus {
        search_resp.request_focus();
    }
    let active = search_resp.has_focus() || search_resp.lost_focus();
    let changed = ST.with(|s| s.borrow().query != query);
    if changed {
        ST.with(|s| {
            let mut s = s.borrow_mut();
            s.query = query.clone();
            s.hl = None;
            s.hover = None;
            s.preview_forced = false;
        });
    }
    // ---- lines: groups, single-preset rows, headings with their presets (always visible)
    let q = query.trim().to_string();
    let mut lines: Vec<Line> = Vec::new();
    let mut i = 0;
    while i < presets.len() {
        let b = &presets[i].block;
        let n = presets[i..].iter().take_while(|p| &p.block == b).count();
        let group = &presets[i];
        let ps: Vec<&Preset> = presets[i..i + n]
            .iter()
            .filter(|p| {
                q.is_empty()
                    || matches(
                        &q,
                        &format!(
                            "{} {} {} {}",
                            p.block_label,
                            p.description,
                            if n > 1 { &p.label } else { "" },
                            p.keywords
                        ),
                    )
            })
            .collect();
        if !ps.is_empty() {
            if !matches!(lines.iter().rev().find(|l| matches!(l, Line::Group(_))), Some(Line::Group(g)) if *g == group.group)
            {
                lines.push(Line::Group(&group.group));
            }
            if n > 1 && q.is_empty() {
                lines.push(Line::Head(ps[0]));
                lines.extend(ps.iter().map(|p| Line::Item {
                    p,
                    sub: true,
                    ctx: false,
                }));
            } else if n > 1 {
                // search results: the matching presets directly, with their block as context
                lines.extend(ps.iter().map(|p| Line::Item {
                    p,
                    sub: false,
                    ctx: true,
                }));
            } else {
                lines.push(Line::Item {
                    p: ps[0],
                    sub: false,
                    ctx: false,
                });
            }
        }
        i += n;
    }
    let items: Vec<&Preset> = lines
        .iter()
        .filter_map(|l| if let Line::Item { p, .. } = l { Some(*p) } else { None })
        .collect();
    // ---- keyboard (only while the palette has the keyboard): ↑/↓, Enter, Space, Esc
    let (up, down, enter, esc, space) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::ArrowUp),
            i.key_pressed(egui::Key::ArrowDown),
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
            i.key_pressed(egui::Key::Space),
        )
    });
    let mut hl_idx = ST.with(|s| {
        let s = s.borrow();
        s.hl.as_ref().and_then(|k| items.iter().position(|p| &p.markdown == k))
    });
    let mut chosen: Option<Preset> = None;
    if let Some(n) = ST.with(|s| s.borrow_mut().script_hl.take()) {
        hl_idx = Some(n.min(items.len().saturating_sub(1)));
        ST.with(|s| {
            let mut s = s.borrow_mut();
            s.kbd = true;
            s.hover = None;
        });
    }
    if active && !items.is_empty() {
        if down || up {
            let cur = hl_idx.map_or(if down { usize::MAX } else { 0 }, |x| x);
            hl_idx = Some(if down {
                if cur == usize::MAX {
                    0
                } else {
                    (cur + 1).min(items.len() - 1)
                }
            } else {
                cur.saturating_sub(1)
            });
            ST.with(|s| {
                let mut s = s.borrow_mut();
                s.kbd = true;
                s.hover = None;
                s.preview_hidden = false;
            });
            search_resp.request_focus();
        }
        if enter {
            chosen = Some(items[hl_idx.unwrap_or(0)].clone());
        }
        if space && query.is_empty() {
            // Space on an empty search: preview the highlighted row now
            ST.with(|s| {
                let mut s = s.borrow_mut();
                s.preview_forced = true;
                s.preview_hidden = false;
            });
        }
    }
    if active && esc {
        // Esc: hide the preview → clear the search → close and give the caret back
        let preview_up = ST.with(|s| {
            let s = s.borrow();
            !s.preview_hidden
                && (s.preview_forced || s.hover.as_ref().is_some_and(|(_, t)| t.elapsed().as_millis() >= 300))
        });
        if preview_up {
            ST.with(|s| s.borrow_mut().preview_hidden = true);
            search_resp.request_focus();
        } else if !query.is_empty() {
            ST.with(|s| s.borrow_mut().query.clear());
            search_resp.request_focus();
        } else {
            close();
            ST.with(|s| s.borrow_mut().refocus_editor = true);
        }
    }
    let kbd = ST.with(|s| s.borrow().kbd);
    // ---- list
    let foot_h = 30.0;
    let list_h = (ui.available_height() - foot_h).max(60.0);
    let mut hovered_item: Option<(usize, Rect)> = None;
    let mut hl_rect: Option<Rect> = None;
    let mut drag_started: Option<Preset> = None;
    let mut clear_search = false;
    let pointer_moved = ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO);
    egui::ScrollArea::vertical()
        .max_height(list_h)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
            if items.is_empty() {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} «{}»",
                            if ru {
                                "Ничего не найдено по"
                            } else {
                                "Nothing found for"
                            },
                            q
                        ))
                        .color(t.muted),
                    );
                    ui.add_space(4.0);
                    if ui
                        .button(if ru {
                            "Сбросить поиск"
                        } else {
                            "Clear search"
                        })
                        .clicked()
                    {
                        clear_search = true;
                    }
                });
            }
            let mut item_i = 0;
            for line in &lines {
                match line {
                    Line::Group(g) => {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.add_space(10.0);
                            ui.label(RichText::new(g.to_uppercase()).size(11.0).color(t.faint));
                        });
                        ui.add_space(2.0);
                    }
                    Line::Head(p) => {
                        let w = ui.available_width();
                        let (r, _) = ui.allocate_exact_size(vec2(w, 42.0), Sense::hover());
                        row_body(ui, r, p, &t, true, false);
                    }
                    Line::Item { p, sub, ctx } => {
                        let w = ui.available_width();
                        let h = if *sub { 34.0 } else { 48.0 };
                        let (r, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click_and_drag());
                        let me = item_i;
                        item_i += 1;
                        if resp.hovered() {
                            hovered_item = Some((me, r));
                            if pointer_moved {
                                hl_idx = Some(me);
                                ST.with(|s| s.borrow_mut().kbd = false);
                            }
                        }
                        let is_hl = hl_idx == Some(me);
                        if is_hl {
                            hl_rect = Some(r);
                        }
                        let painter = ui.painter_at(r.expand(1.0));
                        if is_hl || resp.hovered() {
                            painter.rect_filled(r, 8.0, t.hover);
                        }
                        if is_hl && kbd {
                            painter.rect_stroke(r, 8.0, Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
                        }
                        if *sub {
                            painter.text(
                                r.left_center() + vec2(48.0, 0.0),
                                egui::Align2::LEFT_CENTER,
                                &p.label,
                                egui::FontId::proportional(13.5),
                                t.fg,
                            );
                        } else if *ctx {
                            let ir =
                                Rect::from_center_size(egui::pos2(r.left() + 23.0, r.center().y), vec2(30.0, 30.0));
                            painter.rect_filled(ir, 7.0, t.accent_soft);
                            paint_icon(
                                &painter,
                                Rect::from_center_size(ir.center(), vec2(18.0, 18.0)),
                                &p.icon,
                                t.accent,
                            );
                            painter.text(
                                egui::pos2(r.left() + 48.0, r.top() + 7.0),
                                egui::Align2::LEFT_TOP,
                                &p.label,
                                egui::FontId::proportional(14.0),
                                t.fg,
                            );
                            painter.text(
                                egui::pos2(r.left() + 48.0, r.top() + 26.0),
                                egui::Align2::LEFT_TOP,
                                &p.block_label,
                                egui::FontId::proportional(11.5),
                                t.muted,
                            );
                        } else {
                            row_body(ui, r, p, &t, false, true);
                        }
                        if resp.hovered() && !resp.dragged() {
                            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                        }
                        if resp.drag_started() {
                            drag_started = Some((*p).clone());
                        }
                        if resp.clicked() {
                            chosen = Some((*p).clone());
                        }
                    }
                }
            }
            // keep the keyboard highlight visible
            if kbd
                && (up || down)
                && let Some(r) = hl_rect
            {
                ui.scroll_to_rect(r, None);
            }
        });
    // ---- footer: keys
    ui.add_space(2.0);
    let fr = ui.available_rect_before_wrap();
    ui.painter()
        .line_segment([fr.left_top(), fr.right_top()], Stroke::new(1.0, t.line));
    let wide = rect.width() > 380.0;
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(
            RichText::new(if ru {
                if wide {
                    "Стрелки — выбор · Enter — вставить · Esc — закрыть · можно перетащить"
                } else {
                    "Стрелки — выбор · Enter — вставить · Esc"
                }
            } else if wide {
                "Arrows select · Enter inserts · Esc closes · drag onto the page"
            } else {
                "Arrows · Enter · Esc"
            })
            .size(11.0)
            .color(t.faint),
        );
    });
    // ---- state updates
    let hl_key = hl_idx.and_then(|i| items.get(i)).map(|p| p.markdown.clone());
    let now = std::time::Instant::now();
    let dragging = egui::DragAndDrop::payload::<DragPreset>(ui.ctx()).is_some() || drag_started.is_some();
    ST.with(|s| {
        let mut s = s.borrow_mut();
        if clear_search {
            s.query.clear();
            s.focus_search = true;
        }
        // the preview follows the hovered row, or the keyboard highlight
        let target = if s.kbd {
            hl_key.clone()
        } else {
            hovered_item.and_then(|(i, _)| items.get(i)).map(|p| p.markdown.clone())
        };
        match (&target, &s.hover) {
            (Some(k), Some((m, _))) if k == m => {}
            (Some(k), _) => {
                s.hover = Some((k.clone(), now));
                s.preview_hidden = false;
                s.preview_forced = false;
            }
            (None, _) => s.hover = None,
        }
        s.hl = hl_key;
        if dragging {
            s.hover = None;
        }
    });
    if let Some(p) = drag_started {
        egui::DragAndDrop::set_payload(ui.ctx(), DragPreset(p));
    }
    if let Some(p) = chosen {
        let tg = ST.with(|s| s.borrow_mut().target.take());
        ST.with(|s| s.borrow_mut().hover = None);
        choose(p, tg);
    }
    // ---- preview popover to the left of the panel (never cropped; scaled to fit the window)
    let (show_md, waiting) = ST.with(|s| {
        let s = s.borrow();
        match &s.hover {
            Some((m, t0)) if !s.preview_hidden && (s.preview_forced || t0.elapsed().as_millis() >= 300) => {
                (Some(m.clone()), false)
            }
            Some(_) if !s.preview_hidden => (None, true),
            _ => (None, false),
        }
    });
    if waiting {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
    if !dragging
        && let Some(md) = show_md
        && let Some(p) = presets.iter().find(|p| p.markdown == md)
    {
        let anchor = if ST.with(|s| s.borrow().kbd) {
            hl_rect
        } else {
            hovered_item.map(|(_, r)| r)
        }
        .unwrap_or(rect);
        let thumb = snaps.as_deref_mut().and_then(|s| s.thumb(&md, THUMB_W));
        let screen = ui.ctx().content_rect();
        let max_h = screen.height() - 70.0;
        let (w, h) = match thumb {
            Some((_, sz)) => {
                let mut w = POP_W.min(rect.left() - screen.left() - 40.0).max(160.0);
                let mut h = sz.y * w / sz.x.max(1.0);
                if h > max_h {
                    w *= max_h / h;
                    h = max_h;
                }
                (w, h)
            }
            None => (POP_W, 60.0),
        };
        let top = (anchor.top() - 10.0)
            .min(screen.bottom() - h - 50.0)
            .max(screen.top() + 8.0);
        let left = (rect.left() - w - 30.0).max(screen.left() + 8.0);
        let title = if p.label != p.block_label && !p.block_label.is_empty() {
            format!("{} · {}", p.block_label, p.label.to_lowercase())
        } else {
            p.label.clone()
        };
        egui::Area::new(egui::Id::unique("markise-block-preview"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(left, top))
            .interactable(false)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .fill(t.field)
                    .stroke(Stroke::new(1.0, t.line))
                    .corner_radius(12.0)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(w);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(title).size(11.0).color(t.muted));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(
                                    RichText::new(if ru { "предпросмотр" } else { "preview" })
                                        .size(11.0)
                                        .color(t.faint),
                                );
                            });
                        });
                        match thumb {
                            Some((tex, _)) => {
                                ui.add(egui::Image::new((tex, vec2(w, h))));
                            }
                            None => {
                                ui.label(
                                    RichText::new(if ru {
                                        "Рисую предпросмотр…"
                                    } else {
                                        "Rendering…"
                                    })
                                    .color(t.muted),
                                );
                                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                            }
                        }
                    });
            });
    }
    // ---- drag ghost: a pill with the preset's label; Esc cancels the drag
    if let Some(d) = egui::DragAndDrop::payload::<DragPreset>(ui.ctx()) {
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            egui::DragAndDrop::clear_payload(ui.ctx());
        } else if let Some(pos) = ui.ctx().pointer_interact_pos() {
            let label = if d.0.label != d.0.block_label && !d.0.block_label.is_empty() {
                format!("{} · {}", d.0.block_label, d.0.label.to_lowercase())
            } else {
                d.0.label.clone()
            };
            let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("markise-block-drag"));
            let painter = ui.ctx().layer_painter(layer);
            let galley = painter.layout_no_wrap(label, egui::FontId::proportional(13.0), Color32::WHITE);
            let r = Rect::from_min_size(pos + vec2(14.0, 6.0), vec2(galley.size().x + 42.0, 28.0));
            painter.rect_filled(r, 14.0, t.accent);
            paint_icon(
                &painter,
                Rect::from_min_size(r.min + vec2(9.0, 5.0), vec2(18.0, 18.0)),
                &d.0.icon,
                Color32::WHITE,
            );
            painter.galley(
                r.min + vec2(32.0, (28.0 - galley.size().y) / 2.0),
                galley,
                Color32::WHITE,
            );
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }
    }
}

/// Icon tile, label and one-line description of a block row (`head`: a heading of presets).
fn row_body(ui: &Ui, r: Rect, p: &Preset, t: &Tok, head: bool, _insertable: bool) {
    let painter = ui.painter_at(r.expand(1.0));
    let ir = Rect::from_center_size(egui::pos2(r.left() + 23.0, r.center().y), vec2(30.0, 30.0));
    painter.rect_filled(ir, 7.0, t.accent_soft);
    paint_icon(
        &painter,
        Rect::from_center_size(ir.center(), vec2(18.0, 18.0)),
        &p.icon,
        t.accent,
    );
    let label = if p.block_label.is_empty() {
        &p.label
    } else {
        &p.block_label
    };
    let x = r.left() + 48.0;
    let max_w = r.right() - x - 8.0;
    let has_desc = !p.description.is_empty();
    let ly = if has_desc {
        r.top() + if head { 4.0 } else { 7.0 }
    } else {
        r.center().y - 9.0
    };
    painter.text(
        egui::pos2(x, ly),
        egui::Align2::LEFT_TOP,
        label,
        egui::FontId::proportional(14.0),
        t.fg,
    );
    if has_desc {
        let font = egui::FontId::proportional(12.0);
        let mut d = p.description.clone();
        while d.chars().count() > 2 && painter.layout_no_wrap(d.clone(), font.clone(), t.muted).size().x > max_w {
            d = d.trim_end_matches('…').to_string();
            d.pop();
            d = d.trim_end().to_string() + "…";
        }
        painter.text(egui::pos2(x, ly + 19.0), egui::Align2::LEFT_TOP, d, font, t.muted);
    }
}

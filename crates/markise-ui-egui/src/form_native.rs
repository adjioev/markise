//! PROTOTYPE: the block form drawn natively in the right side panel: the host side of
//! [`egui_schema_form`]. The crate draws the fields from JSON; this module adds what belongs to
//! Markise: the panel (title, footer, banner, shortcuts, preview debounce), and the two widgets
//! that need the host, pictures (thumbnails, native file picker) and direction buttons.
//! `MARKISE_FORM=web` keeps the JSON Forms WebView ([`crate::form_panel`]) for comparison.
//!
//! Save / draft / cancel go through the same event channel as the WebView's, so
//! [`crate::block_edit::FormState::poll`] and the codec are shared by both.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use egui::{Color32, FontId, Key, Modifiers, Rect, RichText, Sense, Stroke, Ui, vec2};
use egui_schema_form as sf;
use serde_json::{Value, json};

use crate::images::TextureStore;

/// Native unless `MARKISE_FORM=web`.
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MARKISE_FORM").map_or(true, |v| v != "web"))
}

const DEBOUNCE: Duration = Duration::from_millis(150);

thread_local! {
    /// The keyboard is in the form (document commands wait).
    static FOCUSED: Cell<bool> = const { Cell::new(false) };
    static PENDING: Cell<Option<Action>> = const { Cell::new(None) };
}

#[derive(Clone, Copy, PartialEq)]
enum Action {
    Save,
    Cancel,
}

/// The form has the keyboard: document commands must not run.
pub fn focused() -> bool {
    enabled() && FOCUSED.with(Cell::get)
}

/// ⌘S and ⌘W act on the form while it has the keyboard. Returns true when handled.
pub fn forward(id: &str) -> bool {
    if !focused() {
        return false;
    }
    let a = match id {
        "file.save" => Action::Save,
        "file.close" => Action::Cancel,
        _ => return false,
    };
    PENDING.with(|p| p.set(Some(a)));
    true
}

/// An open form.
pub struct NativeForm {
    id: u64,
    title: String,
    note: String,
    warning: Option<String>,
    is_draft: bool,
    ru: bool,
    schema: sf::FormSchema,
    pub values: Value,
    pub state: sf::FormState,
    tx: Sender<Value>,
    /// Codec / save message over the footer.
    pub banner: Option<String>,
    /// Changed since this moment and not sent to the preview yet.
    dirty: Option<Instant>,
    shown: bool,
    /// Whether a picture path is a file (asked once per path).
    files: HashMap<String, bool>,
    /// Current errors (last frame), for scripts.
    pub errors: Vec<sf::FieldError>,
}

impl NativeForm {
    pub fn new(id: u64, req: &Value, schema: &Value, ui_schema: &Value, values: Value, tx: Sender<Value>) -> Self {
        let lang = req["lang"].as_str().unwrap_or("en");
        let mut state = sf::FormState::new();
        if std::env::var_os("MARKISE_PROTO_FORM_SCRIPT").is_some() {
            state.rects = Some(HashMap::new());
        }
        NativeForm {
            id,
            title: req["title"].as_str().unwrap_or("").to_string(),
            note: req["note"].as_str().unwrap_or("").to_string(),
            warning: req["warning"].as_str().map(str::to_string),
            is_draft: req["draft"] == true,
            ru: lang == "ru",
            schema: sf::FormSchema::parse(schema, ui_schema, lang),
            values,
            state,
            tx,
            banner: None,
            dirty: None,
            shown: false,
            files: HashMap::new(),
            errors: Vec::new(),
        }
    }

    fn emit(&self, event: &str, ctx: &egui::Context) {
        let _ = self
            .tx
            .send(json!({"event": event, "id": self.id, "data": self.values, "cold": false, "ms_open": 0.0}));
        ctx.request_repaint();
    }
}

/// Reserves the panel at the right of `rect` and draws the open form in it; returns the rest.
pub fn dock(
    ui: &mut Ui,
    rect: Rect,
    snaps: Option<&mut crate::block_snap::BlockSnaps>,
    images: &mut TextureStore,
) -> Rect {
    if !enabled() {
        return crate::form_panel::reserve(ui, rect);
    }
    let (panel, rest) = crate::form_panel::reserve_rect(ui, rect);
    if let Some(s) = snaps {
        let dir = s.form.doc_dir();
        let mut to_markdown = false;
        if let Some(n) = s.form.native.as_mut() {
            show(ui, panel, n, dir.as_deref(), images, &mut to_markdown);
        }
        if to_markdown {
            s.form.request_markdown();
        }
    }
    rest
}

fn show(ui: &mut Ui, rect: Rect, f: &mut NativeForm, dir: Option<&Path>, images: &mut TextureStore, md: &mut bool) {
    let ctx = ui.ctx().clone();
    let ru = f.ru;
    let style = sf::Style::from_dark(ui.visuals().dark_mode);
    f.state.begin(&ctx);
    // Esc clears egui's focus before the frame starts: "the form had it" is from the last frame
    let mine = f.state.had_focus() || ctx.memory(|m| m.focused()).is_none();
    let mut action = PENDING.with(Cell::take);
    if mine {
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                action = Some(Action::Cancel);
            }
            if i.consume_key(Modifiers::COMMAND, Key::Enter) {
                action = Some(Action::Save);
            }
        });
    }
    let inner = rect.shrink2(vec2(14.0, 12.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).id_salt("markise-native-form"));
    let ui = &mut child;
    ui.set_clip_rect(rect);
    // ---- header
    let head = ui.scope(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
        ui.horizontal(|ui| {
            let (r, resp) = {
                let right = ui.max_rect().right();
                let r = Rect::from_min_size(egui::pos2(right - 26.0, ui.min_rect().top()), vec2(26.0, 26.0));
                (r, ui.interact(r, ui.scope_id().with("close"), Sense::click()))
            };
            if resp.hovered() {
                ui.painter().rect_filled(r, 6.0, style.hover);
            }
            let (c, st) = (r.center(), Stroke::new(1.5, style.muted));
            ui.painter()
                .line_segment([c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0)], st);
            ui.painter()
                .line_segment([c + vec2(-5.0, 5.0), c + vec2(5.0, -5.0)], st);
            if resp.on_hover_text(if ru { "Отмена" } else { "Cancel" }).clicked() {
                action = Some(Action::Cancel);
            }
            let w = ui.available_width() - 32.0;
            ui.scope(|ui| {
                ui.set_max_width(w);
                ui.label(RichText::new(&f.title).size(15.0).strong().color(style.fg));
            });
        });
        if !f.note.is_empty() {
            ui.label(RichText::new(&f.note).size(11.5).color(style.muted));
        }
        if let Some(w) = &f.warning {
            ui.label(RichText::new(w).size(12.0).color(Color32::from_rgb(0xc9, 0x8a, 0x1a)));
        }
    });
    let top = head.response.rect.bottom() + 8.0;
    // ---- footer, drawn first: its height decides the body's
    let mut foot = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::bottom_up(egui::Align::Min)),
    );
    let (save_clicked, cancel_clicked) = foot
        .allocate_ui_with_layout(
            vec2(inner.width(), 32.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                let s = sf::button(ui, if ru { "Сохранить" } else { "Save" }, true, style);
                let c = sf::button(ui, if ru { "Отмена" } else { "Cancel" }, false, style);
                if ui.available_width() > 180.0 {
                    let hint = if ru {
                        "Esc — отмена, ⌘↩ — сохранить"
                    } else {
                        "Esc cancels, ⌘↩ saves"
                    };
                    ui.label(RichText::new(hint).size(11.5).color(style.muted));
                }
                // Tab order: the fields, Cancel, Save
                f.state.add_stop(&c);
                f.state.add_stop(&s);
                f.state.name("$save", s.rect);
                f.state.name("$cancel", c.rect);
                (s.clicked(), c.clicked())
            },
        )
        .inner;
    if let Some(b) = f.banner.clone() {
        foot.label(RichText::new(b).size(12.5).color(style.danger));
    }
    let foot_h = foot.min_rect().height();
    // ---- body
    let body = Rect::from_min_max(
        egui::pos2(inner.left(), top),
        egui::pos2(inner.right(), inner.bottom() - foot_h - 8.0),
    );
    let mut body_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(body)
            .id_salt("markise-native-form-body"),
    );
    let id = f.id;
    let NativeForm {
        schema,
        values,
        state,
        files,
        ..
    } = &mut *f;
    let resp = egui::ScrollArea::vertical()
        .id_salt(("native-form-scroll", id))
        .auto_shrink([false, false])
        .show(&mut body_ui, |ui| {
            ui.set_width(ui.available_width() - 10.0);
            let mut widgets = sf::Widgets::default();
            widgets
                .add("pictures", |ui, c| pictures(ui, c, dir, images, files))
                .add("direction-picker", directions);
            sf::show(ui, schema, values, state, &mut widgets)
        })
        .inner;
    if resp.changed {
        f.dirty = Some(Instant::now());
        f.banner = None;
    }
    f.errors = resp.errors;
    if !resp.unsupported.is_empty() {
        // the block can be edited as source instead (an existing block; a draft is cancelled)
        let t = if ru {
            "Открыть как Markdown"
        } else {
            "Open as Markdown"
        };
        let r = Rect::from_min_size(egui::pos2(inner.left(), body.bottom() - 34.0), vec2(180.0, 32.0));
        let mut b = ui.new_child(egui::UiBuilder::new().max_rect(r).id_salt("native-form-md"));
        if !f.is_draft {
            let btn = sf::button(&mut b, t, false, style);
            f.state.name("$markdown", btn.rect);
            if btn.clicked() {
                *md = true;
            }
        }
    }
    if !f.shown {
        f.shown = true;
        f.emit("shown", &ctx);
    }
    FOCUSED.with(|c| c.set(f.state.had_focus()));
    // ---- live preview
    if let Some(t) = f.dirty {
        let left = DEBOUNCE.saturating_sub(t.elapsed());
        if left.is_zero() {
            f.dirty = None;
            f.emit("draft", &ctx);
        } else {
            ctx.request_repaint_after(left);
        }
    }
    // ---- actions
    if cancel_clicked {
        action = Some(Action::Cancel);
    }
    if save_clicked {
        action = Some(Action::Save);
    }
    match action {
        Some(Action::Cancel) => {
            FOCUSED.with(|c| c.set(false));
            f.emit("cancel", &ctx);
        }
        Some(Action::Save) if !f.errors.is_empty() => {
            f.banner = Some(
                if ru {
                    "Исправьте ошибки в форме"
                } else {
                    "Fix the errors in the form"
                }
                .to_string(),
            );
        }
        Some(Action::Save) => {
            FOCUSED.with(|c| c.set(false));
            f.emit("save", &ctx);
        }
        None => {}
    }
}

/// Clears the keyboard flag when no form was drawn (call at the start of a frame).
pub fn frame_start() {
    FOCUSED.with(|c| c.set(false));
}

// ---------------------------------------------------------------- pictures

fn pictures(
    ui: &mut Ui,
    c: &mut sf::WidgetCtx<'_>,
    dir: Option<&Path>,
    images: &mut TextureStore,
    files: &mut HashMap<String, bool>,
) {
    let ru = c.texts.lang == "ru";
    let s = c.style;
    let min = c.field.schema["minItems"].as_u64().unwrap_or(0) as usize;
    let max = c.field.schema["maxItems"].as_u64().map_or(usize::MAX, |m| m as usize);
    let n = c.items().len();
    let uids = c.rows(n);
    let mut remove = None;
    for i in 0..n {
        let uid = uids[i];
        egui::Frame::new()
            .stroke(Stroke::new(1.0, s.line))
            .corner_radius(10.0)
            .inner_margin(10)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 5.0);
                ui.horizontal(|ui| {
                    let t = format!("{} {}", if ru { "Картинка" } else { "Picture" }, i + 1);
                    ui.label(RichText::new(t).size(12.5).strong().color(s.fg));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let remove_text = c.texts.remove;
                        let b = c.icon_button(ui, sf::Icon::Close, n > min, remove_text);
                        c.rec(b.rect, || format!("{i}/remove"));
                        if b.clicked() {
                            remove = Some(i);
                        }
                    });
                });
                let src = c.items()[i]["src"].as_str().unwrap_or("").to_string();
                thumbnail(ui, &src, dir, images, files, s, ru);
                let sub = |ui: &mut Ui, t: &str| {
                    ui.label(RichText::new(t).size(11.5).color(s.muted));
                };
                sub(
                    ui,
                    if ru {
                        "Описание (alt)"
                    } else {
                        "Description (alt)"
                    },
                );
                let mut alt = c.items()[i]["alt"].as_str().unwrap_or("").to_string();
                let r = c.text_field(
                    ui,
                    (uid, "alt"),
                    &mut alt,
                    sf::TextOpts {
                        rows: 2,
                        ..Default::default()
                    },
                );
                c.rec(r.rect, || format!("{i}/alt"));
                if r.changed() {
                    c.items()[i]["alt"] = Value::String(alt);
                }
                sub(ui, if ru { "Путь к файлу" } else { "File path" });
                let errs = c.take_errors(&format!("{i}/src"));
                ui.horizontal_top(|ui| {
                    let mut p = src.clone();
                    let label = if ru { "Выбрать…" } else { "Choose…" };
                    let width = ui.available_width() - 96.0;
                    let r = c.text_field(
                        ui,
                        (uid, "src"),
                        &mut p,
                        sf::TextOpts {
                            width,
                            invalid: !errs.is_empty(),
                            ..Default::default()
                        },
                    );
                    c.rec(r.rect, || format!("{i}/src"));
                    if r.changed() {
                        c.items()[i]["src"] = Value::String(p);
                    }
                    if c.text_button(ui, label).clicked() {
                        let mut dlg =
                            rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp", "svg"]);
                        if let Some(d) = dir {
                            dlg = dlg.set_directory(d);
                        }
                        if let (Some(file), Some(d)) = (dlg.pick_file(), dir) {
                            c.items()[i]["src"] = json!(crate::block_edit::relative_path(d, &file));
                            c.changed = true;
                        }
                    }
                });
                c.show_errors(ui, &errs);
            });
    }
    if let Some(i) = remove {
        c.remove_item(i);
    }
    let b = c.add_button(
        ui,
        if ru {
            "Добавить картинку"
        } else {
            "Add picture"
        },
        n < max,
    );
    c.rec(b.rect, || "add".into());
    if b.clicked() {
        let uid = c.push_item(json!({"alt": "", "src": ""}));
        c.focus_later((uid, "alt"));
    }
}

fn thumbnail(
    ui: &mut Ui,
    src: &str,
    dir: Option<&Path>,
    images: &mut TextureStore,
    files: &mut HashMap<String, bool>,
    s: sf::Style,
    ru: bool,
) {
    let w = ui.available_width();
    let clean = src.trim().trim_start_matches('<').trim_end_matches('>');
    let abs: Option<PathBuf> = (!clean.is_empty()).then(|| match dir {
        Some(d) => d.join(clean),
        None => PathBuf::from(clean),
    });
    let abs = abs.filter(|p| {
        *files
            .entry(p.to_string_lossy().to_string())
            .or_insert_with(|| p.is_file())
    });
    let tex = abs.and_then(|p| {
        let key = p.to_string_lossy().to_string();
        images.request(ui.ctx(), &key);
        images.texture(&key)
    });
    match tex {
        Some(t) => {
            let size = t.size_vec2();
            let scale = (w / size.x).min(150.0 / size.y).min(1.0);
            let (r, _) = ui.allocate_exact_size(vec2(w, size.y * scale), Sense::hover());
            let img = Rect::from_center_size(r.center(), size * scale);
            egui::Image::new(&t).corner_radius(6.0).paint_at(ui, img);
        }
        None => {
            let (r, _) = ui.allocate_exact_size(vec2(w, 54.0), Sense::hover());
            ui.painter().rect_filled(r, 6.0, s.hover);
            ui.painter().text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                if ru { "нет превью" } else { "no preview" },
                FontId::proportional(12.0),
                s.muted,
            );
        }
    }
}

// ---------------------------------------------------------------- direction buttons

/// The fixed answers as arrows on a crossroads (`options.answers`: `{icon, exclusive}` per
/// answer); an exclusive answer clears the arrows and vice versa.
fn directions(ui: &mut Ui, c: &mut sf::WidgetCtx<'_>) {
    let s = c.style;
    let ru = c.texts.lang == "ru";
    let meta = c.field.options["answers"].as_array().cloned().unwrap_or_default();
    let idx = |icon: &str| meta.iter().position(|m| m["icon"] == icon);
    let gap = 8.0;
    let cell = vec2(((ui.available_width().min(320.0) - 2.0 * gap) / 3.0).floor(), 64.0);
    let left =
        ui.available_rect_before_wrap().left() + ((ui.available_width() - 3.0 * cell.x - 2.0 * gap) / 2.0).max(0.0);
    let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), 3.0 * cell.y + 2.0 * gap), Sense::hover());
    let at = |col: f32, row: f32| {
        Rect::from_min_size(
            egui::pos2(left + col * (cell.x + gap), area.top() + row * (cell.y + gap)),
            cell,
        )
    };
    // the road and the car: decoration
    let mid = at(1.0, 1.0);
    let road = s.line;
    ui.painter().rect_filled(
        Rect::from_center_size(mid.center(), vec2(cell.x, cell.y * 0.3)),
        0.0,
        road,
    );
    ui.painter().rect_filled(
        Rect::from_center_size(mid.center(), vec2(cell.x * 0.3, cell.y)),
        0.0,
        road,
    );
    let car = Rect::from_center_size(at(1.0, 2.0).center(), vec2(18.0, 26.0));
    ui.painter().rect_filled(car, 5.0, s.muted);
    let mut toggled = None;
    let slots = [
        ("straight", 1.0, 0.0),
        ("left", 0.0, 1.0),
        ("right", 2.0, 1.0),
        ("u-turn", 0.0, 2.0),
    ];
    for (icon, col, row) in slots {
        let Some(i) = idx(icon) else { continue };
        let on = c.items().get(i).is_some_and(|x| x["correct"] == true);
        let text = c
            .items()
            .get(i)
            .and_then(|x| x["text"].as_str())
            .unwrap_or("")
            .to_string();
        let r = at(col, row);
        let resp = ui.interact(r, c.id_of(("dir", icon)), Sense::click());
        c.add_control(&resp);
        c.rec(r, || format!("dir/{icon}"));
        let (fill, stroke, fg) = if on {
            (s.accent_soft, Stroke::new(1.5, s.accent), s.accent)
        } else if resp.hovered() {
            (s.hover, Stroke::new(1.0, s.line), s.muted)
        } else {
            (s.field, Stroke::new(1.0, s.line), s.muted)
        };
        ui.painter().rect(r, 10.0, fill, stroke, egui::StrokeKind::Inside);
        sf::focus_ring(ui, &resp, s);
        paint_arrow(ui, r.center() + vec2(0.0, -8.0), icon, fg);
        ui.painter().text(
            r.center() + vec2(0.0, 18.0),
            egui::Align2::CENTER_CENTER,
            &text,
            FontId::proportional(12.0),
            if on { s.fg } else { s.muted },
        );
        if resp.clicked() {
            toggled = Some(i);
        }
    }
    if let Some(i) = idx("stop")
        && c.items().get(i).is_some()
    {
        let on = c.items()[i]["correct"] == true;
        let text = c.items()[i]["text"].as_str().unwrap_or("").to_string();
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
        c.add_control(&resp);
        c.rec(r, || "dir/stop".into());
        let (fill, stroke) = if on {
            (s.danger.gamma_multiply(0.15), Stroke::new(1.5, s.danger))
        } else if resp.hovered() {
            (s.hover, Stroke::new(1.0, s.line))
        } else {
            (s.field, Stroke::new(1.0, s.line))
        };
        ui.painter().rect(r, 10.0, fill, stroke, egui::StrokeKind::Inside);
        sf::focus_ring(ui, &resp, s);
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            &text,
            FontId::proportional(13.0),
            s.fg,
        );
        if resp.clicked() {
            toggled = Some(i);
        }
    }
    if let Some(i) = toggled {
        let exclusive = |j: usize| meta.get(j).is_some_and(|m| m["exclusive"] == true);
        let on = c.items()[i]["correct"] != true;
        let ex = exclusive(i);
        for (j, x) in c.items().iter_mut().enumerate() {
            if j == i {
                x["correct"] = json!(on);
            } else if on && (ex || exclusive(j)) {
                x["correct"] = json!(false);
            }
        }
        c.changed = true;
    }
    if !c.items().iter().any(|x| x["correct"] == true) {
        let msg = if ru {
            "Отметьте хотя бы одно направление или «Ехать нельзя»."
        } else {
            "Pick at least one direction."
        };
        // the codec's own rule says the same; one message is enough
        c.errors.clear();
        c.error(msg);
    }
}

fn paint_arrow(ui: &Ui, c: egui::Pos2, icon: &str, col: Color32) {
    let st = Stroke::new(2.2, col);
    let p = ui.painter();
    let seg = |pts: &[(f32, f32)]| {
        let v: Vec<egui::Pos2> = pts.iter().map(|&(x, y)| c + vec2(x, y)).collect();
        p.add(egui::Shape::line(v, st));
    };
    match icon {
        "left" => {
            seg(&[(9.0, 0.0), (-9.0, 0.0)]);
            seg(&[(-3.0, -6.0), (-9.0, 0.0), (-3.0, 6.0)]);
        }
        "right" => {
            seg(&[(-9.0, 0.0), (9.0, 0.0)]);
            seg(&[(3.0, -6.0), (9.0, 0.0), (3.0, 6.0)]);
        }
        "straight" => {
            seg(&[(0.0, 9.0), (0.0, -9.0)]);
            seg(&[(-6.0, -3.0), (0.0, -9.0), (6.0, -3.0)]);
        }
        _ => {
            // u-turn: up, over the top to the left, down with an arrow head
            let mut v = vec![(5.0, 10.0), (5.0, -2.0)];
            for k in 1..=8 {
                let a = std::f32::consts::PI * k as f32 / 8.0;
                v.push((5.0 * a.cos(), -2.0 - 5.0 * a.sin()));
            }
            v.push((-5.0, 2.0));
            seg(&v);
            seg(&[(-9.0, -2.0), (-5.0, 2.0), (-1.0, -2.0)]);
        }
    }
}

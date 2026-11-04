//! PROTOTYPE: the block form drawn natively in the right side panel: the host side of
//! [`egui_schema_form`]. The crate draws the fields from JSON; this module adds what belongs to
//! Markise: the panel (title, footer, banner, shortcuts, preview debounce), and the two
//! primitives that need the host: `image` (thumbnail, alt, path, native file picker) and
//! `icon-choices` (choice buttons whose icons are files of the package).
//! `MARKISE_FORM=web` keeps the JSON Forms WebView ([`crate::form_panel`]) for comparison.
//!
//! Save / draft / cancel go through the same event channel as the WebView's, so
//! [`crate::block_edit::FormState::poll`] and the codec are shared by both.

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::DerefMut;
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
    /// The library's root (`icon` paths are relative to it) and icons rasterized so far.
    root: Option<PathBuf>,
    icons: HashMap<String, Option<egui::TextureHandle>>,
    /// Current errors (last frame), for scripts.
    pub errors: Vec<sf::FieldError>,
}

impl NativeForm {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: u64,
        req: &Value,
        schema: &Value,
        ui_schema: &Value,
        values: Value,
        root: Option<PathBuf>,
        tx: Sender<Value>,
    ) -> Self {
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
            root,
            icons: HashMap::new(),
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
        icons,
        root,
        ..
    } = &mut *f;
    let root = root.as_deref();
    let resp = egui::ScrollArea::vertical()
        .id_salt(("native-form-scroll", id))
        .auto_shrink([false, false])
        .show(&mut body_ui, |ui| {
            ui.set_width(ui.available_width() - 10.0);
            let imgs = std::cell::RefCell::new(&mut *images);
            let mut widgets = sf::Widgets::default();
            widgets
                .add("image", |ui, c| {
                    image_field(ui, c, dir, imgs.borrow_mut().deref_mut(), files)
                })
                .add("icon-choices", |ui, c| {
                    icon_choices(ui, c, root, imgs.borrow_mut().deref_mut(), icons)
                });
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

// ---------------------------------------------------------------- image

fn image_field(
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

// ---------------------------------------------------------------- icon choices

/// A multiple-choice field drawn as buttons, all from the package: `options.answers` is
/// `[{label, icon?, exclusive?}]` per answer in order, `options.columns` the grid. `icon` is a
/// file of the package (SVG or PNG); an answer without one takes a whole row. `exclusive`:
/// selecting it clears the others and vice versa.
fn icon_choices(
    ui: &mut Ui,
    c: &mut sf::WidgetCtx<'_>,
    root: Option<&Path>,
    images: &mut TextureStore,
    icons: &mut HashMap<String, Option<egui::TextureHandle>>,
) {
    let s = c.style;
    let meta = c.field.options["answers"].as_array().cloned().unwrap_or_default();
    let columns = c.field.options["columns"].as_u64().unwrap_or(3).clamp(1, 8) as usize;
    let gap = 8.0;
    let total = ui.available_width();
    let cell_w = ((total - gap * (columns as f32 - 1.0)) / columns as f32).floor();
    let n = c.items().len();
    let mut toggled = None;
    let mut i = 0;
    while i < n {
        let has_icon = |j: usize| meta.get(j).is_some_and(|m| m["icon"].is_string());
        // one row: consecutive answers with icons up to `columns`, or one without
        let end = if has_icon(i) {
            (i..n).take(columns).take_while(|&j| has_icon(j)).last().unwrap_or(i) + 1
        } else {
            i + 1
        };
        let h = if has_icon(i) { 64.0 } else { 36.0 };
        let (row, _) = ui.allocate_exact_size(vec2(total, h), Sense::hover());
        for j in i..end {
            let r = if has_icon(j) {
                Rect::from_min_size(row.min + vec2((j - i) as f32 * (cell_w + gap), 0.0), vec2(cell_w, h))
            } else {
                row
            };
            let m = meta.get(j).cloned().unwrap_or(Value::Null);
            let on = c.items()[j]["correct"] == true;
            let text = c.items()[j]["text"]
                .as_str()
                .or(m["label"].as_str())
                .unwrap_or("")
                .to_string();
            let resp = ui.interact(r, c.id_of(("choice", j)), Sense::click());
            c.add_control(&resp);
            c.rec(r, || format!("{j}/choice"));
            let (fill, stroke) = if on {
                (s.accent_soft, Stroke::new(1.5, s.accent))
            } else if resp.hovered() {
                (s.hover, Stroke::new(1.0, s.line))
            } else {
                (s.field, Stroke::new(1.0, s.line))
            };
            ui.painter().rect(r, 10.0, fill, stroke, egui::StrokeKind::Inside);
            sf::focus_ring(ui, &resp, s);
            let fg = if on { s.fg } else { s.muted };
            let mut text_at = r.center();
            if let Some(icon) = m["icon"].as_str() {
                text_at += vec2(0.0, 18.0);
                let square = Rect::from_center_size(r.center() + vec2(0.0, -8.0), vec2(26.0, 26.0));
                paint_package_icon(
                    ui,
                    square,
                    icon,
                    root,
                    images,
                    icons,
                    if on { s.accent } else { s.muted },
                );
            }
            let g = ui.painter().layout(
                text,
                FontId::proportional(if m["icon"].is_string() { 12.0 } else { 13.0 }),
                fg,
                r.width() - 8.0,
            );
            ui.painter().galley(text_at - g.size() / 2.0, g, fg);
            if resp.clicked() {
                toggled = Some(j);
            }
        }
        i = end;
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
}

/// Draws a file of the package in `r`: an SVG (its `currentColor` follows `col`) or a raster
/// image. A missing file draws nothing.
fn paint_package_icon(
    ui: &Ui,
    r: Rect,
    icon: &str,
    root: Option<&Path>,
    images: &mut TextureStore,
    icons: &mut HashMap<String, Option<egui::TextureHandle>>,
    col: Color32,
) {
    let Some(root) = root else { return };
    let rel = Path::new(icon);
    if rel.is_absolute() || rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return;
    }
    let path = root.join(rel);
    if icon.ends_with(".svg") {
        let tex = icons
            .entry(icon.to_string())
            .or_insert_with(|| svg_texture(ui.ctx(), &path, icon));
        if let Some(t) = tex {
            egui::Image::new(&*t).tint(col).paint_at(ui, r);
        }
    } else {
        let key = path.to_string_lossy().to_string();
        images.request(ui.ctx(), &key);
        if let Some(t) = images.texture(&key) {
            egui::Image::new(&t).paint_at(ui, r);
        }
    }
}

/// Rasterizes an SVG file white on transparent, for tinting (`currentColor` becomes white).
fn svg_texture(ctx: &egui::Context, path: &Path, name: &str) -> Option<egui::TextureHandle> {
    const PX: u32 = 72;
    let text = std::fs::read_to_string(path).ok()?.replace("currentColor", "#ffffff");
    let tree = resvg::usvg::Tree::from_str(&text, &resvg::usvg::Options::default()).ok()?;
    let mut pix = resvg::tiny_skia::Pixmap::new(PX, PX)?;
    let size = tree.size();
    let scale = PX as f32 / size.width().max(size.height());
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pix.as_mut(),
    );
    let img = egui::ColorImage::from_rgba_premultiplied([PX as usize, PX as usize], pix.data());
    Some(ctx.load_texture(format!("form-icon-{name}"), img, egui::TextureOptions::LINEAR))
}

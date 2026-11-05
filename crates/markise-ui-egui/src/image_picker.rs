//! A window of thumbnails to choose an image from: the images of a folder and its subfolders, the nearest
//! first. Listing and decoding run on workers, which stop when it closes, and it keeps a bounded number of
//! thumbnails. It requests no frame by itself: a worker wakes the app when it has something. It opens no
//! dialog and copies nothing: see [`crate::app`] for what becomes of a choice.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use egui::accesskit::Role;
use egui::{
    Color32, ColorImage, Context, CornerRadius, FontId, Id, Key, Modal, Modifiers, Rect, Sense, Stroke, StrokeKind,
    TextureHandle, TextureOptions, Ui, WidgetInfo, pos2, vec2,
};
use markise_blocks::{DECODE_BUDGET, DECODE_STALE, MemBudget};
use markise_engine::attach;
use markise_engine::image_list::{ImageListing, destinations_in, list_images};

const WINDOW: (f32, f32) = (668.0, 520.0);
const CELL: f32 = 124.0;
const NAME_H: f32 = 22.0;
const GAP: f32 = 8.0;
/// The side of a thumbnail's bitmap, enough for the window's zoom.
const THUMB_PX: u32 = 256;
/// Thumbnails kept: at this size about 64 MB at worst.
const MAX_THUMBS: usize = 256;
/// Decodes waiting at most: those scrolled past long ago go, and are asked for again if they come back.
const MAX_WAITING: usize = 64;
const WORKERS: usize = 2;
const QUERY_ID: &str = "markise-image-picker-query";

/// What came of a frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerAct {
    Choose(PathBuf),
    /// Browse…: the system's dialog.
    Browse,
    Close,
}

struct Item {
    path: PathBuf,
    rel: String,
    in_document: bool,
    current: bool,
}

enum Thumb {
    Waiting,
    Ready(TextureHandle),
    Failed(String),
}

type Decoded = (PathBuf, Result<ColorImage, String>);

/// The thumbnails, and the workers making them. The workers end when nothing waits.
struct Thumbs {
    cells: HashMap<PathBuf, Thumb>,
    /// Those ready, the oldest first.
    order: VecDeque<PathBuf>,
    /// The newest last: what is on screen now goes first.
    waiting: Arc<Mutex<Vec<PathBuf>>>,
    workers: Arc<AtomicUsize>,
    budget: MemBudget,
    tx: Sender<Decoded>,
    rx: Receiver<Decoded>,
}

/// An open picker.
pub struct ImagePicker {
    cancel: Arc<AtomicBool>,
    wake: Arc<dyn Fn() + Send + Sync>,
    listing: Option<Receiver<Found>>,
    items: Vec<Item>,
    truncated: bool,
    unreadable: usize,
    query: String,
    /// Places in `items` that match.
    rows: Vec<usize>,
    /// A place in `rows`.
    selected: Option<usize>,
    columns: usize,
    focus: bool,
    follow: bool,
    last_scroll: (f32, f32),
    note: Option<String>,
    thumbs: Thumbs,
}

/// A listing with what the document says about it.
struct Found {
    listing: ImageListing,
    in_document: HashSet<String>,
    current: Option<String>,
}

impl Drop for ImagePicker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.thumbs.waiting.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// How the destinations a Markdown document would write for `rel` may read, as a key to compare by.
fn key_of(dest: &str) -> String {
    let d = dest.strip_prefix("./").unwrap_or(dest);
    markise_engine::vault::percent_decode(d)
}

impl ImagePicker {
    /// Opens on `folder`. `document` is the text, to mark what it uses; `current` is the destination of the
    /// image this is for. `wake` wakes the app when a worker has something.
    pub fn open(
        folder: &Path,
        document: &str,
        current: Option<&str>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> ImagePicker {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, listing) = channel();
        let (root, stop, waker) = (folder.to_path_buf(), cancel.clone(), wake.clone());
        // a copy, of what matters only: the worker has it for as long as it looks
        let in_document: HashSet<String> = destinations_in(document).into_iter().map(key_of).collect();
        let current = current.map(key_of);
        let spawned = std::thread::Builder::new()
            .name("markise-image-list".into())
            .spawn(move || {
                let found = Found {
                    listing: list_images(&root, &stop),
                    in_document,
                    current,
                };
                let published = crate::wake_track::publishing();
                let _ = tx.send(found);
                waker();
                drop(published);
            });
        let (ttx, trx) = channel();
        ImagePicker {
            cancel,
            wake,
            listing: spawned.is_ok().then_some(listing),
            items: Vec::new(),
            truncated: false,
            unreadable: 0,
            query: String::new(),
            rows: Vec::new(),
            selected: None,
            columns: 1,
            focus: true,
            follow: false,
            last_scroll: (0.0, 0.0),
            note: None,
            thumbs: Thumbs {
                cells: HashMap::new(),
                order: VecDeque::new(),
                waiting: Arc::default(),
                workers: Arc::default(),
                budget: MemBudget::new(DECODE_BUDGET),
                tx: ttx,
                rx: trx,
            },
        }
    }

    /// A worker is still at it.
    pub fn is_busy(&self) -> bool {
        self.listing.is_some()
            || self.thumbs.workers.load(Ordering::SeqCst) > 0
            || self.thumbs.cells.values().any(|c| matches!(c, Thumb::Waiting))
    }

    /// The names shown, as the search leaves them.
    pub fn names(&self) -> Vec<&str> {
        self.rows.iter().map(|&i| self.items[i].rel.as_str()).collect()
    }

    pub fn selected_name(&self) -> Option<&str> {
        self.items
            .get(self.rows.get(self.selected?).copied()?)
            .map(|i| i.rel.as_str())
    }

    /// What it says under the list.
    pub fn note(&self) -> String {
        let mut parts = Vec::new();
        if self.listing.is_some() {
            parts.push("Looking for images…".to_string());
        } else if self.items.is_empty() {
            parts.push("No images in this folder. Use Browse… or drop a file here.".to_string());
        }
        if self.truncated {
            parts.push(format!(
                "Showing the nearest {} images only: use Browse… for the others.",
                self.items.len()
            ));
        }
        if self.unreadable > 0 {
            parts.push(format!("{} folders could not be read.", self.unreadable));
        }
        if let Some(n) = &self.note {
            parts.push(n.clone());
        }
        parts.join(" ")
    }

    /// Something to say.
    pub fn set_note(&mut self, text: String) {
        self.note = Some(text);
    }

    /// A file was added: it comes first, selected.
    pub fn add(&mut self, path: PathBuf, rel: String) {
        self.items.retain(|i| i.path != path);
        self.items.insert(
            0,
            Item {
                path,
                rel,
                in_document: false,
                current: false,
            },
        );
        self.query.clear();
        self.filter();
        self.selected = Some(0);
        self.follow = true;
    }

    fn filter(&mut self) {
        let words: Vec<String> = self.query.split_whitespace().map(str::to_lowercase).collect();
        let was = self.selected.and_then(|s| self.rows.get(s).copied());
        self.rows = (0..self.items.len())
            .filter(|&i| {
                let name = self.items[i].rel.to_lowercase();
                words.iter().all(|w| name.contains(w))
            })
            .collect();
        self.selected = was
            .and_then(|w| self.rows.iter().position(|&r| r == w))
            .or_else(|| (!self.rows.is_empty()).then_some(0));
    }

    fn poll(&mut self, ctx: &Context) {
        if let Some(rx) = &self.listing
            && let Ok(found) = rx.try_recv()
        {
            self.listing = None;
            self.truncated = found.listing.truncated;
            self.unreadable = found.listing.unreadable;
            let current = found.current;
            let mut items: Vec<Item> = (found.listing.images.into_iter())
                .map(|f| {
                    let key = key_of(&f.rel);
                    Item {
                        in_document: found.in_document.contains(&key),
                        current: current.as_deref() == Some(key.as_str()),
                        path: f.path,
                        rel: f.rel,
                    }
                })
                .collect();
            items.append(&mut self.items);
            self.items = items;
            self.filter();
            if let Some(at) = self.rows.iter().position(|&i| self.items[i].current) {
                self.selected = Some(at);
                self.follow = true;
            }
        }
        for (path, result) in self.thumbs.rx.try_iter() {
            match result {
                Ok(image) => {
                    let t = ctx.load_texture(path.to_string_lossy(), image, TextureOptions::LINEAR);
                    self.thumbs.cells.insert(path.clone(), Thumb::Ready(t));
                    self.thumbs.order.push_back(path);
                    while self.thumbs.order.len() > MAX_THUMBS {
                        if let Some(old) = self.thumbs.order.pop_front() {
                            self.thumbs.cells.remove(&old);
                        }
                    }
                }
                Err(why) if why == DECODE_STALE => {
                    self.thumbs.cells.remove(&path);
                }
                Err(why) => {
                    self.thumbs.cells.insert(path, Thumb::Failed(why));
                }
            }
        }
    }

    /// Asks for this one, once.
    fn want_thumb(&mut self, path: &Path) {
        if self.thumbs.cells.contains_key(path) {
            return;
        }
        self.thumbs.cells.insert(path.to_path_buf(), Thumb::Waiting);
        {
            let mut waiting = self.thumbs.waiting.lock().unwrap_or_else(|e| e.into_inner());
            waiting.push(path.to_path_buf());
            let over = waiting.len().saturating_sub(MAX_WAITING);
            if over > 0 {
                for old in waiting.drain(..over) {
                    self.thumbs.cells.remove(&old);
                }
            }
        }
        if self.thumbs.workers.load(Ordering::SeqCst) >= WORKERS {
            return;
        }
        self.thumbs.workers.fetch_add(1, Ordering::SeqCst);
        let (waiting, workers, budget) = (
            self.thumbs.waiting.clone(),
            self.thumbs.workers.clone(),
            self.thumbs.budget.clone(),
        );
        let (tx, wake, cancel) = (self.thumbs.tx.clone(), self.wake.clone(), self.cancel.clone());
        let spawned = std::thread::Builder::new().name("markise-image-thumbs".into()).spawn({
            let workers = workers.clone();
            move || {
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let next = waiting.lock().unwrap_or_else(|e| e.into_inner()).pop();
                    let Some(path) = next else { break };
                    let src = path.to_string_lossy();
                    let stale = || cancel.load(Ordering::Relaxed);
                    let decoded = markise_blocks::decode_unless(&src, THUMB_PX, Default::default(), &budget, &stale)
                        .map(|d| ColorImage::from_rgba_unmultiplied(d.pixels_size, &d.rgba));
                    let published = crate::wake_track::publishing();
                    let _ = tx.send((path, decoded));
                    wake();
                    drop(published);
                }
                workers.fetch_sub(1, Ordering::SeqCst);
                wake();
            }
        });
        if spawned.is_err() {
            workers.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// One frame of the window.
    pub fn show(&mut self, ctx: &Context) -> Option<PickerAct> {
        self.poll(ctx);
        let mut act = None;
        let (up, down, left, right, enter) = ctx.input_mut(|i| {
            let mut k = |key| i.consume_key(Modifiers::NONE, key);
            (
                k(Key::ArrowUp),
                k(Key::ArrowDown),
                k(Key::ArrowLeft),
                k(Key::ArrowRight),
                k(Key::Enter),
            )
        });
        if !self.rows.is_empty() {
            let last = self.rows.len() - 1;
            let at = self.selected.unwrap_or(0);
            let c = self.columns.max(1);
            let to = if up {
                at.saturating_sub(c)
            } else if down {
                (at + c).min(last)
            } else if left {
                at.saturating_sub(1)
            } else if right {
                (at + 1).min(last)
            } else {
                at
            };
            if up || down || left || right {
                self.selected = Some(to);
                self.follow = true;
            }
            if enter && let Some(&i) = self.selected.and_then(|s| self.rows.get(s)) {
                act = Some(PickerAct::Choose(self.items[i].path.clone()));
            }
        }
        let shown = Modal::new(Id::unique("markise-image-picker")).show(ctx, |ui| {
            ui.set_width(WINDOW.0.min(ctx.content_rect().width() - 40.0));
            // no blink: at rest it asks for no frame
            ui.visuals_mut().text_cursor.blink = false;
            self.contents(ui, &mut act);
        });
        if shown.should_close() && act.is_none() {
            act = Some(PickerAct::Close);
        }
        act
    }

    fn contents(&mut self, ui: &mut Ui, act: &mut Option<PickerAct>) {
        ui.label(egui::RichText::new("Choose Image").strong());
        ui.add_space(4.0);
        let query_id = Id::unique(QUERY_ID);
        let field = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .id(query_id)
                .hint_text("Search by name")
                .desired_width(f32::INFINITY),
        );
        field.widget_info(|| WidgetInfo::labeled(Role::TextInput, true, "Search Images"));
        if std::mem::take(&mut self.focus) {
            field.request_focus();
        }
        if field.changed() {
            self.filter();
        }
        ui.add_space(GAP);
        let height = (ui.ctx().content_rect().height() - 180.0).clamp(160.0, WINDOW.1 - 120.0);
        let width = ui.available_width();
        self.columns = (((width + GAP) / (CELL + GAP)).floor() as usize).max(1);
        let rows = self.rows.len().div_ceil(self.columns);
        let row_h = CELL + NAME_H + GAP;
        let mut area = egui::ScrollArea::vertical()
            .max_height(height)
            .min_scrolled_height(height)
            .auto_shrink(false);
        if std::mem::take(&mut self.follow)
            && let Some(s) = self.selected
        {
            let y = (s / self.columns) as f32 * row_h;
            let (top, view) = self.last_scroll;
            if y < top {
                area = area.vertical_scroll_offset(y);
            } else if y + row_h > top + view {
                area = area.vertical_scroll_offset(y + row_h - view);
            }
        }
        let out = area.show_rows(ui, row_h, rows, |ui, range| {
            for r in range {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    for c in 0..self.columns {
                        let place = r * self.columns + c;
                        if place < self.rows.len() {
                            self.cell(ui, place, act);
                        }
                    }
                });
            }
        });
        self.last_scroll = (out.state.clamped_offset().y, out.inner_rect.height());
        ui.add_space(GAP);
        let note = self.note();
        if !note.is_empty() {
            ui.label(egui::RichText::new(note).small().color(ui.visuals().weak_text_color()));
        }
        ui.horizontal(|ui| {
            let chosen = self.selected.and_then(|s| self.rows.get(s)).copied();
            if ui
                .add_enabled(chosen.is_some(), egui::Button::new("Use Image"))
                .clicked()
                && let Some(i) = chosen
            {
                *act = Some(PickerAct::Choose(self.items[i].path.clone()));
            }
            if ui.button("Browse…").clicked() {
                *act = Some(PickerAct::Browse);
            }
            if ui.button("Cancel").clicked() {
                *act = Some(PickerAct::Close);
            }
        });
    }

    fn cell(&mut self, ui: &mut Ui, place: usize, act: &mut Option<PickerAct>) {
        let i = self.rows[place];
        let path = self.items[i].path.clone();
        let (rect, resp) = ui.allocate_exact_size(vec2(CELL, CELL + NAME_H), Sense::click());
        let selected = self.selected == Some(place);
        let (rel, current, in_document) = {
            let it = &self.items[i];
            (it.rel.clone(), it.current, it.in_document)
        };
        let mut label = format!("Image: {rel}");
        if current {
            label.push_str(", the current one");
        } else if in_document {
            label.push_str(", in this document");
        }
        resp.widget_info(|| WidgetInfo::selected(Role::Button, true, selected, &label));
        if resp.clicked() {
            self.selected = Some(place);
        }
        if resp.double_clicked() {
            *act = Some(PickerAct::Choose(path.clone()));
        }
        if ui.is_rect_visible(rect) {
            self.want_thumb(&path);
        }
        let visuals = ui.visuals().clone();
        let accent = visuals.selection.bg_fill;
        let painter = ui.painter_at(rect.expand(2.0));
        let frame = Rect::from_min_size(rect.min, vec2(CELL, CELL));
        painter.rect_filled(frame, CornerRadius::same(4), visuals.faint_bg_color);
        match self.thumbs.cells.get(&path) {
            Some(Thumb::Ready(t)) => {
                let size = t.size_vec2();
                let scale = ((CELL - 8.0) / size.x).min((CELL - 8.0) / size.y).min(1.0);
                let r = Rect::from_center_size(frame.center(), size * scale);
                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                painter.image(t.id(), r, uv, Color32::WHITE);
            }
            Some(Thumb::Failed(why)) => {
                painter.text(
                    frame.center(),
                    egui::Align2::CENTER_CENTER,
                    "Cannot show",
                    FontId::proportional(11.0),
                    visuals.weak_text_color(),
                );
                let why = why.clone();
                let _ = resp.clone().on_hover_text(why);
            }
            _ => {}
        }
        if selected || current || resp.hovered() {
            let width = if selected { 2.0 } else { 1.0 };
            let tone = if selected || current {
                accent
            } else {
                visuals.weak_text_color()
            };
            painter.rect_stroke(
                frame,
                CornerRadius::same(4),
                Stroke::new(width, tone),
                StrokeKind::Inside,
            );
        }
        if in_document || current {
            let tip = pos2(frame.right() - 9.0, frame.top() + 9.0);
            painter.circle_filled(tip, 4.0, accent);
        }
        let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
        let tip = format!("{rel}{}", if in_document { " · in this document" } else { "" });
        let _ = resp.on_hover_text(tip);
        // one line, cut off with a mark: the whole name is on hover
        let mut job = egui::text::LayoutJob::simple_singleline(name, FontId::proportional(12.0), visuals.text_color());
        job.wrap = egui::text::TextWrapping::truncate_at_width(CELL);
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        let clip = Rect::from_min_size(pos2(rect.left(), frame.bottom() + 2.0), vec2(CELL, NAME_H - 2.0));
        ui.painter_at(clip).galley(clip.min, galley, visuals.text_color());
    }
}

/// Is this a file the window takes when it is dropped there?
pub fn is_droppable(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(attach::is_image)
}

//! The block library (ADR 0022): a small list by the caret with a package's presets, in its groups
//! and order. All it says comes from the package; it knows no block. It reads no file: icons come
//! through the form's worker ([`FormAssets`]) by the same rules. See [`crate::app`] for when it opens.

use egui::accesskit::Role;
use egui::text::LayoutJob;
use egui::{
    Align2, Context, CornerRadius, CursorIcon, FontId, Id, Key, Modifiers, Order, Rect, RichText, Sense, Stroke, Ui,
    WidgetInfo, pos2, vec2,
};
use egui_schema_form::{Style, Thumbnail};
use markise_package::Library;

use crate::block_form::FormAssets;

const WIDTH: f32 = 340.0;
const LIST_HEIGHT: f32 = 340.0;
const ROW_HEIGHT: f32 = 44.0;
const ROW_ONE_LINE: f32 = 30.0;
const ICON: f32 = 22.0;
const MARGIN: f32 = 12.0;
const QUERY_ID: &str = "markise-block-library-query";

/// How the library was opened, which decides what it gives back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opened {
    /// By `/` on a free line: nothing is written yet, and the person may have meant the text.
    Typed,
    /// By a command.
    Command,
}

/// What came of a frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibraryAct {
    /// A row: the preset's place in the package.
    Choose(usize),
    /// Esc, or Backspace with nothing typed: the keyboard goes back.
    Close,
    /// A press outside: it goes on to what is there.
    Dismiss,
    /// What was typed was text after all, `/` first.
    Type(String),
}

/// The open library.
pub struct OpenLibrary {
    /// The document and revision it was opened for: another one closes it.
    pub(crate) origin: (u64, u64),
    library: Library,
    opened: Opened,
    query: String,
    /// Places in [`Library::entries`] that match.
    rows: Vec<usize>,
    /// A place in `rows`.
    selected: usize,
    focus: bool,
    composing: bool,
    /// The caret on screen when it opened.
    anchor: Option<Rect>,
    /// Where it was drawn last.
    rect: Option<Rect>,
    /// Rows that got their picture in the last frame.
    pictures: std::cell::Cell<usize>,
    pub(crate) previous_focus: Option<Id>,
}

impl OpenLibrary {
    pub fn new(
        origin: (u64, u64),
        library: Library,
        opened: Opened,
        query: String,
        anchor: Option<Rect>,
        previous_focus: Option<Id>,
    ) -> OpenLibrary {
        let rows = library.filter(&query);
        OpenLibrary {
            origin,
            library,
            opened,
            query,
            rows,
            selected: 0,
            focus: true,
            composing: false,
            anchor,
            rect: None,
            pictures: std::cell::Cell::new(0),
            previous_focus,
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// The titles of the rows showing.
    pub fn titles(&self) -> Vec<&str> {
        (self.rows.iter())
            .map(|&i| self.library.entries()[i].title.as_str())
            .collect()
    }

    /// The title of the selected row.
    pub fn selected_title(&self) -> Option<&str> {
        let i = *self.rows.get(self.selected)?;
        Some(self.library.entries()[i].title.as_str())
    }

    /// Rows drawn with a picture in the last frame.
    pub fn pictures(&self) -> usize {
        self.pictures.get()
    }

    /// Where it stands on screen, as last drawn.
    pub fn rect(&self) -> Option<Rect> {
        self.rect
    }

    fn set_query_rows(&mut self) {
        self.rows = self.library.filter(&self.query);
        self.selected = 0;
    }

    fn track_composition(&mut self, ctx: &Context) {
        ctx.input(|i| {
            for e in &i.events {
                match e {
                    egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => self.composing = !text.is_empty(),
                    egui::Event::Ime(egui::ImeEvent::Commit(_)) => self.composing = false,
                    _ => {}
                }
            }
        });
    }

    /// Keys, before the field and the document see them: an action, whether the selection moved and
    /// whether Return was pressed. Return is acted on after the field has read this frame's text. Keys
    /// belong to a composition while one is going on, and the Return that ends one is its own.
    fn keys(&mut self, ctx: &Context) -> (Option<LibraryAct>, bool, bool) {
        let was_composing = self.composing;
        self.track_composition(ctx);
        let (commit, typed) = ctx.input(|i| {
            let commit = (i.events.iter()).any(|e| matches!(e, egui::Event::Ime(egui::ImeEvent::Commit(_))));
            (commit, i.events.iter().any(|e| matches!(e, egui::Event::Text(_))))
        });
        if was_composing || self.composing {
            ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
            return (None, false, false);
        }
        let (up, down, enter, escape, back) = ctx.input_mut(|i| {
            let back = self.query.is_empty() && !typed && i.consume_key(Modifiers::NONE, Key::Backspace);
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
                back,
            )
        });
        let n = self.rows.len();
        let moved = (up || down) && n > 0;
        if up && n > 0 {
            self.selected = (self.selected + n - 1) % n;
        }
        if down && n > 0 {
            self.selected = (self.selected + 1) % n;
        }
        if escape || (back && self.opened == Opened::Typed) {
            return (Some(LibraryAct::Close), false, false);
        }
        (None, moved, enter && !commit)
    }

    /// One frame.
    pub fn ui(
        &mut self,
        ctx: &Context,
        style: &Style,
        fill: egui::Color32,
        assets: &mut FormAssets,
    ) -> Option<LibraryAct> {
        // the field cannot read yet in the frame it first shows
        if self.focus {
            ctx.input_mut(|i| {
                i.events.retain(|e| match e {
                    egui::Event::Text(t) | egui::Event::Ime(egui::ImeEvent::Commit(t)) => {
                        self.query.push_str(t);
                        false
                    }
                    _ => true,
                });
            });
            self.set_query_rows();
        }
        let (act, moved, enter) = self.keys(ctx);
        if act.is_some() {
            return act;
        }
        if let Some(r) = self.rect
            && ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !r.contains(p)))
        {
            return Some(LibraryAct::Dismiss);
        }
        let screen = ctx.content_rect();
        let width = WIDTH.min(screen.width() - 2.0 * MARGIN).max(120.0);
        // below the caret; above it when there is more room there
        let (pos, pivot, room) = match self.anchor {
            Some(c) => {
                let x = c
                    .left()
                    .clamp(screen.left() + MARGIN, (screen.right() - width - MARGIN).max(MARGIN));
                let below = screen.bottom() - c.bottom() - MARGIN - 6.0;
                let above = c.top() - screen.top() - MARGIN - 6.0;
                if below >= LIST_HEIGHT.min(above) {
                    (pos2(x, c.bottom() + 6.0), Align2::LEFT_TOP, below)
                } else {
                    (pos2(x, c.top() - 6.0), Align2::LEFT_BOTTOM, above)
                }
            }
            None => (
                pos2(screen.center().x - width / 2.0, screen.top() + 60.0),
                Align2::LEFT_TOP,
                screen.height() - 60.0 - MARGIN,
            ),
        };
        let mut chosen = None;
        let area = egui::Area::new(Id::unique("markise-block-library"))
            .order(Order::Foreground)
            .pivot(pivot)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(fill)
                    .stroke(Stroke::new(1.0, style.line))
                    .corner_radius(CornerRadius::same(8))
                    .shadow(ctx.global_style().visuals.popup_shadow)
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                        chosen = self.contents(ui, style, assets, moved, (room - 56.0).clamp(ROW_HEIGHT, LIST_HEIGHT));
                    });
            });
        self.rect = Some(area.response.rect);
        if let Some(act) = chosen {
            return Some(act);
        }
        if self.opened == Opened::Typed && self.query == "/" {
            return Some(LibraryAct::Type("/".to_string()));
        }
        if !enter {
            return None;
        }
        match self.rows.get(self.selected) {
            Some(&i) => Some(LibraryAct::Choose(self.library.entries()[i].preset)),
            None if self.opened == Opened::Typed => Some(LibraryAct::Type(format!("/{}", self.query))),
            None => None,
        }
    }

    fn contents(
        &mut self,
        ui: &mut Ui,
        style: &Style,
        assets: &mut FormAssets,
        moved: bool,
        list: f32,
    ) -> Option<LibraryAct> {
        let field = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .id(Id::unique(QUERY_ID))
                .hint_text(RichText::new("Find a block…").color(style.muted))
                .text_color(style.fg)
                .frame(egui::Frame::NONE)
                .margin(egui::Margin::symmetric(8, 6))
                .desired_width(f32::INFINITY),
        );
        field.widget_info(|| WidgetInfo::labeled(Role::TextInput, true, "Find a block"));
        if std::mem::take(&mut self.focus) {
            field.request_focus();
        }
        if field.changed() {
            self.set_query_rows();
        }
        let line = ui.available_rect_before_wrap().x_range();
        ui.painter()
            .hline(line, field.rect.bottom(), Stroke::new(1.0, style.line));
        ui.add_space(4.0);
        self.pictures.set(0);
        let ctx = ui.ctx().clone();
        let with_icons = (self.rows.iter()).any(|&i| {
            (self.library.entries()[i].icon.as_deref())
                .is_some_and(|n| !matches!(assets.svg(&ctx, n), Thumbnail::Failed(_) | Thumbnail::None))
        });
        let mut chosen = None;
        if self.rows.is_empty() {
            ui.add_space(4.0);
            let text = format!("No block matches “{}”.", self.query);
            ui.label(RichText::new(text).color(style.muted));
            if self.opened == Opened::Typed {
                ui.label(RichText::new("Return types it as text.").color(style.muted).small());
            }
            ui.add_space(4.0);
            return None;
        }
        egui::ScrollArea::vertical()
            .max_height(list)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                let mut group = None;
                for (row, &i) in self.rows.clone().iter().enumerate() {
                    let entry = &self.library.entries()[i];
                    if entry.group != group {
                        group = entry.group;
                        let label = entry.group.and_then(|g| self.library.groups().get(g));
                        group_head(ui, label.map(String::as_str), style, row == 0);
                    }
                    let selected = row == self.selected;
                    let resp = self.row(ui, i, selected, with_icons, style, assets);
                    if selected && moved {
                        resp.scroll_to_me(None);
                    }
                    if resp.hovered() && ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                        self.selected = row;
                    }
                    if resp.clicked() {
                        chosen = Some(LibraryAct::Choose(self.library.entries()[i].preset));
                    }
                }
            });
        chosen
    }

    fn row(
        &self,
        ui: &mut Ui,
        i: usize,
        selected: bool,
        with_icons: bool,
        style: &Style,
        assets: &mut FormAssets,
    ) -> egui::Response {
        let entry = &self.library.entries()[i];
        let height = if entry.detail.is_some() {
            ROW_HEIGHT
        } else {
            ROW_ONE_LINE
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
        let resp = resp.on_hover_cursor(CursorIcon::Default);
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, &entry.title));
        if !ui.is_rect_visible(rect) {
            return resp;
        }
        let painter = ui.painter_at(rect);
        if selected {
            painter.rect_filled(rect, 5.0, style.accent_soft);
        }
        let mut left = rect.left() + 8.0;
        if with_icons {
            let icon = Rect::from_center_size(pos2(left + ICON / 2.0, rect.center().y), vec2(ICON, ICON));
            let picture = entry.icon.as_deref().map(|n| assets.svg(ui.ctx(), n));
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            match picture {
                Some(Thumbnail::Mask { texture, .. }) => {
                    painter.image(texture, icon, uv, style.fg);
                    self.pictures.set(self.pictures.get() + 1);
                }
                Some(Thumbnail::Ready { texture, .. }) => {
                    painter.image(texture, icon, uv, egui::Color32::WHITE);
                    self.pictures.set(self.pictures.get() + 1);
                }
                _ => {}
            }
            left += ICON + 10.0;
        }
        let width = rect.right() - left - 8.0;
        let line = |text: &str, size: f32, color| {
            let mut job = LayoutJob::simple_singleline(text.to_owned(), FontId::proportional(size), color);
            job.wrap.max_width = width;
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            ui.ctx().fonts_mut(|f| f.layout_job(job))
        };
        let title = line(&entry.title, 14.0, style.fg);
        match &entry.detail {
            Some(detail) => {
                let detail = line(detail, 11.5, style.muted);
                let top = rect.center().y - (title.size().y + detail.size().y + 1.0) / 2.0;
                painter.galley(pos2(left, top + title.size().y + 1.0), detail, style.muted);
                painter.galley(pos2(left, top), title, style.fg);
            }
            None => {
                painter.galley(pos2(left, rect.center().y - title.size().y / 2.0), title, style.fg);
            }
        }
        resp
    }
}

fn group_head(ui: &mut Ui, label: Option<&str>, style: &Style, first: bool) {
    ui.add_space(if first { 2.0 } else { 8.0 });
    let Some(label) = label else { return };
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(RichText::new(label).color(style.muted).size(11.5).strong());
    });
    ui.add_space(3.0);
}

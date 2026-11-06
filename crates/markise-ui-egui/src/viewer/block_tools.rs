//! The button on a block's card that opens its form (ADR 0020). It shows only for a block whose
//! package is in use and has a schema for it; a document with no package never gets here.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use egui::accesskit::Role;
use egui::{Pos2, Rect, Sense, Stroke, TextureId, Ui, WidgetInfo, pos2, vec2};
use markise_engine::block_view::PackageViews;
use markise_layout::block::Prim;
use markise_layout::{BlockLook, BlockViewMode, BlockViews, ItemKind, SiteQuery, SiteShot, Visible};
use markise_package::{InstalledPackage, PackageIndex};
use markise_theme::Theme;

use super::site_board::{HelperSource, SiteBoard};
use super::{DocViewer, ViewerOutput};

/// What the app knows about the pictures of block icons. The layout asks without blocking; the app
/// loads what was asked for through the worker the form's icons use ([`crate::block_form`]), once
/// per frame when there is something to do, and draws them.
#[derive(Default)]
pub struct IconBoard {
    state: Mutex<IconState>,
}

#[derive(Default)]
struct IconState {
    ready: HashMap<Box<str>, (TextureId, bool)>,
    /// Asked for, with no answer yet.
    wanted: HashSet<Box<str>>,
    /// Something new was asked for since the host last looked.
    fresh: bool,
    /// No such picture, or one that cannot be drawn: no icon, and no more questions.
    gone: HashSet<Box<str>>,
}

impl IconBoard {
    fn lock(&self) -> std::sync::MutexGuard<'_, IconState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn ready(&self, name: &str) -> bool {
        let mut s = self.lock();
        if s.ready.contains_key(name) {
            return true;
        }
        if !s.gone.contains(name) && !s.wanted.contains(name) {
            s.wanted.insert(name.into());
            s.fresh = true;
        }
        false
    }

    fn texture(&self, name: &str) -> Option<(TextureId, bool)> {
        self.lock().ready.get(name).copied()
    }

    /// Something new was asked for, once: a frame has to come for it to be loaded.
    pub fn take_fresh(&self) -> bool {
        std::mem::take(&mut self.lock().fresh)
    }

    /// Something was asked for and has no answer yet.
    pub fn is_waiting(&self) -> bool {
        !self.lock().wanted.is_empty()
    }

    /// Goes through what is wanted with `load`, which must not block: `None` means not yet. Returns
    /// whether a picture is new, so that what asked for it is drawn again.
    pub fn take_answers(&self, mut load: impl FnMut(&str) -> Option<Option<(TextureId, bool)>>) -> bool {
        let mut s = self.lock();
        if s.wanted.is_empty() {
            return false;
        }
        let mut arrived = false;
        let wanted: Vec<Box<str>> = s.wanted.iter().cloned().collect();
        for name in wanted {
            match load(&name) {
                None => {}
                Some(Some(picture)) => {
                    arrived = true;
                    s.wanted.remove(&name);
                    s.ready.insert(name, picture);
                }
                Some(None) => {
                    s.wanted.remove(&name);
                    s.gone.insert(name);
                }
            }
        }
        arrived
    }

    /// Another package: nothing here is its.
    pub fn clear(&self) {
        *self.lock() = IconState::default();
    }
}

/// Where the board for the package in use is, if any: the view holds it through its views.
pub(super) type SiteSlot = Arc<Mutex<Option<Arc<SiteBoard>>>>;

/// A package's views with its pictures.
struct ViewsWithIcons {
    views: Arc<PackageViews>,
    icons: Arc<IconBoard>,
    site: SiteSlot,
}

impl ViewsWithIcons {
    fn board(&self) -> Option<Arc<SiteBoard>> {
        self.site.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl BlockViews for ViewsWithIcons {
    fn id(&self) -> u64 {
        self.views.id()
    }
    fn has_view(&self, name: &str) -> bool {
        self.views.has_view(name)
    }
    fn has_schema(&self, name: &str) -> bool {
        self.views.has_schema(name)
    }
    fn has_practice(&self, name: &str) -> bool {
        self.views.has_practice(name)
    }
    fn any_practice(&self) -> bool {
        BlockViews::any_practice(&*self.views)
    }
    fn practice_hides_quotes(&self, name: &str) -> bool {
        self.views.practice_hides_quotes(name)
    }
    fn icon_ready(&self, icon: &str) -> bool {
        self.icons.ready(icon)
    }
    fn has_site(&self) -> bool {
        self.board().is_some_and(|b| b.usable())
    }
    fn site_shot(&self, query: &SiteQuery<'_>) -> (u64, SiteShot) {
        self.board().map_or((0, SiteShot::Pending(None)), |b| b.shot(query))
    }
    fn site_seen(&self, key: u64) {
        if let Some(b) = self.board() {
            b.seen(key);
        }
    }
    fn look(&self, name: &str, args: &str, source: &str) -> BlockLook {
        self.views.look(name, args, source)
    }
}

impl DocViewer {
    /// The package whose blocks can be drawn by its code; `None` when none is in use. No helper starts
    /// here: only when a block on screen needs one. Called per frame: a comparison when nothing changed.
    pub fn set_block_site(
        &mut self,
        package: Option<&InstalledPackage>,
        helpers: Option<&HelperSource>,
        ctx: &egui::Context,
    ) {
        let folder = self.ed.path().and_then(|p| p.parent()).map(|p| p.to_path_buf());
        let current = self.block_site.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let fine = match (package, &current) {
            (None, None) => true,
            (Some(p), Some(b)) => b.is_for(p, folder.as_ref()),
            _ => false,
        };
        if fine {
            return;
        }
        let board = package
            .zip(self.block_fields.clone())
            .map(|(package, views)| Arc::new(SiteBoard::new(views, package.clone(), folder, helpers, ctx)));
        *self.block_site.lock().unwrap_or_else(|e| e.into_inner()) = board;
        self.site_read_at = u64::MAX;
        self.ed.view.site_changed();
    }

    /// A block on screen is being drawn: its picture is still to come (tests).
    pub fn site_busy(&self) -> bool {
        let board = self.block_site.lock().unwrap_or_else(|e| e.into_inner()).clone();
        board.is_some_and(|b| b.is_busy())
    }

    /// Before the frame is laid out: what came from the helper, and the document's language.
    pub(super) fn site_before_frame(&mut self, ctx: &egui::Context) {
        let Some(board) = self.block_site.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
            return;
        };
        let changes = self.ed.changes();
        let mut again = false;
        if self.site_read_at != changes {
            self.site_read_at = changes;
            let language = markise_engine::block_page::document_language(self.ed.session.doc());
            again |= board.set_language(language);
        }
        again |= board.take_arrivals(ctx);
        if again {
            self.ed.view.site_changed();
        }
    }

    fn paint_pictures(&self, ui: &Ui, v: &Visible, surface: Rect, left: f32, board: &SiteBoard) {
        let origin = vec2(left, surface.top() + v.y);
        for p in &v.layout.prims {
            let Prim::Picture { rect, id } = p else { continue };
            let r = rect.translate(origin);
            let painter = ui.painter_at(surface);
            match board.texture(*id) {
                Some(texture) => {
                    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                    painter.image(texture, r, uv, egui::Color32::WHITE);
                }
                None => {
                    painter.rect_filled(r, 4.0, ui.visuals().faint_bg_color);
                }
            }
        }
    }

    /// The quiet reason a card shows no picture, on hover, and for a screen reader.
    fn note_tip(&self, ui: &mut Ui, v: &Visible, surface: Rect, left: f32) {
        let Some(rect) = v.layout.note else { return };
        let Some(why) = self.ed.view.block_note(v.idx) else {
            return;
        };
        let r = rect.translate(vec2(left, surface.top() + v.y));
        if !surface.contains_rect(r) {
            return;
        }
        let id = ui.scope_id().with(("markise-block-note", v.idx));
        let resp = ui.interact(r, id, Sense::hover()).on_hover_text(&*why);
        resp.widget_info(|| WidgetInfo::labeled(Role::Label, true, format!("Not drawn by its site's code: {why}")));
    }

    fn paint_icons(&self, ui: &Ui, v: &Visible, surface: Rect, left: f32) {
        let origin = vec2(left, surface.top() + v.y);
        for p in &v.layout.prims {
            let Prim::Icon { rect, name, tint } = p else { continue };
            let Some((texture, mask)) = self.block_icons.texture(name) else {
                continue;
            };
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            let tint = if mask { *tint } else { egui::Color32::WHITE };
            ui.painter_at(surface).image(texture, rect.translate(origin), uv, tint);
        }
    }

    /// All the reasons on hover, and for a screen reader, on a block's warning.
    fn warning_tip(&self, ui: &mut Ui, v: &Visible, surface: Rect, left: f32) {
        let Some(rect) = v.layout.warning else { return };
        let Some(why) = self.ed.view.block_warnings(v.idx) else {
            return;
        };
        let r = rect.translate(vec2(left, surface.top() + v.y));
        if !surface.contains_rect(r) {
            return;
        }
        let all = why.join("\n");
        let id = ui.scope_id().with(("markise-block-warning", v.idx));
        let resp = ui.interact(r, id, Sense::hover()).on_hover_text(&all);
        // (what a block in Practice says is all it says: nothing here can tell an answer)
        let at = self.ed.view.model().item_start(v.idx);
        let what = if self.ed.in_practice(at) {
            "Practice"
        } else if self.ed.view.block_outside(v.idx) {
            "Outside its schema"
        } else {
            "Not filled in yet"
        };
        resp.widget_info(|| WidgetInfo::labeled(Role::Label, true, format!("{what}: {all}")));
    }

    /// The button that switches a block's view: it says what comes next.
    #[allow(clippy::too_many_arguments)]
    fn block_view_button(
        &self,
        ui: &mut Ui,
        r: Rect,
        surface: Rect,
        opener: usize,
        next: BlockViewMode,
        name: &str,
        out: &mut ViewerOutput,
    ) {
        let says = match next {
            BlockViewMode::Native => "Show Native View",
            BlockViewMode::Practice => "Show Native View",
            BlockViewMode::Site => "Show as on the Site",
            BlockViewMode::Markdown => "Show Markdown",
        };
        let id = ui.scope_id().with(("markise-block-view", opener));
        let resp = ui.interact(r, id, Sense::click()).on_hover_text(says);
        let label = format!("{says}: {name}");
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, &label));
        if resp.clicked() {
            out.switch_block_view = Some(opener);
            ui.ctx().request_repaint();
        }
        let tone = if resp.hovered() {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        let painter = ui.painter_at(surface);
        if resp.hovered() {
            painter.rect_filled(r, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
        }
        let (c, s) = (r.center(), r.width() * 0.26);
        let stroke = Stroke::new((r.width() * 0.08).max(1.0), tone);
        match next {
            // the source: an M
            BlockViewMode::Markdown => {
                let pts = [
                    c + vec2(-s, s),
                    c + vec2(-s, -s),
                    c + vec2(0.0, s * 0.3),
                    c + vec2(s, -s),
                    c + vec2(s, s),
                ];
                painter.add(egui::Shape::line(pts.to_vec(), stroke));
            }
            // a page in a frame: what a site draws
            BlockViewMode::Site => {
                painter.circle_stroke(c, s * 1.1, stroke);
                painter.line_segment([c + vec2(-s * 1.1, 0.0), c + vec2(s * 1.1, 0.0)], stroke);
                let waist = Rect::from_center_size(c, vec2(s, 2.2 * s));
                painter.add(egui::Shape::ellipse_stroke(c, waist.size() * 0.5, stroke));
            }
            // a card
            BlockViewMode::Native | BlockViewMode::Practice => {
                let rect = Rect::from_center_size(c, vec2(2.0 * s, 1.6 * s));
                painter.rect_stroke(rect, 1.0, stroke, egui::StrokeKind::Middle);
                painter.line_segment(
                    [
                        rect.left_top() + vec2(0.0, s * 0.5),
                        rect.right_top() + vec2(0.0, s * 0.5),
                    ],
                    stroke,
                );
            }
        }
    }

    /// The language this viewer shows package text in, as of the last frame.
    pub fn block_language(&self) -> &str {
        &self.block_language
    }

    /// What the blocks are read with now: built anew only for another package or language.
    pub fn block_fields(&self) -> Option<&Arc<PackageViews>> {
        self.block_fields.as_ref()
    }

    /// The package whose blocks have a form; `None` when there is none in use. Its text follows the
    /// document's language. Called per frame: a comparison unless the document changed, and only a
    /// changed language rebuilds anything.
    pub fn set_block_index(&mut self, index: Option<Arc<PackageIndex>>) {
        let same = match (&index, &self.block_index) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        let changes = self.ed.changes();
        if same && (index.is_none() || self.block_language_read_at == changes) {
            return;
        }
        self.block_language_read_at = changes;
        let language = crate::block_form::package_language(self.ed.session.doc());
        if same && language == self.block_language {
            return;
        }
        self.block_language = language;
        if !same {
            self.block_icons.clear();
            // what it drew is another package's
            *self.block_site.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
        let fields = index
            .clone()
            .map(|i| Arc::new(PackageViews::new(i, &self.block_language)));
        let views = fields.clone().map(|views| -> Arc<dyn BlockViews> {
            Arc::new(ViewsWithIcons {
                views,
                icons: self.block_icons.clone(),
                site: self.block_site.clone(),
            })
        });
        self.block_fields = fields.clone();
        self.ed.view.set_block_views(views);
        self.ed.set_block_fields(fields);
        self.block_index = index;
    }

    /// The pictures of block icons this viewer shows.
    pub fn block_icons(&self) -> &Arc<IconBoard> {
        &self.block_icons
    }

    pub(super) fn block_tools_ui(
        &mut self,
        ui: &mut Ui,
        visible: &[Visible],
        surface: Rect,
        left: f32,
        theme: &Theme,
        out: &mut ViewerOutput,
    ) {
        self.pointer_on_block_control = false;
        let Some(index) = self.block_index.clone() else {
            return;
        };
        if self.block_icons.take_fresh() {
            ui.ctx().request_repaint();
        }
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let model = self.ed.view.model().clone();
        let side = theme.line_px().min(22.0);
        let board = self.block_site.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(b) = &board {
            b.tell_client();
        }
        for v in visible {
            if let Some(b) = &board {
                self.paint_pictures(ui, v, surface, left, b);
                self.note_tip(ui, v, surface, left);
            }
            self.paint_icons(ui, v, surface, left);
            self.warning_tip(ui, v, surface, left);
            let opener = model
                .items
                .get(v.idx)
                .is_some_and(|it| it.kind == (ItemKind::ContainerMarker { close: false }));
            if !opener {
                continue;
            }
            let start = model.item_start(v.idx);
            let containers = self.ed.session.doc().named_containers_at(start);
            let Some(c) = containers.last().filter(|c| index.block(&c.name).is_some()) else {
                continue;
            };
            // the card's top edge, the line a marker draws from side to side
            let Some((_, b)) = v.layout.card_top else { continue };
            let origin = vec2(left, surface.top() + v.y);
            let top_right: Pos2 = b + origin;
            let r = Rect::from_min_size(pos2(top_right.x - side - 4.0, top_right.y + 4.0), vec2(side, side));
            if !surface.contains_rect(r) {
                continue;
            }
            // a block's first row is no text, and its buttons are controls
            let row = Rect::from_min_max(
                pos2(left, surface.top() + v.y),
                pos2(top_right.x, surface.top() + v.y + v.layout.height),
            );
            let has_view = index.block(&c.name).is_some_and(|e| e.view.is_some());
            let outside = self.ed.view.block_outside(v.idx);
            if pointer.is_some_and(|p| row.contains(p)) {
                self.pointer_on_block_control = true;
            }
            // over a picture the buttons need a ground of their own, and only show when asked for
            let over_picture = v.layout.has_picture();
            if over_picture && pointer.is_some_and(|p| row.contains(p)) {
                let ground = ui.visuals().window_fill().gamma_multiply(0.85);
                let all = r.union(r.translate(vec2(-(side + 4.0), 0.0)));
                ui.painter_at(surface).rect_filled(all, 4.0, ground);
            }
            // Read has no block controls (ADR 0024)
            if self.ed.read_mode() {
                continue;
            }
            if (has_view || self.ed.view.can_draw_site()) && !outside {
                let next = self.ed.next_block_view(start);
                let m = r.translate(vec2(-(side + 4.0), 0.0));
                if !v.layout.has_picture() || pointer.is_some_and(|p| row.contains(p)) {
                    self.block_view_button(ui, m, surface, start, next, &c.name, out);
                }
            }
            let label = format!("Edit Block: {}", c.name);
            let resp = ui
                .interact(r, ui.scope_id().with(("markise-block-edit", start)), Sense::click())
                .on_hover_text("Edit Block");
            resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, &label));
            if resp.clicked() {
                out.open_block_form = Some(start);
                ui.ctx().request_repaint();
            }
            let tone = if resp.hovered() {
                ui.visuals().text_color()
            } else {
                ui.visuals().weak_text_color()
            };
            let painter = ui.painter_at(surface);
            if resp.hovered() {
                painter.rect_filled(r, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            if over_picture && !pointer.is_some_and(|p| row.contains(p)) {
                continue;
            }
            // a pencil: a slanted stroke and its tip
            let s = side * 0.28;
            let c0 = r.center();
            let stroke = Stroke::new((side * 0.09).max(1.0), tone);
            painter.line_segment([c0 + vec2(s, -s), c0 + vec2(-s * 0.6, s * 0.6)], stroke);
            painter.line_segment([c0 + vec2(-s * 0.6, s * 0.6), c0 + vec2(-s, s)], stroke);
            painter.line_segment([c0 + vec2(s * 0.45, -s * 1.1), c0 + vec2(s * 1.1, -s * 0.45)], stroke);
        }
    }

    /// The delete button of the block under the pointer, next to its other buttons. It shows for any
    /// paired block, with no package as well: a press takes one undo step away from nothing lost.
    pub(super) fn block_delete_ui(
        &mut self,
        ui: &mut Ui,
        visible: &[Visible],
        surface: Rect,
        left: f32,
        theme: &Theme,
        out: &mut ViewerOutput,
    ) {
        self.block_delete_button = None;
        if self.ed.session.read_only().is_some() || self.review_preview() || self.block_drag_going() {
            return;
        }
        let Some(hovered) = self.hovered_block(ui, visible, surface) else {
            return;
        };
        let model = self.ed.view.model().clone();
        let Some(v) = visible.iter().find(|v| model.item_start(v.idx) == hovered) else {
            return;
        };
        let Some((_, b)) = v.layout.card_top else { return };
        let containers = self.ed.session.doc().named_containers_at(hovered);
        let Some(c) = containers.last().filter(|c| c.opener.start == hovered) else {
            return;
        };
        // after the buttons the block has
        let entry = self.block_index.as_ref().and_then(|i| i.block(&c.name));
        let mut before = 0.0;
        if let Some(e) = entry {
            before += 1.0;
            if (e.view.is_some() || self.ed.view.can_draw_site()) && !self.ed.view.block_outside(v.idx) {
                before += 1.0;
            }
        }
        let side = theme.line_px().min(22.0);
        let top_right = b + vec2(left, surface.top() + v.y);
        let r = Rect::from_min_size(
            pos2(top_right.x - side - 4.0 - before * (side + 4.0), top_right.y + 4.0),
            vec2(side, side),
        );
        if !surface.contains_rect(r) {
            return;
        }
        let name = c.name.clone();
        let id = ui.scope_id().with(("markise-block-delete", hovered));
        let resp = ui.interact(r, id, Sense::click()).on_hover_text("Delete Block");
        let label = format!("Delete Block: {name}");
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, &label));
        self.block_delete_button = Some(r);
        if resp.hovered() {
            self.pointer_on_block_control = true;
        }
        if resp.clicked() {
            out.delete_block = Some(hovered);
            ui.ctx().request_repaint();
        }
        let tone = if resp.hovered() {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        let painter = ui.painter_at(surface);
        if resp.hovered() {
            painter.rect_filled(r, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
        } else if v.layout.has_picture() {
            painter.rect_filled(r, 4.0, ui.visuals().window_fill().gamma_multiply(0.85));
        }
        // a bin: a lid, and a body narrowing to its foot
        let (c0, s) = (r.center(), side * 0.26);
        let stroke = Stroke::new((side * 0.08).max(1.0), tone);
        painter.line_segment(
            [c0 + vec2(-s * 1.15, -s * 0.75), c0 + vec2(s * 1.15, -s * 0.75)],
            stroke,
        );
        painter.line_segment([c0 + vec2(-s * 0.4, -s * 1.1), c0 + vec2(s * 0.4, -s * 1.1)], stroke);
        let body = [
            c0 + vec2(-s * 0.85, -s * 0.75),
            c0 + vec2(-s * 0.6, s * 1.15),
            c0 + vec2(s * 0.6, s * 1.15),
            c0 + vec2(s * 0.85, -s * 0.75),
        ];
        painter.add(egui::Shape::line(body.to_vec(), stroke));
    }
}

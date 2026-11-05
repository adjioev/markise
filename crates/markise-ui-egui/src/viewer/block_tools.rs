//! The button on a block's card that opens its form (ADR 0020). It shows only for a block whose
//! package is in use and has a schema for it; a document with no package never gets here.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use egui::accesskit::Role;
use egui::{Pos2, Rect, Sense, Stroke, TextureId, Ui, WidgetInfo, pos2, vec2};
use markise_engine::block_view::PackageViews;
use markise_layout::block::Prim;
use markise_layout::{BlockLook, BlockViewMode, BlockViews, ItemKind, Visible};
use markise_package::PackageIndex;
use markise_theme::Theme;

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

/// A package's views with its pictures.
struct ViewsWithIcons {
    views: Arc<PackageViews>,
    icons: Arc<IconBoard>,
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
    fn icon_ready(&self, icon: &str) -> bool {
        self.icons.ready(icon)
    }
    fn look(&self, name: &str, args: &str, source: &str) -> BlockLook {
        self.views.look(name, args, source)
    }
}

impl DocViewer {
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
        resp.widget_info(|| WidgetInfo::labeled(Role::Label, true, format!("Outside its schema: {all}")));
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
            // a card
            BlockViewMode::Native => {
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

    /// The package whose blocks have a form; `None` when there is none in use.
    pub fn set_block_index(&mut self, index: Option<Arc<PackageIndex>>) {
        let same = match (&index, &self.block_index) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.block_icons.clear();
        let fields = index
            .clone()
            .map(|i| Arc::new(PackageViews::new(i, crate::block_form::LANGUAGE)));
        let views = fields.clone().map(|views| -> Arc<dyn BlockViews> {
            Arc::new(ViewsWithIcons {
                views,
                icons: self.block_icons.clone(),
            })
        });
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
        let Some(index) = self.block_index.clone() else {
            return;
        };
        if self.block_icons.take_fresh() {
            ui.ctx().request_repaint();
        }
        self.pointer_on_block_control = false;
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let model = self.ed.view.model().clone();
        let side = theme.line_px().min(22.0);
        for v in visible {
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
            let outside = self.ed.view.block_warnings(v.idx).is_some();
            if pointer.is_some_and(|p| row.contains(p)) {
                self.pointer_on_block_control = true;
            }
            if has_view && !outside {
                let next = self.ed.view.block_mode_of(start).next();
                let m = r.translate(vec2(-(side + 4.0), 0.0));
                self.block_view_button(ui, m, surface, start, next, &c.name, out);
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
            // a pencil: a slanted stroke and its tip
            let s = side * 0.28;
            let c0 = r.center();
            let stroke = Stroke::new((side * 0.09).max(1.0), tone);
            painter.line_segment([c0 + vec2(s, -s), c0 + vec2(-s * 0.6, s * 0.6)], stroke);
            painter.line_segment([c0 + vec2(-s * 0.6, s * 0.6), c0 + vec2(-s, s)], stroke);
            painter.line_segment([c0 + vec2(s * 0.45, -s * 1.1), c0 + vec2(s * 1.1, -s * 0.45)], stroke);
        }
    }
}

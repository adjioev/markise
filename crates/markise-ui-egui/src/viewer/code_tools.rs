//! Fenced code block tools on the rendered surface (spec 02 §7, issue #76): a copy button at
//! the block's top-right and a language field at its bottom-right. Both show while the
//! pointer is over the block or the caret is inside it.
//!
//! - Copy puts the block's content on the clipboard as stored (no fence lines, no info string)
//!   and shows a short "Copied" state. It never moves the caret.
//! - The language field shows the first info word, or a muted "language" placeholder. A click
//!   or Esc with the caret in the block edits it; typing shows a fuzzy language list (names and
//!   aliases of the highlighter's table). ↑/↓ move, Enter/Tab pick the selected row, Esc
//!   reverts, Left at the start / Right at the end leave the field into the code (committing),
//!   a click elsewhere commits the typed text. Free text is allowed. A commit rewrites only
//!   the language word of the fence (format seam: `FormatDoc::code_lang_patch`) as one undo
//!   step. Keys are left to an IME composition while one is in progress.

use std::sync::Arc;

use egui::{Color32, CornerRadius, Event, EventFilter, Key, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use markise_doc::{CodeFenceView, PlanStep, Selection};
use markise_layout::block::{Prim, color};
use markise_layout::text::FragmentKind;
use markise_layout::{ItemKind, TextShaper, Visible};
use markise_theme::Theme;

use super::{DocViewer, ViewerOutput};

/// Runs a closure with the surface's text shaper and images.
pub(super) type WithShaper<'a> = dyn FnMut(&mut dyn FnMut(&mut dyn TextShaper, &dyn markise_layout::Images)) + 'a;

/// How long the copy button shows its "Copied" state (seconds).
const COPIED_FOR: f64 = 1.5;
/// Rows of the language list.
const LANG_ROWS: usize = 50;
/// Undo label of a language change.
const LANG_LABEL: &str = "Change Code Language";

/// A row of the language list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LangItem {
    /// What a pick writes into the fence.
    pub word: String,
    /// What it is (shown dimmed): the syntax name, "diagram", "plain"…
    pub hint: String,
}

/// The language field being edited.
#[derive(Clone, Debug)]
pub struct LangEdit {
    /// Opening fence offset of the block.
    pub block: usize,
    /// The language when editing started (Esc restores it).
    pub original: String,
    /// The field's text.
    pub text: String,
    /// The list for `text` (shown once the field opened).
    pub items: Vec<LangItem>,
    pub selected: usize,
    /// Scroll the selected row into view (the list opened, the keyboard moved it).
    scroll: bool,
    /// The list's scroll offset at the last frame (tests).
    pub list_offset: f32,
    /// Focus the field and select its text on the next frame.
    fresh: bool,
}

/// State of the code block tools of one surface.
#[derive(Default)]
pub(crate) struct CodeTools {
    pub(crate) edit: Option<LangEdit>,
    /// The block whose content was copied, and when.
    copied: Option<(usize, f64)>,
    /// An IME composition is in progress in the field.
    composing: bool,
    /// The surface had the keyboard focus at the end of the last frame (Esc clears the focus
    /// before the frame runs).
    surface_focused: bool,
    /// The surface accepts edits (otherwise the field only shows the language).
    editable: bool,
    /// Field events postponed to the next frame (they followed text input; see `lang_keys`).
    held: Vec<Event>,
    /// Fences read this revision: (item start, revision counter, fence).
    fences: Vec<(usize, u64, Option<Arc<CodeFenceView>>)>,
    /// Screen rectangles of the tools drawn last frame (tests).
    pub(crate) shown: Vec<ShownTools>,
}

/// Where the tools of one block were drawn (screen points).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShownTools {
    /// Opening fence offset of the block.
    pub block: usize,
    pub copy: Rect,
    pub lang: Rect,
}

/// What the field asks for after this frame's keys.
#[derive(Clone, Debug, PartialEq)]
enum Finish {
    /// Write this language and give the focus back to the code.
    Commit(String),
    /// Write it and leave the field into the code (`forward`: after the block's content).
    Exit(String, bool),
    /// A click elsewhere: write the typed text, leave the focus where it went.
    Blur(String),
    Revert,
}

/// One fenced block on screen.
struct OnScreen {
    /// Item start (the leaf's first byte).
    start: usize,
    /// The code box (union of its visible segments).
    rect: Rect,
    /// The first segment's top-right language label, when visible (covered by the button).
    label: Option<Rect>,
}

impl DocViewer {
    /// The language field being edited (tests).
    pub fn lang_edit(&self) -> Option<&LangEdit> {
        self.code.edit.as_ref()
    }

    /// Where the code block tools were drawn last frame (tests).
    pub fn code_tools_shown(&self) -> &[ShownTools] {
        &self.code.shown
    }

    /// Fenced code blocks of the visible items, by item start.
    fn fenced_on_screen(&self, visible: &[Visible], left: f32, top: f32) -> Vec<OnScreen> {
        let model = self.ed.view.model();
        let mut out: Vec<OnScreen> = Vec::new();
        for v in visible {
            let Some(it) = model.items.get(v.idx) else { continue };
            if !matches!(
                it.kind,
                ItemKind::Code { .. } | ItemKind::Diagram { .. } | ItemKind::Math
            ) {
                continue;
            }
            let start = model.item_start(v.idx);
            let origin = vec2(left, top + v.y);
            // the code box: the largest rectangle of the layout (the item when there is none,
            // e.g. a rendered diagram)
            let mut best: Option<Rect> = None;
            let mut label = None;
            let diagram = matches!(it.kind, ItemKind::Diagram { .. });
            for p in v.layout.prims.iter().filter(|_| !diagram) {
                match p {
                    Prim::Rect { rect, .. } if best.is_none_or(|b| rect.area() > b.area()) => best = Some(*rect),
                    Prim::Text { pos, frag, text } if frag.kind == FragmentKind::Label => {
                        label = Some(text.rect().translate(pos.to_vec2() + origin));
                    }
                    _ => {}
                }
            }
            let rect = best
                .unwrap_or_else(|| Rect::from_min_size(Pos2::ZERO, vec2(self.ed.view.width(), v.layout.height)))
                .translate(origin);
            match out.iter_mut().find(|b| b.start == start) {
                Some(b) => {
                    b.rect = b.rect.union(rect);
                    b.label = b.label.or(label);
                }
                None => out.push(OnScreen { start, rect, label }),
            }
        }
        // only fenced blocks (not `$$` math or indented code): a fenced leaf starts at its
        // fence (cheap check; the fence itself is read only for the blocks that show tools)
        let text = self.ed.session.text();
        out.retain(|b| {
            text.get(b.start..)
                .is_some_and(|t| t.starts_with("```") || t.starts_with("~~~"))
        });
        out
    }

    /// Draws the copy buttons and language fields and handles them (after the document was
    /// painted, before the caret). `focused`: the surface has the keyboard focus.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn code_tools_ui(
        &mut self,
        ui: &mut Ui,
        visible: &[Visible],
        surface: Rect,
        left: f32,
        theme: &Theme,
        focused: bool,
        editable: bool,
        out: &mut ViewerOutput,
        sh: &mut WithShaper<'_>,
    ) {
        self.code.editable = editable;
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let was_focused = std::mem::replace(&mut self.code.surface_focused, focused);
        self.code.shown.clear();
        let blocks = self.fenced_on_screen(visible, left, surface.top());
        let caret_block = {
            let at = self.ed.session.selection().primary().focus.offset;
            let model = self.ed.view.model();
            let start = model.item_start(model.item_at_offset(at));
            blocks.iter().position(|b| b.start == start)
        };

        // Esc with the caret in a block opens its language field (spec 02 §5.5: Esc in the
        // code block's language field lists the languages). egui clears the surface's focus
        // on Esc before the frame, so the last frame's focus counts.
        if self.code.edit.is_none()
            && (focused || was_focused)
            && let Some(bi) = caret_block
            && esc_pressed(ui)
        {
            ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
            self.open_lang(blocks[bi].start);
        }

        // the edited block scrolled away (or vanished): keep what was typed
        if let Some(e) = &self.code.edit
            && !blocks.iter().any(|b| b.start == e.block)
        {
            let text = e.text.clone();
            self.finish_lang(&ctx, Finish::Blur(text), out, sh);
        }

        let clip = ui.clip_rect().intersect(surface);
        let painter = ui.painter_at(clip);
        for (bi, b) in blocks.iter().enumerate() {
            let editing = self.code.edit.as_ref().is_some_and(|e| b.start == e.block);
            let show =
                editing || ui.rect_contains_pointer(b.rect.intersect(surface)) || (focused && caret_block == Some(bi));
            if !show {
                continue;
            }
            let Some(fence) = self.fence(b.start) else {
                continue;
            };
            let copy = self.copy_button(ui, &painter, b, &fence, theme, now);
            let lang = self.lang_field(ui, &painter, b, &fence, theme, out, sh);
            self.code.shown.push(ShownTools {
                block: fence.opener.start,
                copy,
                lang,
            });
        }
    }

    /// The fence of the block starting at `start`, read once per document revision (a large
    /// block's lines are not copied every frame).
    fn fence(&mut self, start: usize) -> Option<Arc<CodeFenceView>> {
        let rev = self.ed.session.revision().counter;
        if let Some((_, _, f)) = self.code.fences.iter().find(|(s, r, _)| *s == start && *r == rev) {
            return f.clone();
        }
        let f = self.ed.session.doc().code_fence(start).map(Arc::new);
        self.code.fences.retain(|(_, r, _)| *r == rev);
        if self.code.fences.len() >= 8 {
            self.code.fences.clear();
        }
        self.code.fences.push((start, rev, f.clone()));
        f
    }

    /// Starts editing the language of the block at `start`.
    fn open_lang(&mut self, start: usize) {
        if !self.code.editable {
            return;
        }
        let Some(f) = self.ed.session.doc().code_fence(start) else {
            return;
        };
        let text = self.ed.session.text();
        let original = f.lang(text).slice(text).to_string();
        self.code.edit = Some(LangEdit {
            block: f.opener.start,
            items: lang_matches(&original, LANG_ROWS),
            text: original.clone(),
            original,
            selected: 0,
            scroll: true,
            list_offset: 0.0,
            fresh: true,
        });
        self.code.composing = false;
    }

    /// The copy button at the block's top-right; returns its rectangle.
    fn copy_button(
        &mut self,
        ui: &mut Ui,
        painter: &egui::Painter,
        b: &OnScreen,
        fence: &CodeFenceView,
        theme: &Theme,
        now: f64,
    ) -> Rect {
        let s = (theme.font_size * 1.35).round().max(18.0);
        let r = Rect::from_min_size(pos2(b.rect.right() - s - 4.0, b.rect.top() + 4.0), vec2(s, s));
        let bg = color(theme.code_block.background);
        let fg = color(theme.meta_text);
        // the static language label sits there: covered while the tools show
        if let Some(l) = b.label {
            painter.rect_filled(l.expand(1.0), 0.0, bg);
        }
        let id = ui.scope_id().with(("markise-code-copy", fence.opener.start));
        // click-and-drag: on top of the surface for presses too (the caret stays)
        let resp = ui.interact(r, id, Sense::click_and_drag());
        let copied = self
            .code
            .copied
            .is_some_and(|(blk, t)| blk == fence.opener.start && now - t < COPIED_FOR);
        let hot = resp.hovered();
        painter.rect(
            r,
            CornerRadius::same(4),
            if hot { mix(bg, fg, 0.12) } else { bg },
            Stroke::new(1.0, mix(bg, fg, 0.35)),
            StrokeKind::Inside,
        );
        let stroke = Stroke::new(1.3, if hot { color(theme.text) } else { fg });
        if copied {
            // ✓ and "Copied" to its left
            let a = pos2(r.left() + s * 0.26, r.top() + s * 0.52);
            let m = pos2(r.left() + s * 0.43, r.top() + s * 0.70);
            let c = pos2(r.left() + s * 0.76, r.top() + s * 0.32);
            painter.line_segment([a, m], stroke);
            painter.line_segment([m, c], stroke);
            let font = egui::FontId::proportional(theme.font_size * 0.8);
            let g = painter.layout_no_wrap("Copied".into(), font, fg);
            let gp = pos2(r.left() - g.size().x - 6.0, r.center().y - g.size().y * 0.5);
            painter.rect_filled(Rect::from_min_size(gp, g.size()).expand(2.0), 3.0, bg);
            painter.galley(gp, g, fg);
            if let Some((_, t)) = self.code.copied {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64((t + COPIED_FOR - now).max(0.02)));
            }
        } else {
            // two overlapping sheets
            let w = s * 0.36;
            let h = s * 0.44;
            let back = Rect::from_min_size(pos2(r.left() + s * 0.24, r.top() + s * 0.2), vec2(w, h));
            let front = back.translate(vec2(s * 0.16, s * 0.14));
            painter.rect_stroke(back, 1.5, stroke, StrokeKind::Middle);
            painter.rect(
                front,
                1.5,
                if hot { mix(bg, fg, 0.12) } else { bg },
                stroke,
                StrokeKind::Middle,
            );
        }
        let resp = resp.on_hover_text("Copy code");
        if hot {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            let text = self.ed.session.text();
            ui.ctx().copy_text(fence.content(text));
            self.code.copied = Some((fence.opener.start, now));
            ui.ctx().request_repaint();
        }
        r
    }

    /// The language field at the block's bottom-right; returns its rectangle.
    #[allow(clippy::too_many_arguments)]
    fn lang_field(
        &mut self,
        ui: &mut Ui,
        painter: &egui::Painter,
        b: &OnScreen,
        fence: &CodeFenceView,
        theme: &Theme,
        out: &mut ViewerOutput,
        sh: &mut WithShaper<'_>,
    ) -> Rect {
        let ctx = ui.ctx().clone();
        let text = self.ed.session.text();
        let lang = fence.lang(text).slice(text).to_string();
        let size = (theme.font_size * 0.8).round().max(10.0);
        let font = egui::FontId::proportional(size);
        let bg = color(theme.code_block.background);
        let fg = color(theme.meta_text);
        let txt = color(theme.text);
        let editing = self
            .code
            .edit
            .as_ref()
            .filter(|e| e.block == fence.opener.start)
            .is_some();
        let shown = match &self.code.edit {
            Some(e) if editing => e.text.clone(),
            _ => lang.clone(),
        };
        let measure = if shown.is_empty() { "language" } else { &shown };
        let tw = ctx.fonts_mut(|f| f.layout_no_wrap(measure.to_string(), font.clone(), txt).size().x);
        let w = (tw + 16.0).clamp(72.0, 220.0);
        let h = size + 8.0;
        let r = Rect::from_min_size(pos2(b.rect.right() - w - 4.0, b.rect.bottom() - h - 4.0), vec2(w, h));
        let field_id = ui.scope_id().with(("markise-code-lang", fence.opener.start));
        painter.rect(
            r,
            CornerRadius::same(4),
            if editing { color(theme.page_bg) } else { bg },
            Stroke::new(1.0, mix(bg, fg, if editing { 0.6 } else { 0.3 })),
            StrokeKind::Inside,
        );
        if !editing {
            let resp = ui.interact(r, field_id.with("idle"), Sense::click_and_drag());
            let (s, c) = if lang.is_empty() {
                ("language", fg.gamma_multiply(0.7))
            } else {
                (lang.as_str(), if resp.hovered() { txt } else { fg })
            };
            let g = painter.layout_no_wrap(s.to_string(), font, c);
            painter.galley(pos2(r.left() + 8.0, r.center().y - g.size().y * 0.5), g, c);
            if resp.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::Text);
            }
            let resp = resp.on_hover_text("Code language");
            if resp.clicked() {
                // finish another block's field first
                if let Some(e) = &self.code.edit {
                    let t = e.text.clone();
                    self.finish_lang(&ctx, Finish::Blur(t), out, sh);
                }
                self.open_lang(fence.opener.start);
                ctx.request_repaint();
            }
            return r;
        }

        // ---- editing: this frame's keys first (the text edit must not see them)
        let finish = self.lang_keys(&ctx, field_id);
        let e = self.code.edit.as_mut().expect("editing");
        let fresh = std::mem::take(&mut e.fresh);
        let before = e.text.clone();
        let te = egui::TextEdit::singleline(&mut e.text)
            .id(field_id)
            .font(font.clone())
            .text_color(txt)
            .hint_text(egui::RichText::new("language").color(fg.gamma_multiply(0.7)))
            .frame(egui::Frame::NONE)
            .return_key(None)
            .event_filter(EventFilter {
                tab: true,
                horizontal_arrows: true,
                vertical_arrows: true,
                escape: true,
            })
            .desired_width(w - 16.0);
        let resp = ui.put(r.shrink2(vec2(8.0, 3.0)), te);
        if fresh {
            resp.request_focus();
            if let Some(mut st) = egui::text_edit::TextEditState::load(&ctx, field_id) {
                let n = e.text.chars().count();
                st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(n),
                )));
                st.store(&ctx, field_id);
            }
        }
        if e.text != before {
            e.items = lang_matches(&e.text, LANG_ROWS);
            e.selected = 0;
            e.scroll = true;
        }
        let picked = self.lang_popup(&ctx, r, theme);
        let finish = match (picked, finish) {
            (Some(w), _) => Some(Finish::Commit(w)),
            (None, Some(f)) => Some(f),
            (None, None) if !fresh && resp.lost_focus() => Some(Finish::Blur(
                self.code.edit.as_ref().map(|e| e.text.clone()).unwrap_or_default(),
            )),
            _ => None,
        };
        if let Some(f) = finish {
            if !matches!(f, Finish::Blur(_)) {
                ctx.memory_mut(|m| m.surrender_focus(field_id));
            }
            self.finish_lang(&ctx, f, out, sh);
        }
        r
    }

    /// Takes the field's navigation keys out of this frame's input, in event order, and says
    /// what they ask for. Keys are left to an IME composition. A navigation key that follows
    /// text input of the same frame (typing, an IME commit) waits for the next frame together
    /// with everything after it, so it acts on the list of the text typed before it.
    fn lang_keys(&mut self, ctx: &egui::Context, field_id: egui::Id) -> Option<Finish> {
        let held = std::mem::take(&mut self.code.held);
        let e = self.code.edit.as_ref()?;
        let n = e.items.len();
        let len = e.text.chars().count();
        let cursor = egui::text_edit::TextEditState::load(ctx, field_id)
            .and_then(|s| s.cursor.char_range())
            .filter(|_| ctx.memory(|m| m.has_focus(field_id)));
        let caret = cursor.filter(|c| c.is_empty()).map(|c| c.primary.index.0);
        let mut composing = self.code.composing;
        let mut sel = e.selected;
        let mut moved = false;
        let mut finish = None;
        let mut later = Vec::new();
        ctx.input_mut(|i| {
            let mut events = held;
            events.append(&mut i.events);
            // text input seen this frame (left to the text edit)
            let mut typed = false;
            let mut iter = events.into_iter();
            for ev in iter.by_ref() {
                if finish.is_some() {
                    i.events.push(ev);
                    continue;
                }
                let ours = match &ev {
                    Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } if !composing && modifiers.is_none() => match key {
                        Key::ArrowDown | Key::ArrowUp | Key::Enter | Key::Tab | Key::Escape => Some(*key),
                        Key::ArrowLeft if caret == Some(0) => Some(*key),
                        Key::ArrowRight if caret == Some(len) => Some(*key),
                        _ => None,
                    },
                    _ => None,
                };
                let Some(key) = ours else {
                    match &ev {
                        Event::Ime(ime) => {
                            composing = ime_step(composing, ime);
                            typed = true;
                        }
                        Event::Text(_) | Event::Paste(_) | Event::Cut | Event::Key { pressed: true, .. } => {
                            typed = true
                        }
                        _ => {}
                    }
                    i.events.push(ev);
                    continue;
                };
                if typed {
                    later.push(ev);
                    break;
                }
                match key {
                    Key::ArrowDown if n > 0 => {
                        sel = (sel + 1) % n;
                        moved = true;
                    }
                    Key::ArrowUp if n > 0 => {
                        sel = (sel + n - 1) % n;
                        moved = true;
                    }
                    Key::Enter | Key::Tab => {
                        // an emptied field clears the language (no row is picked)
                        finish = Some(Finish::Commit(match e.items.get(sel.min(n.saturating_sub(1))) {
                            Some(it) if !e.text.trim().is_empty() => it.word.clone(),
                            _ => e.text.clone(),
                        }));
                    }
                    Key::Escape => finish = Some(Finish::Revert),
                    Key::ArrowLeft => finish = Some(Finish::Exit(e.text.clone(), false)),
                    Key::ArrowRight => finish = Some(Finish::Exit(e.text.clone(), true)),
                    _ => {}
                }
            }
            later.extend(iter);
        });
        self.code.composing = composing;
        if !later.is_empty() {
            self.code.held = later;
            ctx.request_repaint();
        }
        if let Some(e) = self.code.edit.as_mut()
            && moved
        {
            e.selected = sel;
            e.scroll = true;
        }
        finish
    }

    /// The language list under (or above) the field; returns a clicked row's word.
    fn lang_popup(&mut self, ctx: &egui::Context, field: Rect, theme: &Theme) -> Option<String> {
        let e = self.code.edit.as_ref()?;
        if e.items.is_empty() {
            return None;
        }
        let look = crate::sidebar::SideLook::new(theme);
        let fill = color(theme.page_bg);
        let text = color(theme.text);
        let dim = color(theme.meta_text);
        let row_h = theme.font_size * 1.6;
        let max_h = (row_h * 8.0).min(260.0);
        let screen = ctx.content_rect();
        let below = screen.bottom() - field.bottom() > max_h + 12.0;
        let (pos, pivot) = if below {
            (pos2(field.right(), field.bottom() + 4.0), egui::Align2::RIGHT_TOP)
        } else {
            (pos2(field.right(), field.top() - 4.0), egui::Align2::RIGHT_BOTTOM)
        };
        let mut clicked = None;
        let selected = e.selected;
        let scroll = e.scroll;
        let mut offset = 0.0;
        egui::Area::new(egui::Id::unique("markise-code-lang-list"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .pivot(pivot)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).fill(fill).show(ui, |ui| {
                    ui.set_min_width(180.0);
                    ui.set_max_width(320.0);
                    let o = egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
                        for (i, it) in e.items.iter().enumerate() {
                            let mut job = egui::text::LayoutJob::default();
                            job.append(
                                &it.word,
                                0.0,
                                egui::TextFormat {
                                    color: text,
                                    ..Default::default()
                                },
                            );
                            if !it.hint.is_empty() {
                                job.append(
                                    &format!("  {}", it.hint),
                                    0.0,
                                    egui::TextFormat {
                                        color: dim,
                                        ..Default::default()
                                    },
                                );
                            }
                            let sel = i == selected;
                            let r = ui.add(egui::Button::selectable(sel, job).fill(if sel {
                                look.active_bg
                            } else {
                                Color32::TRANSPARENT
                            }));
                            // only when the list opens or the keyboard moves the selection: a
                            // mouse scroll of the list is not undone
                            if sel && scroll {
                                r.scroll_to_me(None);
                            }
                            if r.clicked() {
                                clicked = Some(it.word.clone());
                            }
                        }
                    });
                    offset = o.state.clamped_offset().y;
                });
            });
        if let Some(e) = self.code.edit.as_mut() {
            e.scroll = false;
            e.list_offset = offset;
        }
        clicked
    }

    /// Ends editing: writes the language (one undo step) unless reverted, and moves the focus
    /// back to the code where asked.
    fn finish_lang(&mut self, ctx: &egui::Context, f: Finish, out: &mut ViewerOutput, sh: &mut WithShaper<'_>) {
        let Some(e) = self.code.edit.take() else { return };
        self.code.composing = false;
        self.code.held.clear();
        let (lang, exit, refocus) = match f {
            Finish::Commit(l) => (Some(l), None, true),
            Finish::Exit(l, fwd) => (Some(l), Some(fwd), true),
            Finish::Blur(l) => (Some(l), None, false),
            Finish::Revert => (None, None, true),
        };
        if let Some(lang) = lang {
            let still = self.ed.session.doc().code_fence(e.block).is_some_and(|f| {
                let t = self.ed.session.text();
                f.opener.start == e.block && f.lang(t).slice(t) == e.original
            });
            if still && self.set_code_lang(e.block, &lang) {
                out.edited = true;
            }
        }
        if let Some(forward) = exit
            && let Some(f) = self.ed.session.doc().code_fence(e.block)
        {
            // the caret goes to the end of the code; Right then leaves the block like the
            // code's own Right at its end (spec 02 §5.4)
            let end = f.lines.last().map_or(f.opener.end, |l| l.end);
            self.ed.set_selection(Selection::caret(end));
            if forward {
                let ev = Event::Key {
                    key: Key::ArrowRight,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                };
                sh(&mut |shaper, images| {
                    self.handle_event(ev.clone(), shaper, images, true, 600.0);
                });
            }
            self.ed.refresh_focus();
            out.edited = true;
        }
        if refocus {
            self.request_refocus();
        }
        ctx.request_repaint();
    }

    /// Makes `lang` the language of the fenced block at `at` as one undoable step; `false`
    /// when nothing changed.
    pub fn set_code_lang(&mut self, at: usize, lang: &str) -> bool {
        let Some(p) = self.ed.session.doc().code_lang_patch(at, lang) else {
            return false;
        };
        let r = self.ed.edit(|s| {
            s.transact(LANG_LABEL, markise_engine::editor::TxKind::Structural, |tx| {
                tx.step(PlanStep::patches(p))
            })
        });
        self.ed.refresh_focus();
        r.is_ok()
    }
}

/// Esc was pressed this frame (no modifiers).
fn esc_pressed(ui: &Ui) -> bool {
    ui.input(|i| {
        i.events.iter().any(|e| {
            matches!(
                e,
                Event::Key {
                    key: Key::Escape,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.is_none()
            )
        })
    })
}

/// The IME composition state after `ime`: a non-empty preedit is composing, a commit or an
/// empty preedit ends it.
fn ime_step(composing: bool, ime: &egui::ImeEvent) -> bool {
    match ime {
        egui::ImeEvent::Preedit { text, .. } => !text.is_empty(),
        egui::ImeEvent::Commit(_) => false,
        _ => composing,
    }
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// The hint shown next to a language word.
fn lang_hint(word: &str, syntax: Option<&str>) -> String {
    match (word, syntax) {
        (_, Some(s)) => s.to_string(),
        ("mermaid" | "flow" | "sequence", None) => "diagram".into(),
        ("math", None) => "math".into(),
        _ => "plain".into(),
    }
}

/// The language list for `query` (spec 02 §7 autocomplete): the highlighter's language words,
/// best first — exact, prefix, the main name of a language one of whose aliases the query is
/// (`js` → `javascript`), contained, a subsequence, the syntax name. When the query is no known
/// word it comes last as free text. All words for an empty query.
pub fn lang_matches(query: &str, max: usize) -> Vec<LangItem> {
    let langs: Vec<(&str, Option<&str>)> = markise_blocks::highlight::known_languages().collect();
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return langs
            .iter()
            .take(max)
            .map(|&(w, s)| LangItem {
                word: w.to_string(),
                hint: lang_hint(w, s),
            })
            .collect();
    }
    // the main word of each syntax: its first word in table order
    let main = |s: Option<&str>| s.and_then(|s| langs.iter().find(|(_, t)| *t == Some(s)).map(|(w, _)| *w));
    let mut scored: Vec<(u8, usize, usize)> = Vec::new();
    for (i, &(w, s)) in langs.iter().enumerate() {
        let alias_hit = s.is_some()
            && main(s) == Some(w)
            && langs
                .iter()
                .any(|&(a, t)| t == s && a != w && (a == q || (q.len() >= 2 && a.starts_with(&q))));
        let score = if w == q {
            0
        } else if w.starts_with(&q) {
            1
        } else if alias_hit {
            2
        } else if w.contains(&q) {
            3
        } else if subsequence(&q, w) {
            4
        } else if s.is_some_and(|s| s.to_lowercase().contains(&q)) && main(s) == Some(w) {
            5
        } else {
            continue;
        };
        scored.push((score, w.len(), i));
    }
    scored.sort();
    let exact = scored.first().is_some_and(|s| s.0 == 0);
    let mut out: Vec<LangItem> = scored
        .into_iter()
        .take(max)
        .map(|(_, _, i)| LangItem {
            word: langs[i].0.to_string(),
            hint: lang_hint(langs[i].0, langs[i].1),
        })
        .collect();
    if !exact {
        if out.len() >= max {
            out.pop();
        }
        out.push(LangItem {
            word: query.trim().to_string(),
            hint: "custom".into(),
        });
    }
    out
}

/// `needle`'s chars appear in `hay` in order.
fn subsequence(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|c| it.any(|h| h == c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(q: &str) -> Vec<String> {
        lang_matches(q, LANG_ROWS).into_iter().map(|i| i.word).collect()
    }

    #[test]
    fn aliases_find_the_language() {
        let js = words("js");
        assert_eq!(js[0], "js");
        assert!(js.contains(&"javascript".to_string()), "{js:?}");
        let rs = words("rs");
        assert_eq!(rs[0], "rs");
        assert!(rs.contains(&"rust".to_string()), "{rs:?}");
        let sh = words("sh");
        assert_eq!(sh[0], "sh");
        assert!(
            sh.contains(&"shell".to_string()) && sh.contains(&"bash".to_string()),
            "{sh:?}"
        );
    }

    #[test]
    fn prefixes_rank_before_fuzzy_matches_and_free_text_comes_last() {
        let p = words("pyt");
        assert_eq!(p[0], "python");
        assert_eq!(p.last().unwrap(), "pyt", "free text: {p:?}");
        assert!(words("mrmd").contains(&"mermaid".to_string()), "subsequence");
        assert_eq!(words("zzzq"), vec!["zzzq".to_string()]);
        assert_eq!(lang_matches("Mermaid", 50)[0].hint, "diagram");
        assert_eq!(words("rust").iter().filter(|w| *w == "rust").count(), 1, "no duplicate");
    }

    #[test]
    fn empty_query_lists_everything() {
        assert_eq!(
            lang_matches("", 1000).len(),
            markise_blocks::highlight::known_languages().count()
        );
        assert_eq!(lang_matches("", 5).len(), 5);
    }
}

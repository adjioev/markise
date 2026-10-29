//! One style for the app's egui dialogs (issue #35): About, Licences, Font & size, Theme
//! report, Preferences and the in-app yes/no question ([`crate::confirm::InApp`]).
//!
//! Every dialog has the same frame (padding, corner radius, shadow), a title line, a width
//! from [`Width`] and a footer of right-aligned buttons: the primary action accented with the
//! theme's accent colour, destructive actions red, everything else plain. Enter chooses the
//! default button and Esc closes or cancels — for the topmost dialog only. Colours follow
//! the active theme (light or dark), published once per theme change with [`Look::store`].

use egui::{Color32, Context, CornerRadius, Frame, Id, Key, Margin, Order, RichText, Stroke, Ui};

use crate::confirm::{Choice, Question, Role};

/// Dialog widths (points).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Width {
    /// Questions, About.
    Narrow,
    /// Settings and reports.
    Medium,
    /// Long documents (licences).
    Wide,
}

impl Width {
    pub fn points(self) -> f32 {
        match self {
            Width::Narrow => 380.0,
            Width::Medium => 460.0,
            Width::Wide => 640.0,
        }
    }
}

/// Inner padding of every dialog.
pub const PADDING: i8 = 18;

/// Colours of the dialogs, from the active theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub dark: bool,
    pub accent: Color32,
    pub on_accent: Color32,
    pub danger: Color32,
    pub on_danger: Color32,
    pub fill: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub border: Color32,
}

fn data_id() -> Id {
    Id::unique("markise-dialog-look")
}

/// Black or white, whichever reads better on `bg`.
fn readable_on(bg: Color32) -> Color32 {
    let l = 0.2126 * bg.r() as f32 + 0.7152 * bg.g() as f32 + 0.0722 * bg.b() as f32;
    if l > 160.0 {
        Color32::from_gray(20)
    } else {
        Color32::WHITE
    }
}

impl Look {
    pub fn from_theme(t: &markise_theme::Theme) -> Look {
        let c = markise_layout::block::color;
        let accent = c(t.chrome.accent);
        let (danger, fill, text) = if t.dark {
            (
                Color32::from_rgb(0xd9, 0x4b, 0x44),
                c(t.chrome.sidebar_bg).lerp_to_gamma(Color32::WHITE, 0.06),
                c(t.text),
            )
        } else {
            (
                Color32::from_rgb(0xc6, 0x28, 0x28),
                c(t.page_bg).lerp_to_gamma(c(t.chrome.sidebar_bg), 0.35),
                c(t.text),
            )
        };
        Look {
            dark: t.dark,
            accent,
            on_accent: readable_on(accent),
            danger,
            on_danger: Color32::WHITE,
            fill,
            text,
            weak: text.lerp_to_gamma(fill, 0.4),
            border: text.lerp_to_gamma(fill, 0.82),
        }
    }

    /// Publishes the look for this context (the app calls it when the theme changes).
    pub fn store(self, ctx: &Context) {
        ctx.data_mut(|d| d.insert_temp(data_id(), self));
    }

    /// The published look, else one from egui's visuals.
    pub fn get(ctx: &Context) -> Look {
        if let Some(l) = ctx.data(|d| d.get_temp::<Look>(data_id())) {
            return l;
        }
        let v = ctx.global_style().visuals.clone();
        let accent = v.selection.bg_fill;
        Look {
            dark: v.dark_mode,
            accent,
            on_accent: readable_on(accent),
            danger: if v.dark_mode {
                Color32::from_rgb(0xd9, 0x4b, 0x44)
            } else {
                Color32::from_rgb(0xc6, 0x28, 0x28)
            },
            on_danger: Color32::WHITE,
            fill: v.window_fill,
            text: v.text_color(),
            weak: v.weak_text_color(),
            border: v.window_stroke.color,
        }
    }

    /// The frame of every dialog.
    pub fn frame(&self) -> Frame {
        Frame::new()
            .fill(self.fill)
            .stroke(Stroke::new(1.0, self.border))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(PADDING))
            .shadow(egui::epaint::Shadow {
                offset: [0, 6],
                blur: 24,
                spread: 0,
                color: Color32::from_black_alpha(if self.dark { 110 } else { 50 }),
            })
    }
}

/// The title line.
pub fn title(ui: &mut Ui, look: &Look, text: &str) {
    ui.label(RichText::new(text).size(15.0).strong().color(look.text));
    ui.add_space(10.0);
}

fn styled(ui: &mut Ui, text: &str, fill: Option<Color32>, fg: Color32, focus: bool) -> egui::Response {
    let mut b = egui::Button::new(RichText::new(text).color(fg))
        .min_size(egui::vec2(84.0, 28.0))
        .corner_radius(CornerRadius::same(6));
    if let Some(f) = fill {
        b = b.fill(f).stroke(Stroke::NONE);
    }
    let r = ui.add(b);
    if focus {
        // the default button (Return) wears a ring
        ui.painter().rect_stroke(
            r.rect.expand(2.0),
            CornerRadius::same(8),
            Stroke::new(1.5, fill.unwrap_or(ui.visuals().selection.bg_fill).gamma_multiply(0.6)),
            egui::StrokeKind::Outside,
        );
    }
    r
}

/// The primary action, accented.
pub fn primary(ui: &mut Ui, look: &Look, text: &str) -> egui::Response {
    styled(ui, text, Some(look.accent), look.on_accent, false)
}

/// A destructive action, red.
pub fn destructive(ui: &mut Ui, look: &Look, text: &str) -> egui::Response {
    styled(ui, text, Some(look.danger), look.on_danger, false)
}

/// A plain button (Cancel, Close, secondary actions).
pub fn secondary(ui: &mut Ui, look: &Look, text: &str) -> egui::Response {
    styled(ui, text, None, look.text, false)
}

/// The button row at the bottom: right-aligned, `add` adds buttons from the right
/// (the primary action first).
pub fn footer<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.add_space(16.0);
    // a row of fixed height: a right-to-left layout would otherwise take all the height left
    let size = egui::vec2(ui.available_width(), 30.0);
    ui.allocate_ui_with_layout(size, egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        add(ui)
    })
    .inner
}

/// Enter and Esc for the dialog on `layer`: only when it is the topmost window and no modal
/// is open; Enter only while no widget outside the dialog has the keyboard focus. The keys
/// are consumed.
pub fn keys(ctx: &Context, layer: egui::LayerId) -> (bool, bool) {
    let top = ctx.memory(|m| m.areas().top_layer_id(Order::Middle)) == Some(layer);
    let modal = ctx.memory(|m| m.top_modal_layer()).is_some();
    if !top || modal {
        return (false, false);
    }
    let focus_elsewhere = ctx
        .memory(|m| m.focused())
        .and_then(|id| ctx.read_response(id))
        .is_some_and(|r| r.layer_id != layer);
    ctx.input_mut(|i| {
        let enter = !focus_elsewhere && i.consume_key(egui::Modifiers::NONE, Key::Enter);
        let esc = i.consume_key(egui::Modifiers::NONE, Key::Escape);
        (enter, esc)
    })
}

/// The ✕ in a tool window's top right corner (painted: not every UI font has the glyph).
fn close_button(ui: &mut Ui, look: &Look, at: egui::Pos2) -> egui::Response {
    let rect = egui::Rect::from_min_size(at, egui::vec2(16.0, 16.0));
    let r = ui
        .interact(rect, ui.scope_id().with("markise-dialog-close"), egui::Sense::click())
        .on_hover_text("Close");
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Close"));
    let c = if r.hovered() { look.text } else { look.weak };
    let k = rect.shrink(4.0);
    let s = Stroke::new(1.5, c);
    ui.painter().line_segment([k.left_top(), k.right_bottom()], s);
    ui.painter().line_segment([k.right_top(), k.left_bottom()], s);
    r
}

/// A tool window (Font & size, Theme report) in the shared style, centred when it opens.
/// Returns false when it should close (✕, its close button, Esc, Enter, or `body` returning
/// true).
pub fn window(
    ctx: &Context,
    id: &str,
    title_text: &str,
    width: Width,
    body: impl FnOnce(&mut Ui, &Look) -> bool,
) -> bool {
    window_with(ctx, id, title_text, width.points(), true, body)
}

/// [`window`] `width_pt` points wide; `enter_closes`: Return closes it too (not for windows
/// whose text fields take Return, e.g. the Preferences search).
pub fn window_with(
    ctx: &Context,
    id: &str,
    title_text: &str,
    width_pt: f32,
    enter_closes: bool,
    body: impl FnOnce(&mut Ui, &Look) -> bool,
) -> bool {
    let look = Look::get(ctx);
    let mut keep = true;
    let out = egui::Window::new(title_text)
        .id(Id::unique(id))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .frame(look.frame())
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.set_width(width_pt);
            let top = ui.cursor().min;
            title(ui, &look, title_text);
            if close_button(ui, &look, egui::pos2(top.x + width_pt - 16.0, top.y + 2.0)).clicked() {
                keep = false;
            }
            if body(ui, &look) {
                keep = false;
            }
        });
    if let Some(r) = out {
        let (enter, esc) = keys(ctx, r.response.layer_id);
        if (enter && enter_closes) || esc {
            keep = false;
        }
    }
    keep
}

/// A modal dialog in the shared style. `body` returns `Some` to close it with that value;
/// Esc (topmost modal only) and a click outside close it with `None` in `.1`.
pub fn modal<R>(
    ctx: &Context,
    id: &str,
    width: Width,
    body: impl FnOnce(&mut Ui, &Look) -> Option<R>,
) -> (Option<R>, bool) {
    let look = Look::get(ctx);
    let r = egui::Modal::new(Id::unique(id)).frame(look.frame()).show(ctx, |ui| {
        ui.set_width(width.points());
        body(ui, &look)
    });
    let dismissed = r.inner.is_none() && r.should_close();
    (r.inner, dismissed)
}

/// The in-app yes/no question ([`crate::confirm::InApp`]): message, detail, buttons in the
/// shared style; Return chooses the question's default button, Esc chooses Cancel. A click
/// outside does nothing (questions need an answer).
pub fn confirm_modal(ctx: &Context, q: &Question) -> Option<Choice> {
    let look = Look::get(ctx);
    let r = egui::Modal::new(Id::unique("markise-question"))
        .frame(look.frame())
        .show(ctx, |ui| {
            ui.set_width(Width::Narrow.points());
            ui.horizontal_top(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
                let warn = q.buttons.iter().any(|b| b.role == Role::Destructive);
                let c = if warn { look.danger } else { look.accent };
                ui.painter().circle_filled(rect.center(), 13.0, c.gamma_multiply(0.18));
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    if warn { "!" } else { "?" },
                    egui::FontId::proportional(16.0),
                    c,
                );
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&q.message).size(14.0).strong().color(look.text));
                    if !q.detail.is_empty() {
                        ui.add_space(4.0);
                        ui.label(RichText::new(&q.detail).color(look.weak));
                    }
                });
            });
            let mut chosen = None;
            footer(ui, |ui| {
                // right to left: the actions (default rightmost), then Cancel
                let mut order: Vec<_> = q.buttons.iter().collect();
                order.sort_by_key(|b| match b.role {
                    Role::Primary => 0,
                    Role::Destructive => 1,
                    Role::Cancel => 2,
                });
                for b in order {
                    let fill = match b.role {
                        Role::Primary => Some(look.accent),
                        Role::Destructive => Some(look.danger),
                        Role::Cancel => None,
                    };
                    let fg = match b.role {
                        Role::Primary => look.on_accent,
                        Role::Destructive => look.on_danger,
                        Role::Cancel => look.text,
                    };
                    if styled(ui, &b.label, fill, fg, b.choice == q.default).clicked() {
                        chosen = Some(b.choice);
                    }
                }
            });
            chosen
        });
    if r.inner.is_some() {
        return r.inner;
    }
    if !r.is_top_modal {
        return None;
    }
    ctx.input_mut(|i| {
        if i.consume_key(egui::Modifiers::NONE, Key::Escape) {
            Some(Choice::Cancel)
        } else if i.consume_key(egui::Modifiers::NONE, Key::Enter) {
            Some(q.default)
        } else {
            None
        }
    })
}

/// A dimmed backdrop that blocks the window while a native alert runs on another thread
/// (Linux) or a test has not answered yet.
pub fn waiting(ctx: &Context, q: &Question) {
    let look = Look::get(ctx);
    egui::Modal::new(Id::unique("markise-question-waiting"))
        .frame(look.frame())
        .show(ctx, |ui| {
            ui.set_max_width(Width::Narrow.points());
            ui.label(RichText::new(&q.message).color(look.weak));
        });
}

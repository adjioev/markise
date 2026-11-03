//! Update check UI (issue #70): the first-launch question, the "new version" notice and the
//! result of Check for Updates…, drawn as one small, non-modal card in the window's bottom
//! right corner (above the status bar). The decisions (channel, throttling, parsing) live in
//! [`markise_engine::update`]; the app glue is `app/updates.rs`.
//!
//! **First launch.** The question ("Check for updates automatically?") is a corner card, not
//! a dialog: it never blocks typing and does not cover the welcome tutorial (which opens as a
//! document in the editor), so both can appear at the first launch. Until it is answered,
//! automatic checks are off; a launch that closes without an answer asks again next time.

use std::sync::Arc;
use std::sync::mpsc::Receiver;

use egui::{Context, RichText};
use markise_engine::update::{Checked, Release, ReleaseClient};

/// Where update checks go.
#[derive(Clone)]
pub enum UpdateSource {
    /// No checks in this session; the text says why (Check for Updates… shows it, Preferences
    /// shows it under the switch). Nothing ever contacts the network.
    Off(String),
    /// Checks go through this client (the desktop app's TLS client; tests: a mock).
    Client(Arc<dyn ReleaseClient>),
}

impl Default for UpdateSource {
    fn default() -> Self {
        UpdateSource::Off("Update checks are not available in this build.".into())
    }
}

/// The result line of a manual check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Checking,
    UpToDate(String),
    Error(String),
    /// Checks are off in this session ([`UpdateSource::Off`]).
    Unavailable(String),
}

impl Message {
    pub fn text(&self) -> String {
        match self {
            Message::Checking => "Checking for updates…".into(),
            Message::UpToDate(v) => format!("Markise {v} is the latest version."),
            Message::Error(e) => format!("Could not check for updates. {e}"),
            Message::Unavailable(why) => why.clone(),
        }
    }
}

/// What the user did in the card this frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardAction {
    /// First-launch question: Check Automatically (`true`) / Not Now (`false`).
    Answer(bool),
    /// "What's new" or "Download": open the release page.
    OpenPage(String),
    /// Close the notice of this version (remembered).
    Dismiss(String),
    /// Close the manual check's message.
    CloseMessage,
}

/// Update check state of the session.
pub struct UpdateState {
    pub source: UpdateSource,
    /// The launch's automatic check was decided (once the settings file was read).
    pub launch_decided: bool,
    /// A running check and whether the user asked for it (its result is then always shown).
    pub running: Option<(Receiver<Checked>, bool)>,
    /// The newer release being announced.
    pub notice: Option<Release>,
    /// The manual check's result line.
    pub message: Option<Message>,
    /// The first-launch question is shown.
    pub asking: bool,
    /// Number of checks started this session (tests, diagnostics).
    pub started: u32,
}

impl UpdateState {
    pub fn new(source: UpdateSource) -> UpdateState {
        UpdateState {
            source,
            launch_decided: false,
            running: None,
            notice: None,
            message: None,
            asking: false,
            started: 0,
        }
    }

    fn visible(&self) -> bool {
        self.asking || self.notice.is_some() || self.message.is_some()
    }

    /// Draws the card (nothing when there is nothing to say).
    pub fn show(&self, ctx: &Context, out: &mut Vec<CardAction>) {
        if !self.visible() {
            return;
        }
        egui::Area::new(egui::Id::unique("markise-update-card"))
            .order(egui::Order::Middle)
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -36.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(360.0);
                    if self.asking {
                        ui.label(RichText::new("Check for updates automatically?").strong());
                        ui.add(
                            egui::Label::new(
                                RichText::new(
                                    "Once a day, Markise can ask GitHub whether a new version is \
                                     out. Only the app's version is sent. You can change this in \
                                     Preferences → General.",
                                )
                                .size(12.0)
                                .weak(),
                            )
                            .wrap(),
                        );
                        ui.horizontal(|ui| {
                            if ui.button("Check Automatically").clicked() {
                                out.push(CardAction::Answer(true));
                            }
                            if ui.button("Not Now").clicked() {
                                out.push(CardAction::Answer(false));
                            }
                        });
                    }
                    if let Some(r) = &self.notice {
                        if self.asking {
                            ui.separator();
                        }
                        let v = r.version.to_string();
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!("Markise {v} is available —"));
                            if ui.link("What's new").clicked() {
                                out.push(CardAction::OpenPage(r.page.clone()));
                            }
                            ui.label("·");
                            if ui.link("Download").clicked() {
                                out.push(CardAction::OpenPage(r.page.clone()));
                            }
                            if close_button(ui, "Dismiss").clicked() {
                                out.push(CardAction::Dismiss(v.clone()));
                            }
                        });
                    }
                    if let Some(m) = &self.message {
                        if self.asking || self.notice.is_some() {
                            ui.separator();
                        }
                        ui.horizontal_wrapped(|ui| {
                            if *m == Message::Checking {
                                ui.spinner();
                            }
                            ui.label(m.text());
                            if *m != Message::Checking && close_button(ui, "Close").clicked() {
                                out.push(CardAction::CloseMessage);
                            }
                        });
                    }
                });
            });
    }
}

/// A small "×" button drawn with strokes (no glyph: system fonts may lack ✕); `name` is its
/// accessible name and tooltip.
fn close_button(ui: &mut egui::Ui, name: &str) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
    let name_owned = name.to_string();
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name_owned.clone()));
    let v = ui.style().interact(&r);
    if r.hovered() {
        ui.painter().rect_filled(rect, 3.0, v.weak_bg_fill);
    }
    let c = rect.center();
    let d = 4.0;
    let stroke = egui::Stroke::new(1.4, v.fg_stroke.color);
    ui.painter()
        .line_segment([c + egui::vec2(-d, -d), c + egui::vec2(d, d)], stroke);
    ui.painter()
        .line_segment([c + egui::vec2(-d, d), c + egui::vec2(d, -d)], stroke);
    r.on_hover_text(name)
}

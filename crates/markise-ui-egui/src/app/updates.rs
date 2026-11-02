//! Update check glue (issue #70): the launch's automatic check, Check for Updates…, the
//! first-launch question and the notice ([`crate::updates`]); persisted state lives in the
//! settings file (`updates.*`, `markise_engine::settings`).

use egui::Context;
use markise_engine::update::{self, Outcome};

use super::MarkiseApp;
use crate::updates::{CardAction, Message, UpdateSource};

impl MarkiseApp {
    /// The running version (the workspace version; it always parses — see the engine test).
    fn running_version() -> Option<update::Version> {
        update::parse_version(env!("CARGO_PKG_VERSION"))
    }

    /// Once the settings file has been read: ask the first-launch question, or start the
    /// launch's automatic check when it is on and due (at most once a day).
    pub(super) fn launch_update_check(&mut self) {
        if self.updates.launch_decided || !self.themes.settings_loaded() {
            return;
        }
        self.updates.launch_decided = true;
        if !matches!(self.updates.source, UpdateSource::Client(_)) {
            return; // no network in this session: nothing to ask, nothing to check
        }
        let auto = self.themes.update_auto();
        if auto.is_none() {
            self.updates.asking = true;
            return;
        }
        if update::due(auto, self.themes.update_last_check(), update::now_secs()) {
            self.start_update_check(false);
        }
    }

    /// App menu → Check for Updates… (works with automatic checks off).
    pub fn check_for_updates(&mut self) {
        if let UpdateSource::Off(why) = &self.updates.source {
            self.updates.message = Some(Message::Unavailable(why.clone()));
            return;
        }
        self.updates.message = Some(Message::Checking);
        match &mut self.updates.running {
            // a launch check is under way: its result answers this request
            Some((_, manual)) => *manual = true,
            None => self.start_update_check(true),
        }
    }

    fn start_update_check(&mut self, manual: bool) {
        let (UpdateSource::Client(client), Some(current)) = (&self.updates.source, Self::running_version()) else {
            return;
        };
        let rx = update::spawn_check(client.clone(), current, self.wake_handle());
        self.updates.running = Some((rx, manual));
        self.updates.started += 1;
    }

    /// Picks up a finished check.
    pub(super) fn poll_update_check(&mut self) {
        let Some((rx, manual)) = &self.updates.running else {
            return;
        };
        let manual = *manual;
        let checked = match rx.try_recv() {
            Ok(c) => c,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => update::Checked {
                outcome: Outcome::Failed(update::CheckError::Offline("the check stopped unexpectedly".into())),
                contacted: false,
            },
        };
        self.updates.running = None;
        if checked.contacted {
            self.themes.set_update_last_check(update::now_secs());
        }
        match checked.outcome {
            Outcome::Available(r) => {
                let dismissed = self.themes.update_dismissed() == Some(r.version.to_string().as_str());
                if manual {
                    self.updates.message = None;
                }
                if manual || !dismissed {
                    self.updates.notice = Some(r);
                }
            }
            Outcome::UpToDate if manual => {
                let v =
                    Self::running_version().map_or_else(|| env!("CARGO_PKG_VERSION").to_string(), |v| v.to_string());
                self.updates.message = Some(Message::UpToDate(v));
            }
            Outcome::Failed(e) if manual => self.updates.message = Some(Message::Error(e.to_string())),
            // automatic checks stay silent unless there is something new
            _ => {}
        }
    }

    /// Draws the update card and applies what the user did in it.
    pub(super) fn update_card(&mut self, ctx: &Context) {
        if self.is_asking() {
            return; // a question is open: nothing competes with it
        }
        let mut actions = Vec::new();
        self.updates.show(ctx, &mut actions);
        for a in actions {
            self.update_card_action(ctx, a);
        }
    }

    /// Applies a click in the update card (tests drive it directly too).
    pub fn update_card_action(&mut self, ctx: &Context, a: CardAction) {
        match a {
            CardAction::Answer(on) => {
                self.updates.asking = false;
                self.set_update_auto(on);
            }
            CardAction::OpenPage(url) => {
                if update::is_release_page(&url) {
                    ctx.open_url(egui::OpenUrl::new_tab(url));
                }
            }
            CardAction::Dismiss(v) => {
                self.updates.notice = None;
                self.themes.set_update_dismissed(&v);
            }
            CardAction::CloseMessage => self.updates.message = None,
        }
    }

    /// Automatic update checks on or off (Preferences → General, first-launch question).
    /// Turning them on checks right away when a check is due.
    pub fn set_update_auto(&mut self, on: bool) {
        self.updates.asking = false;
        self.themes.set_update_auto(on);
        if on
            && self.updates.running.is_none()
            && matches!(self.updates.source, UpdateSource::Client(_))
            && update::due(Some(true), self.themes.update_last_check(), update::now_secs())
        {
            self.start_update_check(false);
        }
    }

    /// Automatic update checks: `None` until the first-launch question was answered.
    pub fn update_auto(&self) -> Option<bool> {
        self.themes.update_auto()
    }

    /// The update card's state (tests).
    pub fn update_state(&self) -> &crate::updates::UpdateState {
        &self.updates
    }

    /// A check is running (tests).
    pub fn is_checking_updates(&self) -> bool {
        self.updates.running.is_some()
    }

    /// The settings' dismissed version and last check time (tests).
    pub fn update_settings(&self) -> (Option<String>, Option<u64>) {
        (
            self.themes.update_dismissed().map(str::to_string),
            self.themes.update_last_check(),
        )
    }
}

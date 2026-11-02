//! Help menu (issue #63): the welcome tutorial ([`crate::tutorial`]) — at the first launch
//! and from Help → Tutorial — and the keyboard shortcut cheat sheet ([`crate::shortcuts`]).

use egui::Context;

use super::{Ask, MarkiseApp, Then};

impl MarkiseApp {
    /// Help → Tutorial: the tutorial as an untitled scratch copy (asks to save unsaved edits
    /// of the current document first). Nothing on disk is read or written.
    pub fn open_tutorial(&mut self) {
        if self.is_dirty() {
            self.set_ask(Ask::Unsaved(Then::Tutorial));
            return;
        }
        self.open_tutorial_now();
    }

    pub(super) fn open_tutorial_now(&mut self) {
        self.install_untitled(crate::tutorial::TEXT.to_string(), crate::tutorial::FILE_NAME);
    }

    /// First launch ([`crate::AppOptions::welcome`]): once the settings file has been read,
    /// opens the tutorial if it was never shown and the launch opened nothing else, and records
    /// that it was shown. A launch that opened a file or folder leaves the flag unset, so the
    /// tutorial comes at the next plain launch.
    pub(super) fn first_launch_tutorial(&mut self) {
        if !self.welcome_pending || !self.themes.settings_loaded() {
            return;
        }
        self.welcome_pending = false;
        if self.themes.tutorial_shown() || self.doc.is_some() || self.load.is_some() || self.tree.is_some() {
            return;
        }
        self.open_tutorial_now();
        self.themes.set_tutorial_shown();
    }

    /// Help → Keyboard Shortcuts (⌘? / Ctrl+?): opens the cheat sheet.
    pub fn open_shortcuts(&mut self, ctx: &Context) {
        self.cheat_sheet.open(ctx);
    }

    /// The settings file has been read (tests: first-launch decisions are made then).
    #[doc(hidden)]
    pub fn settings_loaded(&self) -> bool {
        self.themes.settings_loaded()
    }

    /// The cheat sheet is open (tests).
    pub fn is_shortcuts_open(&self) -> bool {
        self.cheat_sheet.open
    }

    /// The find bar and Quick Open are open (tests).
    #[doc(hidden)]
    pub fn find_and_quick_open(&self) -> (bool, bool) {
        (self.find.open, self.quick.open)
    }

    /// Sets the cheat sheet's search text (tests).
    pub fn set_shortcuts_query(&mut self, q: &str) {
        self.cheat_sheet.query = q.to_string();
    }

    pub(super) fn shortcuts_window(&mut self, ctx: &Context) {
        if !self.cheat_sheet.open {
            return;
        }
        let groups = self.shortcut_groups();
        self.cheat_sheet.show(ctx, &groups);
    }
}

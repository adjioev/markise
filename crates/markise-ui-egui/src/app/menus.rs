//! The application's menus (spec 04 §2): [`MarkiseApp::menu_model`] builds the toolkit-neutral
//! [`MenuBar`] from the command registry, [`APP_COMMANDS`](crate::menu::APP_COMMANDS) and the
//! window state; [`MarkiseApp::menu_action`] performs a chosen item exactly like its shortcut.
//! A native front end ([`MenuHost`]) shows the model; without one the in-window egui fallback
//! does.

use std::sync::OnceLock;

use egui::{Context, Key, Modifiers};
use markise_engine::editor::command::Registry;
use markise_engine::editor::keymap::{Keymap, Platform};
use markise_engine::export::ExportKind;
use markise_engine::settings::Appearance;
use markise_export::pandoc::PandocFormat;
use markise_md::MarkdownFormat;
use markise_theme::catalog::{Origin, Selection, ThemeFamily, ThemeId};

use super::{MarkiseApp, ViewMode};
use crate::export_ui::ExportAction;
use crate::menu::{
    Check, EDIT_ORDER, Entry, FORMAT_ORDER, Item, Menu, MenuAction, MenuBar, MenuKind, PARAGRAPH_ORDER, Role, Trigger,
    app_command, descriptor_chord, menu_ids,
};

/// A native menu bar front end (implemented by the desktop app; ADR 0006 §3.3 "native menus").
pub trait MenuHost {
    /// Items chosen since the last call and how (the host wakes the UI when one arrives).
    fn take_actions(&mut self) -> Vec<(MenuAction, Trigger)>;
    /// Shows `bar`: applies enabled/checked/label changes in place and rebuilds only when
    /// [`MenuBar::structure`] changed.
    fn sync(&mut self, bar: &MenuBar);
}

/// The Markdown registry and its default keymap for `platform` (labels and chords of the
/// menus also while no document is open).
fn defaults(platform: Platform) -> &'static (Registry, Keymap) {
    static MAC: OnceLock<(Registry, Keymap)> = OnceLock::new();
    static OTHER: OnceLock<(Registry, Keymap)> = OnceLock::new();
    let cell = match platform {
        Platform::Mac => &MAC,
        Platform::Other => &OTHER,
    };
    cell.get_or_init(|| {
        let r = Registry::for_format(&MarkdownFormat::default());
        let k = Keymap::new(&r, platform);
        (r, k)
    })
}

/// Commands that a focused text field (the source editor) performs itself: with one focused,
/// the menu item sends the shortcut to it instead of running the registry command.
const TEXT_FIELD_KEYS: [(&str, Modifiers, Key); 3] = [
    ("edit.undo", Modifiers::COMMAND, Key::Z),
    ("edit.redo", Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z),
    ("select.all", Modifiers::COMMAND, Key::A),
];

fn text_field_focused(ctx: &Context) -> bool {
    ctx.memory(|m| m.focused())
        .is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some())
}

impl MarkiseApp {
    /// Installs a native menu bar; the egui menu bar is then not shown.
    pub fn set_menu_host(&mut self, host: Box<dyn MenuHost>) {
        self.menu_host = Some(host);
    }

    /// Whether a native menu bar is installed.
    pub fn has_native_menu(&self) -> bool {
        self.menu_host.is_some()
    }

    fn registry(&self) -> &Registry {
        match &self.doc {
            Some(d) => &d.viewer.ed.registry,
            None => &defaults(self.menu_platform).0,
        }
    }

    /// Registry commands can run now (not while the source buffer is ahead of the session or
    /// in source mode, as [`MarkiseApp::run_command`]).
    fn command_enabled(&self, id: &str, text_field: bool) -> bool {
        let Some(d) = &self.doc else { return false };
        match id {
            "edit.cut" | "edit.copy" | "edit.paste" => return true,
            _ => {}
        }
        if text_field && TEXT_FIELD_KEYS.iter().any(|(k, ..)| *k == id) {
            return true;
        }
        !d.buffer_ahead() && !self.source_visible && d.viewer.ed.registry.enabled(&d.viewer.ed.session, id).is_ok()
    }

    /// Format and block-style toggles show whether the selection has them (spec 04 §3.2).
    fn command_check(&self, id: &str, enabled: bool) -> Check {
        let toggle = (id.starts_with("format.") && !matches!(id, "format.link" | "format.image" | "format.clear"))
            || matches!(
                id,
                "block.quote" | "block.bulletList" | "block.orderedList" | "block.taskList" | "block.paragraph"
            )
            || id.starts_with("block.heading") && !matches!(id, "block.headingUp" | "block.headingDown");
        if !toggle {
            return Check::None;
        }
        let on = enabled
            && self
                .doc
                .as_ref()
                .is_some_and(|d| d.viewer.ed.registry.checked(&d.viewer.ed.session, id));
        Check::Toggle(on)
    }

    /// Edit menu tail: auto-pair and smart punctuation preferences (spec 02 §4, §13).
    fn editing_pref_entries(&self) -> Vec<Entry> {
        let e = self.themes.editing();
        vec![
            Entry::Separator,
            self.app_item("edit.autoPairBrackets", true, Check::Toggle(e.auto_pair_brackets)),
            self.app_item("edit.autoPairMarkdown", true, Check::Toggle(e.auto_pair_markdown)),
            self.app_item("edit.smartPunctuation", true, Check::Toggle(e.smart_punctuation)),
            self.app_item(
                "edit.smartDashesEm",
                e.smart_punctuation,
                Check::Toggle(e.smart_dashes_em),
            ),
        ]
    }

    fn item(&self, action: MenuAction, label: impl Into<String>, enabled: bool, check: Check) -> Entry {
        let chord = match &action {
            MenuAction::Command(id) => {
                // the keymap in effect (`keyBinding` applied, conflicts resolved): each chord
                // is shown — and installed as a native key equivalent — on its winner only
                let km = if self.menu_platform == Platform::current() {
                    self.effective_keymap()
                } else {
                    &defaults(self.menu_platform).1
                };
                match app_command(id) {
                    // an application command's chord yields to a registry binding of it
                    Some(d) => descriptor_chord(d, self.menu_platform).filter(|c| km.lookup(c).is_empty()),
                    None => km.shown_chord_of(id).cloned(),
                }
            }
            _ => None,
        };
        Entry::Item(Item {
            key: action.key(),
            action,
            label: label.into(),
            chord,
            enabled,
            check,
        })
    }

    /// An application command item.
    fn app_item(&self, id: &'static str, enabled: bool, check: Check) -> Entry {
        let label = app_command(id).map_or(id, |d| d.label);
        self.item(MenuAction::Command(id), label, enabled, check)
    }

    /// Items for command ids in display order (`None`: separator): registry commands with
    /// their enabled state, application commands (Cut/Copy/Paste) when listed.
    fn command_entries(&self, ids: &[Option<&'static str>], text_field: bool) -> Vec<Entry> {
        let reg = self.registry();
        ids.iter()
            .filter_map(|id| match id {
                None => Some(Entry::Separator),
                Some(id) => {
                    let label = reg
                        .get(id)
                        .map(|s| s.desc.label)
                        .or_else(|| app_command(id).map(|d| d.label))?;
                    let enabled = self.command_enabled(id, text_field);
                    Some(self.item(MenuAction::Command(id), label, enabled, self.command_check(id, enabled)))
                }
            })
            .collect()
    }

    fn registry_menu(&self, title: &str, order: &[Option<&'static str>], text_field: bool) -> Menu {
        let reg = self.registry();
        let ids = menu_ids(title, order, reg.describe(), |id| {
            reg.get(id).is_some() || app_command(id).is_some()
        });
        Menu::new(title, self.command_entries(&ids, text_field))
    }

    /// The menu bar for the current state (cheap: no layout or document scans; planner
    /// commands answer `enabled` with a dry run at the selection).
    pub fn menu_model(&self) -> MenuBar {
        let mac = self.menu_platform == Platform::Mac;
        let has_doc = self.doc.is_some();
        let text_field = self.ctx.as_ref().is_some_and(text_field_focused);
        let mut menus = Vec::new();
        if mac {
            menus.push(
                Menu::new(
                    "Markise",
                    vec![
                        self.app_item("app.about", true, Check::None),
                        Entry::Separator,
                        // the preferences window (spec 05) does not exist yet
                        self.app_item("app.preferences", false, Check::None),
                        Entry::Separator,
                        Entry::Role(Role::Services),
                        Entry::Separator,
                        Entry::Role(Role::Hide),
                        Entry::Role(Role::HideOthers),
                        Entry::Role(Role::ShowAll),
                        Entry::Separator,
                        self.app_item("app.quit", true, Check::None),
                    ],
                )
                .kind(MenuKind::App),
            );
        }
        let mut file = vec![
            self.item(MenuAction::Command("file.open"), "Open…", true, Check::None),
            self.app_item("file.openFolder", true, Check::None),
            Entry::Separator,
            self.item(MenuAction::Command("file.save"), "Save", has_doc, Check::None),
            Entry::Separator,
            Entry::Submenu(self.export_menu()),
            Entry::Separator,
            self.app_item("file.close", true, Check::None),
        ];
        if !mac {
            file.push(Entry::Separator);
            file.push(self.app_item("app.exit", true, Check::None));
        }
        menus.push(Menu::new("File", file));
        let mut edit = self.registry_menu("Edit", EDIT_ORDER, text_field);
        edit.entries.extend(self.editing_pref_entries());
        menus.push(edit);
        menus.push(self.registry_menu("Paragraph", PARAGRAPH_ORDER, text_field));
        let format = self.registry_menu("Format", FORMAT_ORDER, text_field);
        if !format.entries.is_empty() {
            menus.push(format);
        }
        menus.push(self.view_menu());
        menus.push(self.themes_menu());
        if mac {
            menus.push(
                Menu::new(
                    "Window",
                    vec![
                        Entry::Role(Role::Minimize),
                        Entry::Role(Role::Zoom),
                        Entry::Separator,
                        Entry::Role(Role::BringAllToFront),
                    ],
                )
                .kind(MenuKind::Window),
            );
            // About is in the application menu; Help content is P2 (spec 04 §2.8); macOS
            // adds its menu search
            menus.push(Menu::new("Help", Vec::new()).kind(MenuKind::Help));
        } else {
            menus.push(Menu::new("Help", vec![self.app_item("app.about", true, Check::None)]).kind(MenuKind::Help));
        }
        MenuBar {
            platform: self.menu_platform,
            menus,
        }
    }

    fn export_menu(&self) -> Menu {
        let mut v: Vec<Entry> = [
            ExportKind::Pdf,
            ExportKind::Html,
            ExportKind::HtmlPlain,
            ExportKind::Png,
        ]
        .into_iter()
        .map(|k| {
            let label = k.label();
            self.item(MenuAction::Export(k), label, true, Check::None)
        })
        .collect();
        let pandoc = PandocFormat::ALL
            .iter()
            .map(|f| self.item(MenuAction::Export(ExportKind::Pandoc(*f)), f.label(), true, Check::None))
            .collect();
        v.push(Entry::Submenu(Menu::new("Other Formats via Pandoc", pandoc)));
        v.push(Entry::Separator);
        let prev = self.export.previous_label();
        let label = match &prev {
            Some(l) => format!("Export with Previous ({l})"),
            None => "Export with Previous".into(),
        };
        v.push(self.item(
            MenuAction::Command("export.previous"),
            label,
            prev.is_some(),
            Check::None,
        ));
        v.push(self.app_item("export.overwritePrevious", prev.is_some(), Check::None));
        Menu::new("Export", v).enabled(self.doc.is_some() && !self.export.is_running())
    }

    fn view_menu(&self) -> Menu {
        let has_doc = self.doc.is_some();
        let mode = self.view_mode();
        let src_label = self
            .registry()
            .get("view.sourceMode")
            .map_or("Source Code Mode", |s| s.desc.label);
        Menu::new(
            "View",
            vec![
                self.app_item("view.rendered", has_doc, Check::Radio(mode == ViewMode::Rendered)),
                self.item(
                    MenuAction::Command("view.sourceMode"),
                    src_label,
                    has_doc,
                    Check::Radio(mode == ViewMode::Source),
                ),
                self.app_item("view.sideBySide", has_doc, Check::Radio(mode == ViewMode::SideBySide)),
                Entry::Separator,
                self.item(
                    MenuAction::Command("view.toggleSidebar"),
                    "Toggle Sidebar",
                    true,
                    Check::None,
                ),
                Entry::Separator,
                self.app_item("view.highlightCode", true, Check::Toggle(self.highlight_code)),
                self.app_item("view.diagrams", true, Check::Toggle(self.diagrams)),
                Entry::Separator,
                self.app_item("view.fullScreen", true, Check::None),
            ],
        )
    }

    /// Themes menu (ADR 0007): Style and Colors submenus pick the two parts independently;
    /// "Complete Themes" pick a theme's own style and colors together.
    fn themes_menu(&self) -> Menu {
        let sel = self.themes.selection().clone();
        let families: &[ThemeFamily] = self.themes.families();
        let label = |f: &ThemeFamily| match (&f.light, &f.dark) {
            (Some(_), Some(_)) => f.name.clone(),
            (None, Some(_)) => format!("{} (dark only)", f.name),
            _ => format!("{} (light only)", f.name),
        };
        let mut style = Vec::new();
        let mut from_themes = false;
        for (id, name, theme) in self.themes.style_choices() {
            if theme && !from_themes {
                from_themes = true;
                style.push(Entry::Separator);
                style.push(Entry::Header("From Themes".into()));
            }
            let on = sel.style == id;
            style.push(self.item(MenuAction::Style(id), name, true, Check::Radio(on)));
        }
        let mut colors = Vec::new();
        let mut section = None;
        for f in families {
            let group = match f.origin {
                Origin::Markise => 0,
                Origin::Palette => 1,
                Origin::Builtin | Origin::User => 2,
            };
            if section != Some(group) {
                if section.is_some() {
                    colors.push(Entry::Separator);
                }
                section = Some(group);
                match group {
                    1 => colors.push(Entry::Header("Palettes".into())),
                    2 => colors.push(Entry::Header("From Themes".into())),
                    _ => {}
                }
            }
            let on = sel.colors == f.id;
            colors.push(self.item(MenuAction::Colors(f.id.clone()), label(f), true, Check::Radio(on)));
        }
        let mut v = vec![
            Entry::Submenu(Menu::new("Style", style)),
            Entry::Submenu(Menu::new("Colors", colors)),
            Entry::Separator,
            Entry::Header("Complete Themes".into()),
        ];
        let mut user = false;
        for f in families
            .iter()
            .filter(|f| f.id == ThemeId::Markise || matches!(f.origin, Origin::Builtin | Origin::User))
        {
            if f.origin == Origin::User && !user {
                user = true;
                v.push(Entry::Header("My Themes".into()));
            }
            let on = sel.complete_theme() == Some(&f.id);
            v.push(self.item(MenuAction::Theme(f.id.clone()), label(f), true, Check::Radio(on)));
        }
        v.push(Entry::Separator);
        v.push(Entry::Header("Appearance".into()));
        let a = self.themes.appearance();
        for (ap, name) in [
            (Appearance::System, "Follow System"),
            (Appearance::Light, "Light"),
            (Appearance::Dark, "Dark"),
        ] {
            v.push(self.item(MenuAction::Appearance(ap), name, true, Check::Radio(a == ap)));
        }
        v.push(Entry::Separator);
        let custom = if self.themes.overrides().is_empty() {
            ""
        } else {
            " (Custom)"
        };
        v.push(self.item(
            MenuAction::Command("theme.fontAndSize"),
            format!("Font & Size…{custom}"),
            true,
            Check::None,
        ));
        v.push(Entry::Separator);
        let has_user = self.themes.has_user_dir();
        v.push(self.app_item("theme.import", has_user, Check::None));
        v.push(self.app_item("theme.openFolder", has_user, Check::None));
        v.push(self.app_item("theme.report", true, Check::None));
        Menu::new("Themes", v)
    }

    /// Performs a menu item exactly like its shortcut: registry commands through
    /// [`MarkiseApp::run_command`] (so Side by Side stays consistent), host commands through
    /// the same methods as their shortcuts. Returns false when nothing happened.
    pub fn menu_action(&mut self, ctx: &Context, action: &MenuAction) -> bool {
        self.menu_action_by(ctx, action, Trigger::Click)
    }

    /// [`MarkiseApp::menu_action`] for an item chosen by `trigger` (a native menu reports
    /// whether its key equivalent or a click chose the item).
    pub fn menu_action_by(&mut self, ctx: &Context, action: &MenuAction, trigger: Trigger) -> bool {
        match action {
            MenuAction::Command(id) => self.menu_command(ctx, id, trigger),
            MenuAction::Style(id) => {
                let colors = self.themes.selection().colors.clone();
                self.themes.set_selection(Selection {
                    style: id.clone(),
                    colors,
                });
                true
            }
            MenuAction::Colors(id) => {
                let style = self.themes.selection().style.clone();
                self.themes.set_selection(Selection {
                    style,
                    colors: id.clone(),
                });
                true
            }
            // a complete theme sets (and marks as chosen) both parts, even an unchanged one
            MenuAction::Theme(id) => {
                self.themes.select(id.clone());
                true
            }
            MenuAction::Appearance(a) => {
                if *a != self.themes.appearance() {
                    self.themes.set_appearance(*a);
                }
                true
            }
            MenuAction::Export(kind) => {
                if self.doc.is_none() || self.export.is_running() {
                    return false;
                }
                self.export_action(ExportAction::Export(kind.clone()));
                true
            }
        }
    }

    pub(super) fn menu_command(&mut self, ctx: &Context, id: &str, trigger: Trigger) -> bool {
        match id {
            "file.open" => self.open_dialog(),
            "file.openFolder" => self.open_folder_dialog(),
            "file.save" => self.save(),
            "file.close" | "app.quit" | "app.exit" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            "app.preferences" => return false,
            "app.about" => self.about.open(),
            // the focused widget (rich view or a text field) handles these as from the keyboard
            "edit.cut" => ctx.send_viewport_cmd(egui::ViewportCommand::RequestCut),
            "edit.copy" => ctx.send_viewport_cmd(egui::ViewportCommand::RequestCopy),
            "edit.paste" => ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste),
            // spec 04 §3.3: a click sets the mode, ⌘/ and ⌃⌘/ toggle
            "view.sourceMode" if self.doc.is_some() => match trigger {
                Trigger::Click => self.set_view_mode(ViewMode::Source),
                Trigger::Shortcut => self.toggle_source(),
            },
            "view.sideBySide" if self.doc.is_some() => match trigger {
                Trigger::Click => self.set_view_mode(ViewMode::SideBySide),
                Trigger::Shortcut => self.toggle_side_by_side(),
            },
            "view.rendered" if self.doc.is_some() => self.set_view_mode(ViewMode::Rendered),
            "view.toggleSidebar" => self.sidebar_visible = !self.sidebar_visible,
            "view.highlightCode" => self.set_highlight_code(!self.highlight_code),
            "view.diagrams" => self.set_diagrams(!self.diagrams),
            "view.fullScreen" => {
                let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
            }
            "export.previous" | "export.overwritePrevious" => {
                if self.doc.is_none() || self.export.is_running() || !self.export.has_previous() {
                    return false;
                }
                self.export_action(if id == "export.previous" {
                    ExportAction::Previous
                } else {
                    ExportAction::OverwritePrevious
                });
            }
            "edit.autoPairBrackets" | "edit.autoPairMarkdown" | "edit.smartPunctuation" | "edit.smartDashesEm" => {
                let mut e = self.themes.editing();
                match id {
                    "edit.autoPairBrackets" => e.auto_pair_brackets = !e.auto_pair_brackets,
                    "edit.autoPairMarkdown" => e.auto_pair_markdown = !e.auto_pair_markdown,
                    "edit.smartPunctuation" => e.smart_punctuation = !e.smart_punctuation,
                    _ => e.smart_dashes_em = !e.smart_dashes_em,
                }
                self.themes.set_editing(e);
            }
            "theme.fontAndSize" => self.font_window = true,
            "theme.import" => self.import_theme_dialog(),
            "theme.openFolder" => self.open_theme_folder(),
            "theme.report" => {
                self.installed = None;
                self.theme_report = true;
            }
            id => {
                if text_field_focused(ctx)
                    && let Some((_, modifiers, key)) = TEXT_FIELD_KEYS.iter().find(|(k, ..)| *k == id)
                {
                    // the text field reads this frame's events after this point
                    ctx.input_mut(|i| {
                        i.events.push(egui::Event::Key {
                            key: *key,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: *modifiers,
                        })
                    });
                    return true;
                }
                return self.run_command(ctx, id);
            }
        }
        ctx.request_repaint();
        true
    }

    /// Native menu events of this frame, then the egui fallback menu bar when there is no
    /// native one.
    pub(super) fn menus_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if let Some(host) = self.menu_host.as_mut() {
            for (a, trigger) in host.take_actions() {
                self.menu_action_by(&ctx, &a, trigger);
            }
            return;
        }
        let bar = self.menu_model();
        let mut chosen = None;
        egui::Panel::top("markise-menu").show(ui, |ui| chosen = crate::menu::show_egui(ui, &bar));
        if let Some(a) = chosen {
            self.menu_action(&ctx, &a);
        }
    }

    /// Key handling for the application commands' chords ([`crate::menu::app_shortcuts`]),
    /// so every shortcut a menu shows works: on Windows/Linux (menus only display them) and
    /// on macOS without a native menu. With the native macOS menu the menu routes them.
    pub(super) fn app_shortcuts(&mut self, ctx: &Context) {
        if self.menu_platform == Platform::Mac && self.menu_host.is_some() {
            return;
        }
        // exact chords (a binding with extra modifiers is another chord), and a chord that a
        // `keyBinding` gives to a registry command is not the application command's any more
        let platform = self.menu_platform;
        let cmds: Vec<(&'static str, markise_engine::editor::keymap::Chord)> = crate::menu::app_shortcuts(platform)
            .into_iter()
            .filter_map(|(id, _)| Some((id, descriptor_chord(app_command(id)?, platform)?)))
            .filter(|(_, c)| self.effective_keymap().lookup(c).is_empty())
            .collect();
        let mut fire = Vec::new();
        ctx.input_mut(|i| {
            i.events.retain(|e| {
                let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = e
                else {
                    return true;
                };
                let Some(chord) = crate::viewer::chord_of(*key, *modifiers, platform) else {
                    return true;
                };
                match cmds.iter().find(|(_, c)| *c == chord) {
                    Some((id, _)) => {
                        fire.push(*id);
                        false
                    }
                    None => true,
                }
            });
        });
        for id in fire {
            self.menu_command(ctx, id, Trigger::Shortcut);
        }
    }

    /// Hands the current menu state to the native menu bar (end of frame).
    pub(super) fn sync_native_menu(&mut self) {
        if self.menu_host.is_none() {
            return;
        }
        let bar = self.menu_model();
        if let Some(host) = self.menu_host.as_mut() {
            host.sync(&bar);
        }
    }

    /// The About window is open (tests).
    pub fn is_about_open(&self) -> bool {
        self.about.is_open()
    }

    /// Builds menus for `platform` (tests of the other platform's menus).
    pub fn set_menu_platform(&mut self, platform: Platform) {
        self.menu_platform = platform;
    }
}

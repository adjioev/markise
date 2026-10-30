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
use markise_engine::settings::{ToolbarPosition, ZOOM_RANGE};
use markise_export::pandoc::PandocFormat;
use markise_md::MarkdownFormat;
use markise_theme::catalog::{Origin, Selection, ThemeId};

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
    /// Shows `bar` (called after every frame): applies enabled/checked/label changes in place,
    /// rebuilds only when [`MenuBar::structure`] changed, makes no native call for an unchanged
    /// model and none while a menu is open ([`crate::menu_sync::MenuSync`], issue #81).
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

/// Application commands that have no item in the macOS menus because AppKit adds its own
/// (View → Enter Full Screen); their chords stay handled by the app's shortcut path.
const MAC_SYSTEM_MENU_COMMANDS: [&str; 1] = ["view.fullScreen"];

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

    /// Registry commands can run now in the focused pane (as [`MarkiseApp::run_command`]).
    fn command_enabled(&self, id: &str, text_field: bool) -> bool {
        let Some(d) = &self.doc else { return false };
        match id {
            "edit.cut" | "edit.copy" | "edit.paste" => return true,
            _ => {}
        }
        if text_field && TEXT_FIELD_KEYS.iter().any(|(k, ..)| *k == id) {
            return true;
        }
        // the source editor (full Source mode, or focused in Side by Side) runs only its own
        // commands; format and block commands come to it in M2d slice 2
        if self.source_focused() && !markise_engine::editor::source::source_command_available(id) {
            return false;
        }
        d.viewer.ed.registry.enabled(&d.viewer.ed.session, id).is_ok()
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

    /// The formatting toolbar's controls (spec 04 §3.2): the registry commands it shows with
    /// their labels, effective shortcuts, enabled state and check state — the same queries as
    /// the Format and Paragraph menus. `active`: the toolbar acts on the visible pane
    /// (otherwise every control is disabled, e.g. over the read-only rendered pane of Side by
    /// Side).
    pub(super) fn toolbar_model(&self, active: bool) -> crate::toolbar::Model {
        let reg = self.registry();
        let km = if self.menu_platform == Platform::current() {
            self.effective_keymap()
        } else {
            &defaults(self.menu_platform).1
        };
        let controls = crate::toolbar::all_ids()
            .filter_map(|id| {
                // Add Comment only while comments and suggestions are on (ADR 0012)
                if id == "review.addComment" && !self.critic_markup() {
                    return None;
                }
                let spec = reg.get(id)?;
                let enabled = active && self.command_enabled(id, false);
                let shortcut = km
                    .shown_chord_of(id)
                    .map(|c| crate::menu::shortcut_text(c, self.menu_platform));
                Some(crate::toolbar::Control {
                    id: spec.desc.id,
                    // the toolbar button reads "Comment" (owner, issue #69)
                    label: if id == "review.addComment" {
                        "Comment"
                    } else {
                        spec.desc.label
                    },
                    shortcut,
                    enabled,
                    // state is shown even where the command is a no-op (Paragraph in a
                    // paragraph) or the pane is read-only
                    active: self.doc.is_some() && self.command_check(id, true) == Check::Toggle(true),
                })
            })
            .collect();
        crate::toolbar::Model { controls }
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
                        self.app_item("app.preferences", true, Check::None),
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
        let has_file = self.document_path().is_some();
        let mut file = vec![
            self.item(MenuAction::Command("file.new"), "New", true, Check::None),
            self.item(MenuAction::Command("file.open"), "Open…", true, Check::None),
            self.app_item("file.openFolder", true, Check::None),
            Entry::Separator,
            self.item(MenuAction::Command("file.save"), "Save", has_doc, Check::None),
            self.item(MenuAction::Command("file.saveAs"), "Save As…", has_doc, Check::None),
            self.app_item("file.saveCopyClean", has_doc, Check::None),
            Entry::Separator,
            self.app_item("file.rename", has_file, Check::None),
            self.item(
                MenuAction::Command("file.reveal"),
                crate::sidebar::reveal_label(),
                has_file,
                Check::None,
            ),
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
        edit.entries.push(Entry::Separator);
        edit.entries.push(Entry::Submenu(self.review_menu(text_field)));
        if !mac {
            // Windows and Linux: Preferences at the end of the Edit menu (the GNOME/KDE
            // convention; macOS has it in the application menu)
            edit.entries.push(Entry::Separator);
            edit.entries.push(self.app_item("app.preferences", true, Check::None));
        }
        menus.push(edit);
        menus.push(self.registry_menu("Paragraph", PARAGRAPH_ORDER, text_field));
        let format = self.registry_menu("Format", FORMAT_ORDER, text_field);
        if !format.entries.is_empty() {
            menus.push(format);
        }
        menus.push(self.view_menu());
        menus.push(Menu::new(
            "Go",
            vec![
                self.app_item("go.back", self.can_go_back(), Check::None),
                self.app_item("go.forward", self.can_go_forward(), Check::None),
            ],
        ));
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

    /// Edit → Comments & Suggestions (ADR 0012).
    fn review_menu(&self, text_field: bool) -> Menu {
        let ids: [Option<&'static str>; 8] = [
            Some("review.addComment"),
            None,
            Some("review.accept"),
            Some("review.reject"),
            Some("review.deleteComment"),
            None,
            Some("review.acceptAll"),
            Some("review.rejectAll"),
        ];
        Menu::new("Comments & Suggestions", self.command_entries(&ids, text_field))
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
        v.push(Entry::Separator);
        v.push(self.app_item("export.comments", true, Check::Toggle(self.export_comments())));
        Menu::new("Export", v).enabled(self.doc.is_some() && !self.export.is_running())
    }

    fn view_menu(&self) -> Menu {
        let has_doc = self.doc.is_some();
        let mode = self.view_mode();
        let src_label = self
            .registry()
            .get("view.sourceMode")
            .map_or("Source Code Mode", |s| s.desc.label);
        let mut entries = vec![
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
            self.app_item("view.showRecent", true, Check::Toggle(self.show_recent())),
            self.app_item("view.toolbar", true, Check::Toggle(self.show_toolbar())),
            Entry::Separator,
            self.app_item(
                "view.blockPalette",
                has_doc,
                Check::Toggle(crate::block_palette::is_open()),
            ),
            self.app_item("insert.block", has_doc, Check::None),
            Entry::Separator,
            self.app_item("view.highlightCode", true, Check::Toggle(self.highlight_code)),
            Entry::Submenu(self.theme_menu()),
            Entry::Separator,
            self.app_item("view.zoomIn", self.zoom() < ZOOM_RANGE.1, Check::None),
            self.app_item("view.zoomOut", self.zoom() > ZOOM_RANGE.0, Check::None),
            self.app_item("view.actualSize", true, Check::None),
        ];
        // macOS adds its own "Enter/Exit Full Screen" item to every menu titled "View" (for
        // full-screen capable windows) and flips its title itself; a second item of ours
        // would show twice. ⌃⌘F stays bound: the system item answers it, and when it does not,
        // the key reaches the app's shortcut path (see `app_shortcuts`). Windows and Linux
        // have no system item: ours, with a check mark.
        if self.menu_platform != Platform::Mac {
            let full = self
                .ctx
                .as_ref()
                .is_some_and(|c| c.input(|i| i.viewport().fullscreen.unwrap_or(false)));
            entries.push(Entry::Separator);
            entries.push(self.item(
                MenuAction::Command("view.fullScreen"),
                "Full Screen",
                true,
                Check::Toggle(full),
            ));
        }
        Menu::new("View", entries)
    }

    /// View → Theme: a quick switcher between the installed complete themes (Markise, the
    /// built-in and the user CSS themes; selecting one sets its style and colors), then
    /// "Theme Settings…" (Preferences → Appearance: style, colors, light/dark, font & size,
    /// theme files). Issue #50 replaced the top-level Themes menu with it.
    fn theme_menu(&self) -> Menu {
        let current = self.themes.selection().complete_theme().cloned();
        let mut v = Vec::new();
        let mut user = false;
        for f in self
            .themes
            .families()
            .iter()
            .filter(|f| f.id == ThemeId::Markise || matches!(f.origin, Origin::Builtin | Origin::User))
        {
            if f.origin == Origin::User && !user {
                user = true;
                v.push(Entry::Separator);
                v.push(Entry::Header("My Themes".into()));
            }
            let label = match (&f.light, &f.dark) {
                (Some(_), Some(_)) => f.name.clone(),
                (None, Some(_)) => format!("{} (dark only)", f.name),
                _ => format!("{} (light only)", f.name),
            };
            let on = current.as_ref() == Some(&f.id);
            v.push(self.item(MenuAction::Theme(f.id.clone()), label, true, Check::Radio(on)));
        }
        v.push(Entry::Separator);
        v.push(self.app_item("view.themeSettings", true, Check::None));
        Menu::new("Theme", v)
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
        if self.is_asking() {
            // a yes/no question is open (a native alert on another thread, or the in-app
            // dialog): no command may replace or bypass the action waiting for the answer
            return false;
        }
        // PROTOTYPE (block forms): edit shortcuts go to the docked form panel when it has focus
        if let MenuAction::Command(id) = action
            && crate::form_panel::forward(id)
        {
            return true;
        }
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
            "file.new" => self.new_document(),
            "file.saveAs" if self.doc.is_some() => self.save_as(),
            "file.rename" | "file.reveal" => {
                let Some(p) = self.document_path().map(std::path::Path::to_path_buf) else {
                    return false;
                };
                if id == "file.rename" {
                    self.start_rename(p);
                } else {
                    self.tree_action(ctx, crate::sidebar::TreeAction::Reveal(p));
                }
            }
            "file.close" | "app.quit" | "app.exit" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            "app.preferences" => self.open_preferences(),
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
            "view.toolbar" => self.set_show_toolbar(!self.show_toolbar()),
            "view.blockPalette" => crate::block_palette::toggle(),
            "insert.block" => crate::block_palette::open_for(None),
            "view.showRecent" => self.set_show_recent(!self.show_recent()),
            "view.toolbarTop" => self.set_toolbar_position(ToolbarPosition::Top),
            "view.toolbarBottom" => self.set_toolbar_position(ToolbarPosition::Bottom),
            "view.highlightCode" => self.set_highlight_code(!self.highlight_code),
            "view.diagrams" => self.set_diagrams(!self.diagrams),
            "view.wikilinks" => self.set_wikilinks(!self.wikilinks()),
            "view.criticMarkup" => self.set_critic_markup(!self.critic_markup()),
            "file.saveCopyClean" if self.doc.is_some() => self.save_copy_without_comments(),
            "export.comments" => self.set_export_comments(!self.export_comments()),
            "go.back" => self.go_back(),
            "go.forward" => self.go_forward(),
            "view.sourceLineNumbers" | "view.sourceSoftWrap" | "view.sourceCurrentLine" | "view.sourceWhitespace" => {
                let mut p = self.themes.source_prefs();
                match id {
                    "view.sourceLineNumbers" => p.line_numbers = !p.line_numbers,
                    "view.sourceSoftWrap" => p.soft_wrap = !p.soft_wrap,
                    "view.sourceCurrentLine" => p.current_line = !p.current_line,
                    _ => p.whitespace_marks = !p.whitespace_marks,
                }
                self.themes.set_source_prefs(p);
            }
            "view.zoomIn" => self.zoom_step(1),
            "view.zoomOut" => self.zoom_step(-1),
            "view.actualSize" => self.set_zoom(1.0),
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
            "view.themeSettings" => self.open_preferences_at(crate::prefs::Section::Appearance),
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
        // with the native macOS menu the menu routes the chords of its items; commands without
        // a menu item there (Full Screen: the system's item replaces ours) are still ours
        let native_mac = self.menu_platform == Platform::Mac && self.menu_host.is_some();
        // exact chords (a binding with extra modifiers is another chord), and a chord that a
        // `keyBinding` gives to a registry command is not the application command's any more
        let platform = self.menu_platform;
        let cmds: Vec<(&'static str, markise_engine::editor::keymap::Chord)> = crate::menu::app_shortcuts(platform)
            .into_iter()
            .filter(|(id, _)| !native_mac || MAC_SYSTEM_MENU_COMMANDS.contains(id))
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
                let Some(mut chord) = crate::viewer::chord_of(*key, *modifiers, platform) else {
                    return true;
                };
                // ⇧= arrives as "+" on most layouts: the zoom chords are written with "="
                if chord.shift && chord.key == "+" {
                    chord.key = "=".into();
                }
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

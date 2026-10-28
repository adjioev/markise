//! Toolkit-neutral menu model (spec 04 §2): the application builds a [`MenuBar`] from the
//! command registry (labels and per-platform chords of the [`CommandDescriptor`]s, ADR 0006
//! §3.1) and its own application commands ([`APP_COMMANDS`]); a front end shows it — the
//! native menu bar (macOS, Windows; `apps/markise`) or the in-window egui fallback
//! ([`show_egui`], Linux and tests). Choosing an item yields its [`MenuAction`], which the
//! application dispatches exactly like the shortcut (`MarkiseApp::menu_action`).
//!
//! Items are keyed by a stable string ([`Item::key`]), so a native front end can apply
//! enabled/checked/label changes to existing items and rebuild only when the structure
//! ([`MenuBar::structure`]) changes.

use std::fmt::Write as _;

use markise_doc::{CommandDescriptor, DefaultKey, Effect, ParamSchema, Scope};
pub use markise_engine::editor::keymap::{Chord, Platform};
use markise_engine::export::ExportKind;
use markise_engine::settings::Appearance;
use markise_theme::catalog::{StyleId, ThemeId};

/// What a menu item does.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    /// A registry command or an [`APP_COMMANDS`] id.
    Command(&'static str),
    /// Theme → Style.
    Style(StyleId),
    /// Theme → Colors.
    Colors(ThemeId),
    /// Theme → a complete theme (style and colors together).
    Theme(ThemeId),
    /// Theme → Appearance.
    Appearance(Appearance),
    /// File → Export → a format.
    Export(ExportKind),
}

impl MenuAction {
    /// Stable key of the action (native menu item id).
    pub fn key(&self) -> String {
        match self {
            MenuAction::Command(id) => (*id).to_string(),
            MenuAction::Style(s) => format!("theme.style:{s:?}"),
            MenuAction::Colors(t) => format!("theme.colors:{t:?}"),
            MenuAction::Theme(t) => format!("theme.complete:{t:?}"),
            MenuAction::Appearance(a) => format!("theme.appearance:{}", a.as_str()),
            MenuAction::Export(k) => format!("export:{k:?}"),
        }
    }
}

/// How a menu item was chosen. The view-mode items differ (spec 04 §3.3): a click sets the
/// mode, the shortcut toggles it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Click,
    Shortcut,
}

/// Check-mark state of an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// A plain command.
    None,
    /// An on/off toggle.
    Toggle(bool),
    /// One of a group of mutually exclusive choices.
    Radio(bool),
}

/// A menu item.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// Stable id ([`MenuAction::key`]).
    pub key: String,
    pub action: MenuAction,
    pub label: String,
    /// Default shortcut (shown; native menus on macOS also route it).
    pub chord: Option<Chord>,
    pub enabled: bool,
    pub check: Check,
}

/// Standard platform items performed by the OS (macOS application and Window menus).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    About,
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Minimize,
    Zoom,
    BringAllToFront,
}

/// The accelerator the native front end gives a standard role item (macOS: Hide ⌘H, Hide
/// Others ⌥⌘H, Minimize ⌘M). These chords are reserved for `keyBinding`.
pub fn role_chord(r: Role, platform: Platform) -> Option<Chord> {
    let t = match (platform, r) {
        (Platform::Mac, Role::Hide) => "Cmd+H",
        (Platform::Mac, Role::HideOthers) => "Opt+Cmd+H",
        (Platform::Mac, Role::Minimize) => "Cmd+M",
        _ => return None,
    };
    Chord::parse(t, platform)
}

/// What a top-level menu is for (native front ends register the Window and Help menus).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKind {
    Normal,
    /// The macOS application menu (titled with the app name).
    App,
    Window,
    Help,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Item(Item),
    Separator,
    /// A non-interactive section label ("From themes").
    Header(String),
    Submenu(Menu),
    Role(Role),
}

/// A (sub)menu.
#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub title: String,
    pub kind: MenuKind,
    pub enabled: bool,
    pub entries: Vec<Entry>,
}

impl Menu {
    pub fn new(title: impl Into<String>, entries: Vec<Entry>) -> Menu {
        Menu {
            title: title.into(),
            kind: MenuKind::Normal,
            enabled: true,
            entries: tidy(entries),
        }
    }

    pub fn kind(mut self, kind: MenuKind) -> Menu {
        self.kind = kind;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Menu {
        self.enabled = enabled;
        self
    }

    /// Every item, depth first.
    pub fn items(&self) -> Vec<&Item> {
        let mut out = Vec::new();
        collect(&self.entries, &mut out);
        out
    }
}

fn collect<'a>(entries: &'a [Entry], out: &mut Vec<&'a Item>) {
    for e in entries {
        match e {
            Entry::Item(i) => out.push(i),
            Entry::Submenu(m) => collect(&m.entries, out),
            _ => {}
        }
    }
}

/// The whole menu bar.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuBar {
    pub platform: Platform,
    pub menus: Vec<Menu>,
}

impl MenuBar {
    /// The top-level menu titled `title`.
    pub fn menu(&self, title: &str) -> Option<&Menu> {
        self.menus.iter().find(|m| m.title == title)
    }

    /// Every item, depth first.
    pub fn items(&self) -> Vec<&Item> {
        self.menus.iter().flat_map(|m| m.items()).collect()
    }

    /// The item with key `key`.
    pub fn item(&self, key: &str) -> Option<&Item> {
        self.items().into_iter().find(|i| i.key == key)
    }

    /// The shape of the menu bar without the per-item state (enabled, checked, label): a
    /// native front end rebuilds its menus only when this changes and otherwise updates the
    /// items in place.
    pub fn structure(&self) -> String {
        fn walk(s: &mut String, entries: &[Entry]) {
            for e in entries {
                match e {
                    Entry::Item(i) => {
                        let kind = match i.check {
                            Check::None => 'i',
                            Check::Toggle(_) => 't',
                            Check::Radio(_) => 'r',
                        };
                        let _ = write!(s, "{kind}{}|{:?};", i.key, i.chord);
                    }
                    Entry::Separator => s.push_str("-;"),
                    Entry::Header(h) => {
                        let _ = write!(s, "h{h};");
                    }
                    Entry::Role(r) => {
                        let _ = write!(s, "o{r:?};");
                    }
                    Entry::Submenu(m) => {
                        let _ = write!(s, "[{}:", m.title);
                        walk(s, &m.entries);
                        s.push(']');
                    }
                }
            }
        }
        let mut s = String::new();
        for m in &self.menus {
            let _ = write!(s, "<{}:{:?}:", m.title, m.kind);
            walk(&mut s, &m.entries);
            s.push('>');
        }
        s
    }
}

/// Drops leading, trailing and repeated separators and empty submenus.
pub fn tidy(entries: Vec<Entry>) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::with_capacity(entries.len());
    for e in entries {
        match e {
            Entry::Separator if matches!(out.last(), None | Some(Entry::Separator)) => {}
            Entry::Submenu(m) if m.entries.is_empty() && m.kind == MenuKind::Normal => {}
            e => out.push(e),
        }
    }
    while matches!(out.last(), Some(Entry::Separator)) {
        out.pop();
    }
    out
}

// ------------------------------------------------------------------ application commands

const fn app(id: &'static str, label: &'static str, keys: &'static [DefaultKey]) -> CommandDescriptor {
    CommandDescriptor {
        id,
        label,
        menu: None,
        keys,
        scope: Scope::Global,
        effect: Effect::View,
        params: ParamSchema::NONE,
        planner: None,
    }
}

const fn both(mac: &'static str, other: &'static str) -> DefaultKey {
    DefaultKey {
        mac: Some(mac),
        other: Some(other),
    }
}

const fn mac_only(mac: &'static str) -> DefaultKey {
    DefaultKey {
        mac: Some(mac),
        other: None,
    }
}

/// Commands performed by the application itself (window, files, view and theme state) that
/// the engine registry does not hold. Same descriptor type as the registry, so labels and
/// per-platform chords have one source (spec 02 §15).
pub static APP_COMMANDS: &[CommandDescriptor] = &[
    app(
        "file.openFolder",
        "Open Folder…",
        &[both("Shift+Cmd+O", "Ctrl+Shift+O")],
    ),
    // the current document's file (the sidebar offers the same for any file)
    app("file.rename", "Rename…", &[]),
    app("file.reveal", "Reveal in File Manager", &[]),
    app("file.close", "Close", &[both("Cmd+W", "Ctrl+W")]),
    app("app.quit", "Quit Markise", &[mac_only("Cmd+Q")]),
    app("app.exit", "Exit", &[]),
    app("app.about", "About Markise", &[]),
    app("app.preferences", "Preferences…", &[both("Cmd+,", "Ctrl+,")]),
    app("edit.cut", "Cut", &[both("Cmd+X", "Ctrl+X")]),
    app("edit.copy", "Copy", &[both("Cmd+C", "Ctrl+C")]),
    app("edit.paste", "Paste", &[both("Cmd+V", "Ctrl+V")]),
    app("edit.autoPairBrackets", "Auto Pair Brackets and Quotes", &[]),
    app("edit.autoPairMarkdown", "Auto Pair Markdown Syntax", &[]),
    app("edit.smartPunctuation", "Smart Punctuation: Convert on Input", &[]),
    app("edit.smartDashesEm", "Smart Dashes: -- as Em Dash", &[]),
    app("view.rendered", "Rendered", &[]),
    // spec 04 §3.3: ⌃⌘/ on macOS, Ctrl+Alt+/ elsewhere (Ctrl+/ is Source Code Mode)
    app("view.sideBySide", "Side by Side", &[both("Ctrl+Cmd+/", "Ctrl+Alt+/")]),
    app("view.toolbar", "Toolbar", &[]),
    // View → Toolbar Position (spec 04 §3.2); the menu shows them as "Top" / "Bottom"
    app("view.toolbarTop", "Toolbar at Top", &[]),
    app("view.toolbarBottom", "Toolbar at Bottom", &[]),
    app("view.highlightCode", "Highlight Code", &[]),
    app("view.diagrams", "Diagrams", &[]),
    app("view.fullScreen", "Toggle Full Screen", &[both("Ctrl+Cmd+F", "F11")]),
    app("export.previous", "Export with Previous", &[]),
    app("export.overwritePrevious", "Export and Overwrite with Previous", &[]),
    app("theme.fontAndSize", "Font & Size…", &[]),
    app("theme.import", "Import Theme…", &[]),
    app("theme.openFolder", "Open Theme Folder", &[]),
    app("theme.report", "Theme Report…", &[]),
];

/// The application command `id`.
pub fn app_command(id: &str) -> Option<&'static CommandDescriptor> {
    APP_COMMANDS.iter().find(|d| d.id == id)
}

/// The chord of a descriptor's default key for `platform` (first one that parses).
pub fn descriptor_chord(d: &CommandDescriptor, platform: Platform) -> Option<Chord> {
    d.keys.iter().find_map(|k| {
        let text = match platform {
            Platform::Mac => k.mac,
            Platform::Other => k.other,
        };
        text.and_then(|t| Chord::parse(t, platform))
    })
}

/// Application commands whose chords egui itself turns into events for the focused widget
/// (`Event::Cut/Copy/Paste`): no app-level key handler.
pub const EGUI_NATIVE_KEYS: [&str; 3] = ["edit.cut", "edit.copy", "edit.paste"];

/// egui shortcut of a chord shown for `platform` (`None` for keys egui cannot name).
pub fn egui_shortcut(c: &Chord, platform: Platform) -> Option<egui::KeyboardShortcut> {
    let key = egui::Key::from_name(&c.key)?;
    let modifiers = match platform {
        Platform::Mac => egui::Modifiers {
            alt: c.alt,
            ctrl: c.ctrl,
            shift: c.shift,
            mac_cmd: c.meta,
            command: c.meta,
        },
        Platform::Other => egui::Modifiers {
            alt: c.alt,
            ctrl: c.ctrl,
            shift: c.shift,
            mac_cmd: false,
            command: c.ctrl,
        },
    };
    Some(egui::KeyboardShortcut::new(modifiers, key))
}

/// The application commands with an app-level key handler on `platform` and their egui
/// shortcuts, most modifiers first (egui matches shortcuts logically: ⌘/ would also match
/// ⌃⌘/).
pub fn app_shortcuts(platform: Platform) -> Vec<(&'static str, egui::KeyboardShortcut)> {
    let mut v: Vec<(&'static str, egui::KeyboardShortcut)> = APP_COMMANDS
        .iter()
        .filter(|d| !EGUI_NATIVE_KEYS.contains(&d.id))
        .filter_map(|d| Some((d.id, egui_shortcut(&descriptor_chord(d, platform)?, platform)?)))
        .collect();
    let n = |m: egui::Modifiers| m.alt as u8 + m.ctrl as u8 + m.shift as u8 + m.command as u8;
    v.sort_by_key(|(_, s)| std::cmp::Reverse(n(s.modifiers)));
    v
}

// ------------------------------------------------------------------ registry placement

/// Preferred order of registry commands in the Edit menu (`None`: separator). Only commands
/// present in the registry (or [`APP_COMMANDS`]) appear.
pub const EDIT_ORDER: &[Option<&str>] = &[
    Some("edit.undo"),
    Some("edit.redo"),
    None,
    Some("edit.cut"),
    Some("edit.copy"),
    Some("edit.paste"),
    Some("edit.copyMarkdown"),
    Some("edit.copyHtml"),
    Some("edit.copyPlain"),
    Some("edit.pastePlain"),
    None,
    Some("select.all"),
    Some("select.line"),
    Some("select.scope"),
    Some("select.word"),
    Some("select.block"),
    None,
    Some("find.open"),
    Some("find.next"),
    Some("find.prev"),
    Some("find.replace"),
];

/// Preferred order of the Paragraph menu.
pub const PARAGRAPH_ORDER: &[Option<&str>] = &[
    Some("block.heading1"),
    Some("block.heading2"),
    Some("block.heading3"),
    Some("block.heading4"),
    Some("block.heading5"),
    Some("block.heading6"),
    None,
    Some("block.paragraph"),
    None,
    Some("block.headingUp"),
    Some("block.headingDown"),
    None,
    Some("insert.table"),
    Some("block.codeFence"),
    Some("block.mathBlock"),
    None,
    Some("block.quote"),
    None,
    Some("block.orderedList"),
    Some("block.bulletList"),
    Some("block.taskList"),
    Some("block.toggleTask"),
    Some("block.indent"),
    Some("block.outdent"),
    None,
    Some("block.insertParagraphBefore"),
    Some("block.insertParagraphAfter"),
    None,
    Some("markdown.insertLinkRef"),
    Some("markdown.insertFootnote"),
    None,
    Some("insert.rule"),
];

/// Preferred order of the Format menu.
pub const FORMAT_ORDER: &[Option<&str>] = &[
    Some("format.strong"),
    Some("format.emph"),
    Some("format.underline"),
    Some("format.code"),
    None,
    Some("format.strike"),
    Some("format.highlight"),
    Some("format.superscript"),
    Some("format.subscript"),
    Some("format.inlineMath"),
    None,
    Some("format.comment"),
    None,
    Some("format.link"),
    Some("format.image"),
    None,
    Some("format.clear"),
];

/// Which top-level menu a registry command belongs to when it is not in a preferred list:
/// the descriptor's own `menu` path if it has one, else by id prefix (`format.*` → Format;
/// `block.*`, `insert.*`, `markdown.insert*` → Paragraph). Caret, selection-extension,
/// typing and host commands have no menu.
pub fn home_menu(d: &CommandDescriptor) -> Option<&'static str> {
    if let Some(path) = d.menu {
        return path.first().copied();
    }
    let id = d.id;
    if id.starts_with("format.") {
        Some("Format")
    } else if id.starts_with("block.") || id.starts_with("insert.") || id.starts_with("markdown.insert") {
        Some("Paragraph")
    } else {
        None
    }
}

/// The command ids of top-level menu `menu`, in display order with `None` for separators:
/// the preferred list filtered to `available` ids, then — after a separator — the other
/// registry commands whose [`home_menu`] is `menu`, in registry order. New registry commands
/// therefore appear without menu code changes.
pub fn menu_ids<'a>(
    menu: &str,
    order: &[Option<&'a str>],
    registry: impl Iterator<Item = &'a CommandDescriptor>,
    available: impl Fn(&str) -> bool,
) -> Vec<Option<&'a str>> {
    let mut out: Vec<Option<&'a str>> = order.iter().filter(|e| e.is_none_or(&available)).copied().collect();
    let mut extra = registry
        .filter(|d| home_menu(d) == Some(menu) && !order.contains(&Some(d.id)))
        .map(|d| Some(d.id))
        .peekable();
    if extra.peek().is_some() {
        out.push(None);
        out.extend(extra);
    }
    // no leading, trailing or repeated separators
    let mut tidy: Vec<Option<&'a str>> = Vec::with_capacity(out.len());
    for e in out {
        if e.is_some() || matches!(tidy.last(), Some(Some(_))) {
            tidy.push(e);
        }
    }
    if tidy.last() == Some(&None) {
        tidy.pop();
    }
    tidy
}

// ------------------------------------------------------------------ shortcut text

/// Shortcut text without symbol glyphs (`Shift+Cmd+O` on macOS, `Ctrl+Shift+O` elsewhere):
/// used where the platform does not draw the shortcut itself (the egui fallback menu, tool
/// tips), so no font needs ⇧/⌥/⌃ (egui's default fonts lack ⇧).
pub fn shortcut_text(c: &Chord, platform: Platform) -> String {
    let mut v: Vec<&str> = Vec::new();
    match platform {
        Platform::Mac => {
            if c.ctrl {
                v.push("Ctrl");
            }
            if c.alt {
                v.push("Opt");
            }
            if c.shift {
                v.push("Shift");
            }
            if c.meta {
                v.push("Cmd");
            }
        }
        Platform::Other => {
            if c.ctrl {
                v.push("Ctrl");
            }
            if c.alt {
                v.push("Alt");
            }
            if c.shift {
                v.push("Shift");
            }
            if c.meta {
                v.push("Super");
            }
        }
    }
    v.push(&c.key);
    v.join("+")
}

/// Accelerator text in the common `Modifier+Key` syntax of native menu libraries (W3C
/// `KeyboardEvent.code` key names for non-character keys); `None` for keys such a library
/// cannot express.
pub fn accelerator_text(c: &Chord) -> Option<String> {
    let key = match c.key.as_str() {
        "Up" => "ArrowUp",
        "Down" => "ArrowDown",
        "Left" => "ArrowLeft",
        "Right" => "ArrowRight",
        "Enter" | "Backspace" | "Delete" | "Escape" | "Space" | "Tab" | "Home" | "End" | "PageUp" | "PageDown" => {
            c.key.as_str()
        }
        k if k.len() >= 2 && k.starts_with('F') && k[1..].parse::<u8>().is_ok() => k,
        k if k.chars().count() == 1 => k,
        _ => return None,
    };
    let mut s = String::new();
    if c.ctrl {
        s.push_str("Ctrl+");
    }
    if c.alt {
        s.push_str("Alt+");
    }
    if c.shift {
        s.push_str("Shift+");
    }
    if c.meta {
        s.push_str("Super+");
    }
    s.push_str(key);
    Some(s)
}

// ------------------------------------------------------------------ egui fallback

/// Shows `bar` as an in-window egui menu bar (platforms without a native menu bar); returns
/// the chosen action.
pub fn show_egui(ui: &mut egui::Ui, bar: &MenuBar) -> Option<MenuAction> {
    let mut chosen = None;
    egui::MenuBar::new().ui(ui, |ui| {
        for m in &bar.menus {
            // the platform menus (application, Window) and empty menus have no egui form
            if m.kind == MenuKind::App || m.items().is_empty() {
                continue;
            }
            ui.add_enabled_ui(m.enabled, |ui| {
                ui.menu_button(&m.title, |ui| entries_egui(ui, &m.entries, bar.platform, &mut chosen));
            });
        }
    });
    chosen
}

/// Label atoms of a checkable item: the label, then its shortcut (weak) pushed to the right.
fn check_atoms(label: &str, shortcut: Option<String>) -> egui::Atoms<'static> {
    let mut atoms = egui::Atoms::new(label.to_string());
    if let Some(sc) = shortcut {
        atoms.push_right(egui::Atom::grow());
        atoms.push_right(egui::RichText::new(sc).weak());
    }
    atoms
}

fn entries_egui(ui: &mut egui::Ui, entries: &[Entry], platform: Platform, chosen: &mut Option<MenuAction>) {
    for e in entries {
        match e {
            Entry::Item(i) => {
                let shortcut = i.chord.as_ref().map(|c| shortcut_text(c, platform));
                let clicked = match (i.check, shortcut) {
                    (Check::None, None) => ui.add_enabled(i.enabled, egui::Button::new(&i.label)).clicked(),
                    (Check::None, Some(sc)) => ui
                        .add_enabled(i.enabled, egui::Button::new(&i.label).shortcut_text(sc))
                        .clicked(),
                    (Check::Toggle(on), sc) => {
                        let mut v = on;
                        let w = egui::Checkbox::new(&mut v, check_atoms(&i.label, sc));
                        ui.add_enabled(i.enabled, w).clicked()
                    }
                    (Check::Radio(on), sc) => ui
                        .add_enabled(i.enabled, egui::RadioButton::new(on, check_atoms(&i.label, sc)))
                        .clicked(),
                };
                if clicked {
                    *chosen = Some(i.action.clone());
                    ui.close();
                }
            }
            Entry::Separator => {
                ui.separator();
            }
            Entry::Header(h) => {
                ui.weak(h);
            }
            Entry::Submenu(m) => {
                ui.add_enabled_ui(m.enabled, |ui| {
                    ui.menu_button(&m.title, |ui| entries_egui(ui, &m.entries, platform, chosen));
                });
            }
            Entry::Role(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(id: &'static str, menu: Option<&'static [&'static str]>) -> CommandDescriptor {
        CommandDescriptor {
            menu,
            ..app(id, id, &[])
        }
    }

    #[test]
    fn tidy_drops_redundant_separators_and_empty_submenus() {
        let item = |k: &'static str| {
            Entry::Item(Item {
                key: k.into(),
                action: MenuAction::Command(k),
                label: k.into(),
                chord: None,
                enabled: true,
                check: Check::None,
            })
        };
        let out = tidy(vec![
            Entry::Separator,
            item("a"),
            Entry::Separator,
            Entry::Separator,
            Entry::Submenu(Menu::new("empty", vec![Entry::Separator])),
            item("b"),
            Entry::Separator,
        ]);
        assert_eq!(out.len(), 3);
        assert!(matches!(out[1], Entry::Separator));
    }

    #[test]
    fn unknown_registry_commands_land_in_their_menu() {
        let reg = [
            d("format.strong", None),
            d("format.fancy", None),
            d("block.heading1", None),
            d("caret.left", None),
            d("x.custom", Some(&["Format", "More"])),
        ];
        let ids = menu_ids("Format", FORMAT_ORDER, reg.iter(), |id| id == "format.strong");
        assert_eq!(
            ids,
            vec![Some("format.strong"), None, Some("format.fancy"), Some("x.custom")]
        );
        let para = menu_ids("Paragraph", PARAGRAPH_ORDER, reg.iter(), |id| id == "block.heading1");
        assert_eq!(para, vec![Some("block.heading1")]);
        assert_eq!(home_menu(&d("caret.left", None)), None);
    }

    #[test]
    fn shortcut_text_has_no_symbol_glyphs() {
        let c = Chord::parse("Shift+Cmd+O", Platform::Mac).unwrap();
        assert_eq!(shortcut_text(&c, Platform::Mac), "Shift+Cmd+O");
        let c = Chord::parse("Shift+Cmd+O", Platform::Other).unwrap();
        assert_eq!(shortcut_text(&c, Platform::Other), "Ctrl+Shift+O");
        let c = Chord::parse("Ctrl+Cmd+/", Platform::Mac).unwrap();
        assert_eq!(shortcut_text(&c, Platform::Mac), "Ctrl+Cmd+/");
    }

    #[test]
    fn accelerator_text_per_platform() {
        let mac = Chord::parse("Ctrl+Cmd+/", Platform::Mac).unwrap();
        assert_eq!(accelerator_text(&mac).as_deref(), Some("Ctrl+Super+/"));
        let other = Chord::parse("Ctrl+Alt+/", Platform::Other).unwrap();
        assert_eq!(accelerator_text(&other).as_deref(), Some("Ctrl+Alt+/"));
        let up = Chord::parse("Cmd+Up", Platform::Mac).unwrap();
        assert_eq!(accelerator_text(&up).as_deref(), Some("Super+ArrowUp"));
        let f11 = Chord::parse("F11", Platform::Other).unwrap();
        assert_eq!(accelerator_text(&f11).as_deref(), Some("F11"));
    }

    #[test]
    fn app_commands_have_unique_ids_and_parse() {
        for (i, a) in APP_COMMANDS.iter().enumerate() {
            assert!(APP_COMMANDS[i + 1..].iter().all(|b| b.id != a.id), "{}", a.id);
            for k in a.keys {
                for (t, p) in [(k.mac, Platform::Mac), (k.other, Platform::Other)] {
                    if let Some(t) = t {
                        assert!(Chord::parse(t, p).is_some(), "{} {t}", a.id);
                    }
                }
            }
        }
        let sbs = app_command("view.sideBySide").unwrap();
        let mac = descriptor_chord(sbs, Platform::Mac).unwrap();
        assert!(mac.ctrl && mac.meta && mac.key == "/");
        let other = descriptor_chord(sbs, Platform::Other).unwrap();
        assert!(other.ctrl && other.alt && !other.meta);
    }
}

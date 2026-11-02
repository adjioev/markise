//! Keyboard shortcut cheat sheet (issue #63): Help → Keyboard Shortcuts (⌘? on macOS,
//! Ctrl+? elsewhere). The list is generated from the command registry and
//! [`APP_COMMANDS`](crate::menu::APP_COMMANDS) with the chords of the keymap in effect, so it
//! matches the real keys, `keyBinding` overrides included: a registry command shows the chord
//! the menus show for it ([`Keymap::shown_chord_of`]); an application command its default
//! chord unless a registry command took that chord. Commands without a chord are not listed.

use markise_doc::CommandDescriptor;
use markise_engine::editor::command::Registry;
use markise_engine::editor::keymap::{Keymap, Platform};

use crate::menu::{
    APP_COMMANDS, EDIT_ORDER, FORMAT_ORDER, PARAGRAPH_ORDER, TABLE_COMMANDS, descriptor_chord, shortcut_text,
};

/// One listed shortcut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: &'static str,
    pub label: &'static str,
    /// The chord as text ([`shortcut_text`]: `Shift+Cmd+O`, `Ctrl+Shift+O`).
    pub chord: String,
}

/// A titled group of shortcuts (a menu, or a category of keys without a menu item).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub title: &'static str,
    pub rows: Vec<Row>,
}

/// Group titles in display order.
pub const GROUPS: &[&str] = &[
    "File",
    "Edit",
    "Find",
    "Paragraph",
    "Table",
    "Format",
    "Comments & Suggestions",
    "View",
    "Go",
    "Typing",
    "Move & Select",
    "Help",
    "Markise",
];

/// The group of a command.
pub fn group_of(d: &CommandDescriptor) -> &'static str {
    let id = d.id;
    let prefix = id.split('.').next().unwrap_or("");
    match prefix {
        "file" | "export" => "File",
        "find" => "Find",
        "table" => "Table",
        "format" => "Format",
        "review" => "Comments & Suggestions",
        "view" | "theme" => "View",
        "go" => "Go",
        "help" => "Help",
        "app" => "Markise",
        "caret" | "nav" => "Move & Select",
        "block" | "insert" => "Paragraph",
        _ if id.starts_with("markdown.insert") => "Paragraph",
        // selection commands of the Edit menu (Select Word, Line…); the others extend the
        // selection by a caret movement
        "select" if id == "select.all" || EDIT_ORDER.contains(&Some(id)) => "Edit",
        "select" => "Move & Select",
        _ if matches!(
            id,
            "edit.newline"
                | "edit.softBreak"
                | "edit.deleteBackward"
                | "edit.deleteForward"
                | "edit.deleteWordBackward"
                | "edit.deleteWordForward"
                | "edit.indentOrNextCell"
                | "edit.outdentOrPrevCell"
                | "delete.toLineStart"
        ) =>
        {
            "Typing"
        }
        _ => "Edit",
    }
}

/// Position of `id` in the menus' preferred orders (rows of a group follow the menu).
fn menu_rank(id: &str) -> usize {
    [EDIT_ORDER, PARAGRAPH_ORDER, FORMAT_ORDER, TABLE_COMMANDS]
        .iter()
        .flat_map(|o| o.iter())
        .position(|e| *e == Some(id))
        .unwrap_or(usize::MAX)
}

/// Extra words a search matches for a command (its label uses the menu's term).
fn aliases(id: &str) -> &'static str {
    match id {
        "format.strong" => "bold",
        "format.emph" => "italic",
        "format.strike" => "strikethrough",
        "format.link" => "link url",
        "block.quote" => "blockquote",
        "block.bulletList" => "bullet",
        "block.orderedList" => "numbered",
        "block.insertRule" => "rule divider",
        "view.sourceMode" => "markdown raw",
        "view.zoomIn" | "view.zoomOut" | "view.actualSize" => "zoom",
        _ => "",
    }
}

/// The cheat sheet for `platform`: every registry command and application command that has a
/// chord in `keymap` (the keymap in effect for `platform`, `keyBinding` applied), grouped
/// ([`GROUPS`]) and ordered like the menus.
pub fn cheat_sheet(registry: &Registry, keymap: &Keymap, platform: Platform) -> Vec<Group> {
    let mut rows: Vec<(&'static str, usize, usize, Row)> = Vec::new();
    let mut seen: Vec<&'static str> = Vec::new();
    for d in registry.describe() {
        seen.push(d.id);
        if let Some(c) = keymap.shown_chord_of(d.id) {
            rows.push((
                group_of(d),
                menu_rank(d.id),
                rows.len(),
                row(d, shortcut_text(c, platform)),
            ));
        }
    }
    for d in APP_COMMANDS.iter().filter(|d| !seen.contains(&d.id)) {
        // an application command's chord yields to a registry binding of it (as in the menus)
        if let Some(c) = descriptor_chord(d, platform).filter(|c| keymap.lookup(c).is_empty()) {
            rows.push((
                group_of(d),
                menu_rank(d.id),
                rows.len(),
                row(d, shortcut_text(&c, platform)),
            ));
        }
    }
    rows.sort_by_key(|(g, rank, seq, _)| (GROUPS.iter().position(|t| t == g), *rank, *seq));
    let mut out: Vec<Group> = Vec::new();
    for (g, _, _, r) in rows {
        match out.last_mut() {
            Some(last) if last.title == g => last.rows.push(r),
            _ => out.push(Group {
                title: g,
                rows: vec![r],
            }),
        }
    }
    out
}

fn row(d: &'static CommandDescriptor, chord: String) -> Row {
    Row {
        id: d.id,
        label: d.label,
        chord,
    }
}

/// The groups' rows matching `query` (every word, case-insensitive, in the label, the chord,
/// the group title, the command id or its aliases); groups without a match are dropped.
pub fn filter(groups: &[Group], query: &str) -> Vec<Group> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return groups.to_vec();
    }
    groups
        .iter()
        .filter_map(|g| {
            let rows: Vec<Row> = g
                .rows
                .iter()
                .filter(|r| {
                    let hay = format!("{} {} {} {} {}", r.label, r.chord, g.title, r.id, aliases(r.id)).to_lowercase();
                    words.iter().all(|w| hay.contains(w.as_str()))
                })
                .cloned()
                .collect();
            (!rows.is_empty()).then_some(Group { title: g.title, rows })
        })
        .collect()
}

/// State of the cheat sheet window.
#[derive(Default)]
pub struct CheatSheet {
    pub open: bool,
    pub query: String,
    focus: bool,
    previous_focus: Option<egui::Id>,
}

impl CheatSheet {
    /// Opens the window (already open: focuses its search field).
    pub fn open(&mut self, ctx: &egui::Context) {
        if !self.open {
            self.previous_focus = ctx.memory(|m| m.focused());
            self.query.clear();
        }
        self.open = true;
        self.focus = true;
    }

    /// Shows the window over `groups`; Escape or its close button closes it and gives the
    /// keyboard focus back.
    pub fn show(&mut self, ctx: &egui::Context, groups: &[Group]) {
        if !self.open {
            return;
        }
        let escape = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let mut open = true;
        let screen = ctx.content_rect();
        egui::Window::new("Keyboard Shortcuts")
            .id(egui::Id::unique("markise-shortcuts"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 48.0))
            .default_width((screen.width() - 48.0).clamp(260.0, 560.0))
            .show(ctx, |ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Search shortcuts…")
                        .desired_width(f32::INFINITY),
                );
                if std::mem::take(&mut self.focus) {
                    field.request_focus();
                }
                let shown = filter(groups, &self.query);
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .max_height((screen.height() - 200.0).max(160.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for g in &shown {
                            ui.add_space(6.0);
                            ui.label(egui::RichText::new(g.title).strong());
                            egui::Grid::new(("markise-shortcuts", g.title))
                                .num_columns(2)
                                .spacing([24.0, 4.0])
                                .striped(true)
                                .show(ui, |ui| {
                                    for r in &g.rows {
                                        ui.label(r.label);
                                        ui.label(egui::RichText::new(&r.chord).monospace());
                                        ui.end_row();
                                    }
                                });
                        }
                        if shown.is_empty() {
                            ui.label("No matching shortcuts.");
                        }
                    });
                ui.separator();
                ui.weak("Change shortcuts with a \"keyBinding\" object in settings.json.    Esc: close");
            });
        if !open || escape {
            self.open = false;
            if let Some(id) = self.previous_focus.take() {
                ctx.memory_mut(|m| m.request_focus(id));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults(p: Platform) -> (Registry, Keymap) {
        let r = Registry::for_format(&markise_md::MarkdownFormat::default());
        let k = Keymap::new(&r, p);
        (r, k)
    }

    fn find<'a>(g: &'a [Group], id: &str) -> Option<(&'static str, &'a Row)> {
        g.iter()
            .find_map(|g| g.rows.iter().find(|r| r.id == id).map(|r| (g.title, r)))
    }

    #[test]
    fn lists_every_command_with_a_chord_once() {
        for p in [Platform::Mac, Platform::Other] {
            let (r, k) = defaults(p);
            let g = cheat_sheet(&r, &k, p);
            let all: Vec<&str> = g.iter().flat_map(|g| g.rows.iter().map(|r| r.id)).collect();
            for d in r.describe() {
                let listed = all.iter().filter(|i| **i == d.id).count();
                assert_eq!(listed, usize::from(k.shown_chord_of(d.id).is_some()), "{p:?} {}", d.id);
            }
            for d in APP_COMMANDS {
                if r.get(d.id).is_some() {
                    continue;
                }
                let has = descriptor_chord(d, p).is_some_and(|c| k.lookup(&c).is_empty());
                assert_eq!(
                    all.iter().filter(|i| **i == d.id).count(),
                    usize::from(has),
                    "{p:?} {}",
                    d.id
                );
            }
            // every group is a known one, in order, and none is empty
            let pos: Vec<usize> = g
                .iter()
                .map(|g| GROUPS.iter().position(|t| *t == g.title).unwrap())
                .collect();
            assert!(pos.windows(2).all(|w| w[0] < w[1]), "{p:?}: {pos:?}");
            assert!(g.iter().all(|g| !g.rows.is_empty()));
        }
    }

    #[test]
    fn shows_the_platform_chords_grouped_by_menu() {
        let (r, k) = defaults(Platform::Mac);
        let g = cheat_sheet(&r, &k, Platform::Mac);
        assert_eq!(
            find(&g, "format.strong").map(|(t, r)| (t, r.chord.as_str())),
            Some(("Format", "Cmd+B"))
        );
        assert_eq!(find(&g, "file.openFolder").unwrap().1.chord, "Shift+Cmd+O");
        assert_eq!(find(&g, "help.shortcuts").unwrap().0, "Help");
        assert_eq!(find(&g, "help.shortcuts").unwrap().1.chord, "Shift+Cmd+/");
        assert_eq!(find(&g, "caret.left").unwrap().0, "Move & Select");
        assert_eq!(find(&g, "select.word").unwrap().0, "Edit");
        assert_eq!(find(&g, "table.addRowBelow").unwrap().0, "Table");
        let (r, k) = defaults(Platform::Other);
        let g = cheat_sheet(&r, &k, Platform::Other);
        assert_eq!(find(&g, "format.strong").unwrap().1.chord, "Ctrl+B");
        assert_eq!(find(&g, "help.shortcuts").unwrap().1.chord, "Ctrl+Shift+/");
        assert_eq!(find(&g, "find.replace").unwrap().1.chord, "Ctrl+H");
        // macOS-only chords are not listed elsewhere
        assert!(find(&g, "app.quit").is_none() && find(&g, "edit.openLine").is_none());
    }

    #[test]
    fn reflects_key_binding_overrides() {
        let p = Platform::Mac;
        let (r, mut k) = defaults(p);
        let problems = k.apply_overrides(
            &r,
            &[
                ("Strong".into(), "Ctrl+Shift+B".into()),
                ("format.emph".into(), String::new()),
                // a registry command takes an application command's chord
                ("format.highlight".into(), "Shift+Cmd+O".into()),
            ],
            p,
        );
        assert!(problems.is_empty(), "{problems:?}");
        let g = cheat_sheet(&r, &k, p);
        assert_eq!(find(&g, "format.strong").unwrap().1.chord, "Ctrl+Shift+B");
        assert!(find(&g, "format.emph").is_none(), "unbound");
        assert_eq!(find(&g, "format.highlight").unwrap().1.chord, "Shift+Cmd+O");
        assert!(find(&g, "file.openFolder").is_none(), "its chord went to Highlight");
    }

    #[test]
    fn search_matches_labels_chords_groups_and_aliases() {
        let (r, k) = defaults(Platform::Mac);
        let g = cheat_sheet(&r, &k, Platform::Mac);
        let ids =
            |q: &str| -> Vec<&'static str> { filter(&g, q).iter().flat_map(|g| g.rows.iter().map(|r| r.id)).collect() };
        assert!(ids("bold").contains(&"format.strong"));
        assert!(ids("HEADING 2").contains(&"block.heading2"));
        assert!(ids("shift+cmd+o").contains(&"file.openFolder"));
        assert!(!ids("shift+cmd+o").contains(&"format.strong"));
        assert!(ids("table row").iter().all(|i| i.starts_with("table.")));
        assert!(ids("zzzz").is_empty());
        assert_eq!(filter(&g, "  "), g);
    }

    #[test]
    fn the_cheat_sheet_chord_clashes_with_no_registry_default() {
        for p in [Platform::Mac, Platform::Other] {
            let (_, k) = defaults(p);
            let c = descriptor_chord(crate::menu::app_command("help.shortcuts").unwrap(), p).unwrap();
            assert!(k.lookup(&c).is_empty(), "{p:?}: {:?}", k.lookup(&c));
            assert!(!markise_engine::editor::keymap::reserved_chords(p).contains(&c));
            let others: Vec<&str> = APP_COMMANDS
                .iter()
                .filter(|d| d.id != "help.shortcuts" && descriptor_chord(d, p).as_ref() == Some(&c))
                .map(|d| d.id)
                .collect();
            assert!(others.is_empty(), "{others:?}");
        }
    }
}

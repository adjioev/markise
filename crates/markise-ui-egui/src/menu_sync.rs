//! What a native menu bar has to change to show a [`MenuBar`] (issue #81).
//!
//! A native front end keeps one [`MenuSync`] and asks it after every frame which native calls
//! to make ([`MenuSync::plan`]). The plan is diffed against the state the native menus show
//! (the last applied plan), so an unchanged model costs no native call at all, and a changed
//! item costs exactly the calls for what changed on it.
//!
//! While a menu is open (`tracking`) the plan is always empty: AppKit dismisses an open menu
//! when the menu bar is replaced under it, and nothing of what the model can change while the
//! user browses a menu is worth that risk. The difference is applied by the first plan after
//! the menu closed (the front end wakes the UI when tracking ends).

use std::collections::HashMap;

use crate::menu::{Check, Entry, Menu, MenuBar, MenuKind};

/// One native menu change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuOp {
    /// Build the native menus from the model again (its structure changed).
    Rebuild,
    /// Set the label of the item with key `key`.
    Label { key: String, label: String },
    /// Enable or disable the item with key `key`.
    Enabled { key: String, enabled: bool },
    /// Set or clear the check mark of the item with key `key`.
    Checked { key: String, checked: bool },
    /// Enable or disable the submenu at `index` (depth-first order over the shown menus, as
    /// [`MenuSync::shown_menus`] lists them).
    SubmenuEnabled { index: usize, enabled: bool },
}

/// The state of an item as the native menu shows it.
#[derive(Clone, Debug)]
struct Shown {
    label: String,
    enabled: bool,
    checked: Option<bool>,
}

/// Diffs menu models against what the native menus show (see the module documentation).
#[derive(Debug)]
pub struct MenuSync {
    /// The macOS application and Window menus are shown (they exist only there).
    app_and_window_menus: bool,
    structure: Option<String>,
    items: HashMap<String, Shown>,
    submenus: Vec<bool>,
}

impl MenuSync {
    /// `app_and_window_menus`: the front end shows the macOS application and Window menus
    /// (other platforms leave them out).
    pub fn new(app_and_window_menus: bool) -> MenuSync {
        MenuSync {
            app_and_window_menus,
            structure: None,
            items: HashMap::new(),
            submenus: Vec::new(),
        }
    }

    /// Whether the front end shows the top-level menu `m`.
    pub fn shows(&self, m: &Menu) -> bool {
        self.app_and_window_menus || !matches!(m.kind, MenuKind::App | MenuKind::Window)
    }

    /// The top-level menus of `bar` the front end shows, in order.
    pub fn shown_menus<'a>(&'a self, bar: &'a MenuBar) -> impl Iterator<Item = &'a Menu> + 'a {
        bar.menus.iter().filter(|m| self.shows(m))
    }

    /// The native calls that make the menus show `bar`, and records them as applied.
    ///
    /// - `tracking`: a menu is open; nothing is changed, not even the first build (see the
    ///   module documentation).
    /// - `native_checked`: the check mark a native item shows now, when the platform can say
    ///   (a native check item flips itself when it is clicked, so clicking the selected radio
    ///   item would otherwise lose its check mark); `None` uses the last applied state.
    ///
    /// [`MenuOp::Rebuild`] comes alone: the front end builds every item from `bar`.
    pub fn plan(
        &mut self,
        bar: &MenuBar,
        tracking: bool,
        native_checked: impl Fn(&str) -> Option<bool>,
    ) -> Vec<MenuOp> {
        // also before the first build: a menu open then (e.g. the default menu AppKit shows
        // before ours) would be dismissed by installing the menu bar
        if tracking {
            return Vec::new();
        }
        let st = bar.structure();
        if self.structure.as_deref() != Some(st.as_str()) {
            self.structure = Some(st);
            self.items = bar
                .items()
                .into_iter()
                .map(|i| {
                    (
                        i.key.clone(),
                        Shown {
                            label: i.label.clone(),
                            enabled: i.enabled,
                            checked: checked(i.check),
                        },
                    )
                })
                .collect();
            self.submenus = self.submenu_states(bar);
            return vec![MenuOp::Rebuild];
        }
        let mut ops = Vec::new();
        for i in bar.items() {
            let Some(s) = self.items.get_mut(&i.key) else {
                continue;
            };
            if s.label != i.label {
                s.label = i.label.clone();
                ops.push(MenuOp::Label {
                    key: i.key.clone(),
                    label: i.label.clone(),
                });
            }
            if s.enabled != i.enabled {
                s.enabled = i.enabled;
                ops.push(MenuOp::Enabled {
                    key: i.key.clone(),
                    enabled: i.enabled,
                });
            }
            if let Some(want) = checked(i.check) {
                let shown = native_checked(&i.key).or(s.checked);
                if shown != Some(want) {
                    ops.push(MenuOp::Checked {
                        key: i.key.clone(),
                        checked: want,
                    });
                }
                s.checked = Some(want);
            }
        }
        let now = self.submenu_states(bar);
        for (index, (was, now)) in self.submenus.iter_mut().zip(now).enumerate() {
            if *was != now {
                *was = now;
                ops.push(MenuOp::SubmenuEnabled { index, enabled: now });
            }
        }
        ops
    }

    /// Enabled state of every shown (sub)menu, depth first (top-level menus included).
    fn submenu_states(&self, bar: &MenuBar) -> Vec<bool> {
        fn walk(m: &Menu, out: &mut Vec<bool>) {
            out.push(m.enabled);
            for e in &m.entries {
                if let Entry::Submenu(c) = e {
                    walk(c, out);
                }
            }
        }
        let mut out = Vec::new();
        for m in self.shown_menus(bar) {
            walk(m, &mut out);
        }
        out
    }
}

/// The check mark of a check item (`None`: a plain item).
fn checked(c: Check) -> Option<bool> {
    match c {
        Check::None => None,
        Check::Toggle(on) | Check::Radio(on) => Some(on),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::{Item, MenuAction, Platform};

    fn item(id: &'static str, enabled: bool, check: Check) -> Entry {
        Entry::Item(Item {
            key: id.to_string(),
            action: MenuAction::Command(id),
            label: id.to_string(),
            chord: None,
            enabled,
            check,
        })
    }

    fn bar(save: bool, recent: bool, zoom_in: bool, theme_menu: Vec<Entry>) -> MenuBar {
        MenuBar {
            platform: Platform::Mac,
            menus: vec![
                Menu::new("Markise", vec![item("app.about", true, Check::None)]).kind(MenuKind::App),
                Menu::new(
                    "File",
                    vec![
                        item("file.save", save, Check::None),
                        Entry::Submenu(Menu::new("Export", vec![item("export.pdf", true, Check::None)]).enabled(save)),
                    ],
                ),
                Menu::new(
                    "View",
                    vec![
                        item("view.showRecent", true, Check::Toggle(recent)),
                        item("view.zoomIn", zoom_in, Check::None),
                        Entry::Submenu(Menu::new("Theme", theme_menu)),
                    ],
                ),
            ],
        }
    }

    fn themes(names: &[&'static str], on: usize) -> Vec<Entry> {
        names
            .iter()
            .enumerate()
            .map(|(n, k)| item(k, true, Check::Radio(n == on)))
            .collect()
    }

    fn base() -> MenuBar {
        bar(true, true, true, themes(&["theme.a", "theme.b"], 0))
    }

    fn synced() -> MenuSync {
        let mut s = MenuSync::new(true);
        assert_eq!(s.plan(&base(), false, |_| None), [MenuOp::Rebuild]);
        s
    }

    #[test]
    fn first_plan_builds_the_menus_once() {
        let mut s = MenuSync::new(true);
        assert_eq!(s.plan(&base(), false, |_| None), [MenuOp::Rebuild]);
        assert!(s.plan(&base(), false, |_| None).is_empty());
    }

    #[test]
    fn unchanged_model_means_no_native_call() {
        let mut s = synced();
        for _ in 0..100 {
            assert!(s.plan(&base(), false, |_| None).is_empty());
        }
    }

    #[test]
    fn a_changed_check_updates_exactly_that_item() {
        let mut s = synced();
        let b = bar(true, false, true, themes(&["theme.a", "theme.b"], 0));
        assert_eq!(
            s.plan(&b, false, |_| None),
            [MenuOp::Checked {
                key: "view.showRecent".into(),
                checked: false
            }]
        );
        assert!(s.plan(&b, false, |_| None).is_empty(), "applied once");
    }

    #[test]
    fn radio_selection_moves_with_two_calls() {
        let mut s = synced();
        let b = bar(true, true, true, themes(&["theme.a", "theme.b"], 1));
        assert_eq!(
            s.plan(&b, false, |_| None),
            [
                MenuOp::Checked {
                    key: "theme.a".into(),
                    checked: false
                },
                MenuOp::Checked {
                    key: "theme.b".into(),
                    checked: true
                },
            ]
        );
    }

    #[test]
    fn enabled_label_and_submenu_changes_are_minimal() {
        let mut s = synced();
        // zoom at its limit: one call
        let b = bar(true, true, false, themes(&["theme.a", "theme.b"], 0));
        assert_eq!(
            s.plan(&b, false, |_| None),
            [MenuOp::Enabled {
                key: "view.zoomIn".into(),
                enabled: false
            }]
        );
        // no document: Save and the Export submenu (index: Markise 0, File 1, Export 2)
        let b = bar(false, true, false, themes(&["theme.a", "theme.b"], 0));
        assert_eq!(
            s.plan(&b, false, |_| None),
            [
                MenuOp::Enabled {
                    key: "file.save".into(),
                    enabled: false
                },
                MenuOp::SubmenuEnabled {
                    index: 2,
                    enabled: false
                },
            ]
        );
        let mut b2 = b.clone();
        if let Entry::Item(i) = &mut b2.menus[1].entries[0] {
            i.label = "Save…".into();
        }
        assert_eq!(
            s.plan(&b2, false, |_| None),
            [MenuOp::Label {
                key: "file.save".into(),
                label: "Save…".into()
            }]
        );
    }

    /// Codex review of PR #34: clicking the selected radio item makes the native item clear
    /// its own check mark; the next plan restores it although the model did not change.
    #[test]
    fn check_marks_follow_the_model_not_the_last_plan() {
        let mut s = synced();
        let native = |k: &str| (k == "theme.a").then_some(false);
        assert_eq!(
            s.plan(&base(), false, native),
            [MenuOp::Checked {
                key: "theme.a".into(),
                checked: true
            }]
        );
        // native state agrees with the model: nothing to do
        assert!(s.plan(&base(), false, |k| (k == "theme.a").then_some(true)).is_empty());
    }

    /// Issue #81: while a menu is open nothing is touched — not even a structural change
    /// (theme list) or an enabled change; both are applied once the menu closed.
    #[test]
    fn nothing_changes_while_a_menu_is_open() {
        let mut s = synced();
        let more = bar(true, false, false, themes(&["theme.a", "theme.b", "theme.c"], 2));
        for _ in 0..10 {
            assert!(s.plan(&more, true, |_| None).is_empty());
        }
        assert_eq!(s.plan(&more, false, |_| None), [MenuOp::Rebuild]);
        assert!(s.plan(&more, false, |_| None).is_empty());

        let fewer_zoom = bar(true, false, true, themes(&["theme.a", "theme.b", "theme.c"], 2));
        assert!(s.plan(&fewer_zoom, true, |_| None).is_empty());
        assert_eq!(
            s.plan(&fewer_zoom, false, |_| None),
            [MenuOp::Enabled {
                key: "view.zoomIn".into(),
                enabled: true
            }]
        );
    }

    /// Codex review of PR #83: a menu already open before the first sync is not replaced
    /// either; the menus are built once it closed.
    #[test]
    fn first_build_waits_for_an_open_menu_to_close() {
        let mut s = MenuSync::new(true);
        for _ in 0..10 {
            assert!(s.plan(&base(), true, |_| None).is_empty());
        }
        assert_eq!(s.plan(&base(), false, |_| None), [MenuOp::Rebuild]);
        assert!(s.plan(&base(), false, |_| None).is_empty());
    }

    #[test]
    fn structure_change_rebuilds_once() {
        let mut s = synced();
        let more = bar(true, true, true, themes(&["theme.a", "theme.b", "theme.c"], 0));
        assert_eq!(s.plan(&more, false, |_| None), [MenuOp::Rebuild]);
        assert!(s.plan(&more, false, |_| None).is_empty());
    }

    #[test]
    fn app_and_window_menus_only_where_shown() {
        let s = MenuSync::new(false);
        let b = base();
        assert_eq!(
            s.shown_menus(&b).map(|m| m.title.as_str()).collect::<Vec<_>>(),
            ["File", "View"]
        );
        let mut s = MenuSync::new(false);
        s.plan(&b, false, |_| None);
        // without the application menu the Export submenu is index 1
        let b = bar(false, true, true, themes(&["theme.a", "theme.b"], 0));
        assert!(s.plan(&b, false, |_| None).contains(&MenuOp::SubmenuEnabled {
            index: 1,
            enabled: false
        }));
    }
}

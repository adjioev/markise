//! Key routing (ADR 0006 §3.2): chords parsed from the descriptors' per-platform default keys
//! (`"Shift+Cmd+Z"`, `"Ctrl+Home"`), looked up to command ids. Toolkit-neutral: the front end
//! turns its key events into [`Chord`]s. `keyBinding` overrides (§3.3, spec 05 §4.7):
//! [`Keymap::apply_overrides`].

use std::collections::HashMap;

use super::command::Registry;

/// Which default key column applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Other,
}

impl Platform {
    pub fn current() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::Mac
        } else {
            Platform::Other
        }
    }
}

/// Physical modifiers + a key name (normalised: `Enter`, `Backspace`, `Left`, `PageUp`, `A`,
/// `/`, `=` …). `meta` is Cmd on macOS (and the Windows/Super key elsewhere).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
    pub key: String,
}

impl Chord {
    pub fn new(key: &str) -> Chord {
        Chord {
            ctrl: false,
            alt: false,
            shift: false,
            meta: false,
            key: normalize_key(key),
        }
    }

    /// Parses canonical chord text for `platform` (`Cmd` is ⌘ on macOS and Ctrl elsewhere;
    /// `Opt` = `Alt`).
    pub fn parse(s: &str, platform: Platform) -> Option<Chord> {
        let mut c = Chord::new("");
        let parts: Vec<&str> = if let Some(head) = s.strip_suffix("++") {
            let mut v: Vec<&str> = head.split('+').collect();
            v.push("+");
            v
        } else {
            s.split('+').collect()
        };
        let (key, mods) = parts.split_last()?;
        for m in mods {
            match m.trim() {
                "Cmd" | "Command" | "⌘" => match platform {
                    Platform::Mac => c.meta = true,
                    Platform::Other => c.ctrl = true,
                },
                "Ctrl" | "Control" => c.ctrl = true,
                "Alt" | "Opt" | "Option" => c.alt = true,
                "Shift" => c.shift = true,
                "Meta" | "Super" | "Win" => c.meta = true,
                _ => return None,
            }
        }
        let key = key.trim();
        if key.is_empty() {
            return None;
        }
        c.key = normalize_key(key);
        known_key(&c.key).then_some(c)
    }
}

/// Chords the operating system (or its standard menu roles) owns: `keyBinding` cannot take
/// them (ADR 0006 §3.3). macOS: Hide ⌘H, Hide Others ⌥⌘H, Minimize ⌘M, Quit ⌘Q,
/// Preferences ⌘,, app switching ⌘Tab / ⌘`, Spotlight ⌘Space; elsewhere: Alt+F4, Alt+Tab.
pub fn reserved_chords(platform: Platform) -> Vec<Chord> {
    let list: &[&str] = match platform {
        Platform::Mac => &[
            "Cmd+H",
            "Opt+Cmd+H",
            "Cmd+M",
            "Opt+Cmd+M",
            "Cmd+Q",
            "Cmd+,",
            "Cmd+Tab",
            "Cmd+`",
            "Cmd+Space",
        ],
        Platform::Other => &["Alt+F4", "Alt+Tab"],
    };
    list.iter().filter_map(|t| Chord::parse(t, platform)).collect()
}

/// Key names the front ends produce (normalised): one character, or a named key.
fn known_key(k: &str) -> bool {
    if k.chars().count() == 1 {
        return true;
    }
    matches!(
        k,
        "Enter"
            | "Escape"
            | "Delete"
            | "Backspace"
            | "Up"
            | "Down"
            | "Left"
            | "Right"
            | "PageUp"
            | "PageDown"
            | "Home"
            | "End"
            | "Tab"
            | "Space"
            | "Insert"
    ) || k
        .strip_prefix('F')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=35).contains(&n))
}

fn normalize_key(k: &str) -> String {
    match k {
        "Return" | "Enter" | "↩" => "Enter".into(),
        "Esc" | "Escape" => "Escape".into(),
        "Del" | "Delete" | "ForwardDelete" => "Delete".into(),
        "Backspace" | "⌫" => "Backspace".into(),
        "↑" | "Up" | "ArrowUp" => "Up".into(),
        "↓" | "Down" | "ArrowDown" => "Down".into(),
        "←" | "Left" | "ArrowLeft" => "Left".into(),
        "→" | "Right" | "ArrowRight" => "Right".into(),
        "PgUp" | "PageUp" => "PageUp".into(),
        "PgDn" | "PageDown" => "PageDown".into(),
        "Space" | " " => "Space".into(),
        k if k.chars().count() == 1 => k.to_uppercase(),
        k => k.to_string(),
    }
}

/// Chord → command ids (in registry order; the host tries them until one is enabled).
#[derive(Clone, Debug, Default)]
pub struct Keymap {
    map: HashMap<Chord, Vec<&'static str>>,
}

impl Keymap {
    /// The default keymap of `registry` for `platform`.
    pub fn new(registry: &Registry, platform: Platform) -> Keymap {
        let mut map: HashMap<Chord, Vec<&'static str>> = HashMap::new();
        for d in registry.describe() {
            for k in d.keys {
                let text = match platform {
                    Platform::Mac => k.mac,
                    Platform::Other => k.other,
                };
                if let Some(c) = text.and_then(|t| Chord::parse(t, platform)) {
                    let ids = map.entry(c).or_default();
                    if !ids.contains(&d.id) {
                        ids.push(d.id);
                    }
                }
            }
        }
        Keymap { map }
    }

    /// Applies `keyBinding` overrides (ADR 0006 §3.3; spec 05 §4.7): each entry maps a command
    /// label (menu title, interop) or id to an accelerator (`"Ctrl+Shift+P"`) or `""` to unbind.
    /// The command loses its default chords; a chord another command also has is a conflict
    /// (reported) that the later entry wins. Unknown commands and unparsable accelerators are
    /// reported, never fatal. Returns the problems found.
    pub fn apply_overrides(
        &mut self,
        registry: &Registry,
        entries: &[(String, String)],
        platform: Platform,
    ) -> Vec<String> {
        let mut problems = Vec::new();
        for (name, accel) in entries {
            let Some(id) = registry
                .describe()
                .find(|d| d.id == name || d.label == name)
                .map(|d| d.id)
            else {
                problems.push(format!("keyBinding: unknown command `{name}`"));
                continue;
            };
            let accel = accel.trim();
            let chord = if accel.is_empty() {
                None
            } else {
                match Chord::parse(accel, platform) {
                    Some(c) if reserved_chords(platform).contains(&c) => {
                        problems.push(format!(
                            "keyBinding: `{accel}` for `{name}` is reserved by the system; ignored"
                        ));
                        continue;
                    }
                    Some(c) => Some(c),
                    None => {
                        problems.push(format!("keyBinding: cannot parse `{accel}` for `{name}`"));
                        continue;
                    }
                }
            };
            for ids in self.map.values_mut() {
                ids.retain(|i| *i != id);
            }
            self.map.retain(|_, ids| !ids.is_empty());
            if let Some(c) = chord {
                let ids = self.map.entry(c).or_default();
                if !ids.is_empty() {
                    problems.push(format!(
                        "keyBinding: `{accel}` of `{name}` was bound to {}; `{name}` wins",
                        ids.join(", ")
                    ));
                }
                ids.insert(0, id);
            }
        }
        problems
    }

    pub fn lookup(&self, c: &Chord) -> &[&'static str] {
        self.map.get(c).map_or(&[], Vec::as_slice)
    }

    /// The chord a menu shows for `id`: the first chord on which `id` is the winner (the
    /// first command of the chord's lookup list). A command shadowed on every chord it has
    /// (a `keyBinding` gave the chord to another command) shows none, so each chord appears
    /// on exactly one menu item.
    pub fn shown_chord_of(&self, id: &str) -> Option<&Chord> {
        self.map
            .iter()
            .filter(|(_, ids)| ids.first() == Some(&id))
            .map(|(c, _)| c)
            .min_by_key(|c| {
                (
                    c.ctrl as u8 + c.alt as u8 + c.shift as u8 + c.meta as u8,
                    c.key.len(),
                    c.key.clone(),
                )
            })
    }

    /// The first chord bound to `id` (menus show it).
    pub fn chord_of(&self, id: &str) -> Option<&Chord> {
        self.map
            .iter()
            .filter(|(_, ids)| ids.contains(&id))
            .map(|(c, _)| c)
            .min_by_key(|c| (c.ctrl as u8 + c.alt as u8 + c.shift as u8 + c.meta as u8, c.key.len()))
    }
}

/// Menu text of a chord (`⇧⌘Z` on macOS, `Ctrl+Shift+Z` elsewhere).
pub fn chord_label(c: &Chord, platform: Platform) -> String {
    match platform {
        Platform::Mac => {
            let mut s = String::new();
            if c.ctrl {
                s.push('⌃');
            }
            if c.alt {
                s.push('⌥');
            }
            if c.shift {
                s.push('⇧');
            }
            if c.meta {
                s.push('⌘');
            }
            s.push_str(&c.key);
            s
        }
        Platform::Other => {
            let mut v = Vec::new();
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
            v.push(&c.key);
            v.join("+")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_binding_overrides() {
        let r = Registry::new();
        let mut km = Keymap::new(&r, Platform::Other);
        let problems = km.apply_overrides(
            &r,
            &[
                ("Undo".into(), "Ctrl+Alt+U".into()),
                ("edit.redo".into(), "".into()),
                ("Nope".into(), "Ctrl+1".into()),
                ("Select All".into(), "Hyper+Q".into()),
                ("edit.redo".into(), "Ctrl+Bogus".into()),
                ("Save".into(), "Ctrl+Alt+U".into()),
            ],
            Platform::Other,
        );
        assert!(km.lookup(&Chord::parse("Ctrl+Z", Platform::Other).unwrap()).is_empty());
        assert_eq!(
            km.lookup(&Chord::parse("Ctrl+Alt+U", Platform::Other).unwrap())[0],
            "file.save"
        );
        assert!(km.chord_of("edit.redo").is_none());
        // an invalid key name is reported and ignored: Select All keeps its default
        assert!(km.chord_of("select.all").is_some());
        assert_eq!(problems.len(), 4, "{problems:?}");
        // the losing command of a conflict shows no chord
        let mut km = Keymap::new(&r, Platform::Other);
        km.apply_overrides(&r, &[("Undo".into(), "Ctrl+S".into())], Platform::Other);
        assert_eq!(
            km.shown_chord_of("edit.undo"),
            Chord::parse("Ctrl+S", Platform::Other).as_ref()
        );
        assert_eq!(km.shown_chord_of("file.save"), None);
        // system chords are reserved: reported, ignored, nothing changes
        let mut km = Keymap::new(&r, Platform::Mac);
        let p = km.apply_overrides(
            &r,
            &[("Undo".into(), "Cmd+H".into()), ("Redo".into(), "Cmd+M".into())],
            Platform::Mac,
        );
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(km.lookup(&Chord::parse("Cmd+H", Platform::Mac).unwrap()).is_empty());
        assert_eq!(km.lookup(&Chord::parse("Cmd+Z", Platform::Mac).unwrap()), ["edit.undo"]);
        let mut km = Keymap::new(&r, Platform::Other);
        let p = km.apply_overrides(&r, &[("Undo".into(), "Ctrl+Nonsense".into())], Platform::Other);
        assert_eq!(p.len(), 1);
        assert_eq!(
            km.lookup(&Chord::parse("Ctrl+Z", Platform::Other).unwrap()),
            ["edit.undo"]
        );
    }

    #[test]
    fn parses_platform_chords() {
        let c = Chord::parse("Shift+Cmd+Z", Platform::Mac).unwrap();
        assert!(c.meta && c.shift && !c.ctrl);
        assert_eq!(c.key, "Z");
        let c = Chord::parse("Shift+Cmd+Z", Platform::Other).unwrap();
        assert!(c.ctrl && c.shift && !c.meta);
        assert_eq!(Chord::parse("Return", Platform::Mac).unwrap().key, "Enter");
        assert_eq!(Chord::parse("Cmd+/", Platform::Mac).unwrap().key, "/");
        assert!(Chord::parse("Hyper+X", Platform::Mac).is_none());
    }

    #[test]
    fn default_keymap_routes_editing_keys() {
        let r = Registry::new();
        let mac = Keymap::new(&r, Platform::Mac);
        assert_eq!(
            mac.lookup(&Chord::parse("Cmd+Z", Platform::Mac).unwrap()),
            ["edit.undo"]
        );
        assert_eq!(mac.lookup(&Chord::new("Left")), ["caret.left"]);
        assert_eq!(
            mac.lookup(&Chord::parse("Ctrl+A", Platform::Mac).unwrap()),
            ["caret.lineStart"]
        );
        assert_eq!(
            mac.lookup(&Chord::parse("Cmd+A", Platform::Mac).unwrap()),
            ["select.all"]
        );
        let other = Keymap::new(&r, Platform::Other);
        assert_eq!(
            other.lookup(&Chord::parse("Ctrl+Y", Platform::Other).unwrap()),
            ["edit.redo"]
        );
        assert_eq!(
            other.lookup(&Chord::parse("Ctrl+Home", Platform::Other).unwrap()),
            ["nav.top"]
        );
        assert_eq!(
            other.lookup(&Chord::parse("Ctrl+A", Platform::Other).unwrap()),
            ["select.all"]
        );
        // no two commands share a default chord within one platform (no scopes yet in M2b)
        for km in [&mac, &other] {
            for (c, ids) in &km.map {
                assert_eq!(ids.len(), 1, "{c:?} → {ids:?}");
            }
        }
    }
}

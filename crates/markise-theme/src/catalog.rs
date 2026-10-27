//! The theme catalog shown in the Theme menu (ADR 0004): theme *families* with an optional
//! light and dark variant, so the appearance setting (light / dark / follow the system) can
//! pick the matching variant of the selected theme.
//!
//! - **Markise** — the original built-in light/dark themes (native, no CSS).
//! - **Built-in** — the vendored community themes ([`crate::builtin`]).
//! - **User** — themes in `<config>/markise/themes/` ([`crate::discover`]). A scheme-aware
//!   theme (`@media (prefers-color-scheme)`) is one family by itself; otherwise files whose
//!   stems differ only by a `-dark`/`-light`/`-night`/`-day` suffix (also `_` or `.`) are paired.

use std::path::{Path, PathBuf};

use crate::builtin::BUILTIN_THEMES;
use crate::discover::{ThemeEntry, ThemeSource};
use crate::import::display_name;

/// Identity of a theme family; its [`ThemeId::to_setting`] form is what the settings file
/// stores.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ThemeId {
    /// Markise Light / Markise Dark.
    Markise,
    /// A vendored theme, by [`crate::builtin::BuiltinTheme::id`].
    Builtin(String),
    /// A user theme family, by its key (lowercase stem without the light/dark suffix).
    User(String),
    /// A native palette theme family ([`crate::palettes::PALETTE_FAMILIES`], or
    /// `markise-night`).
    Palette(String),
}

impl ThemeId {
    pub fn to_setting(&self) -> String {
        match self {
            ThemeId::Markise => "markise".into(),
            ThemeId::Builtin(id) => format!("builtin:{id}"),
            ThemeId::User(key) => format!("user:{key}"),
            ThemeId::Palette(id) => format!("palette:{id}"),
        }
    }

    pub fn from_setting(s: &str) -> Option<ThemeId> {
        let s = s.trim();
        if s == "markise" {
            return Some(ThemeId::Markise);
        }
        let (kind, rest) = s.split_once(':')?;
        if rest.is_empty() || rest.len() > 256 {
            return None;
        }
        match kind {
            "builtin" => Some(ThemeId::Builtin(rest.to_string())),
            "user" => Some(ThemeId::User(rest.to_string())),
            "palette" => Some(ThemeId::Palette(rest.to_string())),
            _ => None,
        }
    }
}

/// Where the style (typography and layout, [`crate::Style`]) comes from (ADR 0007).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum StyleId {
    /// A built-in style ([`crate::style::BUILTIN_STYLES`]) by id.
    Builtin(String),
    /// The style of a complete (CSS) theme family.
    Theme(ThemeId),
}

impl Default for StyleId {
    fn default() -> Self {
        StyleId::Builtin(crate::style::DEFAULT_STYLE.into())
    }
}

impl StyleId {
    /// `style:<id>` or `theme:<theme setting>`.
    pub fn to_setting(&self) -> String {
        match self {
            StyleId::Builtin(id) => format!("style:{id}"),
            StyleId::Theme(t) => format!("theme:{}", t.to_setting()),
        }
    }

    pub fn from_setting(s: &str) -> Option<StyleId> {
        let s = s.trim();
        if let Some(id) = s.strip_prefix("style:") {
            return (!id.is_empty() && id.len() <= 64).then(|| StyleId::Builtin(id.to_string()));
        }
        let t = ThemeId::from_setting(s.strip_prefix("theme:")?)?;
        Some(StyleId::of_theme(&t))
    }

    /// The style a theme family brings along: CSS themes their own, native families (Markise,
    /// palettes) the default style.
    pub fn of_theme(t: &ThemeId) -> StyleId {
        match t {
            ThemeId::Builtin(_) | ThemeId::User(_) => StyleId::Theme(t.clone()),
            ThemeId::Markise | ThemeId::Palette(_) => StyleId::default(),
        }
    }

    pub fn is_default(&self) -> bool {
        *self == StyleId::default()
    }
}

/// What the Theme menu has selected: a style and a color family (ADR 0007). A *complete theme*
/// is the selection `(StyleId::of_theme(t), t)`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Selection {
    pub style: StyleId,
    pub colors: ThemeId,
}

impl Default for Selection {
    fn default() -> Self {
        Selection::complete(ThemeId::Markise)
    }
}

impl Selection {
    /// A complete theme: its own style and colors.
    pub fn complete(t: ThemeId) -> Selection {
        Selection {
            style: StyleId::of_theme(&t),
            colors: t,
        }
    }

    /// The complete theme this selection is, if any.
    pub fn complete_theme(&self) -> Option<&ThemeId> {
        (self.style == StyleId::of_theme(&self.colors)).then_some(&self.colors)
    }

    /// From the settings values (`style`, `colors`); unknown values fall back to the defaults.
    pub fn from_settings(style: &str, colors: &str) -> Selection {
        Selection {
            style: StyleId::from_setting(style).unwrap_or_default(),
            colors: ThemeId::from_setting(colors).unwrap_or(ThemeId::Markise),
        }
    }
}

/// Where a variant's theme comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VariantSource {
    /// A native theme by id ([`crate::Theme::native`]): Markise Light/Dark or a palette theme.
    Native { id: String },
    /// A CSS file imported by the importer; `dark_variant` selects the
    /// `prefers-color-scheme: dark` variant of a scheme-aware file.
    Css { path: PathBuf, dark_variant: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Markise,
    Builtin,
    Palette,
    User,
}

/// One menu entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeFamily {
    pub id: ThemeId,
    /// Menu label.
    pub name: String,
    pub origin: Origin,
    pub light: Option<VariantSource>,
    pub dark: Option<VariantSource>,
}

impl ThemeFamily {
    /// The variant for the wanted appearance; falls back to the other variant when the family
    /// has only one.
    pub fn pick(&self, dark: bool) -> &VariantSource {
        let (want, other) = if dark {
            (&self.dark, &self.light)
        } else {
            (&self.light, &self.dark)
        };
        want.as_ref()
            .or(other.as_ref())
            .expect("a family has at least one variant")
    }

    /// Has both a light and a dark variant.
    pub fn has_both(&self) -> bool {
        self.light.is_some() && self.dark.is_some()
    }

    /// CSS files of this family (for live reload matching).
    pub fn css_paths(&self) -> Vec<&Path> {
        let mut v = Vec::new();
        for s in [&self.light, &self.dark].into_iter().flatten() {
            if let VariantSource::Css { path, .. } = s
                && !v.contains(&path.as_path())
            {
                v.push(path.as_path());
            }
        }
        v
    }
}

/// The Markise family (always first in the menu, the default).
pub fn markise_family() -> ThemeFamily {
    ThemeFamily {
        id: ThemeId::Markise,
        name: "Markise".into(),
        origin: Origin::Markise,
        light: Some(VariantSource::Native {
            id: "markise-light".into(),
        }),
        dark: Some(VariantSource::Native {
            id: "markise-dark".into(),
        }),
    }
}

/// Markise Night: the original signature dark theme (dark only, listed with Markise).
pub fn markise_night_family() -> ThemeFamily {
    ThemeFamily {
        id: ThemeId::Palette("markise-night".into()),
        name: "Markise Night".into(),
        origin: Origin::Markise,
        light: None,
        dark: Some(VariantSource::Native {
            id: "markise-night".into(),
        }),
    }
}

/// The palette pack families (original themes from published palettes), in menu order.
pub fn palette_families() -> Vec<ThemeFamily> {
    crate::palettes::PALETTE_FAMILIES
        .iter()
        .map(|f| ThemeFamily {
            id: ThemeId::Palette(f.id.to_string()),
            name: f.name.to_string(),
            origin: Origin::Palette,
            light: f.light.map(|id| VariantSource::Native { id: id.into() }),
            dark: f.dark.map(|id| VariantSource::Native { id: id.into() }),
        })
        .collect()
}

/// Families of the vendored themes, extracted below `extracted` (see
/// [`crate::builtin::extract`]).
pub fn builtin_families(extracted: &Path) -> Vec<ThemeFamily> {
    BUILTIN_THEMES
        .iter()
        .map(|t| {
            let dir = extracted.join(t.id);
            ThemeFamily {
                id: ThemeId::Builtin(t.id.to_string()),
                name: t.name.to_string(),
                origin: Origin::Builtin,
                light: Some(VariantSource::Css {
                    path: dir.join(t.light),
                    dark_variant: false,
                }),
                dark: Some(VariantSource::Css {
                    path: dir.join(t.dark),
                    dark_variant: false,
                }),
            }
        })
        .collect()
}

/// Splits a theme stem into its family key and the appearance its suffix names, if any:
/// `solar-dark` → (`solar`, Some(true)), `Solar_Light` → (`solar`, Some(false)).
pub fn family_key(stem: &str) -> (String, Option<bool>) {
    let lower = stem.to_lowercase();
    for (suffix, dark) in [("dark", true), ("night", true), ("light", false), ("day", false)] {
        for sep in ['-', '_', '.', ' '] {
            if let Some(base) = lower.strip_suffix(suffix)
                && let Some(base) = base.strip_suffix(sep)
                && !base.is_empty()
            {
                return (base.to_string(), Some(dark));
            }
        }
    }
    (lower, None)
}

/// Groups discovered user themes into families (see the module docs). Entries that do not
/// pair (two light themes with the same key, …) become families of their own.
pub fn user_families(entries: &[ThemeEntry]) -> Vec<ThemeFamily> {
    let mut out: Vec<ThemeFamily> = Vec::new();
    let mut groups: Vec<(String, Vec<&ThemeEntry>)> = Vec::new();
    for e in entries.iter().filter(|e| e.source == ThemeSource::User) {
        if e.scheme_aware {
            out.push(ThemeFamily {
                id: ThemeId::User(e.stem.to_lowercase()),
                name: e.name.clone(),
                origin: Origin::User,
                light: Some(VariantSource::Css {
                    path: e.path.clone(),
                    dark_variant: false,
                }),
                dark: Some(VariantSource::Css {
                    path: e.path.clone(),
                    dark_variant: true,
                }),
            });
            continue;
        }
        let (key, _) = family_key(&e.stem);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(e),
            None => groups.push((key, vec![e])),
        }
    }
    let single = |e: &ThemeEntry| ThemeFamily {
        id: ThemeId::User(e.stem.to_lowercase()),
        name: e.name.clone(),
        origin: Origin::User,
        light: (!e.dark).then(|| VariantSource::Css {
            path: e.path.clone(),
            dark_variant: false,
        }),
        dark: e.dark.then(|| VariantSource::Css {
            path: e.path.clone(),
            dark_variant: false,
        }),
    };
    for (key, members) in groups {
        let lights: Vec<&&ThemeEntry> = members.iter().filter(|e| !e.dark).collect();
        let darks: Vec<&&ThemeEntry> = members.iter().filter(|e| e.dark).collect();
        if members.len() == 2 && lights.len() == 1 && darks.len() == 1 {
            let (l, d) = (lights[0], darks[0]);
            let name = if family_key(&l.stem).1.is_none() {
                l.name.clone()
            } else {
                display_name(&key)
            };
            out.push(ThemeFamily {
                id: ThemeId::User(key.clone()),
                name,
                origin: Origin::User,
                light: Some(VariantSource::Css {
                    path: l.path.clone(),
                    dark_variant: false,
                }),
                dark: Some(VariantSource::Css {
                    path: d.path.clone(),
                    dark_variant: false,
                }),
            });
        } else {
            out.extend(members.iter().map(|e| single(e)));
        }
    }
    // ids must be unique: a paired key may equal a single theme's stem in rare layouts
    let mut seen = std::collections::HashSet::new();
    out.retain(|f| seen.insert(f.id.clone()));
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(stem: &str, dark: bool, scheme_aware: bool) -> ThemeEntry {
        ThemeEntry {
            name: display_name(stem),
            stem: stem.into(),
            path: PathBuf::from(format!("/t/{stem}.css")),
            dark,
            scheme_aware,
            source: ThemeSource::User,
        }
    }

    #[test]
    fn setting_round_trip() {
        for id in [
            ThemeId::Markise,
            ThemeId::Builtin("drake".into()),
            ThemeId::User("solar".into()),
            ThemeId::Palette("nord".into()),
        ] {
            assert_eq!(ThemeId::from_setting(&id.to_setting()), Some(id));
        }
        assert_eq!(ThemeId::from_setting("nonsense"), None);
        assert_eq!(ThemeId::from_setting("user:"), None);
    }

    #[test]
    fn style_setting_round_trip_and_complete_themes() {
        for id in [
            StyleId::Builtin("classic".into()),
            StyleId::Theme(ThemeId::Builtin("drake".into())),
            StyleId::Theme(ThemeId::User("solar".into())),
        ] {
            assert_eq!(StyleId::from_setting(&id.to_setting()), Some(id));
        }
        // native families have no style of their own
        assert_eq!(StyleId::from_setting("theme:palette:nord"), Some(StyleId::default()));
        assert_eq!(StyleId::from_setting("theme:markise"), Some(StyleId::default()));
        assert_eq!(StyleId::from_setting("style:"), None);
        assert_eq!(StyleId::from_setting("classic"), None);
        let drake = ThemeId::Builtin("drake".into());
        assert_eq!(Selection::complete(drake.clone()).complete_theme(), Some(&drake));
        let mixed = Selection {
            style: StyleId::Builtin("classic".into()),
            colors: drake.clone(),
        };
        assert_eq!(mixed.complete_theme(), None);
        assert_eq!(
            Selection::complete(ThemeId::Palette("nord".into())).style,
            StyleId::default()
        );
        assert_eq!(Selection::from_settings("bogus", "nope"), Selection::default());
    }

    #[test]
    fn family_keys() {
        assert_eq!(family_key("solar-dark"), ("solar".into(), Some(true)));
        assert_eq!(family_key("Solar_Light"), ("solar".into(), Some(false)));
        assert_eq!(family_key("night"), ("night".into(), None));
        assert_eq!(family_key("paper"), ("paper".into(), None));
    }

    #[test]
    fn pairs_light_and_dark_files() {
        let v = user_families(&[
            entry("solar", false, false),
            entry("solar-dark", true, false),
            entry("night", true, false),
            entry("auto", false, true),
            entry("twin", false, false),
            entry("twin-light", false, false),
        ]);
        let names: Vec<_> = v.iter().map(|f| (f.name.as_str(), f.has_both())).collect();
        assert_eq!(
            names,
            vec![
                ("Auto", true),
                ("Night", false),
                ("Solar", true),
                ("Twin", false),
                ("Twin Light", false)
            ]
        );
        let solar = v.iter().find(|f| f.name == "Solar").unwrap();
        assert_eq!(
            solar.pick(true),
            &VariantSource::Css {
                path: "/t/solar-dark.css".into(),
                dark_variant: false
            }
        );
        let night = v.iter().find(|f| f.name == "Night").unwrap();
        // only a dark variant: light mode still shows it
        assert_eq!(night.pick(false), night.pick(true));
        let auto = v.iter().find(|f| f.name == "Auto").unwrap();
        assert!(matches!(auto.pick(true), VariantSource::Css { dark_variant: true, .. }));
    }
}

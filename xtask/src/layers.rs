//! `cargo xtask layers`: enforce the crate layering of AGENTS.md ("Architecture").
//!
//! Derived from WordCraft's `xtask/src/layers.rs` (https://github.com/storytold/wordcraft, rev
//! 6a2b520fa49aa0d0595cea36589aaf16ca371cd3, MIT OR Apache-2.0, Copyright (c) 2026 ArtCraft Team
//! and the WordCraft contributors; see NOTICE). Changes for Markise: the layer table is read
//! from `layers.toml` instead of being compiled in; packages under excluded paths (spikes) are
//! skipped; apps skip only the layer order and the UI rule (the test tool stays dev-only and
//! nothing may depend on an app); path dependencies outside the workspace are reported; UI
//! crates are matched by name whatever their source; and `layers.toml` is cross-checked against
//! the AGENTS.md table.
//!
//! The rule engine works on a small, metadata-independent model so it can be unit-tested;
//! `from_metadata` builds that model from `cargo metadata`.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

/// Where a workspace crate sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Regular layered crate.
    Layer(u8),
    /// Test tooling: may depend on anything below the UI layer; other crates may use it only as
    /// a dev-dependency.
    Testkit,
    /// Binary on top of the stack: may depend on anything, nothing may depend on it.
    App,
    /// Build tooling: exempt from the rules.
    Tool,
}

/// The parsed `layers.toml`.
#[derive(Debug, Clone)]
pub struct LayerMap {
    pub classes: BTreeMap<String, Class>,
    /// Chains of crates within one layer; a crate may use the ones listed before it.
    pub intra_layer: Vec<Vec<String>>,
    pub ui_crates: Vec<String>,
    pub ui_min_layer: u8,
    pub exclude_paths: Vec<String>,
    /// Format crates (`[format_seam] formats`) …
    pub formats: Vec<String>,
    /// … that these format-neutral crates may use only as dev-dependencies (ADR 0006 §1.5).
    pub format_neutral: Vec<String>,
}

impl LayerMap {
    pub fn parse(text: &str) -> Result<Self, String> {
        let t: toml::Table = text.parse().map_err(|e| format!("layers.toml: {e}"))?;
        let ui_min_layer = t
            .get("ui_min_layer")
            .and_then(toml::Value::as_integer)
            .and_then(|n| u8::try_from(n).ok())
            .ok_or("layers.toml: `ui_min_layer` must be a small integer")?;
        let strings = |key: &str| -> Result<Vec<String>, String> {
            match t.get(key) {
                None => Ok(Vec::new()),
                Some(v) => string_array(v).ok_or(format!("layers.toml: `{key}` must be an array of strings")),
            }
        };
        let ui_crates = strings("ui_crates")?;
        let exclude_paths = strings("exclude_paths")?;

        let mut classes = BTreeMap::new();
        for (name, v) in table(&t, "layers")? {
            let layer = v
                .as_integer()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or(format!("layers.toml: [layers] {name} must be a layer number"))?;
            classes.insert(name.clone(), Class::Layer(layer));
        }
        for (name, v) in table(&t, "roles")? {
            let class = match v.as_str() {
                Some("app") => Class::App,
                Some("testkit") => Class::Testkit,
                Some("tool") => Class::Tool,
                _ => {
                    return Err(format!(
                        "layers.toml: [roles] {name} must be \"app\", \"testkit\" or \"tool\""
                    ));
                }
            };
            if classes.insert(name.clone(), class).is_some() {
                return Err(format!("layers.toml: {name} is listed in both [layers] and [roles]"));
            }
        }

        let mut intra_layer = Vec::new();
        for (label, v) in table(&t, "intra_layer")? {
            let chain = string_array(v).ok_or(format!("layers.toml: [intra_layer] {label} must be an array"))?;
            for name in &chain {
                if classes
                    .get(name)
                    .map(|c| format!("L{}", layer_of(*c, ui_min_layer).unwrap_or(u8::MAX)))
                    != Some(label.clone())
                {
                    return Err(format!(
                        "layers.toml: [intra_layer] {label}: {name} is not a crate of layer {label}"
                    ));
                }
            }
            intra_layer.push(chain);
        }
        let (mut formats, mut format_neutral) = (Vec::new(), Vec::new());
        for (key, v) in table(&t, "format_seam")? {
            let list = string_array(v).ok_or(format!("layers.toml: [format_seam] {key} must be an array"))?;
            match key.as_str() {
                "formats" => formats = list,
                "neutral" => format_neutral = list,
                _ => return Err(format!("layers.toml: [format_seam] unknown key {key}")),
            }
        }
        for name in formats.iter().chain(&format_neutral) {
            if !classes.contains_key(name) {
                return Err(format!("layers.toml: [format_seam] {name} is not a listed crate"));
            }
        }
        Ok(LayerMap {
            classes,
            intra_layer,
            ui_crates,
            ui_min_layer,
            exclude_paths,
            formats,
            format_neutral,
        })
    }

    pub fn classify(&self, pkg: &str) -> Option<Class> {
        self.classes.get(pkg).copied()
    }

    fn layer(&self, class: Class) -> Option<u8> {
        layer_of(class, self.ui_min_layer)
    }

    fn intra_layer_allowed(&self, from: &str, to: &str) -> bool {
        self.intra_layer.iter().any(|chain| {
            match (chain.iter().position(|n| n == from), chain.iter().position(|n| n == to)) {
                (Some(f), Some(t)) => t < f,
                _ => false,
            }
        })
    }

    fn is_ui_crate(&self, name: &str) -> bool {
        self.ui_crates.iter().any(|p| match p.strip_suffix('*') {
            Some(prefix) => name.starts_with(prefix),
            None => name == p,
        })
    }

    fn excluded(&self, manifest: &str) -> bool {
        self.exclude_paths.iter().any(|p| manifest.starts_with(p.as_str()))
    }
}

fn layer_of(class: Class, ui_min_layer: u8) -> Option<u8> {
    match class {
        Class::Layer(l) => Some(l),
        // Sits at the UI layer for rule purposes: may use everything below it.
        Class::Testkit => Some(ui_min_layer),
        Class::App | Class::Tool => None,
    }
}

fn table<'a>(t: &'a toml::Table, key: &str) -> Result<Vec<(&'a String, &'a toml::Value)>, String> {
    match t.get(key) {
        None => Ok(Vec::new()),
        Some(toml::Value::Table(tab)) => Ok(tab.iter().collect()),
        Some(_) => Err(format!("layers.toml: `{key}` must be a table")),
    }
}

fn string_array(v: &toml::Value) -> Option<Vec<String>> {
    v.as_array()?.iter().map(|s| s.as_str().map(str::to_owned)).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Normal,
    Dev,
    Build,
}

#[derive(Debug, Clone)]
pub struct Dep {
    pub name: String,
    pub kind: DepKind,
    /// `true` if the dependency is a workspace member.
    pub workspace: bool,
    /// `true` if the dependency is given by `path` (a member or an outside checkout).
    pub path: bool,
}

#[derive(Debug, Clone)]
pub struct Crate {
    pub name: String,
    /// Manifest path relative to the workspace root (`crates/x/Cargo.toml`).
    pub manifest: String,
    pub deps: Vec<Dep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    Unregistered {
        krate: String,
    },
    Upward {
        krate: String,
        dep: String,
        from: u8,
        to: u8,
        kind: DepKind,
    },
    OutsidePath {
        krate: String,
        dep: String,
    },
    OnApp {
        krate: String,
        dep: String,
    },
    TestkitAsNormalDep {
        krate: String,
        dep: String,
    },
    UiBelowMin {
        krate: String,
        dep: String,
        layer: u8,
        min: u8,
    },
    FormatInNeutral {
        krate: String,
        dep: String,
    },
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Violation::Unregistered { krate } => {
                write!(
                    f,
                    "{krate}: unknown workspace crate; add it to layers.toml and the AGENTS.md table"
                )
            }
            Violation::Upward {
                krate,
                dep,
                from,
                to,
                kind,
            } => write!(
                f,
                "{krate} (L{from}) -> {dep} (L{to}) [{kind:?}]: may only depend on lower layers \
                 (or an earlier crate of an [intra_layer] chain in layers.toml)"
            ),
            Violation::OutsidePath { krate, dep } => write!(
                f,
                "{krate} -> {dep}: path dependency outside the workspace escapes the layer check; \
                 make it a workspace member (and register it) or use a registry/git dependency"
            ),
            Violation::OnApp { krate, dep } => {
                write!(f, "{krate} -> {dep}: apps sit on top; nothing may depend on them")
            }
            Violation::TestkitAsNormalDep { krate, dep } => {
                write!(f, "{krate} -> {dep}: test tooling may only be a dev-dependency")
            }
            Violation::FormatInNeutral { krate, dep } => write!(
                f,
                "{krate} -> {dep}: format-neutral crates read documents through the markise-doc seam; \
                 a format crate is allowed only as a dev-dependency ([format_seam] in layers.toml)"
            ),
            Violation::UiBelowMin { krate, dep, layer, min } => {
                write!(
                    f,
                    "{krate} (L{layer}) depends on UI crate `{dep}`; UI toolkits are only allowed in L{min}+"
                )
            }
        }
    }
}

/// Check all rules. Returns violations sorted for stable output.
pub fn check(map: &LayerMap, crates: &[Crate]) -> Vec<Violation> {
    let mut out = Vec::new();
    for c in crates.iter().filter(|c| !map.excluded(&c.manifest)) {
        let Some(class) = map.classify(&c.name) else {
            out.push(Violation::Unregistered { krate: c.name.clone() });
            continue;
        };
        if class == Class::Tool {
            continue; // build tooling may depend on anything
        }
        // Apps have no layer: they skip the layer order and the UI rule, but not the rest.
        let layer = map.layer(class);
        for d in &c.deps {
            // Self dev-dependencies (e.g. to enable features in tests) are fine.
            if d.name == c.name {
                continue;
            }
            // By name, whatever the source (registry, git or an outside path checkout).
            if let Some(layer) = layer
                && layer < map.ui_min_layer
                && map.is_ui_crate(&d.name)
            {
                out.push(Violation::UiBelowMin {
                    krate: c.name.clone(),
                    dep: d.name.clone(),
                    layer,
                    min: map.ui_min_layer,
                });
            }
            if d.kind != DepKind::Dev && map.format_neutral.contains(&c.name) && map.formats.contains(&d.name) {
                out.push(Violation::FormatInNeutral {
                    krate: c.name.clone(),
                    dep: d.name.clone(),
                });
            }
            if !d.workspace {
                if d.path {
                    out.push(Violation::OutsidePath {
                        krate: c.name.clone(),
                        dep: d.name.clone(),
                    });
                }
                continue;
            }
            match map.classify(&d.name) {
                Some(Class::Testkit) if d.kind != DepKind::Dev => {
                    out.push(Violation::TestkitAsNormalDep {
                        krate: c.name.clone(),
                        dep: d.name.clone(),
                    });
                }
                // Unregistered deps are reported on their own entry; testkit dev deps are allowed.
                None | Some(Class::Testkit | Class::Tool) => {}
                Some(Class::App) => out.push(Violation::OnApp {
                    krate: c.name.clone(),
                    dep: d.name.clone(),
                }),
                Some(dc) => {
                    let Some(layer) = layer else { continue };
                    let to = map.layer(dc).unwrap_or(u8::MAX);
                    if to >= layer && !(to == layer && map.intra_layer_allowed(&c.name, &d.name)) {
                        out.push(Violation::Upward {
                            krate: c.name.clone(),
                            dep: d.name.clone(),
                            from: layer,
                            to,
                            kind: d.kind,
                        });
                    }
                }
            }
        }
    }
    out.sort_by_key(ToString::to_string);
    out.dedup();
    out
}

/// Build the model from `cargo metadata --format-version 1 --no-deps`.
pub fn from_metadata(meta: &Value) -> Result<Vec<Crate>, String> {
    let pkgs = meta["packages"].as_array().ok_or("metadata: no packages array")?;
    let root = meta["workspace_root"].as_str().unwrap_or_default();
    let members: Vec<&str> = pkgs.iter().filter_map(|p| p["name"].as_str()).collect();
    let mut out = Vec::new();
    for p in pkgs {
        let name = p["name"].as_str().ok_or("package without name")?.to_owned();
        let manifest_abs = p["manifest_path"].as_str().unwrap_or_default();
        let manifest = Path::new(manifest_abs)
            .strip_prefix(root)
            .map(|m| m.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| manifest_abs.to_owned());
        let mut deps = Vec::new();
        for d in p["dependencies"].as_array().into_iter().flatten() {
            // `rename = "x"` is irrelevant: the rules are about packages.
            let dname = d["name"].as_str().unwrap_or_default().to_owned();
            let kind = match d["kind"].as_str() {
                Some("dev") => DepKind::Dev,
                Some("build") => DepKind::Build,
                _ => DepKind::Normal,
            };
            let path = d["path"].is_string();
            // With `--no-deps` the packages are exactly the workspace members; a path dependency
            // on anything else is an outside checkout.
            let workspace = path && members.contains(&dname.as_str());
            deps.push(Dep {
                name: dname,
                kind,
                workspace,
                path,
            });
        }
        out.push(Crate { name, manifest, deps });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// The crate → layer label ("L2", "apps", "tests") table from the AGENTS.md "Architecture"
/// section: every table row whose first cell is a layer label, with the backticked crate names
/// of its second cell.
pub fn agents_table(agents_md: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let section = agents_md.split("\n## Architecture").nth(1).unwrap_or_default();
    let section = section.split("\n## ").next().unwrap_or_default();
    for line in section.lines() {
        let cells: Vec<&str> = line.trim().trim_matches('|').split('|').map(str::trim).collect();
        let [label, names, ..] = cells[..] else { continue };
        let is_layer = label
            .strip_prefix('L')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        if !(is_layer || label == "apps" || label == "tests") {
            continue;
        }
        for name in names.split('`').skip(1).step_by(2) {
            out.insert(name.to_owned(), label.to_owned());
        }
    }
    out
}

/// Differences between `layers.toml` and the AGENTS.md table (tools are not documented there).
pub fn agents_mismatches(map: &LayerMap, agents: &BTreeMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    for (name, class) in &map.classes {
        let want = match class {
            Class::Layer(l) => format!("L{l}"),
            Class::App => "apps".into(),
            Class::Testkit => "tests".into(),
            Class::Tool => continue,
        };
        match agents.get(name) {
            None => out.push(format!(
                "{name}: in layers.toml ({want}) but not in the AGENTS.md table"
            )),
            Some(got) if *got != want => {
                out.push(format!("{name}: layers.toml says {want}, AGENTS.md says {got}"));
            }
            Some(_) => {}
        }
    }
    for (name, got) in agents {
        if !map.classes.contains_key(name) {
            out.push(format!("{name}: in the AGENTS.md table ({got}) but not in layers.toml"));
        }
    }
    out
}

pub fn describe(class: Option<Class>) -> String {
    match class {
        Some(Class::Layer(l)) => format!("L{l}"),
        Some(Class::Testkit) => "testkit".into(),
        Some(Class::App) => "app".into(),
        Some(Class::Tool) => "tool".into(),
        None => "UNREGISTERED".into(),
    }
}

/// Run the check on the workspace at `root` with `cargo metadata` output `meta`.
pub fn run(root: &Path, meta: &Value) -> Result<(), String> {
    let map = LayerMap::parse(&read(root, "layers.toml")?)?;
    let crates = from_metadata(meta)?;
    let mut errors: Vec<String> = check(&map, &crates).iter().map(ToString::to_string).collect();
    errors.extend(agents_mismatches(&map, &agents_table(&read(root, "AGENTS.md")?)));
    for c in crates.iter().filter(|c| !map.excluded(&c.manifest)) {
        println!("  {:<18} {}", c.name, describe(map.classify(&c.name)));
    }
    if errors.is_empty() {
        println!(
            "layers: OK ({} workspace crates, layers.toml matches AGENTS.md)",
            crates.len()
        );
        Ok(())
    } else {
        Err(format!(
            "{} layering problem(s):\n  {}",
            errors.len(),
            errors.join("\n  ")
        ))
    }
}

fn read(root: &Path, name: &str) -> Result<String, String> {
    std::fs::read_to_string(root.join(name)).map_err(|e| format!("{name}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use DepKind::*;

    const MAP: &str = r#"
        ui_min_layer = 6
        ui_crates = ["egui", "eframe", "winit", "egui_kittest", "rfd", "bevy*"]
        exclude_paths = ["spikes/"]
        [layers]
        markise-geom = 0
        markise-md = 1
        markise-layout = 2
        markise-theme = 2
        markise-blocks = 2
        markise-export = 3
        markise-engine = 4
        markise-mcp = 5
        markise-ui-egui = 6
        [roles]
        markise = "app"
        markise-cli = "app"
        markise-corpus = "testkit"
        xtask = "tool"
        [intra_layer]
        L2 = ["markise-theme", "markise-layout"]
        [format_seam]
        formats = ["markise-md"]
        neutral = ["markise-layout"]
    "#;

    fn map() -> LayerMap {
        LayerMap::parse(MAP).unwrap()
    }

    fn c(name: &str, deps: &[(&str, DepKind, bool)]) -> Crate {
        Crate {
            name: name.into(),
            manifest: format!("crates/{name}/Cargo.toml"),
            deps: deps
                .iter()
                .map(|(n, k, w)| Dep {
                    name: (*n).into(),
                    kind: *k,
                    workspace: *w,
                    path: *w,
                })
                .collect(),
        }
    }

    #[test]
    fn format_crates_are_dev_only_in_neutral_crates() {
        let v = check(&map(), &[c("markise-layout", &[("markise-md", Normal, true)])]);
        assert_eq!(
            v,
            vec![Violation::FormatInNeutral {
                krate: "markise-layout".into(),
                dep: "markise-md".into()
            }]
        );
        assert!(check(&map(), &[c("markise-layout", &[("markise-md", Dev, true)])]).is_empty());
        // other crates may use the format
        assert!(check(&map(), &[c("markise-engine", &[("markise-md", Normal, true)])]).is_empty());
        assert!(LayerMap::parse(&MAP.replace("neutral = [\"markise-layout\"]", "neutral = [\"nope\"]")).is_err());
    }

    #[test]
    fn clean_downward_graph_passes() {
        let g = [
            c("markise-geom", &[("memchr", Normal, false)]),
            c(
                "markise-md",
                &[("markise-geom", Normal, true), ("markise-corpus", Dev, true)],
            ),
            c("markise-engine", &[("markise-md", Normal, true)]),
            c(
                "markise-ui-egui",
                &[("markise-engine", Normal, true), ("egui", Normal, false)],
            ),
            c(
                "markise",
                &[("markise-ui-egui", Normal, true), ("eframe", Normal, false)],
            ),
            c("markise-corpus", &[]),
        ];
        assert!(check(&map(), &g).is_empty(), "{:?}", check(&map(), &g));
    }

    #[test]
    fn upward_dependency_flagged() {
        let v = check(&map(), &[c("markise-md", &[("markise-engine", Normal, true)])]);
        assert!(matches!(v[..], [Violation::Upward { from: 1, to: 4, .. }]), "{v:?}");
    }

    #[test]
    fn sideways_dependency_flagged_unless_chained() {
        let v = check(&map(), &[c("markise-layout", &[("markise-blocks", Normal, true)])]);
        assert!(matches!(v[..], [Violation::Upward { from: 2, to: 2, .. }]), "{v:?}");
        // The chain allows theme -> layout order only one way.
        assert!(check(&map(), &[c("markise-layout", &[("markise-theme", Normal, true)])]).is_empty());
        let v = check(&map(), &[c("markise-theme", &[("markise-layout", Normal, true)])]);
        assert!(matches!(v[..], [Violation::Upward { from: 2, to: 2, .. }]), "{v:?}");
    }

    #[test]
    fn self_dev_dependency_ignored() {
        assert!(check(&map(), &[c("markise-md", &[("markise-md", Dev, true)])]).is_empty());
    }

    #[test]
    fn upward_dev_and_build_dependencies_flagged() {
        let v = check(
            &map(),
            &[c(
                "markise-geom",
                &[("markise-md", Dev, true), ("markise-layout", Build, true)],
            )],
        );
        assert!(
            matches!(
                v[..],
                // sorted by message: markise-layout before markise-md
                [
                    Violation::Upward { kind: Build, .. },
                    Violation::Upward { kind: Dev, .. }
                ]
            ),
            "{v:?}"
        );
    }

    #[test]
    fn ui_crates_below_ui_layer_flagged() {
        for dep in ["egui", "eframe", "winit", "egui_kittest", "rfd", "bevy_ecs"] {
            let v = check(&map(), &[c("markise-engine", &[(dep, Normal, false)])]);
            assert!(
                matches!(v[..], [Violation::UiBelowMin { layer: 4, min: 6, .. }]),
                "{dep}: {v:?}"
            );
        }
        // epaint (text shaping in markise-layout until ADR 0005) is not a UI toolkit.
        assert!(check(&map(), &[c("markise-layout", &[("epaint", Normal, false)])]).is_empty());
        assert!(!check(&map(), &[c("markise-mcp", &[("winit", Dev, false)])]).is_empty());
    }

    #[test]
    fn unregistered_crate_is_error() {
        let v = check(&map(), &[c("markise-mystery", &[])]);
        assert!(matches!(&v[..], [Violation::Unregistered { krate }] if krate == "markise-mystery"));
        assert!(v[0].to_string().contains("layers.toml"));
    }

    #[test]
    fn testkit_only_as_dev_dependency() {
        let v = check(&map(), &[c("markise-export", &[("markise-corpus", Normal, true)])]);
        assert!(matches!(v[..], [Violation::TestkitAsNormalDep { .. }]), "{v:?}");
        assert!(check(&map(), &[c("markise-export", &[("markise-corpus", Dev, true)])]).is_empty());
        // The testkit may use layers below the UI layer, not the UI layer itself.
        assert!(check(&map(), &[c("markise-corpus", &[("markise-engine", Normal, true)])]).is_empty());
        assert!(!check(&map(), &[c("markise-corpus", &[("markise-ui-egui", Normal, true)])]).is_empty());
    }

    #[test]
    fn nothing_may_depend_on_an_app() {
        let v = check(&map(), &[c("markise-ui-egui", &[("markise-cli", Normal, true)])]);
        assert!(matches!(v[..], [Violation::OnApp { .. }]), "{v:?}");
    }

    #[test]
    fn apps_skip_layer_order_and_ui_rule_only() {
        for app in ["markise", "markise-cli"] {
            let g = [c(
                app,
                &[
                    ("egui", Normal, false),
                    ("markise-ui-egui", Normal, true),
                    ("markise-md", Normal, true),
                    ("markise-corpus", Dev, true),
                ],
            )];
            assert!(check(&map(), &g).is_empty(), "{app}");
        }
        // The test tool stays dev-only for apps too.
        let v = check(&map(), &[c("markise", &[("markise-corpus", Normal, true)])]);
        assert!(matches!(v[..], [Violation::TestkitAsNormalDep { .. }]), "{v:?}");
        let v = check(&map(), &[c("markise-cli", &[("markise-corpus", Build, true)])]);
        assert!(matches!(v[..], [Violation::TestkitAsNormalDep { .. }]), "{v:?}");
        // No app depends on another app.
        let v = check(&map(), &[c("markise", &[("markise-cli", Normal, true)])]);
        assert!(matches!(v[..], [Violation::OnApp { .. }]), "{v:?}");
        // Build tooling is exempt.
        let g = [c(
            "xtask",
            &[
                ("egui", Normal, false),
                ("markise-corpus", Normal, true),
                ("markise", Normal, true),
            ],
        )];
        assert!(check(&map(), &g).is_empty());
    }

    #[test]
    fn ui_rule_applies_to_path_dependencies_outside_the_workspace() {
        let outside_egui = Dep {
            name: "egui".into(),
            kind: Normal,
            workspace: false,
            path: true,
        };
        let mut md = c("markise-md", &[]);
        md.deps.push(outside_egui.clone());
        let v = check(&map(), &[md]);
        assert!(
            matches!(
                v[..],
                [Violation::UiBelowMin { layer: 1, .. }, Violation::OutsidePath { .. }]
            ),
            "{v:?}"
        );
        // Any outside path dependency is reported, also for apps.
        let mut app = c("markise", &[]);
        app.deps.push(Dep {
            name: "vendored-thing".into(),
            ..outside_egui
        });
        assert!(matches!(check(&map(), &[app])[..], [Violation::OutsidePath { .. }]));
    }

    #[test]
    fn excluded_paths_skipped() {
        let mut spike = c("spike-parley-text", &[("egui", Normal, false)]);
        spike.manifest = "spikes/adr-0005/parley-text/Cargo.toml".into();
        assert!(check(&map(), &[spike]).is_empty());
    }

    #[test]
    fn violating_fake_metadata_fails() {
        let meta: Value = serde_json::from_str(
            r#"{"workspace_root":"/w","packages":[
                {"name":"markise-md","manifest_path":"/w/crates/markise-md/Cargo.toml","dependencies":[
                    {"name":"markise-layout","kind":null,"path":"/w/crates/markise-layout"},
                    {"name":"memchr","kind":null},
                    {"name":"proptest","kind":"dev"}]},
                {"name":"markise-layout","manifest_path":"/w/crates/markise-layout/Cargo.toml","dependencies":[
                    {"name":"markise-md","kind":null,"path":"/w/crates/markise-md"},
                    {"name":"egui","kind":null,"path":"/elsewhere/egui/crates/egui"}]}
            ]}"#,
        )
        .unwrap();
        let g = from_metadata(&meta).unwrap();
        assert_eq!(g[1].manifest, "crates/markise-md/Cargo.toml");
        let md = g.iter().find(|c| c.name == "markise-md").unwrap();
        assert!(md.deps[0].workspace && !md.deps[1].workspace);
        assert_eq!(md.deps[2].kind, Dev);
        let layout = g.iter().find(|c| c.name == "markise-layout").unwrap();
        assert!(!layout.deps[1].workspace && layout.deps[1].path, "{:?}", layout.deps[1]);
        let v = check(&map(), &g);
        assert!(
            matches!(
                v[..],
                [
                    Violation::UiBelowMin { layer: 2, .. },
                    Violation::OutsidePath { .. },
                    Violation::FormatInNeutral { .. },
                    Violation::Upward { from: 1, to: 2, .. }
                ]
            ),
            "{v:?}"
        );
    }

    #[test]
    fn map_parse_errors() {
        assert!(LayerMap::parse("ui_min_layer = 6\n[layers]\na = \"two\"").is_err());
        assert!(LayerMap::parse("ui_min_layer = 6\n[layers]\na = 1\n[roles]\na = \"app\"").is_err());
        assert!(LayerMap::parse("ui_min_layer = 6\n[roles]\na = \"bin\"").is_err());
        // An intra-layer chain naming a crate of another layer.
        assert!(
            LayerMap::parse("ui_min_layer = 6\n[layers]\na = 1\nb = 2\n[intra_layer]\nL2 = [\"a\", \"b\"]").is_err()
        );
        assert!(LayerMap::parse("[layers]\na = 1").is_err());
    }

    #[test]
    fn agents_table_parsed_and_compared() {
        let md = "# X\n\n## Architecture\n\n| Layer | Crate | Job |\n|---|---|---|\n\
                  | L0 | `markise-geom` | units |\n| L3 | `markise-export`, `markise-render` | out |\n\
                  | apps | `markise` | app |\n| tests | `markise-corpus` | corpus |\n\n\
                  Text with `markise-other`.\n\n## Next\n| L9 | `markise-late` | x |\n";
        let t = agents_table(md);
        assert_eq!(t.len(), 5, "{t:?}");
        assert_eq!(t["markise-render"], "L3");
        assert_eq!(t["markise"], "apps");
        let map = LayerMap::parse(
            "ui_min_layer = 6\n[layers]\nmarkise-geom = 0\nmarkise-export = 2\nmarkise-new = 4\n\
             [roles]\nmarkise = \"app\"\nmarkise-corpus = \"testkit\"\nxtask = \"tool\"",
        )
        .unwrap();
        let m = agents_mismatches(&map, &t);
        assert_eq!(m.len(), 3, "{m:?}");
        assert!(
            m.iter()
                .any(|e| e.contains("markise-export: layers.toml says L2, AGENTS.md says L3"))
        );
        assert!(m.iter().any(|e| e.starts_with("markise-new: in layers.toml")));
        assert!(
            m.iter()
                .any(|e| e.starts_with("markise-render: in the AGENTS.md table"))
        );
    }

    /// The committed files agree (the same check `cargo xtask layers` runs in CI).
    #[test]
    fn repository_layers_toml_matches_agents_md() {
        let root = crate::root();
        let map = LayerMap::parse(&read(&root, "layers.toml").unwrap()).unwrap();
        let m = agents_mismatches(&map, &agents_table(&read(&root, "AGENTS.md").unwrap()));
        assert!(m.is_empty(), "{m:#?}");
    }
}

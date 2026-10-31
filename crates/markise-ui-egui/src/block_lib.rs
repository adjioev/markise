//! PROTOTYPE (branch `proto/block-packages`, not product code): block libraries — a site's
//! authoring blocks as a pinned, offline package.
//!
//! - **Package:** an immutable `.tar.gz` built by the site (`manifest.json` with
//!   `format: "markise-block-package/1"`, `library`, `version`; `components/blocks.json`;
//!   `blocks/<name>/…`; shared fonts/styles; `presets.json`). Its layout is the site's, so the
//!   existing block code paths work on the unpacked folder unchanged.
//! - **Binding:** `blocks.config.json` in a content folder:
//!   `{ "library", "version", "source": <archive path | folder (dev mode) | https (not yet)>,
//!   "sha256" }`. Markise walks up from the document to the nearest binding (stopping at the
//!   home folder); a nested binding replaces the parent's. Front matter `blocks: <library>`
//!   names an already installed library (never a URL). The old walk-up to
//!   `components/blocks.json` stays as the developer fallback.
//! - **Cache and trust:** the first use asks the author once; on yes the archive is verified
//!   (sha256, safe paths, no links) and unpacked into
//!   `~/Library/Caches/Markise/blocks/<sha256>/`; afterwards it works offline. Nothing is ever
//!   updated automatically. A dev-mode folder is trusted per folder.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

/// A `blocks.config.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub file: PathBuf,
    pub library: String,
    pub version: String,
    pub source: String,
    pub sha256: String,
}

impl Binding {
    /// The source as a local path (relative to the binding's folder), if it is one.
    pub fn local_source(&self) -> Option<PathBuf> {
        if self.source.starts_with("https://") || self.source.starts_with("http://") {
            return None;
        }
        let p = PathBuf::from(shellexpand_home(&self.source));
        Some(if p.is_absolute() {
            p
        } else {
            self.file.parent().unwrap_or(Path::new(".")).join(p)
        })
    }

    pub fn is_folder(&self) -> bool {
        self.local_source().is_some_and(|p| p.is_dir())
    }
}

fn shellexpand_home(s: &str) -> String {
    match (s.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(h)) => format!("{h}/{rest}"),
        _ => s.to_string(),
    }
}

/// What the document's blocks use.
#[derive(Clone, Debug, PartialEq)]
pub enum Lib {
    /// An installed package (unpacked, trusted) or a trusted dev folder.
    Ready {
        root: PathBuf,
        label: String,
        package: bool,
    },
    /// A binding that was not accepted yet (ask the author).
    Pending(Binding),
    /// A binding that cannot be used (message for the author).
    Broken(String),
    /// No library: blocks show as Markdown.
    None,
}

pub fn cache_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join("Library/Caches/Markise/blocks")
}

fn index_path() -> PathBuf {
    cache_dir().join("installed.json")
}

fn read_index() -> Value {
    std::fs::read_to_string(index_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({"libraries": [], "trusted_folders": []}))
}

fn write_index(v: &Value) -> Result<(), String> {
    std::fs::create_dir_all(cache_dir()).map_err(|e| e.to_string())?;
    std::fs::write(index_path(), serde_json::to_string_pretty(v).unwrap_or_default()).map_err(|e| e.to_string())
}

/// An installed library (Preferences list).
#[derive(Clone, Debug)]
pub struct Installed {
    pub library: String,
    pub version: String,
    pub sha256: String,
    pub source: String,
    pub root: PathBuf,
}

pub fn installed() -> Vec<Installed> {
    read_index()["libraries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|l| Installed {
            library: l["library"].as_str().unwrap_or("").into(),
            version: l["version"].as_str().unwrap_or("").into(),
            sha256: l["sha256"].as_str().unwrap_or("").into(),
            source: l["source"].as_str().unwrap_or("").into(),
            root: PathBuf::from(l["root"].as_str().unwrap_or("")),
        })
        .filter(|l| l.root.join("components/blocks.json").is_file())
        .collect()
}

pub fn trusted_folders() -> Vec<PathBuf> {
    read_index()["trusted_folders"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(PathBuf::from))
        .collect()
}

/// The nearest `blocks.config.json` above `doc` (up to the home folder).
pub fn find_binding(doc: &Path) -> Option<Result<Binding, String>> {
    let home = std::env::var("HOME").ok().map(PathBuf::from);
    let mut dir = doc.parent();
    while let Some(d) = dir {
        let f = d.join("blocks.config.json");
        if f.is_file() {
            let parsed = std::fs::read_to_string(&f)
                .map_err(|e| e.to_string())
                .and_then(|s| serde_json::from_str::<Value>(&s).map_err(|e| e.to_string()))
                .map(|v| Binding {
                    file: f.clone(),
                    library: v["library"].as_str().unwrap_or("").into(),
                    version: v["version"].as_str().unwrap_or("").into(),
                    source: v["source"].as_str().unwrap_or("").into(),
                    sha256: v["sha256"].as_str().unwrap_or("").to_lowercase(),
                })
                .map_err(|e| format!("{}: {e}", f.display()));
            return Some(parsed);
        }
        if home.as_deref() == Some(d) {
            break;
        }
        dir = d.parent();
    }
    None
}

/// Front matter `blocks: <library>` of the document text.
pub fn front_matter_library(text: &str) -> Option<String> {
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    rest[..end].lines().find_map(|l| {
        let v = l.strip_prefix("blocks:")?.trim().trim_matches('"').trim_matches('\'');
        (!v.is_empty() && !v.contains("://")).then(|| v.to_string())
    })
}

/// Resolves the library of a document (`text`: its source, for the front matter).
pub fn resolve(doc: &Path, text: &str) -> Lib {
    if let Some(id) = front_matter_library(text) {
        return match installed().into_iter().rev().find(|l| l.library == id) {
            Some(l) => Lib::Ready {
                label: format!("{} {}", l.library, l.version),
                root: l.root,
                package: true,
            },
            None => Lib::Broken(format!("библиотека «{id}» (front matter) не установлена")),
        };
    }
    match find_binding(doc) {
        Some(Err(e)) => return Lib::Broken(e),
        Some(Ok(b)) => {
            if b.source.starts_with("http") {
                return Lib::Broken("источник по HTTPS в прототипе не поддерживается".into());
            }
            if b.is_folder() {
                let p = b.local_source().unwrap_or_default();
                let canon = p.canonicalize().unwrap_or(p.clone());
                return if trusted_folders().iter().any(|t| *t == canon) {
                    Lib::Ready {
                        root: canon,
                        label: format!("{} (папка разработки)", b.library),
                        package: false,
                    }
                } else {
                    Lib::Pending(b)
                };
            }
            return match installed()
                .into_iter()
                .find(|l| !b.sha256.is_empty() && l.sha256 == b.sha256)
            {
                Some(l) => Lib::Ready {
                    label: format!("{} {}", l.library, l.version),
                    root: l.root,
                    package: true,
                },
                None => Lib::Pending(b),
            };
        }
        None => {}
    }
    match markise_engine::block_form::site_root(doc) {
        Some(root) => Lib::Ready {
            label: "dev: components/blocks.json".into(),
            root,
            package: false,
        },
        None => Lib::None,
    }
}

fn sha256_of(p: &Path) -> Result<String, String> {
    let out = Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(p)
        .output()
        .map_err(|e| e.to_string())?;
    let s = String::from_utf8_lossy(&out.stdout);
    s.split_whitespace()
        .next()
        .map(str::to_lowercase)
        .ok_or_else(|| "shasum failed".into())
}

/// Accepts a binding: trusts a dev folder, or verifies and unpacks an archive into the cache.
pub fn install(b: &Binding) -> Result<PathBuf, String> {
    let src = b
        .local_source()
        .ok_or("источник по HTTPS в прототипе не поддерживается")?;
    let mut idx = read_index();
    if src.is_dir() {
        let canon = src.canonicalize().map_err(|e| e.to_string())?;
        if let Some(a) = idx["trusted_folders"].as_array_mut() {
            a.push(json!(canon.to_string_lossy()));
        }
        write_index(&idx)?;
        return Ok(canon);
    }
    if !src.is_file() {
        return Err(format!("архив не найден: {}", src.display()));
    }
    let sum = sha256_of(&src)?;
    if b.sha256.is_empty() || sum != b.sha256 {
        return Err(format!("sha256 не совпадает: в привязке {}, у файла {sum}", b.sha256));
    }
    // safe paths: no absolute, no `..`, regular files and folders only
    let list = Command::new("/usr/bin/tar")
        .arg("-tvzf")
        .arg(&src)
        .output()
        .map_err(|e| e.to_string())?;
    if !list.status.success() {
        return Err("архив не читается".into());
    }
    for line in String::from_utf8_lossy(&list.stdout).lines() {
        if !(line.starts_with('-') || line.starts_with('d')) {
            return Err(format!("в архиве недопустимая запись: {line}"));
        }
    }
    let names = Command::new("/usr/bin/tar")
        .arg("-tzf")
        .arg(&src)
        .output()
        .map_err(|e| e.to_string())?;
    for n in String::from_utf8_lossy(&names.stdout).lines() {
        if n.starts_with('/') || n.split('/').any(|c| c == "..") {
            return Err(format!("в архиве недопустимый путь: {n}"));
        }
    }
    let dest = cache_dir().join(&sum);
    if !dest.join("manifest.json").is_file() {
        let tmp = cache_dir().join(format!(".tmp-{sum}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        let st = Command::new("/usr/bin/tar")
            .arg("-xzf")
            .arg(&src)
            .arg("-C")
            .arg(&tmp)
            .status()
            .map_err(|e| e.to_string())?;
        if !st.success() {
            return Err("распаковка не удалась".into());
        }
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
    }
    let man: Value = std::fs::read_to_string(dest.join("manifest.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .ok_or("в пакете нет manifest.json")?;
    if man["format"] != "markise-block-package/1" {
        return Err(format!("неизвестный формат пакета {:?}", man["format"]));
    }
    if let Some(a) = idx["libraries"].as_array_mut() {
        a.retain(|l| l["sha256"] != json!(sum));
        a.push(json!({
            "library": man["library"], "version": man["version"], "sha256": sum,
            "source": src.to_string_lossy(), "root": dest.to_string_lossy(),
        }));
    }
    write_index(&idx)?;
    Ok(dest)
}

/// Whether `root` is an unpacked package (declarative forms only: no site form JS).
pub fn is_package(root: &Path) -> bool {
    root.starts_with(cache_dir())
}

/// An insertion preset.
#[derive(Clone, Debug)]
pub struct Preset {
    pub block: String,
    /// The block's label from blocks.json (UI language).
    pub block_label: String,
    pub label: String,
    pub description: String,
    pub markdown: String,
}

/// The insertion palette of a library: `presets.json` presets, plus a generated skeleton for
/// every block of `blocks.json` without a preset.
pub fn presets(root: &Path, lang: &str) -> Vec<Preset> {
    let tr = |v: &Value| -> String {
        v.as_str()
            .map(str::to_string)
            .or_else(|| v.get(lang).or(v.get("en")).and_then(Value::as_str).map(str::to_string))
            .unwrap_or_default()
    };
    let manifest = markise_engine::block_form::load_manifest(root).unwrap_or(Value::Null);
    let p: Value = std::fs::read_to_string(root.join("presets.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let desc = |b: &str| tr(&p["blocks"][b]["description"]);
    let mut out: Vec<Preset> = p["presets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|x| {
            let block = x["block"].as_str().unwrap_or("").to_string();
            Preset {
                label: tr(&x["label"]),
                description: desc(&block),
                markdown: x["markdown"].as_str().unwrap_or("").into(),
                block_label: tr(&manifest[&block]["label"]),
                block,
            }
        })
        .filter(|x| x.markdown.starts_with(":::"))
        .collect();
    if let Some(blocks) = manifest.as_object() {
        for (name, def) in blocks {
            if out.iter().any(|x| &x.block == name) {
                continue;
            }
            out.push(Preset {
                block: name.clone(),
                block_label: tr(&def["label"]),
                label: tr(&def["label"]),
                description: desc(name),
                markdown: skeleton(name, def, lang),
            });
        }
    }
    out
}

/// A minimal block from its field definitions (no preset): placeholders, no correct answer.
fn skeleton(name: &str, def: &Value, lang: &str) -> String {
    let ru = lang == "ru";
    let mut parts = vec![format!(":::{name}")];
    for f in def["fields"].as_array().cloned().unwrap_or_default() {
        let min = f.get("min").or(f.get("minItems")).and_then(Value::as_u64).unwrap_or(1);
        match f["type"].as_str().unwrap_or("") {
            "heading" if min > 0 || f.get("min").is_none() => {
                let l = f["level"].as_u64().unwrap_or(2) as usize;
                parts.push(format!(
                    "{} {}",
                    "#".repeat(l),
                    if ru { "Заголовок" } else { "Heading" }
                ));
            }
            "text" if min > 0 => parts.push((if ru { "Текст." } else { "Text." }).into()),
            "list" => parts.push((if ru { "- Пункт" } else { "- Item" }).into()),
            "choices" => parts.push(
                (1..=f["minItems"].as_u64().unwrap_or(2).max(2))
                    .map(|i| format!("- [ ] {} {i} {{#a{i}}}", if ru { "Ответ" } else { "Answer" }))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            "quote" => parts.push((if ru { "> Пояснение." } else { "> Explanation." }).into()),
            _ => {}
        }
    }
    parts.push(":::".into());
    parts.join("\n\n") + "\n"
}

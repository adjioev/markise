//! Export variables (spec 04 §8.3, A17): `${name}` substitution in custom export commands,
//! post-export commands and PDF header/footer templates.
//!
//! Built-in names: `outputPath`, `outputFolder` (alias `outputFolderPath`), `outputFolderName`,
//! `outputFileName` (no extension), `outputFileFullName`, `currentPath`, `currentFolder`,
//! `currentFileName`, `currentFileFullName`, `openedFolder`, `today` (YYYY-MM-DD); the PDF
//! layout adds `pageNo`, `totalPages`, `title`, `author`. Any front-matter key is available
//! as `${key}` (case-insensitive); dotted paths (`${a.b}`) look into nested YAML maps, arrays
//! are joined with ", ", multi-line values are cut to their first line and `unset` is empty.
//! Unknown variables stay literal. Commands are never run through a shell: see
//! [`crate::process`] (values are substituted per argument).

use std::path::Path;

/// A tiny YAML value tree: enough for front matter lookups (maps, lists, scalars).
#[derive(Clone, Debug, PartialEq)]
pub enum Yaml {
    Scalar(String),
    List(Vec<Yaml>),
    Map(Vec<(String, Yaml)>),
}

impl Yaml {
    /// Parses block-style YAML (indentation maps, `- ` lists, `[a, b]` flow lists, `|`/`>`
    /// block scalars, quoted scalars, `#` comments). Anything else becomes a scalar; never
    /// fails.
    pub fn parse(src: &str) -> Yaml {
        let lines: Vec<(usize, &str)> = src
            .lines()
            .map(|l| l.trim_start_matches('\u{feff}').trim_end())
            .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
            .map(|l| (l.len() - l.trim_start().len(), l.trim_start()))
            .collect();
        let mut pos = 0;
        parse_node(&lines, &mut pos, 0).unwrap_or(Yaml::Map(Vec::new()))
    }

    /// Looks up a dotted path (case-insensitive keys; numeric segments index lists).
    pub fn path(&self, path: &str) -> Option<&Yaml> {
        let mut cur = self;
        for seg in path.split('.') {
            cur = match cur {
                Yaml::Map(m) => m.iter().find(|(k, _)| k.eq_ignore_ascii_case(seg)).map(|(_, v)| v)?,
                Yaml::List(l) => l.get(seg.parse::<usize>().ok()?)?,
                Yaml::Scalar(_) => return None,
            };
        }
        Some(cur)
    }

    /// The value as substitution text (lists joined with ", ", first line only).
    pub fn display(&self) -> String {
        let s = match self {
            Yaml::Scalar(s) => s.clone(),
            Yaml::List(l) => l.iter().map(Yaml::display).collect::<Vec<_>>().join(", "),
            Yaml::Map(_) => String::new(),
        };
        let first = s.lines().next().unwrap_or("").to_string();
        if first.trim() == "unset" { String::new() } else { first }
    }
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) {
        v[1..v.len() - 1].to_string()
    } else {
        // trailing comment
        match v.find(" #") {
            Some(i) => v[..i].trim_end().to_string(),
            None => v.to_string(),
        }
    }
}

fn scalar_or_flow(v: &str) -> Yaml {
    let t = v.trim();
    if t.starts_with('[') && t.ends_with(']') {
        let inner = &t[1..t.len() - 1];
        return Yaml::List(
            inner
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| Yaml::Scalar(unquote(s)))
                .collect(),
        );
    }
    Yaml::Scalar(unquote(t))
}

/// Splits `key: value` (quoted keys allowed).
fn split_key(line: &str) -> Option<(String, &str)> {
    if let Some(q) = line.strip_prefix('"').or_else(|| line.strip_prefix('\'')) {
        let qc = line.as_bytes()[0] as char;
        let end = q.find(qc)?;
        let rest = q[end + 1..].trim_start().strip_prefix(':')?;
        return Some((q[..end].to_string(), rest));
    }
    let colon = line.find(": ").or_else(|| line.strip_suffix(':').map(|l| l.len()))?;
    Some((line[..colon].trim().to_string(), line.get(colon + 1..).unwrap_or("")))
}

fn parse_node(lines: &[(usize, &str)], pos: &mut usize, min_indent: usize) -> Option<Yaml> {
    let &(indent, first) = lines.get(*pos)?;
    if indent < min_indent {
        return None;
    }
    if first == "-" || first.starts_with("- ") {
        let mut items = Vec::new();
        while let Some(&(ind, l)) = lines.get(*pos) {
            if ind != indent || !(l == "-" || l.starts_with("- ")) {
                break;
            }
            *pos += 1;
            let rest = l[1..].trim();
            if rest.is_empty() {
                items.push(parse_node(lines, pos, indent + 1).unwrap_or(Yaml::Scalar(String::new())));
            } else if split_key(rest).is_some() {
                // `- key: v` starts a map item
                let mut sub = vec![(indent + 2, rest)];
                while let Some(&(i2, l2)) = lines.get(*pos) {
                    if i2 <= indent {
                        break;
                    }
                    sub.push((i2, l2));
                    *pos += 1;
                }
                let mut p = 0;
                items.push(parse_node(&sub, &mut p, 0).unwrap_or(Yaml::Scalar(String::new())));
            } else {
                items.push(scalar_or_flow(rest));
            }
        }
        return Some(Yaml::List(items));
    }
    let mut map = Vec::new();
    while let Some(&(ind, l)) = lines.get(*pos) {
        if ind != indent {
            if ind < indent {
                break;
            }
            *pos += 1; // stray deeper line
            continue;
        }
        let Some((key, rest)) = split_key(l) else {
            if map.is_empty() {
                *pos += 1;
                return Some(Yaml::Scalar(unquote(l)));
            }
            *pos += 1;
            continue;
        };
        *pos += 1;
        let rest = rest.trim();
        let value = if rest == "|" || rest == ">" || rest.starts_with("|-") || rest.starts_with(">-") {
            let mut parts = Vec::new();
            while let Some(&(i2, l2)) = lines.get(*pos) {
                if i2 <= indent {
                    break;
                }
                parts.push(l2);
                *pos += 1;
            }
            Yaml::Scalar(parts.join(if rest.starts_with('|') { "\n" } else { " " }))
        } else if rest.is_empty() {
            match lines.get(*pos) {
                Some(&(i2, _)) if i2 > indent => {
                    parse_node(lines, pos, indent + 1).unwrap_or(Yaml::Scalar(String::new()))
                }
                Some(&(i2, l2)) if i2 == indent && (l2 == "-" || l2.starts_with("- ")) => {
                    parse_node(lines, pos, indent).unwrap_or(Yaml::Scalar(String::new()))
                }
                _ => Yaml::Scalar(String::new()),
            }
        } else {
            scalar_or_flow(rest)
        };
        map.push((key, value));
    }
    Some(Yaml::Map(map))
}

/// A variable environment.
#[derive(Clone, Debug, Default)]
pub struct Vars {
    builtins: Vec<(String, String)>,
    meta: Option<Yaml>,
}

impl Vars {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets (or replaces) a built-in variable.
    pub fn set(&mut self, name: &str, value: impl Into<String>) -> &mut Self {
        let value = value.into();
        match self.builtins.iter_mut().find(|(k, _)| k == name) {
            Some((_, v)) => *v = value,
            None => self.builtins.push((name.to_string(), value)),
        }
        self
    }

    /// Makes the front-matter keys available (raw YAML of the front matter).
    pub fn with_front_matter(mut self, yaml: &str) -> Self {
        self.meta = Some(Yaml::parse(yaml));
        self
    }

    /// The standard export variables for exporting `current` (the document file, if saved) to
    /// `output`.
    pub fn for_export(current: Option<&Path>, output: &Path, opened_folder: Option<&Path>, today: &str) -> Self {
        let mut v = Vars::new();
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let name = |p: &Path| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let stem = |p: &Path| {
            p.file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let out_dir = output.parent().unwrap_or(Path::new(""));
        v.set("outputPath", s(output));
        v.set("outputFolder", s(out_dir));
        v.set("outputFolderPath", s(out_dir));
        v.set("outputFolderName", name(out_dir));
        v.set("outputFileName", stem(output));
        v.set("outputFileFullName", name(output));
        if let Some(c) = current {
            let dir = c.parent().unwrap_or(Path::new(""));
            v.set("currentPath", s(c));
            v.set("currentFolder", s(dir));
            v.set("currentFileName", stem(c));
            v.set("currentFileFullName", name(c));
        }
        if let Some(o) = opened_folder {
            v.set("openedFolder", s(o));
        }
        v.set("today", today);
        v
    }

    /// Value of a variable: built-ins first (case-insensitive), then front-matter keys.
    pub fn get(&self, name: &str) -> Option<String> {
        if let Some((_, v)) = self
            .builtins
            .iter()
            .find(|(k, _)| k == name)
            .or_else(|| self.builtins.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)))
        {
            return Some(v.clone());
        }
        self.meta.as_ref()?.path(name).map(Yaml::display)
    }

    /// Replaces every known `${name}`; unknown variables stay literal.
    pub fn expand(&self, template: &str) -> String {
        self.expand_with(template, |v| v.to_string())
    }

    fn expand_with(&self, template: &str, quote: impl Fn(&str) -> String) -> String {
        let mut out = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(i) = rest.find("${") {
            out.push_str(&rest[..i]);
            let after = &rest[i + 2..];
            match after.find('}') {
                Some(j) if !after[..j].is_empty() && !after[..j].contains(['{', '$', '\n']) => {
                    let name = after[..j].trim();
                    match self.get(name) {
                        Some(v) => out.push_str(&quote(&v)),
                        None => out.push_str(&rest[i..i + 3 + j]),
                    }
                    rest = &after[j + 1..];
                }
                _ => {
                    out.push_str("${");
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }
}

/// Today's date (local time where available, else UTC) as `YYYY-MM-DD`.
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    date_string(secs + local_offset_secs(secs))
}

#[cfg(unix)]
fn local_offset_secs(t: i64) -> i64 {
    // SAFETY: `localtime_r` writes only into the provided `tm`; `time_t` is passed by pointer.
    unsafe {
        let tt = t as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&tt, &mut tm).is_null() {
            0
        } else {
            tm.tm_gmtoff as i64
        }
    }
}

#[cfg(not(unix))]
fn local_offset_secs(_t: i64) -> i64 {
    0
}

/// `YYYY-MM-DD` of a Unix timestamp (days-from-civil inverse, proleptic Gregorian).
pub fn date_string(unix_secs: i64) -> String {
    let (y, m, d) = civil_from_days(unix_secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_and_unknown_stay_literal() {
        let v = Vars::for_export(
            Some(Path::new("/docs/a b/note.md")),
            Path::new("/out/note.pdf"),
            Some(Path::new("/docs")),
            "2026-10-27",
        );
        assert_eq!(
            v.expand("${outputPath}|${outputFolder}|${outputFolderName}|${outputFileName}|${outputFileFullName}"),
            "/out/note.pdf|/out|out|note|note.pdf"
        );
        assert_eq!(
            v.expand("${currentFolder}/${currentFileName} ${openedFolder} ${today} ${nope} ${ x"),
            "/docs/a b/note /docs 2026-10-27 ${nope} ${ x"
        );
        assert_eq!(v.expand("${OUTPUTPATH}"), "/out/note.pdf", "case-insensitive");
    }

    #[test]
    fn front_matter_keys_nested_lists_unset_multiline() {
        let yaml = "title: \"My T\"\nAuthor: Ann\nmeta:\n  version: 3\n  owner:\n    name: Bob\ntags: [a, b]\nlist:\n  - x\n  - y\nnote: |\n  first\n  second\nempty: unset\n";
        let v = Vars::new().with_front_matter(yaml);
        assert_eq!(v.expand("${title}"), "My T");
        assert_eq!(v.expand("${author}"), "Ann");
        assert_eq!(v.expand("${meta.version}/${meta.owner.name}"), "3/Bob");
        assert_eq!(v.expand("${tags}|${list}"), "a, b|x, y");
        assert_eq!(v.expand("${note}"), "first");
        assert_eq!(v.expand("[${empty}]"), "[]");
        assert_eq!(v.expand("${meta.missing}"), "${meta.missing}");
    }

    #[test]
    fn dates() {
        assert_eq!(date_string(0), "1970-01-01");
        assert_eq!(date_string(1_792_800_000), "2026-10-24");
        assert_eq!(date_string(951_782_400), "2000-02-29");
        assert_eq!(today().len(), 10);
    }
}

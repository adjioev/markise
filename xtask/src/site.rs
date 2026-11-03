//! `cargo xtask site [--out <dir>]`: build the static website (issue #62).
//!
//! Copies `site/` (front page, stylesheet, images) to the output folder (default
//! `target/site/`) and renders the user guide in `docs/user/*.md` to `docs/<page>.html` through
//! Markise's own pipeline: `markise-md` parses, `markise-export` writes the HTML body (math and
//! diagrams as inline SVG, code with highlight classes, raw HTML sanitized). Every page gets the
//! same header, guide navigation and footer. Local `.md` links become `.html` (fragments kept),
//! `../../site/…` image paths (which work when the Markdown is read on GitHub) become paths in
//! the built site.
//!
//! Then every built HTML file is checked: each local `href`/`src`/`srcset` target must exist and
//! each `#fragment` must name an `id` in its page. Any broken link fails the command. External
//! links are not fetched (the build is offline).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use markise_export::{CodeHighlight, HtmlOptions, escape_html, render_body};
use markise_md::{Document, ParseOptions};

pub const USAGE: &str = "\
usage: cargo xtask site [--out <dir>]

Builds the website: site/ plus the user guide docs/user/*.md rendered to docs/*.html,
then checks every local link, image and #fragment (fails on any broken one).

  --out   output folder (default: target/site/); it is replaced, so it must be under the
          workspace's target/ folder, or outside the workspace and either new or a folder
          an earlier run built (it holds the .markise-site marker)
";

/// The guide in navigation order: file stem in `docs/user/`, navigation label. Every published
/// Markdown file must be listed (and every listed one must exist).
const PAGES: &[(&str, &str)] = &[
    ("index", "Overview"),
    ("getting-started", "Getting started"),
    ("editing", "Editing"),
    ("folders-and-vaults", "Folders and vaults"),
    ("shortcuts", "Keyboard shortcuts"),
    ("themes", "Themes"),
    ("math-and-diagrams", "Math and diagrams"),
    ("comments-and-suggestions", "Comments and suggestions"),
    ("export", "Export"),
    ("large-files", "Large files"),
    ("saving-and-recovery", "Saving and recovery"),
    ("settings", "Settings"),
    ("updates-and-privacy", "Updates and privacy"),
    ("faq", "FAQ and troubleshooting"),
];

/// Maintainer notes in `docs/user/` that are not published.
const INTERNAL: &[&str] = &["CLAIMS.md"];

const REPO_URL: &str = "https://github.com/adjioev/markise";

/// Written into every built site: an existing folder outside `target/` is only replaced when
/// it carries this marker (an earlier build), never an arbitrary folder.
const MARKER: &str = ".markise-site";

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let mut out = root.join("target").join("site");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => {
                let o = PathBuf::from(it.next().ok_or("--out needs a value")?);
                out = std::env::current_dir()
                    .map_err(|e| format!("current dir: {e}"))?
                    .join(o);
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            other => return Err(format!("unexpected argument `{other}`\n\n{USAGE}")),
        }
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let out = check_out_dir(root, home.as_deref(), &out)?;
    build(root, &out)?;
    let broken = check_links(&out)?;
    if !broken.is_empty() {
        return Err(format!(
            "{} broken link(s) in the built site:\n  {}",
            broken.len(),
            broken.join("\n  ")
        ));
    }
    println!("site: {} (links, images and fragments checked)", out.display());
    Ok(())
}

/// `p` made absolute with symlinks resolved: `.`/`..` are applied lexically, then the longest
/// existing ancestor is canonicalized and the rest appended.
fn resolve(p: &Path) -> Result<PathBuf, String> {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("current dir: {e}"))?
            .join(p)
    };
    let mut lexical = PathBuf::new();
    for c in abs.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                lexical.pop();
            }
            c => lexical.push(c),
        }
    }
    let mut existing = lexical;
    let mut rest = Vec::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .ok_or_else(|| format!("cannot resolve {}", p.display()))?
            .to_os_string();
        rest.push(name);
        if !existing.pop() {
            return Err(format!("cannot resolve {}", p.display()));
        }
    }
    let mut out = std::fs::canonicalize(&existing).map_err(|e| format!("{}: {e}", existing.display()))?;
    for part in rest.iter().rev() {
        out.push(part);
    }
    Ok(out)
}

/// The output folder, resolved, if the generator may replace it (it deletes it first):
/// under the workspace's `target/`, or outside the workspace and either missing or marked
/// by an earlier build ([`MARKER`]). Never the workspace, `site/`, `docs/`, the home folder or
/// a folder that contains one of them.
fn check_out_dir(root: &Path, home: Option<&Path>, out: &Path) -> Result<PathBuf, String> {
    let root = resolve(root)?;
    let out = resolve(out)?;
    let refuse = |why: &str| Err(format!("refusing to use {} as the site output: {why}", out.display()));
    let mut protected = vec![root.clone(), root.join("site"), root.join("docs")];
    if let Some(h) = home {
        protected.push(resolve(h)?);
    }
    for p in &protected {
        if p.starts_with(&out) {
            return refuse(&format!("it is or contains {}", p.display()));
        }
    }
    let target = root.join("target");
    if out.starts_with(&target) {
        return Ok(out);
    }
    if out.starts_with(&root) {
        return refuse("inside the workspace, only folders under target/ may be replaced");
    }
    if out.exists() && !out.join(MARKER).is_file() {
        return refuse(&format!(
            "the folder exists and was not built by `cargo xtask site` (no {MARKER} file)"
        ));
    }
    Ok(out)
}

fn build(root: &Path, out: &Path) -> Result<(), String> {
    if out.exists() {
        std::fs::remove_dir_all(out).map_err(|e| format!("remove {}: {e}", out.display()))?;
    }
    copy_dir(&root.join("site"), out)?;
    std::fs::write(
        out.join(MARKER),
        "Built by `cargo xtask site`; the folder is replaced by the next build.\n",
    )
    .map_err(|e| format!("{}: {e}", out.display()))?;

    let src = root.join("docs").join("user");
    let listed: BTreeSet<String> = PAGES.iter().map(|(s, _)| format!("{s}.md")).collect();
    let mut found = BTreeSet::new();
    for entry in std::fs::read_dir(&src).map_err(|e| format!("{}: {e}", src.display()))? {
        let name = entry.map_err(|e| e.to_string())?.file_name();
        let name = name.to_string_lossy().into_owned();
        if name.ends_with(".md") && !INTERNAL.contains(&name.as_str()) {
            found.insert(name);
        }
    }
    let unlisted: Vec<_> = found.difference(&listed).collect();
    let missing: Vec<_> = listed.difference(&found).collect();
    if !unlisted.is_empty() || !missing.is_empty() {
        return Err(format!(
            "docs/user and the page list in xtask/src/site.rs disagree: not listed {unlisted:?}, missing {missing:?}"
        ));
    }

    let docs_out = out.join("docs");
    std::fs::create_dir_all(&docs_out).map_err(|e| format!("{}: {e}", docs_out.display()))?;
    for (stem, _) in PAGES {
        let path = src.join(format!("{stem}.md"));
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let html = render_page(stem, &text);
        let dest = docs_out.join(format!("{stem}.html"));
        std::fs::write(&dest, html).map_err(|e| format!("{}: {e}", dest.display()))?;
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        // attribution records stay in the repository
        if name == "SOURCE.md" {
            continue;
        }
        let (src, dst) = (entry.path(), to.join(&name));
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| format!("copy {}: {e}", src.display()))?;
        }
    }
    Ok(())
}

/// The text of the first `# ` heading (Markdown inline markers removed), for `<title>`.
fn first_heading(md: &str) -> Option<String> {
    md.lines()
        .find_map(|l| l.strip_prefix("# "))
        .map(|h| h.replace(['`', '*'], "").trim().to_string())
}

fn render_page(stem: &str, md: &str) -> String {
    let doc = Document::parse(md, ParseOptions::default());
    let opts = HtmlOptions {
        highlight: CodeHighlight::Classes,
        ..Default::default()
    };
    let body = rewrite_links(&render_body(&doc, &opts));
    let title = first_heading(md).unwrap_or_else(|| stem.to_string());
    let mut nav = String::new();
    for (s, label) in PAGES {
        let current = if *s == stem { " aria-current='page'" } else { "" };
        let _ = writeln!(nav, "        <li><a href='{s}.html'{current}>{label}</a></li>");
    }
    let mut t = String::new();
    escape_html(&title, &mut t);
    let edit = format!("{REPO_URL}/blob/main/docs/user/{stem}.md");
    format!(
        "<!doctype html>
<html lang='en'>
<head>
  <meta charset='utf-8'>
  <meta name='viewport' content='width=device-width, initial-scale=1'>
  <title>{t} · Markise guide</title>
  <meta name='color-scheme' content='dark light'>
  <meta name='theme-color' content='#1a1b26'>
  <link rel='icon' href='../assets/favicon.svg' type='image/svg+xml'>
  <link rel='icon' href='../assets/favicon-32.png' sizes='32x32' type='image/png'>
  <link rel='stylesheet' href='../style.css'>
</head>
<body class='doc-page'>
  <a class='skip' href='#write'>Skip to content</a>
  <header class='doc-header'>
    <nav class='nav wrap' aria-label='Main'>
      <a class='brand' href='../index.html' aria-label='Markise home'>
        <img src='../assets/markise-lockup-dark.svg' alt='Markise' width='185' height='44'>
      </a>
      <ul>
        <li><a href='../index.html#features'>Features</a></li>
        <li><a href='index.html'>Docs</a></li>
        <li><a href='../index.html#download'>Download</a></li>
        <li><a href='{REPO_URL}'>GitHub</a></li>
      </ul>
    </nav>
  </header>
  <div class='doc-layout wrap'>
    <nav class='doc-nav' aria-label='User guide'>
      <details open>
        <summary>User guide</summary>
        <ul>
{nav}        </ul>
      </details>
    </nav>
    <main id='write' class='doc-content'>
{body}
      <p class='doc-edit'><a href='{edit}'>Edit this page on GitHub</a></p>
    </main>
  </div>
{footer}
</body>
</html>
",
        footer = footer("../")
    )
}

/// The footer shared by the guide pages (the front page carries the same text in
/// `site/index.html`).
fn footer(prefix: &str) -> String {
    format!(
        "  <footer class='footer'>
    <div class='wrap'>
      <img src='{prefix}assets/markise-mark-dark.svg' alt='' width='36' height='43' class='foot-mark'>
      <div>
        <p>Markise — the marquise of Markdown editors. © 2026 The Markise contributors.
          Dual-licensed under MIT or Apache-2.0. This site uses no cookies, trackers or
          third-party requests.</p>
        <p class='trademarks'>Obsidian is a trademark of Dynalist Inc. Markise is not affiliated with or endorsed by Obsidian.</p>
      </div>
    </div>
  </footer>"
    )
}

/// Local `x.md#frag` links to `x.html#frag`; `../../site/…` paths to `../…`.
fn rewrite_links(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some((before, attr, quote, value, after)) = next_url_attr(rest) {
        out.push_str(before);
        out.push_str(attr);
        out.push(quote);
        out.push_str(&rewrite_url(value));
        out.push(quote);
        rest = after;
    }
    out.push_str(rest);
    out
}

fn rewrite_url(url: &str) -> String {
    if is_external(url) || url.starts_with('#') {
        return url.to_string();
    }
    if let Some(p) = url.strip_prefix("../../site/") {
        return format!("../{p}");
    }
    let (path, frag) = match url.find('#') {
        Some(i) => (&url[..i], &url[i..]),
        None => (url, ""),
    };
    match path.strip_suffix(".md") {
        Some(stem) if !stem.contains('/') => format!("{stem}.html{frag}"),
        _ => url.to_string(),
    }
}

fn is_external(url: &str) -> bool {
    let scheme_end = url.find(':');
    let first_sep = url.find(['/', '#', '?']);
    match (scheme_end, first_sep) {
        (Some(c), Some(s)) => c < s,
        (Some(_), None) => true,
        _ => url.starts_with("//"),
    }
}

/// The next quoted ` href=`/` src=`/` srcset=` attribute: (text before, ` attr=`, quote,
/// value, rest). Unquoted values are skipped (the generator writes none).
fn next_url_attr(s: &str) -> Option<(&str, &str, char, &str, &str)> {
    let mut from = 0;
    loop {
        let mut best: Option<(usize, usize)> = None;
        for attr in [" href=", " src=", " srcset="] {
            if let Some(i) = s[from..].find(attr).map(|i| i + from)
                && best.is_none_or(|(b, _)| i < b)
            {
                best = Some((i, attr.len()));
            }
        }
        let (i, len) = best?;
        let after_eq = i + len;
        let quote = s[after_eq..].chars().next()?;
        if quote != '"' && quote != '\'' {
            from = after_eq;
            continue;
        }
        let vstart = after_eq + 1;
        let vend = vstart + s[vstart..].find(quote)?;
        return Some((&s[..i], &s[i..after_eq], quote, &s[vstart..vend], &s[vend + 1..]));
    }
}

/// Every broken local reference in the built site: `file: url (reason)`.
fn check_links(out: &Path) -> Result<Vec<String>, String> {
    let mut pages = Vec::new();
    collect_html(out, &mut pages)?;
    let mut ids: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    let mut texts = Vec::new();
    for p in &pages {
        let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        ids.insert(p.clone(), collect_ids(&text));
        texts.push(text);
    }
    let mut broken = Vec::new();
    for (page, text) in pages.iter().zip(&texts) {
        let rel = page.strip_prefix(out).unwrap_or(page).display().to_string();
        let mut rest = text.as_str();
        while let Some((_, attr, _, value, after)) = next_url_attr(rest) {
            rest = after;
            let urls: Vec<&str> = if attr == " srcset=" {
                value.split(',').filter_map(|c| c.split_whitespace().next()).collect()
            } else {
                vec![value]
            };
            for url in urls {
                if let Some(why) = check_url(page, url, &ids) {
                    broken.push(format!("{rel}: {url} ({why})"));
                }
            }
        }
    }
    Ok(broken)
}

fn check_url(page: &Path, url: &str, ids: &BTreeMap<PathBuf, BTreeSet<String>>) -> Option<String> {
    if url.is_empty() {
        return Some("empty".into());
    }
    if is_external(url) {
        return None;
    }
    if url.starts_with('/') {
        return Some("root-relative paths break under a project subpath".into());
    }
    let (path, frag) = match url.find('#') {
        Some(i) => (&url[..i], Some(&url[i + 1..])),
        None => (url, None),
    };
    let path = path.split('?').next().unwrap_or(path);
    let target = if path.is_empty() {
        page.to_path_buf()
    } else {
        let mut t = page.parent()?.to_path_buf();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    t.pop();
                }
                p => t.push(p),
            }
        }
        if t.is_dir() {
            t.push("index.html");
        }
        t
    };
    if !target.is_file() {
        return Some("missing file".into());
    }
    if let Some(f) = frag.filter(|f| !f.is_empty()) {
        match ids.get(&target) {
            Some(set) if set.contains(f) => {}
            Some(_) => return Some("missing #fragment".into()),
            None => {}
        }
    }
    None
}

fn collect_html(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if p.is_dir() {
            collect_html(&p, out)?;
        } else if p.extension().is_some_and(|e| e == "html") {
            out.push(p);
        }
    }
    out.sort();
    Ok(())
}

/// Values of `id=` and `name=` attributes (either quote style).
fn collect_ids(html: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for attr in [" id=", " name="] {
        let mut rest = html;
        while let Some(i) = rest.find(attr) {
            rest = &rest[i + attr.len()..];
            let Some(q) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                continue;
            };
            if let Some(end) = rest[1..].find(q) {
                set.insert(rest[1..1 + end].to_string());
            }
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_markdown_links_and_site_paths() {
        assert_eq!(rewrite_url("editing.md"), "editing.html");
        assert_eq!(rewrite_url("editing.md#find"), "editing.html#find");
        assert_eq!(rewrite_url("#top"), "#top");
        assert_eq!(rewrite_url("https://x.org/a.md"), "https://x.org/a.md");
        assert_eq!(rewrite_url("../../site/assets/shots/a.jpg"), "../assets/shots/a.jpg");
        assert_eq!(rewrite_url("../adr/0017.md"), "../adr/0017.md");
    }

    #[test]
    fn rewrites_both_quote_styles() {
        let html = "<a href='a.md#x'>a</a> <a href=\"b.md\">b</a> <img src='../../site/assets/i.png'>";
        assert_eq!(
            rewrite_links(html),
            "<a href='a.html#x'>a</a> <a href=\"b.html\">b</a> <img src='../assets/i.png'>"
        );
    }

    #[test]
    fn external_detection() {
        assert!(is_external("https://github.com"));
        assert!(is_external("mailto:a@b"));
        assert!(is_external("//cdn.example"));
        assert!(!is_external("docs/a.html"));
        assert!(!is_external("a.html#b:c"));
    }

    #[test]
    fn finds_broken_links_and_fragments() {
        let dir = std::env::temp_dir().join(format!("markise-site-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(
            dir.join("index.html"),
            "<h2 id='a'>A</h2><a href='docs/p.html#b'>ok</a><a href='docs/p.html#nope'>x</a>\
             <img src='missing.png'><img srcset='index.html 1x, gone.webp 2x'><a href='#a'>a</a>",
        )
        .unwrap();
        std::fs::write(
            dir.join("docs/p.html"),
            "<h2 id=\"b\">B</h2><a href='../index.html'>up</a>",
        )
        .unwrap();
        let broken = check_links(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            broken,
            vec![
                "index.html: docs/p.html#nope (missing #fragment)",
                "index.html: missing.png (missing file)",
                "index.html: gone.webp (missing file)",
            ]
        );
    }

    #[test]
    fn output_folder_guard() {
        let base = std::env::temp_dir().join(format!("markise-site-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("repo");
        let home = base.join("home");
        for d in ["site", "docs/user", "target", "src"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::create_dir_all(&home).unwrap();
        let other = base.join("elsewhere");
        std::fs::create_dir_all(other.join("built")).unwrap();
        std::fs::write(other.join("built").join(MARKER), "").unwrap();
        std::fs::create_dir_all(other.join("photos")).unwrap();
        let ok = |p: &Path| check_out_dir(&root, Some(&home), p).is_ok();

        assert!(ok(&root.join("target/site")));
        assert!(ok(&root.join("target/a/../site")));
        assert!(ok(&other.join("new-folder")));
        assert!(ok(&other.join("built")));
        // the sources, the workspace, its parents and the home folder are never replaced
        assert!(!ok(&root));
        assert!(!ok(&root.join("site")));
        assert!(!ok(&root.join("docs")));
        assert!(!ok(&root.join("docs/user")));
        assert!(!ok(&root.join("src")));
        assert!(!ok(&root.join("new-out")));
        assert!(!ok(&root.join("target/../site")));
        assert!(!ok(&base));
        assert!(!ok(&home));
        // an existing folder that an earlier build did not mark
        assert!(!ok(&other.join("photos")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn page_list_is_unique() {
        let set: BTreeSet<_> = PAGES.iter().map(|(s, _)| s).collect();
        assert_eq!(set.len(), PAGES.len());
    }
}

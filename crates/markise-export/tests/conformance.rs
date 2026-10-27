//! CommonMark 0.31.2 and GFM extension spec examples rendered through the **production**
//! export path ([`markise_export::render_body`]) and compared after normalisation. The
//! normaliser removes only presentation that the export adds on purpose (spec 03 DOM contract:
//! classes, heading ids, `lang`, the table `<figure>`, `style="text-align"` vs `align`,
//! checkbox attribute spelling) and canonicalises whitespace, quoting, entities and URL
//! percent-encoding.
//!
//! The spec files are public standards but CC-BY-SA licensed, so they are not committed:
//! `scripts/fetch-spec-tests.sh` downloads them into `.spec-cache/` (gitignored). Without the
//! cache the test is skipped. Known dialect deviations are listed with their spec / decision
//! id in `tests/conformance-expected-failures.txt`; every other example must pass.

use std::collections::BTreeMap;

use markise_export::{HtmlOptions, render_body};
use markise_md::{Document, ParseOptions};

struct Example {
    suite: &'static str,
    number: usize,
    section: String,
    extension: Option<String>,
    markdown: String,
    html: String,
}

fn cache(name: &str) -> Option<String> {
    let p = format!("{}/../../.spec-cache/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(p).ok()
}

fn commonmark_examples() -> Option<Vec<Example>> {
    let json: serde_json::Value = serde_json::from_str(&cache("commonmark-0.31.2.json")?).ok()?;
    Some(
        json.as_array()?
            .iter()
            .map(|e| Example {
                suite: "commonmark",
                number: e["example"].as_u64().unwrap() as usize,
                section: e["section"].as_str().unwrap().to_string(),
                extension: None,
                markdown: e["markdown"].as_str().unwrap().to_string(),
                html: e["html"].as_str().unwrap().to_string(),
            })
            .collect(),
    )
}

/// GFM spec.txt: only the examples tagged with an extension (the rest duplicates CommonMark).
fn gfm_examples() -> Option<Vec<Example>> {
    let text = cache("gfm-spec.txt")?;
    let fence = "````````````````````````````````";
    let mut out = Vec::new();
    let mut section = String::new();
    let mut number = 0;
    let mut lines = text.lines();
    while let Some(l) = lines.next() {
        if let Some(h) = l.strip_prefix("## ").or_else(|| l.strip_prefix("# ")) {
            section = h.trim().to_string();
        }
        if let Some(rest) = l.strip_prefix(fence) {
            let Some(tag) = rest.trim().strip_prefix("example") else {
                continue;
            };
            number += 1;
            let ext = tag.trim();
            let mut md = String::new();
            let mut html = String::new();
            let mut in_html = false;
            for l in lines.by_ref() {
                if l.starts_with(fence) {
                    break;
                }
                if l == "." {
                    in_html = true;
                    continue;
                }
                let l = l.replace('→', "\t");
                if in_html {
                    html.push_str(&l);
                    html.push('\n');
                } else {
                    md.push_str(&l);
                    md.push('\n');
                }
            }
            if !ext.is_empty() {
                out.push(Example {
                    suite: "gfm",
                    number,
                    section: section.clone(),
                    extension: Some(ext.to_string()),
                    markdown: md,
                    html,
                });
            }
        }
    }
    Some(out)
}

/// Options closest to CommonMark: the dialect extensions that change CommonMark output are off.
fn conformance_options(ext: Option<&str>) -> ParseOptions {
    ParseOptions {
        strict: true,
        inline_math: false,
        latex_delimiters: false,
        code_math: false,
        alerts: false,
        emoji: false,
        auto_links: ext == Some("autolink"),
        strict_number_start: true,
        ..ParseOptions::default()
    }
}

fn expected_failures() -> BTreeMap<String, String> {
    let text = include_str!("conformance-expected-failures.txt");
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') {
                return None;
            }
            let (id, reason) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
            Some((id.to_string(), reason.trim().to_string()))
        })
        .collect()
}

#[test]
fn spec_examples() {
    let (Some(cm), Some(gfm)) = (commonmark_examples(), gfm_examples()) else {
        let msg = "CONFORMANCE SKIPPED: .spec-cache/ missing — run scripts/fetch-spec-tests.sh";
        // Write to the real stderr (bypasses the test harness capture) so the skip is visible.
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), "{msg}");
        assert!(
            std::env::var("CI").as_deref() != Ok("true"),
            "{msg} (required when CI=true)"
        );
        return;
    };
    let expected = expected_failures();
    let mut unexpected = Vec::new();
    let mut fixed = Vec::new();
    let mut stats: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let mut by_section: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut report = String::new();
    let mut untrusted_pass = 0usize;
    for ex in cm.iter().chain(gfm.iter()) {
        let doc = Document::parse(ex.markdown.as_str(), conformance_options(ex.extension.as_deref()));
        assert_eq!(
            doc.serialize(),
            ex.markdown,
            "round-trip changed {}:{}",
            ex.suite,
            ex.number
        );
        // Production export; `trusted` keeps raw HTML as the spec expects (the untrusted
        // default sanitizes it, see C40), everything else uses the default options.
        let got = render_body(
            &doc,
            &HtmlOptions {
                trusted: true,
                ..Default::default()
            },
        );
        let ok = normalize_html(&got) == normalize_html(&ex.html);
        let untrusted = render_body(&doc, &HtmlOptions::default());
        if normalize_html(&untrusted) == normalize_html(&ex.html) {
            untrusted_pass += 1;
        }
        let id = format!("{}:{}", ex.suite, ex.number);
        let st = stats.entry(ex.suite).or_default();
        st.1 += 1;
        let sec = by_section.entry(format!("{}/{}", ex.suite, ex.section)).or_default();
        sec.1 += 1;
        if ok {
            st.0 += 1;
            sec.0 += 1;
            if expected.contains_key(&id) {
                fixed.push(id);
            }
        } else {
            report.push_str(&format!(
                "== {id} [{}]\n--- markdown\n{}--- expected\n{}--- got\n{}\n",
                ex.section, ex.markdown, ex.html, got
            ));
            if !expected.contains_key(&id) {
                unexpected.push(id);
            }
        }
    }
    let out_dir = format!("{}/../../.spec-cache", env!("CARGO_MANIFEST_DIR"));
    let _ = std::fs::write(format!("{out_dir}/conformance-failures.txt"), &report);
    let mut summary = String::new();
    for (sec, (p, n)) in &by_section {
        summary.push_str(&format!("{p:4}/{n:<4} {sec}\n"));
    }
    for (suite, (p, n)) in &stats {
        summary.push_str(&format!(
            "TOTAL {suite}: {p}/{n} pass ({:.1}%)\n",
            100.0 * *p as f64 / *n as f64
        ));
    }
    summary.push_str(&format!(
        "untrusted (sanitized) export: {untrusted_pass}/{} identical\n",
        cm.len() + gfm.len()
    ));
    let _ = std::fs::write(format!("{out_dir}/conformance-summary.txt"), &summary);
    eprintln!("{summary}");
    assert!(
        unexpected.is_empty(),
        "{} unexpected failures (see .spec-cache/conformance-failures.txt): {:?}",
        unexpected.len(),
        unexpected
    );
    assert!(fixed.is_empty(), "now passing, remove from expected list: {fixed:?}");
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (
                b.get(i + 1).and_then(|c| (*c as char).to_digit(16)),
                b.get(i + 2).and_then(|c| (*c as char).to_digit(16)),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn esc(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
}

/// Canonical form of one start/end tag.
fn normalize_tag(raw: &str) -> Option<(String, String, bool)> {
    let inner = raw
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim_end_matches('/')
        .trim();
    let closing = inner.starts_with('/');
    let body = inner.trim_start_matches('/');
    let name_end = body.find(char::is_whitespace).unwrap_or(body.len());
    let name = body[..name_end].to_ascii_lowercase();
    if name.is_empty() || name.starts_with('!') || name.starts_with('?') {
        return None;
    }
    // attributes (quoted values may contain spaces)
    let mut attrs: Vec<(String, Option<String>)> = Vec::new();
    let a = &body[name_end..];
    let ab = a.as_bytes();
    let mut i = 0;
    while i < ab.len() {
        while i < ab.len() && ab[i].is_ascii_whitespace() {
            i += 1;
        }
        let ks = i;
        while i < ab.len() && !ab[i].is_ascii_whitespace() && ab[i] != b'=' {
            i += 1;
        }
        if ks == i {
            i += 1;
            continue;
        }
        let key = a[ks..i].to_ascii_lowercase();
        let mut val = None;
        if i < ab.len() && ab[i] == b'=' {
            i += 1;
            if i < ab.len() && (ab[i] == b'"' || ab[i] == b'\'') {
                let q = ab[i];
                let vs = i + 1;
                let ve = a[vs..].find(q as char).map(|e| vs + e).unwrap_or(a.len());
                val = Some(a[vs..ve].to_string());
                i = ve + 1;
            } else {
                let vs = i;
                while i < ab.len() && !ab[i].is_ascii_whitespace() {
                    i += 1;
                }
                val = Some(a[vs..i].to_string());
            }
        }
        attrs.push((key, val));
    }
    let mut out_attrs = Vec::new();
    for (k, v) in attrs {
        let v = v.map(|v| markise_md::inline::decode_refs(&v));
        match k.as_str() {
            // presentation added by the export DOM contract (spec 03)
            "class" | "id" | "lang" | "mdtype" | "role" | "disabled" => {}
            k if k.starts_with("data-") => {}
            "style" => {
                let v = v.unwrap_or_default();
                if let Some(al) = v.split(';').find_map(|d| d.trim().strip_prefix("text-align:")) {
                    out_attrs.push(format!("align=\"{}\"", al.trim()));
                }
            }
            "checked" => out_attrs.push("checked=\"\"".to_string()),
            "href" | "src" => {
                let mut e = String::new();
                esc(&percent_decode(&v.unwrap_or_default()), &mut e);
                out_attrs.push(format!("{k}=\"{e}\""));
            }
            _ => match v {
                Some(v) => {
                    let mut e = String::new();
                    esc(&v, &mut e);
                    out_attrs.push(format!("{k}=\"{e}\""));
                }
                None => out_attrs.push(k.to_string()),
            },
        }
    }
    out_attrs.sort();
    let mut t = String::from("<");
    if closing {
        t.push('/');
    }
    t.push_str(&name);
    for a in out_attrs {
        t.push(' ');
        t.push_str(&a);
    }
    t.push('>');
    Some((t, name, closing))
}

/// Normalises HTML for comparison (in the spirit of the CommonMark test normaliser).
fn normalize_html(html: &str) -> String {
    const BLOCK: &[&str] = &[
        "p",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "ul",
        "ol",
        "li",
        "blockquote",
        "pre",
        "hr",
        "table",
        "thead",
        "tbody",
        "tr",
        "th",
        "td",
        "div",
        "br",
        "input",
    ];
    enum Tok {
        Tag { s: String, block: bool },
        Text(String),
    }
    let mut toks: Vec<Tok> = Vec::new();
    let mut in_pre = 0i32;
    let mut i = 0;
    while i < html.len() {
        if html.as_bytes()[i] == b'<' {
            if html[i..].starts_with("<!--") {
                // comments are dropped by the export (spec 03) and ignored here
                i = html[i..].find("-->").map(|e| i + e + 3).unwrap_or(html.len());
                continue;
            }
            let e = html[i..].find('>').map(|e| i + e + 1).unwrap_or(html.len());
            let raw = &html[i..e];
            i = e;
            match normalize_tag(raw) {
                None => toks.push(Tok::Tag {
                    s: raw.to_string(),
                    block: false,
                }),
                Some((t, name, closing)) => {
                    if name == "figure" {
                        continue; // export wraps tables in <figure class='table-figure'>
                    }
                    if name == "pre" {
                        in_pre += if closing { -1 } else { 1 };
                    }
                    toks.push(Tok::Tag {
                        s: t,
                        block: BLOCK.contains(&name.as_str()),
                    });
                }
            }
        } else {
            let e = html[i..].find('<').map(|e| i + e).unwrap_or(html.len());
            let text = markise_md::inline::decode_refs(&html[i..e]);
            let mut o = String::new();
            if in_pre > 0 {
                esc(&text, &mut o);
            } else {
                let mut ws = false;
                for c in text.chars() {
                    if c.is_whitespace() {
                        ws = true;
                    } else {
                        if ws {
                            o.push(' ');
                        }
                        ws = false;
                        let mut b = [0u8; 4];
                        esc(c.encode_utf8(&mut b), &mut o);
                    }
                }
                if ws {
                    o.push(' ');
                }
            }
            toks.push(Tok::Text(o));
            i = e;
        }
    }
    let mut out = String::new();
    let mut pre = 0i32;
    for k in 0..toks.len() {
        match &toks[k] {
            Tok::Tag { s, .. } => {
                if s.starts_with("<pre") {
                    pre += 1;
                } else if s.starts_with("</pre") {
                    pre -= 1;
                }
                out.push_str(s);
            }
            Tok::Text(t) => {
                if pre > 0 {
                    out.push_str(t);
                    continue;
                }
                let prev_block = k == 0 || matches!(toks[k - 1], Tok::Tag { block: true, .. });
                let next_block = k + 1 == toks.len() || matches!(toks[k + 1], Tok::Tag { block: true, .. });
                let mut t = t.as_str();
                if prev_block {
                    t = t.trim_start();
                }
                if next_block {
                    t = t.trim_end();
                }
                out.push_str(t);
            }
        }
    }
    out
}

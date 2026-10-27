//! Safe-by-default handling of raw HTML and URLs in exported HTML (decision C1).
//!
//! - URLs: only `http`, `https`, `mailto`, `tel`, relative URLs and `#anchors`; images may also
//!   use `file:` and `data:image/{png,gif,jpeg,webp}`.
//! - Raw HTML: an allow-list of elements and attributes; `on*` handlers, scripts, styles and
//!   other active content are removed; `<iframe>` always gets `sandbox`.
//!
//! [`crate::HtmlOptions::trusted`] disables all of this for documents the user trusts.

use std::fmt::Write as _;

use crate::escape_html;

const ALLOWED_TAGS: &[&str] = &[
    "a",
    "abbr",
    "address",
    "article",
    "aside",
    "audio",
    "b",
    "bdi",
    "bdo",
    "blockquote",
    "br",
    "caption",
    "center",
    "cite",
    "code",
    "col",
    "colgroup",
    "dd",
    "del",
    "details",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "font",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "i",
    "iframe",
    "img",
    "ins",
    "kbd",
    "li",
    "main",
    "mark",
    "nav",
    "ol",
    "p",
    "picture",
    "pre",
    "q",
    "rp",
    "rt",
    "ruby",
    "s",
    "samp",
    "section",
    "small",
    "source",
    "span",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "time",
    "tr",
    "track",
    "u",
    "ul",
    "var",
    "video",
    "wbr",
];

/// Elements removed together with their content.
const DROP_CONTENT: &[&str] = &[
    "script",
    "style",
    "noscript",
    "template",
    "textarea",
    "title",
    "head",
    "object",
    "applet",
    "embed",
    "frameset",
    "svg",
    "math",
    "select",
    "button",
    "xmp",
    "plaintext",
    "noembed",
    "noframes",
];

const ALLOWED_ATTRS: &[&str] = &[
    "align",
    "allowfullscreen",
    "alt",
    "bgcolor",
    "border",
    "cellpadding",
    "cellspacing",
    "cite",
    "class",
    "color",
    "colspan",
    "controls",
    "datetime",
    "dir",
    "face",
    "frameborder",
    "headers",
    "height",
    "href",
    "id",
    "lang",
    "loop",
    "muted",
    "name",
    "open",
    "poster",
    "rel",
    "reversed",
    "rowspan",
    "scope",
    "size",
    "span",
    "src",
    "start",
    "style",
    "target",
    "title",
    "type",
    "valign",
    "value",
    "width",
];

const URL_ATTRS: &[&str] = &["href", "src", "cite", "poster"];

/// Decodes character references in an attribute value (so `&#106;avascript:` is caught).
fn decode_refs(v: &str) -> String {
    markise_md::inline::decode_refs(v)
}

/// `Some(url)` when the URL may be emitted (scheme allow-list); `image` also allows `file:`
/// and raster `data:` images.
pub fn safe_url(url: &str, image: bool) -> Option<&str> {
    let probe: String = url.chars().filter(|c| !c.is_whitespace() && !c.is_control()).collect();
    let lower = probe.to_ascii_lowercase();
    let scheme_end = lower.find(':');
    let first_sep = lower.find(['/', '?', '#']);
    let scheme = match (scheme_end, first_sep) {
        (Some(c), Some(s)) if s < c => None,
        (Some(c), _) => Some(&lower[..c]),
        (None, _) => None,
    };
    let ok = match scheme {
        None => true,
        Some("http" | "https" | "mailto" | "tel") => true,
        Some("file") => image,
        Some("data") => {
            image
                && ["data:image/png", "data:image/gif", "data:image/jpeg", "data:image/webp"]
                    .iter()
                    .any(|p| lower.starts_with(p))
        }
        // Windows drive paths `C:\…` / `C:/…` are local files.
        Some(s) if s.len() == 1 && s.as_bytes()[0].is_ascii_alphabetic() => image,
        _ => false,
    };
    ok.then_some(url)
}

/// CSS properties kept in untrusted export (C40); everything else is dropped.
const STYLE_PROPS: &[&str] = &[
    "text-align",
    "vertical-align",
    "color",
    "background-color",
    "font-weight",
    "font-style",
    "text-decoration",
    "width",
    "height",
    "max-width",
    "zoom",
];

/// Filters a `style` attribute to allow-listed properties with plain values (no escapes,
/// functions, at-rules, comments or URLs). Returns `None` when nothing remains.
fn filter_style(v: &str) -> Option<String> {
    let l = v.to_ascii_lowercase();
    if l.contains(['\\', '(', '@', '/']) || l.contains("expression") || l.contains("url") || l.contains("javascript") {
        return None;
    }
    let mut kept = Vec::new();
    for decl in l.split(';') {
        let Some((prop, val)) = decl.split_once(':') else {
            continue;
        };
        let (prop, val) = (prop.trim(), val.trim());
        if STYLE_PROPS.contains(&prop)
            && !val.is_empty()
            && val.bytes().all(|c| c.is_ascii_alphanumeric() || b" #%.,-".contains(&c))
        {
            kept.push(format!("{prop}:{val}"));
        }
    }
    (!kept.is_empty()).then(|| kept.join(";"))
}

/// Streaming sanitizer; keeps state across fragments (inline HTML of one block arrives as
/// separate tag nodes).
#[derive(Default)]
pub struct Sanitizer {
    /// Inside an element whose content is dropped (`script` …): its name.
    skip_until: Option<String>,
}

struct Tag<'a> {
    name: String,
    closing: bool,
    self_closing: bool,
    attrs: Vec<(String, Option<String>)>,
    len: usize,
    _src: &'a str,
}

fn parse_tag<'a>(s: &'a str, quote_fail: &mut [bool; 2]) -> Option<Tag<'a>> {
    let b = s.as_bytes();
    let mut q = 1;
    let closing = b.get(1) == Some(&b'/');
    if closing {
        q = 2;
    }
    let ns = q;
    while q < b.len() && (b[q].is_ascii_alphanumeric() || b[q] == b'-') {
        q += 1;
    }
    if q == ns || !b[ns].is_ascii_alphabetic() {
        return None;
    }
    let name = s[ns..q].to_ascii_lowercase();
    let mut attrs = Vec::new();
    loop {
        while q < b.len() && b[q].is_ascii_whitespace() {
            q += 1;
        }
        match b.get(q)? {
            b'>' => {
                return Some(Tag {
                    name,
                    closing,
                    self_closing: false,
                    attrs,
                    len: q + 1,
                    _src: s,
                });
            }
            b'/' if b.get(q + 1) == Some(&b'>') => {
                return Some(Tag {
                    name,
                    closing,
                    self_closing: true,
                    attrs,
                    len: q + 2,
                    _src: s,
                });
            }
            _ => {
                let an = q;
                while q < b.len() && !b[q].is_ascii_whitespace() && !matches!(b[q], b'=' | b'>' | b'/' | b'"' | b'\'') {
                    if b[q] == b'<' {
                        return None; // a new tag starts: not a well-formed tag (keeps scans short)
                    }
                    q += 1;
                }
                if q == an {
                    // stray quote or slash: skip it
                    q += 1;
                    continue;
                }
                let aname = s[an..q].to_ascii_lowercase();
                let mut p = q;
                while p < b.len() && b[p].is_ascii_whitespace() {
                    p += 1;
                }
                if b.get(p) == Some(&b'=') {
                    p += 1;
                    while p < b.len() && b[p].is_ascii_whitespace() {
                        p += 1;
                    }
                    let val;
                    match b.get(p)? {
                        &qc @ (b'"' | b'\'') => {
                            let qi = (qc == b'\'') as usize;
                            if quote_fail[qi] {
                                return None;
                            }
                            let Some(e) = memchr::memchr(qc, &b[p + 1..]).map(|e| e + p + 1) else {
                                quote_fail[qi] = true; // no such quote in the rest of the input
                                return None;
                            };
                            val = s[p + 1..e].to_string();
                            q = e + 1;
                        }
                        _ => {
                            let vs = p;
                            while p < b.len() && !b[p].is_ascii_whitespace() && b[p] != b'>' {
                                if b[p] == b'<' {
                                    return None;
                                }
                                p += 1;
                            }
                            val = s[vs..p].to_string();
                            q = p;
                        }
                    }
                    attrs.push((aname, Some(val)));
                } else {
                    attrs.push((aname, None));
                }
            }
        }
    }
}

impl Sanitizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` while content is being dropped (inside `<script>` etc.).
    pub fn skipping(&self) -> bool {
        self.skip_until.is_some()
    }

    /// Sanitizes `html` and appends the result to `out`.
    pub fn push(&mut self, html: &str, out: &mut String) {
        let mut quote_fail = [false; 2];
        let mut i = 0;
        let b = html.as_bytes();
        while i < b.len() {
            if let Some(name) = &self.skip_until {
                // look for the matching end tag (case-insensitive, no copying)
                match find_end_tag(html, i, name) {
                    Some(after) => {
                        let close = html[after..].find('>').map(|e| after + e + 1).unwrap_or(html.len());
                        self.skip_until = None;
                        i = close;
                        continue;
                    }
                    None => return,
                }
            }
            let next = html[i..].find('<').map(|p| i + p).unwrap_or(html.len());
            // text: keep entities, escape stray `>`
            for c in html[i..next].chars() {
                if c == '>' {
                    out.push_str("&gt;");
                } else {
                    out.push(c);
                }
            }
            i = next;
            if i >= b.len() {
                break;
            }
            let rest = &html[i..];
            if rest.starts_with("<!--") {
                i = rest.find("-->").map(|e| i + e + 3).unwrap_or(html.len());
                continue;
            }
            if rest.starts_with("<!") || rest.starts_with("<?") {
                i = rest.find('>').map(|e| i + e + 1).unwrap_or(html.len());
                continue;
            }
            match parse_tag(rest, &mut quote_fail) {
                None => {
                    out.push_str("&lt;");
                    i += 1;
                }
                Some(tag) => {
                    i += tag.len;
                    if DROP_CONTENT.contains(&tag.name.as_str()) {
                        if !tag.closing && !tag.self_closing {
                            self.skip_until = Some(tag.name);
                        }
                        continue;
                    }
                    if !ALLOWED_TAGS.contains(&tag.name.as_str()) {
                        continue;
                    }
                    if tag.closing {
                        let _ = write!(out, "</{}>", tag.name);
                        continue;
                    }
                    let _ = write!(out, "<{}", tag.name);
                    let mut has_sandbox = false;
                    for (k, v) in &tag.attrs {
                        if k == "sandbox" && tag.name == "iframe" {
                            has_sandbox = true;
                        }
                        if !ALLOWED_ATTRS.contains(&k.as_str()) {
                            continue;
                        }
                        match v {
                            None => {
                                let _ = write!(out, " {k}");
                            }
                            Some(v) => {
                                let dv = decode_refs(v);
                                if URL_ATTRS.contains(&k.as_str())
                                    && safe_url(&dv, tag.name == "img" || tag.name == "source").is_none()
                                {
                                    continue;
                                }
                                let dv = if k == "style" {
                                    match filter_style(&dv) {
                                        Some(f) => f,
                                        None => continue,
                                    }
                                } else {
                                    dv
                                };
                                let _ = write!(out, " {k}=\"");
                                escape_html(&dv, out);
                                out.push('"');
                            }
                        }
                    }
                    if tag.name == "iframe" || has_sandbox {
                        out.push_str(" sandbox=\"\"");
                    }
                    out.push_str(if tag.self_closing { " />" } else { ">" });
                }
            }
        }
    }
}

/// Position of the next `</name` (ASCII case-insensitive) at or after `from`.
fn find_end_tag(html: &str, from: usize, name: &str) -> Option<usize> {
    let b = html.as_bytes();
    let n = name.as_bytes();
    let mut i = from;
    while let Some(off) = memchr::memmem::find(&b[i..], b"</") {
        let p = i + off;
        let s = p + 2;
        if b.len() >= s + n.len()
            && b[s..s + n.len()].eq_ignore_ascii_case(n)
            && b.get(s + n.len())
                .is_none_or(|c| c.is_ascii_whitespace() || *c == b'>' || *c == b'/')
        {
            return Some(p);
        }
        i = p + 2;
    }
    None
}

/// Sanitizes a complete HTML fragment.
pub fn sanitize(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    Sanitizer::new().push(html, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert!(safe_url("https://x", false).is_some());
        assert!(safe_url("rel/path.html", false).is_some());
        assert!(safe_url("#anchor", false).is_some());
        assert!(safe_url("mailto:a@b", false).is_some());
        assert!(safe_url("javascript:alert(1)", false).is_none());
        assert!(safe_url(" JaVa\tScript:alert(1)", false).is_none());
        assert!(safe_url("vbscript:x", false).is_none());
        assert!(safe_url("data:text/html,<b>", false).is_none());
        assert!(safe_url("data:image/png;base64,AA", true).is_some());
        assert!(safe_url("file:///a.png", true).is_some());
        assert!(safe_url("file:///a.png", false).is_none());
        assert!(safe_url("C:\\img\\a.png", true).is_some());
        assert!(safe_url("a/b:c", false).is_some());
    }

    #[test]
    fn strips_active_content() {
        assert_eq!(sanitize("<script>alert(1)</script>ok"), "ok");
        assert_eq!(sanitize("<SCRIPT src=x></SCRIPT>ok"), "ok");
        assert_eq!(sanitize("<b onclick=\"x()\">b</b>"), "<b>b</b>");
        assert_eq!(sanitize("<a href=\"javascript:alert(1)\">x</a>"), "<a>x</a>");
        assert_eq!(sanitize("<a href=\"&#106;avascript:alert(1)\">x</a>"), "<a>x</a>");
        assert_eq!(sanitize("<img src=x onerror=alert(1)>"), "<img src=\"x\">");
        assert_eq!(
            sanitize("<iframe src=\"https://e.com\"></iframe>"),
            "<iframe src=\"https://e.com\" sandbox=\"\"></iframe>"
        );
        assert_eq!(
            sanitize("<div style=\"background:url(javascript:x)\">a</div>"),
            "<div>a</div>"
        );
        assert_eq!(sanitize("<style>body{}</style><p>t</p>"), "<p>t</p>");
        assert_eq!(sanitize("<!-- c --><u>x</u>"), "<u>x</u>");
        assert_eq!(sanitize("<p title='a\"b'>t</p>"), "<p title=\"a&quot;b\">t</p>");
        assert_eq!(sanitize("<form><input></form>t"), "t");
    }

    // Review pass 2 #7: CSS escapes / functions cannot smuggle URLs; allow-listed props only.
    #[test]
    fn style_policy() {
        assert_eq!(
            sanitize("<div style=\"background:u\\72l(https://e.com/p)\">x</div>"),
            "<div>x</div>"
        );
        assert_eq!(
            sanitize("<div style=\"color:red;position:fixed\">x</div>"),
            "<div style=\"color:red\">x</div>"
        );
        assert_eq!(
            sanitize("<img src=\"a.png\" style=\"zoom:50%;\" />"),
            "<img src=\"a.png\" style=\"zoom:50%\" />"
        );
        assert_eq!(
            sanitize("<p style=\"text-align: center\">t</p>"),
            "<p style=\"text-align:center\">t</p>"
        );
        assert_eq!(sanitize("<p style=\"color:rgb(1,2,3)\">t</p>"), "<p>t</p>");
        assert_eq!(sanitize("<p style=\"width:1px/**/\">t</p>"), "<p>t</p>");
        assert_eq!(sanitize("<p style=\"@import x\">t</p>"), "<p>t</p>");
    }

    // Review pass 2 #4: bounded entity search, copy-free end-tag search.
    #[test]
    fn entity_and_end_tag_edge_cases() {
        assert_eq!(
            sanitize("<div title=\"&&&&;\">x</div>"),
            "<div title=\"&amp;&amp;&amp;&amp;;\">x</div>"
        );
        assert_eq!(sanitize("<div title=\"&amp;\">x</div>"), "<div title=\"&amp;\">x</div>");
        assert_eq!(sanitize("<ScRiPt>x</sCrIpT >ok"), "ok");
        assert_eq!(sanitize("<script>x</scripts></script>ok"), "ok");
        assert_eq!(sanitize("<a title=\"x <b>y</b>"), "&lt;a title=\"x <b>y</b>");
    }

    #[test]
    fn streaming_skip_across_fragments() {
        let mut s = Sanitizer::new();
        let mut out = String::new();
        s.push("<script>", &mut out);
        assert!(s.skipping());
        s.push("</script>", &mut out);
        assert!(!s.skipping());
        assert_eq!(out, "");
    }
}

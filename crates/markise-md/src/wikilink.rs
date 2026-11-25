//! Wikilink syntax (spec 06 V2.1): the parts of the text between `[[` and `]]`.
//!
//! Recognition inside inline content lives in [`crate::inline`]; this module only splits the
//! content into target, subpath and display text, and derives the collapsed label (V2.2). The
//! source is never rewritten (V2.9): everything here is a read-only view of it.

use std::ops::Range;

/// The parts of a wikilink's content (`target#subpath|display`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WikiParts {
    /// The link target, trimmed (V2.1.2 rule 4); empty for a same-note link (`[[#Heading]]`).
    /// A backslash directly before the display separator is not part of it (rule 2).
    pub target: String,
    /// Everything after the first `#` of the destination, trimmed (`Heading`, `A#B`, `^id`).
    pub subpath: Option<String>,
    /// The display text's range in the content (after the first `|`), untrimmed.
    pub display: Option<Range<usize>>,
}

/// Splits wikilink content (the text between `[[` and `]]`, V2.1.2). `None` when it is not a
/// link: empty, spanning lines, containing `[[`, or with neither a target nor a subpath.
pub fn split(content: &str) -> Option<WikiParts> {
    if content.is_empty() || content.contains('\n') || content.contains("[[") || content.contains("]]") {
        return None;
    }
    // rule 2: the first `|` ends the destination (a backslash before it is a table escape)
    let (dest, display) = match content.find('|') {
        Some(i) => {
            let d = &content[..i];
            (d.strip_suffix('\\').unwrap_or(d), Some(i + 1..content.len()))
        }
        None => (content, None),
    };
    // rule 3: the first `#` ends the target
    let (target, subpath) = match dest.find('#') {
        Some(i) => (&dest[..i], Some(dest[i + 1..].trim())),
        None => (dest, None),
    };
    let target = target.trim();
    let subpath = subpath.filter(|s| !s.is_empty());
    if target.is_empty() && subpath.is_none() {
        return None;
    }
    Some(WikiParts {
        target: target.to_string(),
        subpath: subpath.map(str::to_string),
        display,
    })
}

impl WikiParts {
    /// Heading segments of the subpath (`A#B` → `["A", "B"]`), trimmed, empty ones dropped;
    /// empty for a block reference (`^id`) or no subpath.
    pub fn headings(&self) -> Vec<&str> {
        match &self.subpath {
            Some(s) if !s.starts_with('^') => s.split('#').map(str::trim).filter(|x| !x.is_empty()).collect(),
            _ => Vec::new(),
        }
    }

    /// The collapsed label (V2.2): the display text when given, else the target without a
    /// trailing `.md` followed by the subpath with each `#` shown as ` › `.
    pub fn label(&self, content: &str) -> String {
        if let Some(r) = &self.display {
            let d = content[r.clone()].trim();
            if !d.is_empty() {
                return d.to_string();
            }
        }
        let t = strip_md(&self.target);
        let mut parts: Vec<&str> = Vec::new();
        if !t.is_empty() {
            parts.push(t);
        }
        if let Some(s) = &self.subpath {
            parts.extend(s.split('#').map(str::trim).filter(|x| !x.is_empty()));
        }
        parts.join(" › ")
    }

    /// The canonical link string the viewer hands to the application for this link:
    /// `[[target#subpath]]` (display text dropped).
    pub fn link_string(&self) -> String {
        match &self.subpath {
            Some(s) => format!("[[{}#{}]]", self.target, s),
            None => format!("[[{}]]", self.target),
        }
    }
}

/// File extensions an embed `![[name.ext]]` shows as an image (compared without case): the
/// formats the image loader decodes (the `image` crate features in the workspace `Cargo.toml`).
/// SVG and AVIF are not decoded for documents, so such an embed is a link like any other file.
pub const IMAGE_EXTENSIONS: &[&str] = &["bmp", "gif", "jpeg", "jpg", "png", "webp"];

/// Whether an embed's target names an image file by its extension.
pub fn is_image(target: &str) -> bool {
    target
        .rsplit_once('.')
        .is_some_and(|(stem, e)| !stem.is_empty() && IMAGE_EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// The size an image embed asks for, in CSS pixels: `|300` (a width) or `|300x200` (width and
/// height); `None` for any other display text, which is then the image's description.
pub fn embed_size(display: &str) -> Option<(u32, Option<u32>)> {
    let d = display.trim();
    let num = |t: &str| -> Option<u32> {
        (!t.is_empty() && t.len() <= 5 && t.bytes().all(|b| b.is_ascii_digit()))
            .then(|| t.parse().ok())
            .flatten()
            .filter(|&n| n > 0)
    };
    match d.split_once(['x', 'X']) {
        Some((w, h)) => Some((num(w)?, Some(num(h)?))),
        None => Some((num(d)?, None)),
    }
}

impl WikiParts {
    /// An image embed's description: its display text unless that is a size.
    pub fn embed_alt(&self, content: &str) -> String {
        match &self.display {
            Some(r) if embed_size(&content[r.clone()]).is_none() => content[r.clone()].trim().to_string(),
            _ => String::new(),
        }
    }

    /// An image embed's size ([`embed_size`]), when its display text is one.
    pub fn embed_size(&self, content: &str) -> Option<(u32, Option<u32>)> {
        self.display.as_ref().and_then(|r| embed_size(&content[r.clone()]))
    }

    /// Whether `![[content]]` shows an image: the target names an image file and no subpath.
    pub fn is_image_embed(&self) -> bool {
        self.subpath.is_none() && is_image(&self.target)
    }
}

/// `t` without a trailing `.md` (any case).
pub fn strip_md(t: &str) -> &str {
    let n = t.len();
    if n >= 3 && t.is_char_boundary(n - 3) && t[n - 3..].eq_ignore_ascii_case(".md") {
        &t[..n - 3]
    } else {
        t
    }
}

/// The parts of a canonical link string ([`WikiParts::link_string`], or any `[[…]]` text).
pub fn parse_link_string(s: &str) -> Option<WikiParts> {
    split(s.strip_prefix("[[")?.strip_suffix("]]")?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Option<(String, Option<String>, Option<String>)> {
        split(s).map(|w| {
            (
                w.target.clone(),
                w.subpath.clone(),
                w.display.clone().map(|r| s[r].to_string()),
            )
        })
    }

    #[test]
    fn forms_v2_1_1() {
        assert_eq!(p("Note"), Some(("Note".into(), None, None)));
        assert_eq!(p("Note.md"), Some(("Note.md".into(), None, None)));
        assert_eq!(p("folder/Note"), Some(("folder/Note".into(), None, None)));
        assert_eq!(
            p("Note|Shown text"),
            Some(("Note".into(), None, Some("Shown text".into())))
        );
        assert_eq!(p("Note#Heading"), Some(("Note".into(), Some("Heading".into()), None)));
        assert_eq!(
            p("Note#Heading#Sub"),
            Some(("Note".into(), Some("Heading#Sub".into()), None))
        );
        assert_eq!(p("Note#^blockid"), Some(("Note".into(), Some("^blockid".into()), None)));
        assert_eq!(p("#Heading"), Some(("".into(), Some("Heading".into()), None)));
        assert_eq!(p("Figure 1.png"), Some(("Figure 1.png".into(), None, None)));
        assert_eq!(
            p("Note#Heading|alias"),
            Some(("Note".into(), Some("Heading".into()), Some("alias".into())))
        );
    }

    #[test]
    fn grammar_v2_1_2() {
        // L31: empty content
        assert_eq!(p(""), None);
        assert_eq!(p("   "), None);
        assert_eq!(p("a\nb"), None);
        assert_eq!(p("a[[b"), None);
        // L24: whitespace trimmed for resolution
        assert_eq!(p("  Plan  "), Some(("Plan".into(), None, None)));
        // L27: the first `|` splits, the display keeps further pipes
        assert_eq!(p("Ideas|a|b"), Some(("Ideas".into(), None, Some("a|b".into()))));
        // L26 / rule 2: a backslash before the first pipe is dropped from the target
        assert_eq!(
            p("Ideas\\|My ideas"),
            Some(("Ideas".into(), None, Some("My ideas".into())))
        );
        // L19: `#` always splits the target
        assert_eq!(p("Hash#Tag"), Some(("Hash".into(), Some("Tag".into()), None)));
        // the subpath's own `|` is the display separator, not part of the subpath
        assert_eq!(p("a#b|c#d"), Some(("a".into(), Some("b".into()), Some("c#d".into()))));
    }

    #[test]
    fn collapsed_label_v2_2() {
        let l = |s: &str| split(s).unwrap().label(s);
        assert_eq!(l("Note"), "Note");
        assert_eq!(l("Note.md"), "Note");
        assert_eq!(l("Note.MD"), "Note");
        assert_eq!(l("Report v1.8"), "Report v1.8");
        assert_eq!(l("Note#Heading"), "Note › Heading");
        assert_eq!(l("Note#A#B"), "Note › A › B");
        assert_eq!(l("Note#^id"), "Note › ^id");
        assert_eq!(l("#Heading"), "Heading");
        assert_eq!(l("Note|  My note "), "My note");
        assert_eq!(l("folder/Note"), "folder/Note");
        assert_eq!(l("Café"), "Café");
    }

    #[test]
    fn link_strings_round_trip() {
        let w = split("Plan#Goals|the plan").unwrap();
        assert_eq!(w.link_string(), "[[Plan#Goals]]");
        assert_eq!(
            parse_link_string(&w.link_string()).unwrap().subpath.as_deref(),
            Some("Goals")
        );
        assert_eq!(w.headings(), vec!["Goals"]);
        assert!(split("x#^b").unwrap().headings().is_empty());
        assert_eq!(parse_link_string("[[x]]").unwrap().target, "x");
        assert!(parse_link_string("x").is_none());
    }

    #[test]
    fn image_embeds() {
        assert!(is_image("a.png") && is_image("dir/B.JPG") && is_image("x y.webp"));
        assert!(!is_image("note") && !is_image("a.md") && !is_image(".png") && !is_image("a.pdf"));
        // not decoded by the image loader: a link, not an error placeholder
        assert!(!is_image("a.svg") && !is_image("a.AVIF"));
        assert_eq!(embed_size("300"), Some((300, None)));
        assert_eq!(embed_size(" 300x200 "), Some((300, Some(200))));
        assert_eq!(embed_size("0"), None);
        assert_eq!(embed_size("300x"), None);
        assert_eq!(embed_size("a cat"), None);
        assert_eq!(embed_size("1234567"), None);
        let c = "cat.png|A cat";
        let w = split(c).unwrap();
        assert!(w.is_image_embed());
        assert_eq!(w.embed_alt(c), "A cat");
        assert_eq!(w.embed_size(c), None);
        let c = "cat.png|120";
        let w = split(c).unwrap();
        assert_eq!((w.embed_alt(c).as_str(), w.embed_size(c)), ("", Some((120, None))));
        assert!(!split("cat.png#x").unwrap().is_image_embed());
    }
}

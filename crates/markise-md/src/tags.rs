//! Tags (spec 06 §10, ADR 0027): `#tag` in text and `tags:` in front matter.
//!
//! Recognition inside inline content is a pass over the parsed text runs
//! ([`crate::inline::parse_inlines`], [`ParseOptions::tags`](crate::ParseOptions::tags)): what
//! is code, math, HTML, a link or a URL is never a text run, so it never holds a tag. This
//! module holds the character rules, the front-matter reader and [`document_tags`], which the
//! folder index uses, so the index and the editor agree on what a tag is. The source is never
//! rewritten: a tag is a read-only view of its bytes.

use crate::Document;
use crate::block::{Block, BlockKind};
use crate::inline::{InlineKind, parse_inlines};

/// Longest tag body taken (bytes): a guard against pathological input, far above any real tag.
pub const MAX_TAG_LEN: usize = 256;

/// A character of a tag body: letters, combining marks and digits of any script, `_`, `-`, `/`,
/// other symbols such as emoji, and the zero-width joiner of emoji sequences.
pub fn is_tag_char(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/');
    }
    use unicode_properties::{GeneralCategory, GeneralCategoryGroup, UnicodeGeneralCategory};
    c == '\u{200D}'
        || matches!(
            c.general_category_group(),
            GeneralCategoryGroup::Letter | GeneralCategoryGroup::Mark | GeneralCategoryGroup::Number
        )
        || c.general_category() == GeneralCategory::OtherSymbol
}

/// The length in bytes of the tag body at the start of `s` (the text right after `#`), or
/// `None` when no tag starts there (V10.1): the body ends at the first character that is not
/// a tag character; a `/` that would end the body or start an empty segment is not part of
/// it; a body of digits only (`#1984`) is no tag.
pub fn body_len(s: &str) -> Option<usize> {
    let mut end = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if !is_tag_char(c) || i + c.len_utf8() > MAX_TAG_LEN {
            break;
        }
        if c == '/' && chars.peek().is_none_or(|&(_, n)| n == '/' || !is_tag_char(n)) {
            break;
        }
        end = i + c.len_utf8();
    }
    let body = &s[..end];
    (!body.is_empty() && body.starts_with(|c: char| c != '/') && !body.chars().all(|c| c.is_numeric())).then_some(end)
}

/// Whether a tag may start at a `#` preceded by `before` (`None`: the start of the content):
/// at the start, after whitespace or after `(` (V10.1), or right after an emphasis delimiter
/// (`**#tag**`, the boundary rule of bare URLs). `a#b` and URL fragments are no tags.
pub fn may_start_after(before: Option<char>) -> bool {
    before.is_none_or(|c| c.is_whitespace() || matches!(c, '(' | '*' | '_' | '~' | '='))
}

/// The tag at byte `pos` of `s` (`s[pos]` is `#`): the end of the tag (exclusive), or `None`.
pub fn tag_at(s: &str, pos: usize) -> Option<usize> {
    if s.as_bytes().get(pos) != Some(&b'#') || !may_start_after(s[..pos].chars().next_back()) {
        return None;
    }
    body_len(&s[pos + 1..]).map(|n| pos + 1 + n)
}

/// The comparison key of a tag: lower case (tags are case-insensitive, V10.3).
pub fn fold(tag: &str) -> String {
    tag.to_lowercase()
}

/// A front-matter item as a tag body: quotes and one leading `#` removed, trimmed. `None` when
/// it is not a whole tag (it holds whitespace or other characters a tag cannot have, V10.2).
fn item_tag(item: &str) -> Option<String> {
    let t = unquote(item.trim());
    let t = t.trim();
    let t = t.strip_prefix('#').unwrap_or(t);
    let t = t.strip_suffix('/').unwrap_or(t);
    (body_len(t) == Some(t.len())).then(|| t.to_string())
}

fn unquote(v: &str) -> &str {
    let b = v.as_bytes();
    if b.len() >= 2 && ((b[0] == b'"' && b[b.len() - 1] == b'"') || (b[0] == b'\'' && b[b.len() - 1] == b'\'')) {
        &v[1..v.len() - 1]
    } else {
        v
    }
}

/// A `key: value` line at the top level of the front matter: the key (unquoted) and the value.
fn top_key(line: &str) -> Option<(&str, &str)> {
    if line.starts_with([' ', '\t', '-', '#']) {
        return None;
    }
    let colon = line.find(':')?;
    let key = unquote(line[..colon].trim());
    Some((key, &line[colon + 1..]))
}

/// A value with a trailing ` # comment` removed (not inside quotes or brackets of a flow list).
fn strip_comment(v: &str) -> &str {
    let mut quote = None;
    let mut prev_ws = true;
    for (i, c) in v.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '#') if prev_ws => return v[..i].trim_end(),
            _ => {}
        }
        prev_ws = c.is_whitespace();
    }
    v
}

/// Splits a flow list's inner text on commas outside quotes.
fn flow_items(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut quote = None;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, ',') => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// The tags of front matter `content` (the lines between the delimiters, V9.2, V10.2): the
/// values of `tags` and `tag` (any letter case) as a flow list `[a, b]`, a block list of
/// `- item` lines, or a scalar split on commas and whitespace (`a, b c`). Items are tag bodies
/// with an optional leading `#`; an item that is not a whole tag is ignored. Other keys,
/// nested values and anything not understood are ignored.
pub fn front_matter_tags(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut lines = content.lines().peekable();
    while let Some(line) = lines.next() {
        let line = line.trim_start_matches('\u{FEFF}').trim_end();
        let Some((key, value)) = top_key(line) else { continue };
        if !key.eq_ignore_ascii_case("tags") && !key.eq_ignore_ascii_case("tag") {
            continue;
        }
        let value = strip_comment(value.trim());
        if value.is_empty() {
            // a block list: `- item` lines, indented or not, until the next key
            while let Some(next) = lines.peek() {
                let t = next.trim();
                if t.is_empty() || t.starts_with('#') {
                    lines.next();
                    continue;
                }
                let Some(item) = t
                    .strip_prefix('-')
                    .filter(|r| r.is_empty() || r.starts_with([' ', '\t']))
                else {
                    break;
                };
                lines.next();
                out.extend(item_tag(strip_comment(item.trim())));
            }
        } else if let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
            out.extend(flow_items(inner).into_iter().filter_map(item_tag));
        } else if value.starts_with(['"', '\'']) {
            out.extend(item_tag(value));
        } else {
            out.extend(
                value
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|s| !s.is_empty())
                    .filter_map(item_tag),
            );
        }
    }
    out
}

/// Adds `tag` unless a tag with the same key is already in `out` (first casing wins).
fn push_unique(out: &mut Vec<String>, tag: String) {
    let k = fold(&tag);
    if !out.iter().any(|t| fold(t) == k) {
        out.push(tag);
    }
}

/// Collects the tags of inline nodes into `out` (`s`: the content the spans point into).
fn inline_tags(s: &str, nodes: &[crate::inline::Inline], out: &mut Vec<String>) {
    for n in nodes {
        match &n.kind {
            InlineKind::Tag => push_unique(out, s[n.span.start + 1..n.span.end].to_string()),
            _ => inline_tags(s, &n.children, out),
        }
    }
}

/// Every tag of a parsed document (spec 06 §6, V10.1–V10.3): front-matter tags first, then the
/// `#tags` of its text in order, each once (compared without regard to case; the first casing
/// is kept). Inline tags are found exactly as the editor finds them (the document's own parse
/// options); front-matter tags are read whatever the options say.
pub fn document_tags(doc: &Document) -> Vec<String> {
    let text = doc.text();
    let mut out = Vec::new();
    let ctx = &doc.ref_context().inline;
    let opts = doc.options();
    fn visit(
        b: &Block,
        text: &str,
        opts: &crate::ParseOptions,
        ctx: &crate::inline::InlineContext,
        out: &mut Vec<String>,
    ) {
        match &b.kind {
            BlockKind::FrontMatter { .. } => {
                for t in front_matter_tags(&b.content(text)) {
                    push_unique(out, t);
                }
            }
            BlockKind::Paragraph | BlockKind::Heading { .. } if opts.tags => {
                let c = b.content(text);
                if c.contains('#') {
                    inline_tags(&c, &parse_inlines(&c, opts, ctx), out);
                }
            }
            BlockKind::Table { .. } if opts.tags => {
                // header and body rows (the delimiter row holds no text)
                for (i, l) in b.lines.iter().enumerate() {
                    let row = l.slice(text);
                    if i != 1 && row.contains('#') {
                        inline_tags(row, &parse_inlines(row, opts, ctx), out);
                    }
                }
            }
            _ => {}
        }
        for c in &b.children {
            visit(c, text, opts, ctx, out);
        }
    }
    for b in doc.tree() {
        visit(&b, text, opts, ctx, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodies() {
        assert_eq!(body_len("idea today"), Some(4));
        assert_eq!(body_len("1984"), None);
        assert_eq!(body_len("y1984"), Some(5));
        assert_eq!(body_len("a/b/c."), Some(5));
        assert_eq!(body_len("tag/"), Some(3));
        assert_eq!(body_len("a//b"), Some(1));
        assert_eq!(body_len("/a"), None);
        assert_eq!(body_len(""), None);
        assert_eq!(body_len(" x"), None);
        assert_eq!(body_len("snake_case-x,"), Some(12));
        assert_eq!(body_len("привет!"), Some("привет".len()));
        assert_eq!(body_len("日本語"), Some("日本語".len()));
        assert_eq!(body_len("🚀ship"), Some("🚀ship".len()));
        assert_eq!(body_len("a.b"), Some(1));
    }

    #[test]
    fn starts() {
        assert_eq!(tag_at("#a", 0), Some(2));
        assert_eq!(tag_at("x #a", 2), Some(4));
        assert_eq!(tag_at("(#a)", 1), Some(3));
        assert_eq!(tag_at("a#b", 1), None);
        assert_eq!(tag_at("/#frag", 1), None);
        assert_eq!(tag_at("# a", 0), None);
    }

    #[test]
    fn front_matter_forms() {
        assert_eq!(front_matter_tags("tags: [a, b/c]"), ["a", "b/c"]);
        assert_eq!(front_matter_tags("tags:\n  - a\n  - \"#b\"\ntitle: x"), ["a", "b"]);
        assert_eq!(front_matter_tags("tags:\n- a\n- b # note\nother: 1"), ["a", "b"]);
        assert_eq!(front_matter_tags("tags: [has space, ok]"), ["ok"]);
        assert_eq!(front_matter_tags("tags: one, two three"), ["one", "two", "three"]);
        assert_eq!(front_matter_tags("tag: single"), ["single"]);
        assert_eq!(front_matter_tags("Tags: '#quoted'"), ["quoted"]);
        assert_eq!(front_matter_tags("tags: [\"a, b\", c]"), ["c"]);
        assert_eq!(
            front_matter_tags("title: x\n  tags: [nested]\ntags: a # comment"),
            ["a"]
        );
        assert_eq!(front_matter_tags("tags:\n\nnext: [x]"), Vec::<String>::new());
        assert_eq!(front_matter_tags("tags: [1984, x1]"), ["x1"]);
    }
}

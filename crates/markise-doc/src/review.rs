//! Review marks (ADR 0012): the comments, highlights and suggested changes of a document,
//! found through the neutral inline trees ([`InlineKind::Critic`]), so the comments panel,
//! the review planners and clean copies work the same for every format that reports them.
//!
//! Cost: a document without review marks costs one byte scan for the opening delimiters
//! (`{++`, `{--`, `{~~`, `{==`, `{>>`: the CriticMarkup spelling, the only one so far); only
//! the leaves holding an opener are parsed.

use std::ops::Range;

use crate::format::FormatDoc;
use crate::view::{CriticKind, InlineKind, InlineNode, InlineSource, LeafKind, LeafView, RichInline};

/// One review mark with absolute source ranges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewMark {
    /// Insert, Delete, Substitute, Highlight or Comment.
    pub kind: CriticKind,
    /// The whole mark, delimiters included.
    pub range: Range<usize>,
    /// Between the delimiters (for a substitution: from the old text to the end of the new).
    pub content: Range<usize>,
    /// Substitution: the old and the new text.
    pub old: Option<Range<usize>>,
    pub new: Option<Range<usize>>,
    /// Rendered text of the content (a substitution: its new text; a comment: its text).
    pub text: String,
    /// Substitution: rendered old text.
    pub old_text: String,
    /// A comment written directly after a highlight or a suggestion (no space between): it
    /// belongs to that mark.
    pub attached: bool,
}

impl ReviewMark {
    /// A suggestion the user can accept or reject.
    pub fn is_suggestion(&self) -> bool {
        self.kind.is_suggestion()
    }

    /// The author of a comment written `@name: text` (a common convention), and the text
    /// without that prefix.
    pub fn author(&self) -> (Option<&str>, &str) {
        split_author(&self.text)
    }
}

/// `@name: text` → (`Some("name")`, `"text"`); anything else → (`None`, the text trimmed).
pub fn split_author(text: &str) -> (Option<&str>, &str) {
    let t = text.trim();
    if let Some(rest) = t.strip_prefix('@')
        && let Some((name, body)) = rest.split_once(':')
        && !name.is_empty()
        && name.len() <= 64
        && !name.contains(char::is_whitespace)
    {
        return (Some(name), body.trim());
    }
    (None, t)
}

/// Offsets of every CriticMarkup opener in `text`, in order.
pub fn critic_openers(text: &str) -> impl Iterator<Item = usize> + '_ {
    let b = text.as_bytes();
    memchr::memchr_iter(b'{', b).filter(move |&i| {
        matches!(
            (b.get(i + 1), b.get(i + 2)),
            (Some(b'+'), Some(b'+'))
                | (Some(b'-'), Some(b'-'))
                | (Some(b'~'), Some(b'~'))
                | (Some(b'='), Some(b'='))
                | (Some(b'>'), Some(b'>'))
        )
    })
}

/// Whether leaves of this kind carry inline content.
fn inline_leaf(k: &LeafKind) -> bool {
    matches!(
        k,
        LeafKind::Paragraph | LeafKind::Heading { .. } | LeafKind::Table { .. }
    )
}

/// The review marks of `leaf`, in source order.
pub fn leaf_marks(doc: &dyn FormatDoc, leaf: &LeafView, out: &mut Vec<ReviewMark>) {
    match leaf.kind {
        LeafKind::Table { .. } => {
            for line in &leaf.lines {
                for cell in doc.table_row_cells(*line) {
                    let r = doc.inline(
                        InlineSource::Cell {
                            span: cell,
                            cap: cell.len().max(1),
                        },
                        cell.start,
                    );
                    collect(&r, &r.nodes, cell.start, out);
                }
            }
        }
        LeafKind::Paragraph | LeafKind::Heading { .. } => {
            let r = doc.inlines(leaf);
            collect(&r, &r.nodes, leaf.content().start, out);
        }
        _ => {}
    }
}

fn collect(r: &RichInline, nodes: &[InlineNode], base: usize, out: &mut Vec<ReviewMark>) {
    let abs = |x: &Range<u32>| base + x.start as usize..base + x.end as usize;
    for n in nodes {
        match &n.kind {
            InlineKind::Critic(k) if !matches!(k, CriticKind::Old | CriticKind::New) => {
                let (old, new, text, old_text) = if *k == CriticKind::Substitute && n.children.len() == 2 {
                    (
                        Some(abs(&n.children[0].source)),
                        Some(abs(&n.children[1].source)),
                        r.plain(&n.children[1].children),
                        r.plain(&n.children[0].children),
                    )
                } else {
                    (None, None, r.plain(&n.children), String::new())
                };
                let range = abs(&n.source);
                let attached = *k == CriticKind::Comment
                    && out
                        .last()
                        .is_some_and(|p| p.range.end == range.start && p.kind != CriticKind::Comment);
                out.push(ReviewMark {
                    kind: *k,
                    range,
                    content: abs(&n.content),
                    old,
                    new,
                    text,
                    old_text,
                    attached,
                });
            }
            _ => collect(r, &n.children, base, out),
        }
    }
}

/// Every review mark of the document, in source order. Only leaves that hold an opener are
/// parsed.
pub fn all_marks(doc: &dyn FormatDoc) -> Vec<ReviewMark> {
    let text = doc.text();
    let mut out = Vec::new();
    let mut done_to = 0usize;
    for at in critic_openers(text) {
        if at < done_to {
            continue;
        }
        let Some(leaf) = doc.leaf(at) else { continue };
        done_to = leaf.span.end.max(at + 1);
        if inline_leaf(&leaf.kind) {
            leaf_marks(doc, &leaf, &mut out);
        }
    }
    out
}

/// The marks of the leaf at `at` (none when it holds no opener).
pub fn marks_near(doc: &dyn FormatDoc, at: usize) -> Vec<ReviewMark> {
    let mut out = Vec::new();
    let Some(leaf) = doc.leaf(at) else { return out };
    if !inline_leaf(&leaf.kind) {
        return out;
    }
    let text = doc.text();
    let span = leaf.span.start.min(text.len())..leaf.span.end.min(text.len());
    if critic_openers(&text[span]).next().is_none() {
        return out;
    }
    leaf_marks(doc, &leaf, &mut out);
    out
}

/// The mark at `at` (its range contains or touches `at`); a suggestion or highlight wins over
/// the comment attached to it when `at` is between them; `want` filters the kinds.
pub fn mark_at(doc: &dyn FormatDoc, at: usize, want: impl Fn(CriticKind) -> bool) -> Option<ReviewMark> {
    marks_near(doc, at)
        .into_iter()
        .find(|m| m.range.start <= at && at <= m.range.end && want(m.kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authors() {
        assert_eq!(split_author("@ai: please check"), (Some("ai"), "please check"));
        assert_eq!(split_author(" @Greg:fine "), (Some("Greg"), "fine"));
        assert_eq!(split_author("@ two words: x"), (None, "@ two words: x"));
        assert_eq!(split_author("plain"), (None, "plain"));
        assert_eq!(split_author("@: x"), (None, "@: x"));
    }

    #[test]
    fn openers() {
        let t = "a {++b++} {-- {~~ {== {>> {+ {{-- }";
        let v: Vec<usize> = critic_openers(t).collect();
        assert_eq!(v, vec![2, 10, 14, 18, 22, 30]);
    }
}

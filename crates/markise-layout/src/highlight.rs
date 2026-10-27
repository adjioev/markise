//! Syntax-highlight input for code items, behind a trait so that layout does not depend on the
//! highlighter (`markise-blocks`); the UI adapts its background service to [`Highlights`].
//!
//! Cache contract: the layout of a code item depends on (a) whether its range is highlighted
//! yet and (b) the highlighter's [`Highlights::generation`] (on/off, configuration). Both are
//! part of the item's [`CacheKey`](crate::CacheKey); the runs themselves are a pure function of
//! the block content and language (`CodeRef::key`), and their colors of the theme, which the key
//! covers already. So a code item is laid out plain first and once more when its highlight
//! arrives — never with stale colors.

use std::ops::Range;
use std::sync::Arc;

use markise_doc::{FormatDoc, Span};
use markise_theme::TokenRun;

use crate::model::{CodeRef, Item, Segment};

/// One code item's highlighting question.
#[derive(Clone, Copy)]
pub struct CodeQuery<'a> {
    pub code: CodeRef,
    /// Fence language as written (first info word).
    pub lang: &'a str,
    /// The item's content range (byte offsets into the block content).
    pub range: (usize, usize),
    /// The document (shared, so the content can be built off the UI thread,
    /// [`CodeQuery::source`]) and the absolute start of the code block.
    pub doc: &'a Arc<dyn FormatDoc>,
    pub start: usize,
    /// The item's index.
    pub item: usize,
}

impl std::fmt::Debug for CodeQuery<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeQuery")
            .field("code", &self.code)
            .field("lang", &self.lang)
            .field("range", &self.range)
            .field("start", &self.start)
            .field("item", &self.item)
            .finish()
    }
}

impl CodeQuery<'_> {
    pub fn range(&self) -> Range<usize> {
        self.range.0..self.range.1
    }

    /// The block content: every line followed by `\n` (O(block); not for the UI thread on
    /// large blocks — use [`CodeQuery::source`]).
    pub fn content(&self) -> String {
        let lines = self.doc.leaf(self.start).map(|l| l.lines).unwrap_or_default();
        block_content(&lines, self.doc.text(), self.code.len)
    }

    /// A `'static` builder of the block content that shares the document (O(1) to create);
    /// call it on a worker thread.
    pub fn source(&self) -> impl FnOnce() -> String + Send + 'static {
        let doc = self.doc.clone();
        let (start, len) = (self.start, self.code.len);
        move || {
            let lines = doc.leaf(start).map(|l| l.lines).unwrap_or_default();
            block_content(&lines, doc.text(), len)
        }
    }
}

fn block_content(lines: &[Span], text: &str, len: usize) -> String {
    let mut s = String::with_capacity(len);
    for l in lines {
        s.push_str(l.slice(text));
        s.push('\n');
    }
    s
}

/// Source of highlight runs for code items.
pub trait Highlights {
    /// Changes when highlighting is switched on/off or reconfigured (part of layout keys).
    fn generation(&self) -> u64;
    /// Whether the query's range is highlighted (no side effects).
    fn ready(&self, q: &CodeQuery) -> bool;
    /// The runs intersecting the query's range (clipped to it), or `None` when not ready.
    fn runs(&self, q: &CodeQuery) -> Option<Vec<TokenRun>>;
    /// Asks for the block to be highlighted (cheap no-op when cached or already requested).
    fn want(&self, q: &CodeQuery);
}

/// Highlighting off: code is plain.
pub struct NoHighlights;

impl Highlights for NoHighlights {
    fn generation(&self) -> u64 {
        0
    }

    fn ready(&self, _q: &CodeQuery) -> bool {
        false
    }

    fn runs(&self, _q: &CodeQuery) -> Option<Vec<TokenRun>> {
        None
    }

    fn want(&self, _q: &CodeQuery) {}
}

/// Content range of a code item: the whole block, or its segment's line pieces.
pub(crate) fn item_range(item: &Item, code: &CodeRef) -> (usize, usize) {
    match &item.segment {
        Some(Segment::Lines { spans, .. }) => {
            let mut pos = code.offset;
            for (k, u) in spans.iter().enumerate() {
                pos += u.len();
                // a piece continued by the next unit has no newline after it
                if k + 1 < spans.len() && spans[k + 1].start != u.end {
                    pos += 1;
                }
            }
            (code.offset, pos)
        }
        _ => (0, code.len.saturating_sub(1)),
    }
}

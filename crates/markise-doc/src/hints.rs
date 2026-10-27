//! Literal-line hints (ADR 0006 §2.6): editor-only interpretation state telling the format to
//! parse one physical line as paragraph text (markers literal and visible) regardless of the
//! block syntax it starts with. Hints are few (normally zero or one), are part of every
//! transaction step and expire at defined points, materialising as escapes.

use crate::patch::PatchSet;
use crate::pos::Affinity;

/// Why a line is hinted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HintOrigin {
    /// Enter-only syntax (heading, fence, rule…) typed into a paragraph; Enter converts it.
    EnterPending,
    /// An immediate input rule's trigger (e.g. `- `); the conversion transaction removes it.
    RuleTrigger,
    /// The opposite of a literal hint: the line opens the block its syntax starts even where
    /// the format would not (Markdown: an empty list item nested by Tab under a paragraph,
    /// which CommonMark reads as paragraph text). Expires like the others; materialising it
    /// removes a still-empty construct.
    ForceBlock,
}

impl HintOrigin {
    /// `true` for hints that make a line literal paragraph text.
    pub fn is_literal(self) -> bool {
        !matches!(self, HintOrigin::ForceBlock)
    }
}

/// A hint on the physical line starting at byte offset `line`. `col` (bytes from the line
/// start) is where the literal text begins: the container prefixes before it (`> `, `- ` of
/// an enclosing quote or item) keep their meaning, and no block opens at or after it (so a
/// rule typed inside a quote or list item keeps the container). `col == 0`: the whole line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LiteralHint {
    pub line: usize,
    pub origin: HintOrigin,
    pub col: u32,
}

impl LiteralHint {
    pub const fn new(line: usize, origin: HintOrigin) -> Self {
        LiteralHint { line, origin, col: 0 }
    }
    /// A hint whose literal text starts `col` bytes after the line start.
    pub const fn at(line: usize, col: u32, origin: HintOrigin) -> Self {
        LiteralHint { line, origin, col }
    }
    /// Absolute offset where the literal text begins.
    pub const fn from(&self) -> usize {
        self.line + self.col as usize
    }
}

/// A set of hints, sorted by line, at most one per line.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct HintSet {
    hints: Vec<LiteralHint>,
}

impl HintSet {
    pub const fn new() -> Self {
        HintSet { hints: Vec::new() }
    }
    pub fn from_hints(hints: impl IntoIterator<Item = LiteralHint>) -> Self {
        let mut s = HintSet::new();
        for h in hints {
            s.insert(h);
        }
        s
    }
    pub fn is_empty(&self) -> bool {
        self.hints.is_empty()
    }
    pub fn len(&self) -> usize {
        self.hints.len()
    }
    pub fn iter(&self) -> impl Iterator<Item = &LiteralHint> {
        self.hints.iter()
    }
    /// Inserts or replaces the hint on `h.line`. Returns `true` when the set changed.
    pub fn insert(&mut self, h: LiteralHint) -> bool {
        match self.hints.binary_search_by_key(&h.line, |x| x.line) {
            Ok(i) if self.hints[i] == h => false,
            Ok(i) => {
                self.hints[i] = h;
                true
            }
            Err(i) => {
                self.hints.insert(i, h);
                true
            }
        }
    }
    pub fn remove_line(&mut self, line: usize) -> Option<LiteralHint> {
        let i = self.hints.binary_search_by_key(&line, |x| x.line).ok()?;
        Some(self.hints.remove(i))
    }
    pub fn get(&self, line: usize) -> Option<&LiteralHint> {
        let i = self.hints.binary_search_by_key(&line, |x| x.line).ok()?;
        Some(&self.hints[i])
    }
    pub fn contains_line(&self, line: usize) -> bool {
        self.get(line).is_some()
    }
    /// Lines that are hinted in exactly one of the two sets (or with different origins).
    pub fn changed_lines(&self, other: &HintSet) -> Vec<usize> {
        let mut v: Vec<usize> = self
            .hints
            .iter()
            .filter(|h| other.get(h.line) != Some(h))
            .chain(other.hints.iter().filter(|h| self.get(h.line) != Some(h)))
            .map(|h| h.line)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Live-mark transform: hint lines move like `Upstream` marks; a hint whose line start lies
    /// strictly inside a replaced range, whose container prefix (`line..line + col`) is edited,
    /// or that no longer starts a line (`is_line_start` on the post-edit text), is dropped.
    pub fn transform(&self, ps: &PatchSet, is_line_start: impl Fn(usize) -> bool) -> HintSet {
        if ps.is_empty() {
            return self.clone();
        }
        let mut out = HintSet::new();
        for h in &self.hints {
            if ps.swallows(h.line) {
                continue;
            }
            let from = h.from();
            if h.col > 0
                && ps.patches().iter().any(|p| {
                    (p.range.start < from && p.range.end > h.line) || (p.range.start > h.line && p.range.start < from)
                })
            {
                continue;
            }
            let line = ps.map_offset(h.line, Affinity::Upstream);
            if is_line_start(line) {
                out.insert(LiteralHint {
                    line,
                    origin: h.origin,
                    col: h.col,
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_moves_and_drops() {
        use crate::text::is_line_start;
        // "a\n- " with the hint on line 2
        let hs = HintSet::from_hints([LiteralHint::new(2, HintOrigin::RuleTrigger)]);
        let t = hs.transform(&PatchSet::insert(0, "xx"), |o| is_line_start("xxa\n- ", o));
        assert_eq!(t.iter().next().unwrap().line, 4);
        // joining the lines: the hinted offset no longer starts a line
        let t = hs.transform(&PatchSet::delete(1..2), |o| is_line_start("a- ", o));
        assert!(t.is_empty());
        // swallowed by a replacement
        let t = hs.transform(&PatchSet::delete(0..4), |_| true);
        assert!(t.is_empty());
    }

    #[test]
    fn changed_lines_symmetric() {
        let a = HintSet::from_hints([LiteralHint::new(0, HintOrigin::RuleTrigger)]);
        let b = HintSet::from_hints([LiteralHint::new(5, HintOrigin::EnterPending)]);
        assert_eq!(a.changed_lines(&b), vec![0, 5]);
        assert!(a.changed_lines(&a).is_empty());
    }
}

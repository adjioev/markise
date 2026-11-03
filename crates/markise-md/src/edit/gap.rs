//! The editor-only empty paragraph between two adjacent blocks (issue #166, ADR 0006 §2.2):
//! which gaps between blocks offer one ([`Document::gap_slot`]) and Enter at the end of a
//! table's last row, which goes there. Typing on it materialises a paragraph through the
//! ordinary virtual-paragraph typing step (`Document::typing_virtual`).

use markise_doc::{
    BlockLeaf, ContainerKind, DocPos, FormatDoc, InlineKind, LeafKind, PatchSet, Plan, PlanStep, Selection,
};

use crate::Document;
use crate::block::Block;

/// Longest paragraph checked for holding only images (a few image links); longer ones are
/// text without parsing them.
const IMAGE_ONLY_MAX: usize = 2048;

/// Largest region re-parsed to verify a gap inside or after containers.
const VERIFY_MAX: usize = 64 * 1024;

impl Document {
    /// The `virt` of the virtual paragraph in the gap after the leaf ending at `end`
    /// (`FormatDoc::gap_virt`), or `None`.
    ///
    /// The gap is between that leaf and the next shown block in document order (link
    /// reference definitions are skipped: they are not shown). The paragraph would stand in
    /// the innermost container both share, so the slot leaves the first leaf's other
    /// containers (`virt = 1 + levels`, as Enter on an empty last item does). There is no slot
    /// - between two items of one list (a paragraph cannot stand in a list outside its items),
    /// - next to a paragraph or heading at that level: Enter at the end of the one before, or
    ///   typing at the start of the one after, already reaches an empty line there (a
    ///   paragraph holding nothing but images counts as a block, not as text),
    /// - before a block without a text leaf at its edge (an empty list item or quote), and
    ///   after the last block (the virtual paragraph after the document's end, C53, is the
    ///   caret geometry's).
    pub(crate) fn gap_slot(&self, end: usize) -> Option<u16> {
        // leaf heads only (no line copies, no inline parse of big paragraphs): this runs on
        // every caret move across a block edge (Codex review of PR #167)
        let head = |at: usize| self.leaf_head_at(at).filter(|(l, _, _)| l.span.end == end);
        let (a, ab, ao) = head(end).or_else(|| head(end.checked_sub(1)?))?;
        if a.kind == LeafKind::Definition || a.hinted {
            return None;
        }
        let mut at = end;
        let (b, bb, bo) = loop {
            let next = self.adjacent_block(at, true)?.leaf?;
            if next.kind != LeafKind::Definition {
                break self.leaf_head_at(next.span.start)?;
            }
            at = next.span.end;
        };
        let ca = self.containers_at(a.span.start);
        let cb = self.containers_at(b.span.start);
        let shared = ca.iter().zip(&cb).take_while(|(x, y)| x == y).count();
        if shared > 0 && matches!(ca[shared - 1].kind, ContainerKind::List { .. }) {
            return None;
        }
        let exit = ca.len() - shared;
        if (exit == 0 && self.is_text_block(&a.kind, ab, ao))
            || (cb.len() == shared && self.is_text_block(&b.kind, bb, bo))
        {
            return None;
        }
        let virt = u16::try_from(exit + 1).ok()?;
        // safety net: inside or after containers, the paragraph typed there must land in the
        // container it is meant for, with both neighbours where they were
        if (!ca.is_empty() || !cb.is_empty()) && !self.gap_typing_lands(end, virt, &a, &b, shared) {
            return None;
        }
        Some(virt)
    }

    /// Typing in the gap after `a` (slot `virt`) parsed again: the new paragraph stands in
    /// `a`'s first `shared` containers, `a` keeps its kind and containers, `b` still starts a
    /// leaf in its containers. The region parsed runs from the top-level block holding `a` to
    /// the end of `b`'s first line; past [`VERIFY_MAX`] bytes it is not parsed (the prefix is
    /// derived from the parsed containers, `Document::container_prefix`, so this only guards
    /// against a mistake there).
    fn gap_typing_lands(&self, end: usize, virt: u16, a: &BlockLeaf, b: &BlockLeaf, shared: usize) -> bool {
        let text = self.text();
        let tp = self.typing_virtual(end, virt, "x");
        let (Some(p), Some(sel)) = (tp.step.patches.patches().first(), tp.selection_after) else {
            return false;
        };
        let i = self.top_partition_point(|t| t.end() < a.span.start);
        let from = crate::line_start(text, self.top(i).map_or(a.span.start, |t| t.start().min(a.span.start)));
        let (to, _) = crate::text::line_end(text, b.span.start);
        if to.saturating_sub(from) > VERIFY_MAX || p.range.start < from || p.range.end > to {
            return to.saturating_sub(from) > VERIFY_MAX;
        }
        #[cfg(test)]
        tests::VERIFIED.with(|v| v.set(v.get() + (to - from)));
        let mut block = String::with_capacity(to - from + p.insert.len());
        block.push_str(&text[from..p.range.start]);
        block.push_str(&p.insert);
        block.push_str(&text[p.range.end..to]);
        let delta = p.insert.len() as isize - p.range.len() as isize;
        let d = Document::parse(block, self.options().clone());
        let kinds = |l: &[ContainerKind]| l.to_vec();
        let x = sel.primary().focus.offset - 1 - from;
        let Some(xl) = d.leaf(x) else { return false };
        let want: Vec<ContainerKind> = a.containers[..shared].to_vec();
        let ok_x = xl.kind == LeafKind::Paragraph && xl.span.len() == 1 && kinds(&xl.containers) == want;
        let ok_a = d
            .leaf(a.span.start - from)
            .is_some_and(|l| l.kind == a.kind && l.containers == a.containers && l.span.start == a.span.start - from);
        let bs = (b.span.start as isize + delta) as usize - from;
        let ok_b = d
            .leaf(bs)
            .is_some_and(|l| l.span.start == bs && l.containers == b.containers);
        ok_x && ok_a && ok_b
    }

    /// A paragraph with text (not only images) or a heading: a block whose own Enter / typing
    /// reaches the empty line next to it. Bounded: a paragraph longer than
    /// [`IMAGE_ONLY_MAX`] bytes is text without looking further; a shorter one that does not
    /// start like an image (`!` or `[`) as well; only the rest are parsed.
    fn is_text_block(&self, kind: &LeafKind, b: &Block, origin: usize) -> bool {
        match kind {
            LeafKind::Heading { .. } => true,
            LeafKind::Paragraph => {
                let text = self.text();
                let (Some(first), Some(last)) = (b.lines.first(), b.lines.last()) else {
                    return true;
                };
                let (from, to) = (first.start + origin, last.end + origin);
                if to.saturating_sub(from) > IMAGE_ONLY_MAX || !matches!(text.as_bytes().get(from), Some(b'!' | b'[')) {
                    return true;
                }
                let lines: Vec<markise_doc::Span> = b
                    .lines
                    .iter()
                    .map(|l| markise_doc::Span::new(l.start + origin, l.end + origin))
                    .collect();
                #[cfg(test)]
                tests::PARSED.with(|v| v.set(v.get() + (to - from)));
                let rich = self.inline(markise_doc::InlineSource::Lines(&lines), from);
                let blank_text = |n: &markise_doc::InlineNode| {
                    matches!(n.kind, InlineKind::Break { .. })
                        || (n.kind == InlineKind::Text && rich.value(n).trim().is_empty())
                };
                let image = |n: &markise_doc::InlineNode| {
                    matches!(n.kind, InlineKind::Image { .. })
                        || (matches!(n.kind, InlineKind::Link { .. })
                            && !n.children.is_empty()
                            && n.children.iter().all(|c| matches!(c.kind, InlineKind::Image { .. })))
                };
                !(rich.nodes.iter().any(image) && rich.nodes.iter().all(|n| image(n) || blank_text(n)))
            }
            _ => false,
        }
    }

    /// Enter at the end of a table's last row (after its last cell's text): the caret goes
    /// to the virtual paragraph after the table, as at the end of a paragraph (no source
    /// change). Elsewhere in a table Enter does nothing (`None`); Cmd+Enter adds a row.
    pub(crate) fn table_enter(&self, at: usize) -> Option<Plan> {
        let leaf = self.leaf(at)?;
        if !matches!(leaf.kind, LeafKind::Table { .. }) {
            return None;
        }
        let t = self.table(leaf.span.start)?;
        let li = t.rows.iter().rposition(|r| r.start <= at)?;
        let last = t.rows.len().checked_sub(1)?;
        // the last body row, or the header of a table without body rows
        if li == 1 || !(li == last || (li == 0 && last == 1)) {
            return None;
        }
        let row = t.rows[li];
        let text_end = self.table_row_cells(row).last().map_or(row.start, |c| c.end);
        if at < text_end || at > row.end {
            return None;
        }
        Some(Plan::single(
            PlanStep::patches(PatchSet::default()),
            Some(Selection::caret_at(DocPos::with_virt(leaf.span.end, 1))),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use markise_doc::{EditBehavior, FormatDoc};

    thread_local! {
        /// Bytes of paragraph text parsed by the image-only check.
        pub(super) static PARSED: Cell<usize> = const { Cell::new(0) };
        /// Bytes re-parsed to verify gaps in containers.
        pub(super) static VERIFIED: Cell<usize> = const { Cell::new(0) };
    }

    fn parsed(f: impl FnOnce()) -> usize {
        PARSED.with(|v| v.set(0));
        f();
        PARSED.with(|v| v.get())
    }

    /// Codex review of PR #167: asking for the gap next to a paragraph costs the same
    /// whatever the paragraph's size (no parse of a long paragraph), and an image-only
    /// paragraph still counts as a block.
    #[test]
    fn gap_check_next_to_a_paragraph_stays_bounded() {
        for words in [10usize, 300_000] {
            let para = "word ".repeat(words);
            for text in [format!("```\nc\n```\n\n{para}\n"), format!("{para}\n\n```\nc\n```\n")] {
                let d = crate::parse(&text);
                let end = d.leaf(0).unwrap().span.end;
                let n = parsed(|| assert_eq!(d.gap_virt(end), None, "{words} words"));
                assert_eq!(n, 0, "{words} words: parsed {n} bytes");
            }
        }
        // a huge image-looking paragraph counts as text without being parsed
        let text = format!("```\nc\n```\n\n![{}](x.png)\n", "word ".repeat(300_000));
        let d = crate::parse(&text);
        let n = parsed(|| assert_eq!(d.gap_virt(9), None));
        assert_eq!(n, 0, "parsed {n} bytes");
        // short image paragraphs are blocks (a slot next to them), parsed within the budget
        for img in ["![a](x.png)", "[![a](x.png)](https://e.x)", "![a](x.png) ![b](y.png)"] {
            let text = format!("```\nc\n```\n\n{img}\n");
            let d = crate::parse(&text);
            let n = parsed(|| assert_eq!(d.gap_virt(9), Some(1), "{img}"));
            assert!(n > 0 && n <= super::IMAGE_ONLY_MAX, "{img}: {n}");
        }
    }

    /// Gaps inside containers are verified by re-parsing their region (bounded); top-level
    /// gaps need no parse.
    #[test]
    fn gap_verification_parses_only_container_regions() {
        let verified = |text: &str, end: usize| {
            let d = crate::parse(text);
            VERIFIED.with(|v| v.set(0));
            let r = d.gap_virt(end);
            (r, VERIFIED.with(|v| v.get()))
        };
        let (r, n) = verified("```\nc\n```\n\n***\n", 9);
        assert_eq!((r, n), (Some(1), 0));
        let text = "- a\n\n  ```\n  c\n  ```\n\n  ***\n";
        let end = text.find("```\n\n").unwrap() + 3;
        let (r, n) = verified(text, end);
        assert_eq!(r, Some(1));
        assert!(n > 0 && n <= text.len(), "{n}");
        // past the budget: not parsed (the derived prefix is trusted)
        let mut big = String::from("- a\n\n");
        for i in 0..20_000 {
            big.push_str(&format!("  line {i}\n"));
        }
        big.push_str("\n  ```\n  c\n  ```\n\n  ***\n");
        let end = big.rfind("```").unwrap() + 3;
        let (r, n) = verified(&big, end);
        assert_eq!((r, n), (Some(1), 0));
    }
}

//! The editor-only empty paragraph between two adjacent blocks (issue #166, ADR 0006 §2.2):
//! which gaps between blocks offer one ([`Document::gap_slot`]) and Enter at the end of a
//! table's last row, which goes there. Typing on it materialises a paragraph through the
//! ordinary virtual-paragraph typing step (`Document::typing_virtual`).

use markise_doc::{
    ContainerKind, DocPos, FormatDoc, InlineKind, LeafKind, LeafView, PatchSet, Plan, PlanStep, Selection,
};

use crate::Document;

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
        let a = self
            .leaf(end)
            .filter(|l| l.span.end == end)
            .or_else(|| self.leaf(end.checked_sub(1)?).filter(|l| l.span.end == end))?;
        if a.kind == LeafKind::Definition || a.hinted {
            return None;
        }
        let mut at = end;
        let b = loop {
            let next = self.adjacent_block(at, true)?.leaf?;
            if next.kind != LeafKind::Definition {
                break self.leaf(next.span.start)?;
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
        if (exit == 0 && self.is_text_block(&a)) || (cb.len() == shared && self.is_text_block(&b)) {
            return None;
        }
        u16::try_from(exit + 1).ok()
    }

    /// A paragraph with text (not only images) or a heading: a block whose own Enter / typing
    /// reaches the empty line next to it.
    fn is_text_block(&self, l: &LeafView) -> bool {
        match l.kind {
            LeafKind::Heading { .. } => true,
            LeafKind::Paragraph => {
                let rich = self.inlines(l);
                let blank_text = |n: &markise_doc::InlineNode| {
                    matches!(n.kind, InlineKind::Break { .. })
                        || (n.kind == InlineKind::Text && rich.value(n).trim().is_empty())
                };
                let images = rich.nodes.iter().any(|n| matches!(n.kind, InlineKind::Image { .. }));
                !(images
                    && rich
                        .nodes
                        .iter()
                        .all(|n| matches!(n.kind, InlineKind::Image { .. }) || blank_text(n)))
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

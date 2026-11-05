//! Where a block can be put (ADR 0022): what the document says about the caret's place, for a host
//! offering blocks to insert. No package and no block name is known here.

use super::EditorView;

impl EditorView {
    /// The caret stands on a line with no text, at the document's level and in no block: typing
    /// there would start a new paragraph of the document.
    pub fn on_free_line(&self) -> bool {
        let p = self.session.selection().primary();
        if !p.is_caret() || self.session.read_only().is_some() {
            return false;
        }
        let at = p.focus;
        let doc = self.session.doc();
        // the virtual paragraph after a closer shares its end, and is outside the block
        let in_block = |closer_end: usize| !(at.virt > 0 && at.offset >= closer_end);
        if doc
            .named_containers_at(at.offset)
            .iter()
            .any(|c| in_block(c.closer.end))
        {
            return false;
        }
        if at.virt > 0 {
            return doc.containers_at(at.offset.saturating_sub(1)).len() < usize::from(at.virt);
        }
        let text = doc.text();
        let line = text[..at.offset].rfind('\n').map_or(0, |i| i + 1);
        let end = text[at.offset..].find('\n').map_or(text.len(), |i| at.offset + i);
        text[line..end].trim().is_empty()
            && doc.leaf(at.offset).is_none_or(|l| l.span.is_empty())
            && doc.containers_at(at.offset).is_empty()
    }
}

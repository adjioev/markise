//! Moving a block among its siblings (ADR 0022): as one undo step, with its bytes as they are. The
//! document's top-level blocks are its siblings, or a parent's when it is nested. Deleting one takes it
//! from its place the same way.

use std::fmt;
use std::ops::Range;

use markise_doc::{DocPos, FormatDoc, HintSet, LeafKind, NamedContainer, Patch, PatchSet, PlanStep, Selection};

use super::block_place::{after_line, blank_before, blank_line_at, blocks_in, blocks_of, line_start, separated};
use super::{EditError, EditorView, TxKind};

const MOVE_LABEL: &str = "Move Block";
const DELETE_LABEL: &str = "Delete Block";

/// Why a block did not move. Nothing changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MoveRefused {
    /// Other blocks would pair differently where it would stand.
    WouldNotStand,
    /// Something other than blank lines would go with it.
    WouldLoseText,
    Edit(EditError),
}

impl fmt::Display for MoveRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MoveRefused::WouldNotStand => write!(f, "it would change how the blocks around it pair"),
            MoveRefused::WouldLoseText => write!(f, "text next to it would be lost"),
            MoveRefused::Edit(e) => write!(f, "{e}"),
        }
    }
}

/// A block as a thing to move.
struct Moving {
    opener: usize,
    /// All of its lines, the last line ending not included.
    range: Range<usize>,
    /// How many blocks are around it, and the one directly around it.
    depth: usize,
    parent: Option<NamedContainer>,
    first_top: usize,
    last_top: usize,
}

fn moving(doc: &dyn FormatDoc, chain: &[NamedContainer], depth: usize) -> Option<Moving> {
    let c = chain.get(depth)?;
    let text = doc.text();
    let end = text[c.closer.end..]
        .find(['\r', '\n'])
        .map_or(text.len(), |i| c.closer.end + i);
    Some(Moving {
        opener: c.opener.start,
        range: line_start(text, c.opener.start)..end,
        depth,
        parent: depth.checked_sub(1).and_then(|p| chain.get(p)).cloned(),
        first_top: doc.top_at(c.opener.start)?,
        last_top: doc.top_at(c.closer.start)?,
    })
}

/// The thing at the block's level starting at top-level block `t`: a sibling block whole, or one
/// top-level block; with the top-level blocks it takes up.
fn element(doc: &dyn FormatDoc, t: usize, depth: usize) -> Option<(Range<usize>, Range<usize>)> {
    let s = doc.top_span(t)?;
    match doc.named_containers_at(s.start).get(depth) {
        Some(c) => {
            let (a, b) = (doc.top_at(c.opener.start)?, doc.top_at(c.closer.start)?);
            Some((c.opener.start..c.closer.end, a..b + 1))
        }
        None => Some((s.start..s.end, t..t + 1)),
    }
}

/// What comes before `m` at its level: its start, and whether anything may go before it. Reference
/// definitions do not show, so a move goes past them to what does. Front matter stays first.
fn before(doc: &dyn FormatDoc, m: &Moving) -> Option<usize> {
    let mut t = m.first_top;
    loop {
        t = t.checked_sub(1)?;
        let s = doc.top_span(t)?;
        if m.parent.as_ref().is_some_and(|p| s.start <= p.opener.start) {
            return None;
        }
        let at = doc
            .named_containers_at(s.start)
            .get(m.depth)
            .map_or(s.start, |c| c.opener.start);
        match doc.leaf(at).map(|l| l.kind) {
            Some(LeafKind::Metadata) => return None,
            Some(LeafKind::Definition) => t = doc.top_at(at)?,
            _ => return Some(at),
        }
    }
}

/// The end of what comes after `m` at its level.
fn after(doc: &dyn FormatDoc, m: &Moving) -> Option<usize> {
    let mut t = m.last_top + 1;
    loop {
        let s = doc.top_span(t)?;
        if m.parent.as_ref().is_some_and(|p| s.start >= p.closer.start) {
            return None;
        }
        let (range, tops) = element(doc, t, m.depth)?;
        if matches!(doc.leaf(range.start).map(|l| l.kind), Some(LeafKind::Definition)) {
            t = tops.end;
        } else {
            return Some(range.end);
        }
    }
}

/// Around `b`: where what comes before it ends, and where what follows it starts, blank lines passed over.
fn surroundings(text: &str, b: &Range<usize>) -> (usize, usize) {
    let mut next = after_line(text, b.end);
    while next < text.len() && blank_line_at(text, next) {
        next = after_line(text, next);
    }
    let mut prev = b.start;
    while prev > 0 && blank_before(text, prev) {
        prev = line_start(text, prev - 1);
    }
    (prev, next)
}

/// What taking `b` from its place replaces, and with what: it, and the blank lines it leaves behind.
fn lift<'a>(text: &str, b: &Range<usize>, eol: &'a str) -> Result<(Range<usize>, &'a str), MoveRefused> {
    let (prev, next) = surroundings(text, b);
    let (remove, keep) = if next >= text.len() {
        // what stood before it is now the end of the text, and ends as the text did
        let tail = text[..prev].trim_end_matches(['\r', '\n']).len();
        (tail..b.end, "")
    } else if prev > 0 && !blank_before(text, b.start) {
        // two things it held apart must not run together
        (b.start..next, eol)
    } else {
        (b.start..next, "")
    };
    // only blank lines go, and the block's own bytes come back
    let blank = |t: &str| t.chars().all(|c| matches!(c, ' ' | '\t' | '\r' | '\n'));
    if !blank(&text[remove.start..b.start.max(remove.start)]) || !blank(&text[b.end..remove.end.max(b.end)]) {
        return Err(MoveRefused::WouldLoseText);
    }
    Ok((remove, keep))
}

/// Where the caret goes when the block is gone and what follows it starts at `at`: the end of the text
/// before, or the free line after a block before, rather than onto the marker of another block.
fn caret_after_delete(doc: &dyn FormatDoc, at: usize) -> DocPos {
    let text = doc.text();
    let tail = text[..at].trim_end_matches([' ', '\t', '\r', '\n']).len();
    let Some((last, _)) = text[..tail].char_indices().next_back() else {
        return DocPos::new(at.min(text.len()));
    };
    if let Some(c) = doc.named_containers_at(last).pop() {
        if c.closer.contains(last) {
            return DocPos::with_virt(tail, 1);
        }
        if c.opener.contains(last) {
            return DocPos::new(at.min(text.len()));
        }
    }
    if matches!(doc.leaf(last).map(|l| l.kind), Some(LeafKind::Metadata)) {
        return DocPos::new(at.min(text.len()));
    }
    DocPos::new(tail)
}

/// The edits moving `b` to line `to`, and where its first byte ends up. `None`: it is there already.
#[allow(clippy::type_complexity)]
fn plan(
    text: &str,
    b: &Range<usize>,
    to: usize,
    eol: &str,
) -> Option<Result<(Vec<Patch>, usize, String), MoveRefused>> {
    let (prev, next) = surroundings(text, b);
    if (prev..=next).contains(&to) || (next >= text.len() && to == text.len()) {
        return None;
    }
    let body = &text[b.clone()];
    let (insert, skip) = separated(text, to, body, eol);
    let (remove, keep) = match lift(text, b, eol) {
        Ok(lifted) => lifted,
        Err(why) => return Some(Err(why)),
    };
    let moved = remove.len() - keep.len();
    let start = if to > b.start { to + skip - moved } else { to + skip };
    let mut patches = vec![
        Patch::verbatim(remove, keep.to_string()),
        Patch::verbatim(to..to, insert),
    ];
    patches.sort_by_key(|p| p.range.start);
    Some(Ok((patches, start, body.to_string())))
}

impl EditorView {
    fn block_at_caret(&self) -> Option<Moving> {
        let at = self.session.selection().primary().focus;
        let doc = self.session.doc();
        let chain = doc.named_containers_at(at.offset);
        let inside = chain
            .iter()
            .filter(|c| !(at.virt > 0 && at.offset >= c.closer.end))
            .count();
        moving(doc, &chain, inside.checked_sub(1)?)
    }

    fn block_with_opener(&self, opener: usize) -> Option<Moving> {
        let doc = self.session.doc();
        let chain = doc.named_containers_at(opener);
        let depth = chain.len().checked_sub(1)?;
        (chain[depth].opener.start == opener).then(|| moving(doc, &chain, depth))?
    }

    fn line_to(&self, m: &Moving, up: bool) -> Option<usize> {
        let doc = self.session.doc();
        if up {
            before(doc, m).map(|at| line_start(doc.text(), at))
        } else {
            after(doc, m).map(|end| after_line(doc.text(), end))
        }
    }

    /// The block around the caret can move up or down: there is something to go past at its level.
    pub fn can_move_block(&self, up: bool) -> bool {
        self.block_at_caret().is_some_and(|m| self.line_to(&m, up).is_some())
    }

    /// Moves the block around the caret past what stands before or after it at its level. `Ok(false)`:
    /// nothing to go past.
    pub fn move_block(&mut self, up: bool, in_source: bool) -> Result<bool, MoveRefused> {
        if in_source {
            self.source_selection_to_session();
        }
        let Some(m) = self.block_at_caret() else {
            return Ok(false);
        };
        let Some(to) = self.line_to(&m, up) else {
            return Ok(false);
        };
        let moved = self.move_range(m.range, to)?;
        if in_source {
            self.source_selection_from_session();
        }
        Ok(moved)
    }

    /// Moves the block with this opener to line `to`, as [`Self::drop_line`] gave it.
    pub fn move_block_to(&mut self, opener: usize, to: usize) -> Result<bool, MoveRefused> {
        let Some(m) = self.block_with_opener(opener) else {
            return Ok(false);
        };
        if !self.is_drop_line(opener, to) {
            return Ok(false);
        }
        self.move_range(m.range, to)
    }

    /// `to` is, in the document as it stands, a line this block can land on.
    fn is_drop_line(&self, opener: usize, to: usize) -> bool {
        let doc = self.session.doc();
        if self.drop_line_at_end(opener) == Some(to) {
            return true;
        }
        if doc.text().get(to..).is_none() {
            return false;
        }
        let top = doc.top_at(to).and_then(|t| doc.top_span(t));
        top.is_some_and(|s| line_start(doc.text(), s.start) == to && self.drop_line(opener, s.start) == Some(to))
    }

    /// The line the block with this opener can be dropped on so as to stand before what starts at
    /// `item`: `None` when `item` starts nothing at the block's level, is inside the block or leaves it
    /// where it is. A list or a quote is one thing there: its items are no places.
    pub fn drop_line(&self, opener: usize, item: usize) -> Option<usize> {
        let doc = self.session.doc();
        let m = self.block_with_opener(opener)?;
        let top = doc.top_span(doc.top_at(item)?)?;
        if line_start(doc.text(), top.start) != line_start(doc.text(), item) {
            return None;
        }
        let chain = doc.named_containers_at(item);
        // the same parent, or the parent's closer: the end of its body
        let same_parent = match &m.parent {
            Some(p) => {
                chain.get(m.depth - 1).is_some_and(|c| c.opener.start == p.opener.start) && item > p.opener.start
            }
            None => true,
        };
        if !same_parent {
            return None;
        }
        if let Some(c) = chain.get(m.depth)
            && c.opener.start != item
        {
            return None; // inside something at this level
        }
        if matches!(
            doc.leaf(item).map(|l| l.kind),
            Some(LeafKind::Metadata | LeafKind::ContainerMarker { close: true })
        ) && m.parent.as_ref().is_none_or(|p| p.closer.start != item)
        {
            return None;
        }
        let line = line_start(doc.text(), item);
        let own = (m.range.start..=after_line(doc.text(), m.range.end)).contains(&line);
        (!own && !self.is_next_line(&m, line)).then_some(line)
    }

    fn is_next_line(&self, m: &Moving, line: usize) -> bool {
        let text = self.session.text();
        let mut next = after_line(text, m.range.end);
        while next < text.len() && blank_line_at(text, next) {
            next = after_line(text, next);
        }
        next == line
    }

    /// The line the block can be dropped on to stand last at its level.
    pub fn drop_line_at_end(&self, opener: usize) -> Option<usize> {
        let m = self.block_with_opener(opener)?;
        let text = self.session.text();
        let doc = self.session.doc();
        if let Some(p) = &m.parent {
            let line = line_start(text, p.closer.start);
            return (!self.is_next_line(&m, line)).then_some(line);
        }
        let last = doc.top_count().checked_sub(1)?;
        if last <= m.last_top {
            return None;
        }
        Some(after_line(text, doc.top_span(last)?.end))
    }

    /// The block around the caret, the innermost, by its opener.
    pub fn block_opener_at_caret(&self) -> Option<usize> {
        self.block_at_caret().map(|m| m.opener)
    }

    /// [`Self::block_opener_at_caret`], by the caret of the pane the keyboard is in.
    pub fn block_opener_in_pane(&mut self, in_source: bool) -> Option<usize> {
        if in_source {
            self.source_selection_to_session();
        }
        self.block_opener_at_caret()
    }

    /// All the lines of the block with this opener, the last line ending not included.
    pub fn block_lines(&self, opener: usize) -> Option<Range<usize>> {
        self.block_with_opener(opener).map(|m| m.range)
    }

    /// Deletes the block with this opener, with what is inside it: one undo step, with nothing taken but
    /// its lines and the blank lines it leaves behind. `Ok(false)`: no block stands there.
    pub fn delete_block(&mut self, opener: usize, in_source: bool) -> Result<bool, MoveRefused> {
        if in_source {
            self.source_selection_to_session();
        }
        let Some(m) = self.block_with_opener(opener) else {
            return Ok(false);
        };
        self.delete_range(m.range)?;
        if in_source {
            self.source_selection_from_session();
        }
        Ok(true)
    }

    fn delete_range(&mut self, b: Range<usize>) -> Result<(), MoveRefused> {
        self.release();
        let doc = self.session.doc();
        let text = doc.text();
        let eol = doc.line_ending().as_str();
        let (remove, keep) = lift(text, &b, eol)?;
        let patches = PatchSet::single_verbatim(remove.clone(), keep);
        // what pairs is what paired, less what was in it
        let mut want = blocks_of(doc);
        let gone = blocks_in(doc, &b);
        want.retain({
            let mut gone = gone;
            move |x| match gone.iter().position(|g| g == x) {
                Some(i) => {
                    gone.swap_remove(i);
                    false
                }
                None => true,
            }
        });
        let hints_after = (!doc.hints().is_empty()).then(HintSet::new);
        let holds = |p: DocPos| p.offset >= b.start && p.offset <= b.end && !(p.virt > 0 && p.offset == b.end);
        let sel = self.session.selection().primary();
        let caret_goes = holds(sel.anchor) && holds(sel.focus);
        let at = remove.start + keep.len();
        let was_sticky = self.sticky_at().is_some_and(|s| b.contains(&s));
        let done = self.session.transact(DELETE_LABEL, TxKind::Structural, |tx| {
            tx.step(PlanStep { patches, hints_after })?;
            if blocks_of(tx.doc()) != want {
                return Err(EditError::Aborted(String::new()));
            }
            if caret_goes {
                let to = caret_after_delete(tx.doc(), at);
                tx.set_selection(Selection::caret_at(to));
            }
            Ok(())
        });
        self.sync();
        match done {
            Ok(()) => {}
            Err(EditError::Aborted(_)) => return Err(MoveRefused::WouldNotStand),
            Err(e) => return Err(MoveRefused::Edit(e)),
        }
        if was_sticky {
            self.end_sticky();
        }
        self.follow_sticky();
        self.follow_block_modes();
        self.pin_caret();
        Ok(())
    }

    fn move_range(&mut self, b: Range<usize>, to: usize) -> Result<bool, MoveRefused> {
        self.release();
        let doc = self.session.doc();
        let eol = doc.line_ending().as_str();
        let Some(planned) = plan(doc.text(), &b, to, eol) else {
            return Ok(false);
        };
        let (patches, start, body) = planned?;
        let patches = PatchSet::new(patches).map_err(|e| MoveRefused::Edit(EditError::Patch(e)))?;
        let want = blocks_of(doc);
        let hints_after = (!doc.hints().is_empty()).then(HintSet::new);
        let shift = |p: DocPos| DocPos {
            offset: p.offset - b.start + start,
            ..p
        };
        let holds = |p: DocPos| p.offset >= b.start && p.offset <= b.end && !(p.virt > 0 && p.offset == b.end);
        let sel = self.session.selection().primary();
        let sel_after =
            (holds(sel.anchor) && holds(sel.focus)).then(|| Selection::single(shift(sel.anchor), shift(sel.focus)));
        // a block's state goes with it (ADR 0021): its marks do not survive the bytes moving
        let modes: Vec<_> = (self.own_modes_now().into_iter())
            .filter(|(at, _)| b.contains(at))
            .map(|(at, mode)| (at - b.start + start, mode))
            .collect();
        let was_sticky = self.sticky_at().is_some_and(|at| b.contains(&at));
        let body_len = body.len();
        let done = self.session.transact(MOVE_LABEL, TxKind::Structural, |tx| {
            tx.step(PlanStep { patches, hints_after })?;
            let doc = tx.doc();
            if doc.text().get(start..start + body_len) != Some(body.as_str()) || blocks_of(doc) != want {
                return Err(EditError::Aborted(String::new()));
            }
            if let Some(s) = sel_after {
                tx.set_selection(s);
            }
            Ok(())
        });
        self.sync();
        match done {
            Ok(()) => {}
            Err(EditError::Aborted(_)) => return Err(MoveRefused::WouldNotStand),
            Err(e) => return Err(MoveRefused::Edit(e)),
        }
        for (at, mode) in modes {
            self.add_own_mode(at, mode);
        }
        if was_sticky {
            self.end_sticky();
            self.begin_sticky(start);
        }
        self.follow_sticky();
        self.follow_block_modes();
        self.pin_caret();
        Ok(true)
    }
}

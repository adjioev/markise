//! Per-block keys and block commands (spec 02 §5, §15; C11, C45, C46, C48, C52): Enter,
//! Backspace and Delete at block boundaries, Tab/Shift+Tab, list renumbering.
//!
//! Lists: the structure comes from the parse tree; every edit is a minimal patch set on the
//! marker lines (container prefixes, markers, separators), so text outside the edited lines
//! stays byte-identical. Ordered lists are renumbered only when they were sequential before
//! the edit (C11), within the edited list only.

use markise_doc::{
    DocPos, FormatDoc, LeafKind, Plan, PlanError, PlanRequest, PlanStep, PlannerId, Selection, planners,
};

use super::{Edits, is_sp};
use crate::block::{Block, BlockKind};
use crate::text::{Span, line_end};
use crate::{Document, line_start};

/// One list item with absolute positions.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    /// Start of the marker line.
    pub ls: usize,
    pub marker: Span,
    pub number: Option<u64>,
    /// End of the task box (or of the marker) — the item's syntax before its content.
    pub syntax_end: usize,
    /// Where the item's content starts on the marker line (end of the line for an empty item).
    pub content: usize,
    /// End of the item's last content.
    pub end: usize,
    /// Content column of the item relative to the marker start (marker width + spacing).
    pub content_offset: usize,
    pub empty: bool,
    /// Path of the item in the block tree.
    pub path: Vec<usize>,
}

/// A list around a position.
#[derive(Clone, Debug)]
pub(crate) struct ListCtx {
    pub items: Vec<Item>,
    /// Index of the item holding the position.
    pub idx: usize,
    pub ordered: bool,
    pub marker: u8,
    pub tight: bool,
    /// The enclosing list item, when the list is nested.
    pub parent: Option<Box<ListCtx>>,
}

impl ListCtx {
    pub(crate) fn item(&self) -> &Item {
        &self.items[self.idx]
    }
    /// C11: ordered and numbered start, start + 1, … before the edit.
    pub(crate) fn sequential(&self) -> bool {
        if !self.ordered {
            return false;
        }
        let Some(s) = self.items.first().and_then(|i| i.number) else {
            return false;
        };
        self.items
            .iter()
            .enumerate()
            .all(|(k, i)| i.number == Some(s + k as u64))
    }
    pub(crate) fn delim(&self) -> char {
        self.marker as char
    }
}

fn abs(s: Span, o: usize) -> Span {
    Span::new(s.start + o, s.end + o)
}

/// The container prefix to repeat for a continuation line: `text[ls..upto]` with everything but
/// quote markers and tabs turned into spaces.
pub(crate) fn cont_prefix(text: &str, ls: usize, upto: usize) -> String {
    text[ls..upto.max(ls)]
        .chars()
        .map(|c| match c {
            '>' | '\t' => c,
            _ => ' ',
        })
        .collect()
}

impl Document {
    fn item_of(&self, b: &Block, o: usize, path: Vec<usize>) -> Option<Item> {
        let BlockKind::ListItem {
            marker,
            number,
            task,
            content_offset,
        } = &b.kind
        else {
            return None;
        };
        let text = self.text();
        let marker = abs(*marker, o);
        let ls = line_start(text, marker.start);
        let (le, _) = line_end(text, marker.start);
        let syntax_end = task.map_or(marker.end, |(_, s)| s.end + o);
        let content = match b.children.first() {
            Some(c) if line_start(text, c.span.start + o) == ls => c.span.start + o,
            Some(_) | None => (marker.start + content_offset).min(le).max(syntax_end.min(le)),
        };
        Some(Item {
            ls,
            marker,
            number: *number,
            syntax_end,
            content: if b.children.is_empty() { le } else { content },
            end: b.span.end + o,
            content_offset: *content_offset,
            empty: b.children.is_empty(),
            path,
        })
    }

    /// The list whose item at depth `depth` of `path` is the item.
    fn list_ctx_at_path(&self, path: &[usize], item_depth: usize) -> Option<ListCtx> {
        let (lb, o) = self.block_at_path_rel(&path[..item_depth])?;
        let BlockKind::List {
            ordered, marker, tight, ..
        } = &lb.kind
        else {
            return None;
        };
        let mut items = Vec::with_capacity(lb.children.len());
        for (k, c) in lb.children.iter().enumerate() {
            let mut p = path[..item_depth].to_vec();
            p.push(k);
            items.push(self.item_of(c, o, p)?);
        }
        // the enclosing item (list → item → … → this list)
        let parent = (item_depth >= 2)
            .then(|| {
                let (pb, _) = self.block_at_path_rel(&path[..item_depth - 1])?;
                matches!(pb.kind, BlockKind::ListItem { .. })
                    .then(|| self.list_ctx_at_path(path, item_depth - 2))
                    .flatten()
            })
            .flatten()
            .map(Box::new);
        Some(ListCtx {
            items,
            idx: *path.get(item_depth)?,
            ordered: *ordered,
            marker: *marker,
            tight: *tight,
            parent,
        })
    }

    /// The path (to the item) of the innermost list item around `at`, with the empty-item
    /// fallback (the caret of an empty item sits after its marker spacing).
    fn item_path(&self, at: usize) -> Option<Vec<usize>> {
        let text = self.text().as_bytes();
        let find = |path: &[usize]| {
            (1..=path.len()).rev().find(|&d| {
                self.block_at_path_rel(&path[..d])
                    .is_some_and(|(b, _)| matches!(b.kind, BlockKind::ListItem { .. }))
            })
        };
        let path = self.block_path_at(at);
        if let Some(d) = find(&path) {
            return Some(path[..d].to_vec());
        }
        let mut q = at;
        while q > 0 && is_sp(text[q - 1]) {
            q -= 1;
        }
        let path = self.block_path_at(q);
        find(&path).map(|d| path[..d].to_vec())
    }

    /// The innermost list item around `at` alone (no siblings: cheap enough per keystroke).
    pub(crate) fn item_at(&self, at: usize) -> Option<Item> {
        let path = self.item_path(at)?;
        let (b, o) = self.block_at_path_rel(&path)?;
        self.item_of(b, o, path)
    }

    /// The innermost list item around `at` (an empty item's caret sits after its marker).
    pub(crate) fn list_ctx(&self, at: usize) -> Option<ListCtx> {
        let text = self.text().as_bytes();
        let mut q = at;
        let mut path = self.block_path_at(q);
        let has_item = |doc: &Document, p: &[usize]| {
            (1..=p.len()).any(|d| {
                doc.block_at_path_rel(&p[..d])
                    .is_some_and(|(b, _)| matches!(b.kind, BlockKind::ListItem { .. }))
            })
        };
        if !has_item(self, &path) {
            while q > 0 && is_sp(text[q - 1]) {
                q -= 1;
            }
            path = self.block_path_at(q);
        }
        let d = (1..=path.len()).rev().find(|&d| {
            self.block_at_path_rel(&path[..d])
                .is_some_and(|(b, _)| matches!(b.kind, BlockKind::ListItem { .. }))
        })?;
        // d is the length of the path to the item; the list is one above
        self.list_ctx_at_path(&path[..d], d - 1)
    }

    /// `true` when `at` is at the start of the first paragraph of the item of `ctx` (or in an
    /// empty item).
    fn at_item_start(&self, ctx: &ListCtx, at: usize) -> bool {
        let it = ctx.item();
        if it.empty {
            return line_start(self.text(), at) == it.ls && at >= it.syntax_end;
        }
        at == it.content
            && self
                .leaf(at)
                .is_some_and(|l| l.kind == LeafKind::Paragraph && l.span.start == at)
    }

    /// The marker text of a new item after `it` in `ctx` (`n + 1` for ordered items, an
    /// unchecked box for task items), with the item's spacing.
    fn next_marker(&self, ctx: &ListCtx, it: &Item, number: Option<u64>) -> String {
        let text = self.text();
        let gap_end = if it.empty { line_end(text, it.ls).0 } else { it.content };
        let mut gap = text[it.syntax_end.min(gap_end)..gap_end].to_string();
        if gap.is_empty() || gap.contains(['\r', '\n']) {
            gap = " ".into();
        }
        let mark = match number {
            Some(n) => format!("{n}{}", ctx.delim()),
            None => text[it.marker.range()].to_string(),
        };
        let task = it.syntax_end > it.marker.end;
        let box_gap = if task {
            text[it.marker.end..it.syntax_end]
                .split('[')
                .next()
                .unwrap_or(" ")
                .to_string()
        } else {
            String::new()
        };
        if task {
            format!("{mark}{box_gap}[ ]{gap}")
        } else {
            format!("{mark}{gap}")
        }
    }

    /// Renumber patches (C11) for items `from..` of `ctx` so that item `k` gets
    /// `first + (k - from)`.
    fn renumber(&self, ed: &mut Edits, ctx: &ListCtx, from: usize, first: u64) {
        for (k, it) in ctx.items.iter().enumerate().skip(from) {
            let n = first + (k - from) as u64;
            if it.number != Some(n) {
                self.set_marker(ed, it, format!("{n}{}", ctx.delim()), 0);
            }
        }
    }

    fn containers_on(&self, path: &[usize]) -> usize {
        (1..=path.len())
            .filter(|&d| {
                self.block_at_path_rel(&path[..d])
                    .is_some_and(|(b, _)| b.is_container())
            })
            .count()
    }

    /// How many containers a virtual paragraph anchored at the leaf ending at `gap` must leave
    /// to stand next to the list at `list_path` (in that list's container).
    pub(crate) fn exit_levels(&self, gap: usize, list_path: &[usize]) -> u16 {
        let leaf_c = self.containers_on(&self.block_path_at(gap.saturating_sub(1)));
        let outer = self.containers_on(&list_path[..list_path.len().saturating_sub(1)]);
        leaf_c.saturating_sub(outer) as u16
    }

    /// Replaces `it`'s marker with `new`; the item's continuation lines move with the content
    /// column when the marker's width changes (`9.` → `10.`), together with an extra shift of
    /// `extra` columns (indent / outdent), so children stay inside the item.
    pub(crate) fn set_marker(&self, ed: &mut Edits, it: &Item, new: String, extra: isize) {
        let delta = new.len() as isize - it.marker.len() as isize + extra;
        if self.text()[it.marker.range()] != new {
            ed.push(it.marker.range(), new);
        }
        if delta != 0 {
            self.reindent_rest(ed, it, delta);
        }
    }

    // ---- Enter -----------------------------------------------------------------------------

    /// Enter in a list item (spec 02 §5.1, S-5.1, S-5.2, C45, C11, C52); `None` when the
    /// caret is not in an empty item or the first paragraph of an item.
    pub(crate) fn list_enter(&self, at: usize) -> Option<Result<Plan, PlanError>> {
        let ctx = self.list_ctx(at)?;
        let it = ctx.item().clone();
        let text = self.text();
        if it.empty {
            if line_start(text, at) != it.ls {
                return None;
            }
            return Some(self.empty_item_enter(&ctx).map(|mut p| {
                self.fix_force_hints(&mut p, None);
                p
            }));
        }
        None
    }

    /// The new sibling item for Enter in an item's first paragraph (whose content starts at
    /// `content`): the split replaces `a..z` (closing `close` and reopening `open` styled
    /// spans), with the list's separator (C52) and renumbering of later items (C11).
    pub(crate) fn item_split_plan(
        &self,
        content: usize,
        a: usize,
        z: usize,
        close: &str,
        open: &str,
    ) -> Option<Result<Plan, PlanError>> {
        let ctx = self.list_ctx(content)?;
        let it = ctx.item();
        if it.empty || it.content != content {
            return None;
        }
        let text = self.text();
        let eol = self.line_ending().as_str();
        let number = it.number.map(|n| n + 1);
        let cprefix = cont_prefix(text, it.ls, it.marker.start);
        let marker = self.next_marker(&ctx, it, number);
        let sep = if ctx.tight {
            String::new()
        } else {
            format!("{}{eol}", cprefix.trim_end())
        };
        let mut ed = Edits::new();
        let para_end = self
            .leaf(content)
            .and_then(|l| l.lines.last().map(|x| x.end))
            .unwrap_or(z);
        let (ins_at, caret_anchor, caret_rel) = if it.end > para_end && z <= para_end {
            // the item has more blocks (a nested list…): they stay with it; the new item (with
            // the text after the caret) goes after the whole item, so no child is detached
            let tail = &text[z..para_end];
            let old_cont = (it.marker.start - it.ls) + it.content_offset;
            let new_cont = format!("{cprefix}{}", " ".repeat(marker.len()));
            let mut moved = String::new();
            for (i, line) in tail.split_inclusive('\n').enumerate() {
                if i == 0 {
                    moved.push_str(line);
                    continue;
                }
                let b = line.as_bytes();
                let mut p = 0;
                while p < b.len() && p < old_cont && (b[p] == b'>' || is_sp(b[p])) {
                    p += 1;
                }
                moved.push_str(&new_cont);
                moved.push_str(&line[p..]);
            }
            ed.push(a..para_end, close.to_string());
            let ins = format!("{eol}{sep}{cprefix}{marker}{open}");
            let rel = ins.len() - open.len();
            ed.insert(it.end, format!("{ins}{moved}"));
            (it.end, it.end, rel)
        } else {
            let ins = format!("{close}{eol}{sep}{cprefix}{marker}{open}");
            let rel = ins.len() - open.len();
            ed.push(a..z, ins);
            (a, a, rel)
        };
        let _ = ins_at;
        if ctx.sequential() {
            self.renumber(&mut ed, &ctx, ctx.idx + 1, it.number.unwrap_or(1) + 2);
        }
        let ps = match ed.build() {
            Some(p) => p,
            None => return Some(Err(PlanError::NotApplicable("overlap"))),
        };
        let caret = ps.map_offset(caret_anchor, markise_doc::Affinity::Upstream) + caret_rel;
        Some(Ok(Plan::single(PlanStep::patches(ps), Some(Selection::caret(caret)))))
    }

    /// Enter in an empty item: outdent a nested one (S-5.2); leave the list from the last
    /// top-level item (S-5.1); split the list at a middle top-level item (C45).
    fn empty_item_enter(&self, ctx: &ListCtx) -> Result<Plan, PlanError> {
        let text = self.text();
        let eol = self.line_ending().as_str();
        let it = ctx.item();
        let (le, next) = line_end(text, it.ls);
        if let Some(parent) = &ctx.parent {
            // S-5.2: the item becomes an item of the parent list, after the parent item
            let pit = parent.item();
            let cprefix = cont_prefix(text, pit.ls, pit.marker.start);
            let number = pit.number.map(|n| n + 1);
            let marker = self.next_marker(parent, pit, number);
            let mut ed = Edits::new();
            let new = format!("{cprefix}{marker}");
            let caret_rel = new.len();
            ed.push(it.ls..le, new);
            if parent.sequential() {
                self.renumber(&mut ed, parent, parent.idx + 1, pit.number.unwrap_or(1) + 2);
            }
            if ctx.sequential() {
                self.renumber(&mut ed, ctx, ctx.idx + 1, it.number.unwrap_or(1));
            }
            let ps = ed.build().ok_or(PlanError::NotApplicable("overlap"))?;
            let caret = ps.map_offset(it.ls, markise_doc::Affinity::Upstream) + caret_rel;
            return Ok(Plan::single(PlanStep::patches(ps), Some(Selection::caret(caret))));
        }
        let cprefix = cont_prefix(text, it.ls, it.marker.start);
        let blank = cprefix.trim_end().to_string();
        // the paragraph leaves every container between the anchor leaf (the end of the previous
        // item, possibly deep in a nested list) and the list's own container (S-5.1, M2c-3)
        let list_path = &it.path[..it.path.len() - 1];
        let exit = |gap: usize| self.exit_levels(gap, list_path);
        if ctx.idx + 1 == ctx.items.len() {
            // S-5.1: remove the empty item; the caret goes to a virtual paragraph after the list
            if ctx.idx == 0 {
                // the only item: the list becomes an empty paragraph (bytes: the prefix only)
                let ps = markise_doc::PatchSet::single_verbatim(it.ls..le, blank.clone());
                let caret = it.ls + blank.len();
                return Ok(Plan::single(PlanStep::patches(ps), Some(Selection::caret(caret))));
            }
            let prev = &ctx.items[ctx.idx - 1];
            let gap = prev.end;
            // delete from the end of the previous item to the end of the empty line
            let ps = markise_doc::PatchSet::delete(gap..le);
            return Ok(Plan::single(
                PlanStep::patches(ps),
                Some(Selection::caret_at(DocPos::with_virt(gap, 1 + exit(gap)))),
            ));
        }
        // C45: split the list around a virtual paragraph
        let mut ed = Edits::new();
        let sep = format!("{blank}{eol}{blank}{eol}");
        if ctx.idx == 0 {
            // first item of a longer list: removed; the caret goes to a virtual paragraph
            // after the block before the list (in the list's container), if there is one
            let Some(prev) = self.leaf_before(it.ls).map(|l| l.span.end) else {
                return Ok(Plan::default()); // nothing before the list: Enter waits
            };
            ed.push(it.ls..next, "");
            if ctx.sequential() {
                self.renumber(&mut ed, ctx, 1, it.number.unwrap_or(1));
            }
            let ps = ed.build().ok_or(PlanError::NotApplicable("overlap"))?;
            let _ = sep;
            return Ok(Plan::single(
                PlanStep::patches(ps),
                Some(Selection::caret_at(DocPos::with_virt(prev, 1))),
            ));
        }
        let prev = &ctx.items[ctx.idx - 1];
        // `prev⏎empty⏎next` → `prev⏎⏎⏎next` (two blank lines end the list, C24)
        ed.push(prev.end..next, format!("{eol}{sep}"));
        if ctx.sequential() {
            self.renumber(&mut ed, ctx, ctx.idx + 1, it.number.unwrap_or(1));
        }
        let ps = ed.build().ok_or(PlanError::NotApplicable("overlap"))?;
        let gap = prev.end;
        Ok(Plan::single(
            PlanStep::patches(ps),
            Some(Selection::caret_at(DocPos::with_virt(gap, 1 + exit(gap)))),
        ))
    }

    // ---- Backspace --------------------------------------------------------------------------

    /// Backspace at the start of an item (spec 02 §5.2 rule 3, S-5.4, C46).
    pub(crate) fn list_backspace(&self, at: usize) -> Option<Result<Plan, PlanError>> {
        // cheap check first: only at an item's start is the whole list needed
        let it0 = self.item_at(at)?;
        let at_start = if it0.empty {
            line_start(self.text(), at) == it0.ls && at >= it0.syntax_end
        } else {
            at == it0.content
        };
        if !at_start {
            return None;
        }
        let ctx = self.list_ctx(at)?;
        if !self.at_item_start(&ctx, at) {
            return None;
        }
        let text = self.text();
        let eol = self.line_ending().as_str();
        let it = ctx.item().clone();
        let (le, next) = line_end(text, it.ls);
        let mut ed = Edits::new();
        if ctx.idx > 0 {
            let prev = &ctx.items[ctx.idx - 1];
            if it.empty {
                // an empty item is removed; caret to the end of the previous item
                ed.push(prev.end..le, "");
                if ctx.sequential() {
                    self.renumber(&mut ed, &ctx, ctx.idx + 1, it.number.unwrap_or(1));
                }
                let ps = ed.build()?;
                return Some(Ok(Plan::single(
                    PlanStep::patches(ps),
                    Some(Selection::caret(prev.end)),
                )
                .labeled("Delete Item")));
            }
            if prev.empty {
                // the previous item is empty: this item's content becomes its text
                ed.push(prev.syntax_end..it.content, " ");
                self.reindent_rest(&mut ed, &it, prev.content_offset as isize - it.content_offset as isize);
                if ctx.sequential() {
                    self.renumber(&mut ed, &ctx, ctx.idx + 1, it.number.unwrap_or(1));
                }
                let ps = ed.build()?;
                let caret = ps.map_offset(it.content, markise_doc::Affinity::Downstream);
                return Some(Ok(
                    Plan::single(PlanStep::patches(ps), Some(Selection::caret(caret))).labeled("Join Item")
                ));
            }
            // S-5.4: the item's content becomes a further paragraph of the previous item
            let pprefix = cont_prefix(text, prev.ls, prev.marker.start);
            let indent = " ".repeat(prev.content_offset);
            let blank = pprefix.trim_end();
            ed.push(prev.end..it.content, format!("{eol}{blank}{eol}{pprefix}{indent}"));
            self.reindent_rest(&mut ed, &it, prev.content_offset as isize - it.content_offset as isize);
            if ctx.sequential() {
                self.renumber(&mut ed, &ctx, ctx.idx + 1, it.number.unwrap_or(1));
            }
            let ps = ed.build()?;
            let caret = ps.map_offset(it.content, markise_doc::Affinity::Downstream);
            return Some(Ok(
                Plan::single(PlanStep::patches(ps), Some(Selection::caret(caret))).labeled("Join Item")
            ));
        }
        // an empty first item of a nested list: its line goes, the caret to the parent item
        if it.empty && ctx.parent.is_some() && it.ls > 0 {
            let pe = crate::text::prev_line_content_end(text, it.ls);
            ed.push(pe..le, "");
            if ctx.sequential() {
                self.renumber(&mut ed, &ctx, 1, it.number.unwrap_or(1));
            }
            let ps = ed.build()?;
            return Some(Ok(
                Plan::single(PlanStep::patches(ps), Some(Selection::caret(pe))).labeled("Delete Item")
            ));
        }
        // C46: the first item is unwrapped before the list
        let first_prefix = if let Some(parent) = &ctx.parent {
            let pit = parent.item();
            format!(
                "{}{}",
                cont_prefix(text, pit.ls, pit.marker.start),
                " ".repeat(pit.content_offset)
            )
        } else {
            text[it.ls..it.marker.start].to_string()
        };
        let blank = first_prefix.trim_end().to_string();
        // a blank line before it unless it starts the document / follows a blank line
        let need_before = it.ls > 0 && {
            let pls = line_start(text, it.ls.saturating_sub(1));
            let prev_line = &text[pls..it.ls];
            !prev_line.trim_start_matches(['>', ' ', '\t']).trim().is_empty()
        };
        let lead = if need_before {
            format!("{blank}{eol}{first_prefix}")
        } else {
            first_prefix.clone()
        };
        // the parent item's marker line holds no text (its first block is this list): the moved
        // paragraph becomes that line's text instead of standing after an empty item
        let parent_bare = ctx.parent.as_ref().is_some_and(|p| {
            let pit = p.item();
            pit.ls != it.ls
                && self
                    .block_at_path_rel(&pit.path)
                    .and_then(|(b, o)| b.children.first().map(|c| c.span.start + o))
                    == Some(it.marker.start)
        });
        if it.empty {
            ed.push(it.ls..le, lead.trim_end().to_string());
        } else if parent_bare {
            let pit = ctx.parent.as_ref().unwrap().item();
            ed.push(pit.syntax_end..it.content, " ");
            self.reindent_rest(&mut ed, &it, -(it.content_offset as isize));
        } else {
            ed.push(it.ls..it.content, lead.clone());
            self.reindent_rest(&mut ed, &it, -(it.content_offset as isize));
        }
        if ctx.items.len() > 1 {
            let n = &ctx.items[1];
            // separate the rest of the list from the moved blocks (one blank line)
            if crate::text::count_terminators(&text[it.end..n.ls]) < 2 {
                ed.insert(n.ls, format!("{blank}{eol}"));
            }
            if ctx.sequential() {
                self.renumber(&mut ed, &ctx, 1, it.number.unwrap_or(1));
            }
        }
        let _ = next;
        let ps = ed.build()?;
        let caret = if it.empty {
            ps.map_offset(it.ls, markise_doc::Affinity::Upstream) + lead.trim_end().len()
        } else {
            ps.map_offset(it.content, markise_doc::Affinity::Downstream)
        };
        Some(Ok(
            Plan::single(PlanStep::patches(ps), Some(Selection::caret(caret))).labeled("Unwrap Item")
        ))
    }

    /// Backspace at the start of a later paragraph of a list item: merge with the previous
    /// paragraph of the same item (spec 02 §5.2 rule 5; C52 rule 4).
    fn item_paragraph_join(&self, at: usize) -> Option<Result<Plan, PlanError>> {
        let leaf = self.leaf(at)?;
        if leaf.kind != LeafKind::Paragraph || leaf.lines.first()?.start != at {
            return None;
        }
        let item = self.item_at(at)?;
        if item.content == at {
            return None;
        }
        let prev = self.leaf_before(leaf.span.start)?;
        if !matches!(prev.kind, LeafKind::Paragraph | LeafKind::Heading { .. }) || prev.span.start < item.content {
            return None;
        }
        let pend = prev.lines.last()?.end;
        // the shared heading-aware merge: `prev` keeps its syntax (a setext underline, an ATX
        // closing sequence); this paragraph's later lines keep the item's continuation prefix
        let mut ed = Edits::new();
        self.push_merge(&mut ed, &prev, pend, &leaf, &|_: &str| (0, 0), "");
        let ps = ed.build()?;
        Some(Ok(
            Plan::single(PlanStep::patches(ps), Some(Selection::caret(pend))).labeled("Merge")
        ))
    }

    /// Forward Delete at the end of an item's last paragraph before the next item (C48 rule
    /// 1): the next item's first paragraph joins; its other blocks stay (nested lists).
    pub(crate) fn list_delete_forward(&self, at: usize) -> Option<Result<Plan, PlanError>> {
        // cheap check first: only at the end of the item's last block is the list needed
        let it0 = self.item_at(at)?;
        if self
            .leaf(at)
            .is_none_or(|l| l.span.end != it0.end || l.lines.last().is_none_or(|x| x.end != at))
        {
            return None;
        }
        let ctx = self.list_ctx(at)?;
        let it = ctx.item();
        let next = ctx.items.get(ctx.idx + 1)?;
        let leaf = self.leaf(at)?;
        if !matches!(leaf.kind, LeafKind::Paragraph | LeafKind::Heading { .. }) || leaf.lines.last()?.end != at {
            return None;
        }
        // the caret's block must be the item's last leaf
        if leaf.span.end != it.end
            && self
                .block_at_path_rel(&it.path)
                .is_none_or(|(b, o)| b.span.end + o != leaf.span.end)
        {
            return None;
        }
        let text = self.text();
        let mut ed = Edits::new();
        if next.empty {
            // this block's own syntax (a setext underline) stays: only the empty item goes
            let keep = leaf.span.end.max(at);
            ed.delete(keep..line_end(text, next.ls).0);
        } else {
            let Some(nleaf) = self.leaf(next.content).filter(|l| {
                l.span.start == next.content && matches!(l.kind, LeafKind::Paragraph | LeafKind::Heading { .. })
            }) else {
                return Some(Ok(Plan::default()));
            };
            // the next item's later lines continue this item's text (its indentation)
            let cpl = next.marker.start - next.ls;
            let co = next.content_offset;
            let cut = move |line: &str| -> (usize, usize) {
                let b = line.as_bytes();
                let mut p = 0;
                while p < b.len() && p < cpl && (b[p] == b'>' || is_sp(b[p])) {
                    p += 1;
                }
                let sp = b[p..].iter().take_while(|c| **c == b' ').count().min(co);
                (p, p + sp)
            };
            let indent = " ".repeat(it.content_offset);
            let next_end = self.push_merge(&mut ed, &leaf, at, &nleaf, &cut, &indent);
            // the next item's other blocks (a nested list) move to this item's column
            let from = line_end(text, next_end).1;
            self.reindent_lines(&mut ed, from, next.end, cpl, it.content_offset as isize - co as isize);
        }
        if ctx.sequential() {
            self.renumber(&mut ed, &ctx, ctx.idx + 2, next.number.unwrap_or(1));
        }
        let ps = ed.build()?;
        Some(Ok(
            Plan::single(PlanStep::patches(ps), Some(Selection::caret(at))).labeled("Merge")
        ))
    }

    /// The merge of C48 / spec 02 §5.2 rule 5 (M2b-2 rules): replaces from `at` (the end of
    /// `leaf`'s text) to the end of `next` (incl. its syntax: ATX markers and closing
    /// sequence, setext underline) with `next`'s text — its later lines with `cut` (the
    /// container prefix it leaves) removed and `indent` added — followed by `leaf`'s own
    /// trailing syntax (ATX closing sequence, setext underline), which is kept. Returns the
    /// end of `next` (pre-edit).
    pub(crate) fn push_merge(
        &self,
        ed: &mut Edits,
        leaf: &markise_doc::LeafView,
        at: usize,
        next: &markise_doc::LeafView,
        cut: &dyn Fn(&str) -> (usize, usize),
        indent: &str,
    ) -> usize {
        let text = self.text();
        let eol = self.line_ending().as_str();
        let first = next.lines[0];
        let mut moved = text[first.start..first.end].to_string();
        for l in &next.lines[1..] {
            let ls = line_start(text, l.start);
            let line = &text[ls..l.end];
            let (a, z) = cut(line);
            moved.push_str(eol);
            moved.push_str(&line[..a]);
            moved.push_str(indent);
            moved.push_str(&line[z..]);
        }
        let trail_end = leaf.span.end.max(at);
        let next_end = next.span.end.max(next.lines.last().map_or(first.end, |l| l.end));
        ed.push(at..next_end, format!("{moved}{}", &text[at..trail_end]));
        next_end
    }

    /// Enter on the virtual empty paragraph: it moves out of one more container (an empty
    /// paragraph in a quote leaves the quote, spec 02 §5.1); no source change.
    pub(crate) fn virtual_enter(&self, gap: usize, virt: u16) -> Result<Plan, PlanError> {
        let probe = gap.saturating_sub(1);
        let path = self.block_path_at(probe);
        let containers: Vec<(&Block, usize)> = (1..=path.len())
            .filter_map(|d| self.block_at_path_rel(&path[..d]))
            .filter(|(b, _)| b.is_container())
            .collect();
        let levels = virt.saturating_sub(1) as usize;
        if levels >= containers.len() {
            return Ok(Plan::default());
        }
        let (x, o) = containers[containers.len() - 1 - levels];
        let step = if matches!(x.kind, BlockKind::ListItem { .. }) {
            2
        } else {
            1
        };
        let caret = Selection::caret_at(DocPos::with_virt(gap, virt + step));
        // C45: leaving the middle of a quote splits it — the separator is written now
        let text = self.text();
        let (gap_le, after) = line_end(text, gap);
        if matches!(x.kind, BlockKind::BlockQuote { .. }) && x.span.end + o > gap_le && after < text.len() {
            let xs = x.span.start + o;
            let depth = text[line_start(text, xs)..=xs].matches('>').count();
            let mut ed = Edits::new();
            let mut ls = after;
            let mut blank = false;
            while ls < text.len() {
                let (le, next) = line_end(text, ls);
                if !text[ls..le].trim_start_matches(['>', ' ', '\t']).is_empty() {
                    break;
                }
                self.unquote_line(&mut ed, ls, depth);
                blank = true;
                if next == le {
                    break;
                }
                ls = next;
            }
            if !blank {
                let eol = self.line_ending().as_str();
                let outer: String = {
                    let pre = cont_prefix(
                        text,
                        after,
                        after + (text[after..].len() - text[after..].trim_start_matches(['>', ' ', '\t']).len()),
                    );
                    let mut seen = 0;
                    pre.chars()
                        .filter(|c| {
                            if *c == '>' {
                                seen += 1;
                                seen < depth
                            } else {
                                true
                            }
                        })
                        .collect()
                };
                ed.insert(after, format!("{}{eol}", outer.trim_end()));
            }
            if let Some(ps) = ed.build() {
                return Ok(Plan::single(PlanStep::patches(ps), Some(caret)));
            }
        }
        Ok(Plan::select(caret))
    }

    /// Shifts the indentation of the item's lines after its marker line by `delta` columns
    /// (spaces after the container prefix).
    pub(crate) fn reindent_rest(&self, ed: &mut Edits, it: &Item, delta: isize) {
        let text = self.text();
        self.reindent_lines(ed, line_end(text, it.ls).1, it.end, it.marker.start - it.ls, delta);
    }

    /// Shifts the indentation of the non-blank lines starting in `from..to` by `delta` columns
    /// (spaces after `cprefix_len` columns of quote markers / outer indentation).
    pub(crate) fn reindent_lines(&self, ed: &mut Edits, from: usize, to: usize, cprefix_len: usize, delta: isize) {
        let text = self.text();
        let mut ls = from;
        while ls < to && ls < text.len() {
            let (le, next) = line_end(text, ls);
            let line = &text[ls..le];
            if !line.trim().is_empty() {
                // skip the container prefix (quote markers / indentation of outer items)
                let b = line.as_bytes();
                let mut p = 0;
                let mut cols = 0;
                while p < b.len() && cols < cprefix_len && (b[p] == b'>' || is_sp(b[p])) {
                    p += 1;
                    cols += 1;
                }
                let spaces = b[p..].iter().take_while(|c| **c == b' ').count();
                if delta < 0 {
                    let k = spaces.min((-delta) as usize);
                    ed.delete(ls + p..ls + p + k);
                } else if delta > 0 {
                    ed.insert(ls + p, " ".repeat(delta as usize));
                }
            }
            if next == le {
                break;
            }
            ls = next;
        }
    }

    // ---- Tab / Shift+Tab ------------------------------------------------------------------

    /// Indents (`out = false`) or outdents the list items touched by the selection (spec 02
    /// §5.3 rule 1). `None` outside lists.
    pub(crate) fn list_indent(&self, req: &PlanRequest, out: bool) -> Option<Result<Plan, PlanError>> {
        let r = req.selection.primary().range();
        let ctx = self.list_ctx(r.start)?;
        let text = self.text();
        // the items of this list touched by the selection
        let last = ctx
            .items
            .iter()
            .rposition(|i| i.ls <= r.end)
            .unwrap_or(ctx.idx)
            .max(ctx.idx);
        let mut ed = Edits::new();
        if !out {
            if ctx.idx == 0 {
                return Some(Ok(Plan::default().labeled("Indent"))); // nothing to nest under
            }
            let prev = &ctx.items[ctx.idx - 1];
            // an empty item followed by a blank line cannot take children (it ends there)
            if prev.empty && crate::text::count_terminators(&text[prev.end..ctx.item().ls]) >= 2 {
                return Some(Ok(Plan::default().labeled("Indent")));
            }
            let target_col = prev.marker.start - prev.ls + prev.content_offset;
            let cur_col = ctx.item().marker.start - ctx.item().ls;
            let delta = target_col as isize - cur_col as isize;
            if delta <= 0 {
                return Some(Err(PlanError::NotApplicable("cannot indent")));
            }
            // does the previous item end with a nested list this item joins?
            let joins = self
                .block_at_path_rel(&prev.path)
                .and_then(|(b, _)| b.children.last())
                .is_some_and(|c| matches!(c.kind, BlockKind::List { .. }));
            let mut n = if joins {
                self.block_at_path_rel(&prev.path)
                    .and_then(|(b, _)| b.children.last())
                    .and_then(|l| l.children.last())
                    .and_then(|i| match i.kind {
                        BlockKind::ListItem { number, .. } => number,
                        _ => None,
                    })
                    .map(|x| x + 1)
            } else {
                Some(1)
            };
            for it in &ctx.items[ctx.idx..=last] {
                ed.insert(it.marker.start, " ".repeat(delta as usize));
                let mark = match (it.number, n) {
                    (Some(_), Some(k)) => {
                        n = Some(k + 1);
                        format!("{k}{}", ctx.delim())
                    }
                    _ => self.text()[it.marker.range()].to_string(),
                };
                self.set_marker(&mut ed, it, mark, delta);
            }
            if ctx.sequential() {
                self.renumber(&mut ed, &ctx, last + 1, ctx.items[ctx.idx].number.unwrap_or(1));
            }
        } else {
            let Some(parent) = &ctx.parent else {
                return Some(Ok(Plan::default().labeled("Outdent"))); // already top level
            };
            let pit = parent.item();
            let delta = (ctx.item().marker.start - ctx.item().ls) as isize - (pit.marker.start - pit.ls) as isize;
            if delta <= 0
                || cont_prefix(text, ctx.item().ls, ctx.item().marker.start).contains('>')
                    != cont_prefix(text, pit.ls, pit.marker.start).contains('>')
            {
                return Some(Err(PlanError::NotApplicable("cannot outdent")));
            }
            let mut k = pit.number.map(|n| n + 1);
            for it in &ctx.items[ctx.idx..=last] {
                let b = text.as_bytes();
                let mut p = it.marker.start;
                let mut removed = 0;
                while p > it.ls && removed < delta as usize && b[p - 1] == b' ' {
                    p -= 1;
                    removed += 1;
                }
                ed.delete(p..it.marker.start);
                let mark = if parent.ordered != ctx.ordered || parent.marker != ctx.marker {
                    match k {
                        Some(n) => format!("{n}{}", parent.delim()),
                        None => (parent.marker as char).to_string(),
                    }
                } else if let (Some(n), Some(_)) = (k, it.number) {
                    format!("{n}{}", parent.delim())
                } else {
                    text[it.marker.range()].to_string()
                };
                self.set_marker(&mut ed, it, mark, -(removed as isize));
                k = k.map(|n| n + 1);
            }
            // everything after the outdented items in the parent item (later siblings, the
            // parent's further blocks) now belongs to the last of them: it moves to (at least)
            // that item's content column so nothing is detached (`100.` is wider)
            let li = &ctx.items[last];
            if pit.end > li.end {
                let new_marker_col = (li.marker.start - li.ls) as isize - delta;
                let new_w = match k.map(|n| n - 1) {
                    Some(n) if parent.ordered || ctx.ordered => format!("{n}{}", parent.delim()).len(),
                    _ => li.marker.len(),
                };
                let new_content_col = new_marker_col + new_w as isize + 1;
                let pit_content_col = (pit.marker.start - pit.ls + pit.content_offset) as isize;
                let shift = new_content_col - pit_content_col;
                if shift > 0 {
                    let from = line_end(text, li.end).1;
                    self.reindent_lines(&mut ed, from, pit.end, pit.marker.start - pit.ls, shift);
                }
            }
            if parent.sequential() {
                let count = (last - ctx.idx + 1) as u64;
                self.renumber(&mut ed, parent, parent.idx + 1, pit.number.unwrap_or(1) + 1 + count);
            }
            if ctx.sequential() {
                self.renumber(&mut ed, &ctx, last + 1, ctx.items[ctx.idx].number.unwrap_or(1));
            }
        }
        let ps = match ed.build() {
            Some(p) => p,
            None => return Some(Err(PlanError::NotApplicable("overlap"))),
        };
        let sel = req.selection.primary();
        let a = ps.map_offset(sel.anchor.offset, markise_doc::Affinity::Downstream);
        let f = ps.map_offset(sel.focus.offset, markise_doc::Affinity::Downstream);
        // an empty item nested under a paragraph reads as paragraph text in CommonMark: a
        // ForceBlock hint keeps it an item while the caret is in it (removed if left empty)
        let force = if out {
            // an empty sibling that now starts the outdented item's child list right after
            // its paragraph would read as a setext underline: keep it an item
            ctx.items.get(last + 1).filter(|i| i.empty).map(|i| i.ls)
        } else {
            (ctx.item().empty && last == ctx.idx && !self.item_joins_nested(&ctx)).then_some(ctx.item().ls)
        };
        let mut plan = Plan::single(PlanStep::patches(ps), Some(Selection::range(a, f))).labeled(if out {
            "Outdent"
        } else {
            "Indent"
        });
        self.fix_force_hints(&mut plan, force);
        Some(Ok(plan))
    }

    fn item_joins_nested(&self, ctx: &ListCtx) -> bool {
        ctx.idx > 0
            && self
                .block_at_path_rel(&ctx.items[ctx.idx - 1].path)
                .and_then(|(b, _)| b.children.last())
                .is_some_and(|c| matches!(c.kind, BlockKind::List { .. }))
    }

    /// ForceBlock hints (see [`markise_doc::HintOrigin::ForceBlock`]) on lines a single-step
    /// plan rewrites are dropped; `add` (a pre-edit line start) gains one.
    pub(crate) fn fix_force_hints(&self, plan: &mut Plan, add: Option<usize>) {
        use markise_doc::{HintOrigin, LiteralHint};
        let has = self.hints().iter().any(|h| h.origin == HintOrigin::ForceBlock);
        if (!has && add.is_none()) || plan.steps.len() != 1 {
            return;
        }
        let text = self.text();
        let ps = plan.steps[0].patches.clone();
        let mut kept = markise_doc::HintSet::new();
        for h in self.hints().iter() {
            let le = line_end(text, h.line).0;
            let touched = ps
                .patches()
                .iter()
                .any(|p| p.range.start <= le && p.range.end >= h.line);
            if h.origin == HintOrigin::ForceBlock && touched {
                continue;
            }
            kept.insert(*h);
        }
        if let Some(ls) = add {
            kept.insert(LiteralHint::new(ls, HintOrigin::ForceBlock));
        }
        let out = kept.transform(&ps, |_| true);
        plan.steps[0].hints_after = Some(out);
    }

    /// Shift+Enter (spec 02 §5.1, C42, C44): a soft line break with the container prefix;
    /// on a soft line empty up to the caret it acts as Enter (C42); in an ATX heading it
    /// inserts `<br/>`, in a setext heading a real break only where the heading stays one
    /// heading with one more line (C44).
    pub(crate) fn plan_soft_break(&self, req: &PlanRequest) -> Result<Plan, PlanError> {
        let sel = req.selection.primary();
        if !sel.is_caret() || sel.focus.virt > 0 {
            return Err(PlanError::NotApplicable("selection"));
        }
        let at = sel.focus.offset;
        let text = self.text();
        let eol = self.line_ending().as_str();
        let leaf = self
            .leaf(at)
            .or_else(|| self.leaf_before(at))
            .ok_or(PlanError::NotApplicable("no block"))?;
        let Some(li) = leaf.lines.iter().position(|l| l.start <= at && at <= l.end) else {
            // an empty soft line after a paragraph (C42): the paragraph ends there
            if leaf.kind == LeafKind::Paragraph
                && let Some(last) = leaf.lines.last()
                && last.end < at
                && text[last.end..at].trim().is_empty()
                && crate::text::count_terminators(&text[last.end..at]) == 1
            {
                // the hard-break marker before it goes too
                let mut pe = last.end;
                if text[last.start..pe].ends_with('\\') {
                    pe -= 1;
                }
                while pe > last.start && is_sp(text.as_bytes()[pe - 1]) {
                    pe -= 1;
                }
                let ps = markise_doc::PatchSet::delete(pe..at);
                return Ok(Plan::single(
                    PlanStep::patches(ps),
                    Some(Selection::caret_at(DocPos::with_virt(pe, 1))),
                )
                .labeled("New Line"));
            }
            return Err(PlanError::NotApplicable("not in text"));
        };
        let line = leaf.lines[li];
        let br = |at: usize| {
            Ok(Plan::single(
                PlanStep::patches(markise_doc::PatchSet::single_verbatim(at..at, "<br/>")),
                Some(Selection::caret(at + 5)),
            ))
        };
        match leaf.kind {
            LeafKind::Heading { .. } => {
                let atx = text[leaf.span.start..].starts_with('#');
                if atx {
                    return br(at);
                }
                // setext: try the real break; keep it only if the heading survives intact
                let ls = line_start(text, line.start);
                let prefix = cont_prefix(text, ls, line.start);
                let ins = format!("{eol}{prefix}");
                let mut cand = text[leaf.span.start..leaf.span.end].to_string();
                let rel = at - leaf.span.start;
                let rest = text[at..line.end].trim_start_matches([' ', '\t']);
                let before = text[line.start..at].trim();
                if before.is_empty() || rest.trim().is_empty() {
                    return br(at);
                }
                cand.insert_str(rel, &ins);
                // judge the heading on its own (container prefixes stripped line by line)
                let plain: String = cand
                    .split('\n')
                    .map(|l| l.trim_start_matches(['>', ' ', '\t']).trim_end_matches('\r'))
                    .collect::<Vec<_>>()
                    .join("\n");
                let d = Document::parse(plain, self.options().clone());
                let ok = d.blocks().len() == 1
                    && matches!(d.blocks()[0].kind, BlockKind::Heading { setext: true, .. })
                    && d.blocks()[0].lines.len() == leaf.lines.len() + 1
                    && matches!((&d.blocks()[0].kind, &leaf.kind), (BlockKind::Heading { level: a, .. }, LeafKind::Heading { level: b }) if a == b);
                if !ok {
                    return br(at);
                }
                // the whitespace kept at the start of the new line is not a caret stop
                let ws = text[at..].len() - text[at..].trim_start_matches([' ', '\t']).len();
                Ok(Plan::single(
                    PlanStep::patches(markise_doc::PatchSet::single_verbatim(at..at, ins.clone())),
                    Some(Selection::caret(at + ins.len() + ws)),
                ))
            }
            LeafKind::Paragraph => {
                // C42: on a soft line empty up to the caret, Shift+Enter acts as Enter
                if li > 0 && text[line.start..at].trim().is_empty() {
                    let prev = leaf.lines[li - 1];
                    let mut pe = prev.end;
                    let pb = text.as_bytes();
                    if text[prev.start..pe].ends_with('\\') {
                        pe -= 1;
                    }
                    while pe > prev.start && is_sp(pb[pe - 1]) {
                        pe -= 1;
                    }
                    let mut z = at;
                    while z < line.end && is_sp(pb[z]) {
                        z += 1;
                    }
                    if z == line.end && li + 1 == leaf.lines.len() {
                        // nothing after: the paragraph ends, the caret on a virtual paragraph
                        let ps = markise_doc::PatchSet::delete(pe..line.end);
                        return Ok(Plan::single(
                            PlanStep::patches(ps),
                            Some(Selection::caret_at(DocPos::with_virt(pe, 1))),
                        )
                        .labeled("New Line"));
                    }
                    let ls = line_start(text, line.start);
                    let prefix = cont_prefix(text, ls, line.start);
                    let ins = format!("{eol}{}{eol}{prefix}", prefix.trim_end());
                    let ps = markise_doc::PatchSet::single_verbatim(pe..z, ins.clone());
                    return Ok(
                        Plan::single(PlanStep::patches(ps), Some(Selection::caret(pe + ins.len()))).labeled("New Line"),
                    );
                }
                // at the very end of the paragraph a soft break would leave an empty line
                // Markdown cannot keep: the paragraph ends (virtual paragraph after it)
                if li + 1 == leaf.lines.len() && text[at..line.end].trim().is_empty() {
                    return Ok(
                        Plan::select(Selection::caret_at(DocPos::with_virt(leaf.span.end, 1))).labeled("New Line")
                    );
                }
                let ls = line_start(text, line.start);
                let prefix = if li == 0 {
                    // the first line: block markers of the leaf's containers become spaces
                    cont_prefix(text, ls, leaf.lines[0].start)
                } else {
                    cont_prefix(text, ls, line.start)
                };
                let ins = format!("{eol}{prefix}");
                Ok(Plan::single(
                    PlanStep::patches(markise_doc::PatchSet::single_verbatim(at..at, ins.clone())),
                    Some(Selection::caret(at + ins.len())),
                ))
            }
            _ => Err(PlanError::NotApplicable("not a paragraph or heading")),
        }
    }

    /// M2c block planners; `None` when `id` is not one of them.
    pub(crate) fn plan_m2c(&self, id: PlannerId, req: &PlanRequest) -> Option<Result<Plan, PlanError>> {
        let sel = req.selection.primary();
        match id {
            planners::DELETE_BACKWARD => {
                if !sel.is_caret() || sel.focus.virt > 0 {
                    return Some(Err(PlanError::NotApplicable("selection")));
                }
                let at = sel.focus.offset;
                if let Some(r) = self.empty_heading_key(at, planners::DELETE_BACKWARD) {
                    return Some(r);
                }
                let text = self.text();
                // auto-pair (spec 02 §4.3): an opener with its closer right after → both go
                if req.prefs.auto_pair_brackets
                    && let (Some(o), Some(c)) = (text[..at].chars().next_back(), text[at..].chars().next())
                    && matches!((o, c), ('(', ')') | ('[', ']') | ('{', '}') | ('"', '"') | ('\'', '\''))
                    && self
                        .leaf(at)
                        .is_some_and(|l| matches!(l.kind, LeafKind::Paragraph | LeafKind::Heading { .. }))
                {
                    let ps = markise_doc::PatchSet::delete(at - 1..at + 1);
                    return Some(Ok(Plan::single(PlanStep::patches(ps), Some(Selection::caret(at - 1)))));
                }
                Some(
                    self.list_backspace(at)
                        .or_else(|| self.quote_backspace(at))
                        .or_else(|| self.item_paragraph_join(at))
                        .unwrap_or(Err(PlanError::NotApplicable("no rule")))
                        .map(|mut p| {
                            self.fix_force_hints(&mut p, None);
                            p
                        }),
                )
            }
            planners::DELETE_FORWARD => {
                if !sel.is_caret() || sel.focus.virt > 0 {
                    return Some(Err(PlanError::NotApplicable("selection")));
                }
                if let Some(r) = self.empty_heading_key(sel.focus.offset, planners::DELETE_FORWARD) {
                    return Some(r);
                }
                Some(
                    self.list_delete_forward(sel.focus.offset)
                        .or_else(|| self.c48_delete_forward(sel.focus.offset))
                        .unwrap_or(Err(PlanError::NotApplicable("no rule"))),
                )
            }
            planners::SOFT_BREAK => Some(self.plan_soft_break(req)),
            planners::MOVE_UP => Some(self.plan_move_block(req, false)),
            planners::MOVE_DOWN => Some(self.plan_move_block(req, true)),
            planners::QUOTE => Some(self.plan_toggle_quote(req)),
            planners::BULLET_LIST => Some(self.plan_toggle_list(req, super::structure::ListKind::Bullet)),
            planners::ORDERED_LIST => Some(self.plan_toggle_list(req, super::structure::ListKind::Ordered)),
            planners::TASK_LIST => Some(self.plan_toggle_list(req, super::structure::ListKind::Task)),
            planners::SELECT_WORD => Some(self.plan_select(super::select::Unit::Word, req)),
            planners::SELECT_LINE => Some(self.plan_select(super::select::Unit::Line, req)),
            planners::SELECT_SCOPE => Some(self.plan_select(super::select::Unit::Scope, req)),
            planners::SELECT_BLOCK => Some(self.plan_select(super::select::Unit::Block, req)),
            planners::DELETE_WORD => Some(self.plan_delete_unit(super::select::Unit::Word, req)),
            planners::DELETE_LINE => Some(self.plan_delete_unit(super::select::Unit::Line, req)),
            planners::DELETE_SCOPE => Some(self.plan_delete_unit(super::select::Unit::Scope, req)),
            planners::DELETE_BLOCK => Some(self.plan_delete_unit(super::select::Unit::Block, req)),
            PlannerId("block.insertRule") => Some(self.plan_insert_rule(req)),
            planners::TAB | planners::INDENT => Some(
                self.list_indent(req, false)
                    .unwrap_or(Err(PlanError::NotApplicable("not in a list"))),
            ),
            planners::SHIFT_TAB | planners::OUTDENT => Some(
                self.list_indent(req, true)
                    .unwrap_or(Err(PlanError::NotApplicable("not in a list"))),
            ),
            _ => None,
        }
    }

    /// Check state of block toggles (Quote, lists).
    pub(crate) fn block_checked(&self, id: PlannerId, req: &PlanRequest) -> bool {
        let at = req.selection.primary().focus.offset;
        let list = || self.list_ctx(at);
        match id {
            planners::QUOTE => self.leaf(at).is_some_and(|l| {
                matches!(
                    l.containers.last(),
                    Some(markise_doc::ContainerKind::Quote | markise_doc::ContainerKind::Callout(_))
                )
            }),
            planners::BULLET_LIST => {
                list().is_some_and(|c| !c.ordered && !c.items.iter().all(|i| i.syntax_end > i.marker.end))
            }
            planners::ORDERED_LIST => list().is_some_and(|c| c.ordered),
            planners::TASK_LIST => {
                list().is_some_and(|c| !c.ordered && c.items.iter().all(|i| i.syntax_end > i.marker.end))
            }
            planners::PARAGRAPH => self.leaf(at).is_some_and(|l| l.kind == LeafKind::Paragraph),
            _ => planners::heading_level(id)
                .is_some_and(|n| self.leaf(at).is_some_and(|l| l.kind == LeafKind::Heading { level: n })),
        }
    }
}

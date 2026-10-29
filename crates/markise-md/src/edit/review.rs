//! Review planners (ADR 0012, issue #69): Add Comment, Accept / Reject (one, all), Delete
//! Comment and the clean copy, over CriticMarkup marks found through the neutral inline trees
//! ([`markise_doc::review`]). Each planner returns one patch set, so every command is one
//! undo step.
//!
//! Whitespace: removing a mark (a rejected insertion, an accepted deletion, a deleted comment)
//! must not leave two spaces or a space before punctuation behind, so one adjacent space goes
//! with it: the space after the mark when there are spaces on both sides or the mark starts
//! its line, otherwise the space before it when the mark is followed by punctuation or the
//! end of the line.

use markise_doc::review::{self, ReviewMark};
use markise_doc::{
    CriticKind, FormatDoc, InlineKind as NK, InlineNode, LeafKind, Plan, PlanError, PlanRequest, PlanStep, PlannerId,
    Selection, planners,
};

use super::Edits;
use crate::Document;

/// What to do with the suggestions while building a result.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Resolve {
    Accept,
    Reject,
}

/// One replacement over the text, with the caret after it.
struct Change {
    range: std::ops::Range<usize>,
    insert: String,
}

impl Document {
    /// Review planners; `None`: `id` is not one of them.
    pub(crate) fn plan_review(&self, id: PlannerId, req: &PlanRequest) -> Option<Result<Plan, PlanError>> {
        let r = match id {
            planners::REVIEW_ADD_COMMENT => self.plan_add_comment(req),
            planners::REVIEW_ACCEPT => self.plan_resolve_one(req, Resolve::Accept),
            planners::REVIEW_REJECT => self.plan_resolve_one(req, Resolve::Reject),
            planners::REVIEW_ACCEPT_ALL => self.plan_resolve_all(Resolve::Accept, false),
            planners::REVIEW_REJECT_ALL => self.plan_resolve_all(Resolve::Reject, false),
            planners::REVIEW_DELETE_COMMENT => self.plan_delete_comment(req),
            planners::REVIEW_CLEAN_COPY => {
                let how = if req.params.get_bool("reject") == Some(true) {
                    Resolve::Reject
                } else {
                    Resolve::Accept
                };
                self.plan_resolve_all(how, true)
            }
            _ => return None,
        };
        Some(r)
    }

    /// `can_plan` for the review planners: whole-document ones answer from the setting alone
    /// (menus ask every frame; no document scan), the others plan at the caret (one leaf).
    pub(crate) fn can_plan_review(&self, id: PlannerId, req: &PlanRequest) -> Option<bool> {
        match id {
            planners::REVIEW_ACCEPT_ALL | planners::REVIEW_REJECT_ALL | planners::REVIEW_CLEAN_COPY => {
                Some(self.options().critic_markup)
            }
            planners::REVIEW_ADD_COMMENT
            | planners::REVIEW_ACCEPT
            | planners::REVIEW_REJECT
            | planners::REVIEW_DELETE_COMMENT => Some(self.plan_review(id, req).is_some_and(|r| r.is_ok())),
            _ => None,
        }
    }

    fn review_on(&self) -> Result<(), PlanError> {
        if self.options().critic_markup {
            Ok(())
        } else {
            Err(PlanError::NotApplicable("comments and suggestions are off"))
        }
    }

    /// The offset a one-mark command acts at: the `at` parameter, else the caret.
    fn review_at(req: &PlanRequest) -> usize {
        match req.params.get_int("at") {
            Some(a) if a >= 0 => a as usize,
            _ => req.selection.primary().focus.offset,
        }
    }

    // ---------------------------------------------------------------- Add Comment

    /// Add Comment: a selection becomes `{==selection==}{>><<}`, a caret gets `{>><<}`; the
    /// caret goes into the empty comment. Selection ends that cut through a styled span are
    /// widened to the whole span; selections over several blocks, inside code, math or raw
    /// HTML, or touching an existing mark are refused (marks do not nest).
    fn plan_add_comment(&self, req: &PlanRequest) -> Result<Plan, PlanError> {
        self.review_on()?;
        let sel = req.selection.primary();
        if sel.focus.virt > 0 || sel.anchor.virt > 0 {
            return Err(PlanError::NotApplicable("virtual paragraph"));
        }
        let text = self.text();
        let r = sel.range();
        let (mut a, mut b) = (r.start, r.end);
        // trim surrounding whitespace (C49)
        while a < b && matches!(text.as_bytes()[a], b' ' | b'\t' | b'\n' | b'\r') {
            a += 1;
        }
        while b > a && matches!(text.as_bytes()[b - 1], b' ' | b'\t' | b'\n' | b'\r') {
            b -= 1;
        }
        let collapsed = a == b;
        if collapsed {
            a = sel.focus.offset;
            b = a;
        }
        let leaf = self.leaf(a).ok_or(PlanError::NotApplicable("no block"))?;
        if !matches!(leaf.kind, LeafKind::Paragraph | LeafKind::Heading { .. }) {
            return Err(PlanError::NotApplicable("not in text"));
        }
        let content = leaf.content();
        if a < content.start || b > content.end {
            return Err(PlanError::NotApplicable("selection spans blocks"));
        }
        let base = content.start;
        let rich = self.inlines(&leaf);
        // widen through styled spans the selection cuts (`**a|b**` → the whole strong span)
        let mut nodes = Vec::new();
        flat(&rich.nodes, &mut nodes);
        if !collapsed {
            loop {
                let (a0, b0) = (a, b);
                for n in &nodes {
                    if n.markers.is_empty() || matches!(n.kind, NK::Critic(_)) {
                        continue;
                    }
                    let (s, e) = (base + n.source.start as usize, base + n.source.end as usize);
                    // one end inside the span, the other outside it
                    if s < a && a < e && b >= e {
                        a = s;
                    }
                    if s < b && b < e && a <= s {
                        b = e;
                    }
                }
                if (a, b) == (a0, b0) {
                    break;
                }
            }
        }
        for n in &nodes {
            let (s, e) = (base + n.source.start as usize, base + n.source.end as usize);
            let inside = |p: usize| s < p && p < e;
            let critic = matches!(n.kind, NK::Critic(_));
            if critic && (inside(a) || inside(b) || (!collapsed && a <= s && e <= b)) {
                return Err(PlanError::NotApplicable("inside a comment or suggestion"));
            }
            let literal = matches!(
                n.kind,
                NK::Code | NK::Math { .. } | NK::RawInline | NK::Atom | NK::Image { .. }
            );
            if literal && (inside(a) || inside(b)) {
                return Err(PlanError::NotApplicable("inside code or math"));
            }
        }
        let mut ed = Edits::new();
        let caret = if collapsed {
            ed.insert(a, "{>><<}");
            a + 3
        } else {
            ed.insert(a, "{==");
            ed.insert(b, "==}{>><<}");
            b + 3 + 6
        };
        let ps = ed.build().ok_or(PlanError::NotApplicable("overlap"))?;
        Ok(Plan::single(PlanStep::patches(ps), Some(Selection::caret(caret))))
    }

    // ---------------------------------------------------------------- accept / reject

    fn plan_resolve_one(&self, req: &PlanRequest, how: Resolve) -> Result<Plan, PlanError> {
        self.review_on()?;
        let at = Self::review_at(req);
        let marks = review::marks_near(self, at);
        let i = marks
            .iter()
            .position(|m| m.is_suggestion() && m.range.start <= at && at <= m.range.end)
            .or_else(|| {
                // between a suggestion and its comment, or on the comment of a suggestion
                marks
                    .iter()
                    .position(|m| {
                        m.kind == CriticKind::Comment && m.attached && m.range.start <= at && at <= m.range.end
                    })
                    .and_then(|c| c.checked_sub(1))
                    .filter(|&p| marks[p].is_suggestion())
            })
            .ok_or(PlanError::NotApplicable("no suggestion here"))?;
        let text = self.text();
        let ch = resolve_change(text, &marks, i, how);
        single_change_plan(text, ch)
    }

    /// Accept or reject every suggestion (`clean`: also remove every comment and highlight,
    /// for a clean copy). The caret is mapped through the patches by the session.
    fn plan_resolve_all(&self, how: Resolve, clean: bool) -> Result<Plan, PlanError> {
        self.review_on()?;
        // literally one mark after another (review round 2, P2-6): each is resolved with the
        // one-mark planner on a working copy that is re-parsed incrementally, so the result is
        // by construction the one-by-one result; the overall difference is one patch (one undo
        // step). Every step removes at least the mark's delimiters, so this terminates.
        let mut work: Box<dyn FormatDoc> = self.clone_box();
        let mut from = 0usize;
        let mut any = false;
        while let Some(m) = review::first_mark_from(work.as_ref(), from, |m| m.is_suggestion() || clean) {
            let id = match (m.is_suggestion(), how) {
                (true, Resolve::Accept) => planners::REVIEW_ACCEPT,
                (true, Resolve::Reject) => planners::REVIEW_REJECT,
                (false, _) => planners::REVIEW_DELETE_COMMENT,
            };
            let req = PlanRequest {
                params: markise_doc::Params::new().with("at", markise_doc::ParamValue::Int(m.content.start as i64)),
                ..Default::default()
            };
            let before = work.text().len();
            let plan = work.plan(id, &req)?;
            for step in plan.steps {
                work.apply(&step.patches);
            }
            if work.text().len() >= before {
                return Err(PlanError::NotApplicable("a mark could not be resolved"));
            }
            any = true;
            // a `{` just before the mark may open a new mark with the restored text
            from = m.range.start.saturating_sub(3);
        }
        if !any {
            return Err(PlanError::NotApplicable(if clean {
                "no comments or suggestions"
            } else {
                "no suggestions"
            }));
        }
        // the overall change: the differing middle between the old and the new text
        let (old, new) = (self.text(), work.text());
        let pre = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
        let max_suf = old.len().min(new.len()) - pre;
        let suf = old
            .bytes()
            .rev()
            .zip(new.bytes().rev())
            .take(max_suf)
            .take_while(|(a, b)| a == b)
            .count();
        // keep the cut on char boundaries
        let (mut a, mut e_old, mut e_new) = (pre, old.len() - suf, new.len() - suf);
        while !old.is_char_boundary(a) || !new.is_char_boundary(a) {
            a -= 1;
        }
        while !old.is_char_boundary(e_old) || !new.is_char_boundary(e_new) {
            e_old += 1;
            e_new += 1;
        }
        let ps = markise_doc::PatchSet::single_verbatim(a..e_old, &new[a..e_new]);
        Ok(Plan::single(PlanStep::patches(ps), None))
    }

    // ---------------------------------------------------------------- Enter in a comment

    /// Enter with the caret inside a comment's text (owner decision, ADR 0012): one line break
    /// inside the comment, followed by the paragraph's continuation prefix (`> ` of a quote,
    /// the indentation of a list item), so the paragraph and the mark stay intact. A blank
    /// line would end the paragraph, so Enter right before or after a line break does
    /// nothing. `None`: the caret is not in a comment.
    pub(crate) fn plan_comment_newline(&self, req: &PlanRequest) -> Option<Result<Plan, PlanError>> {
        if !self.options().critic_markup || req.source_mode {
            return None;
        }
        let sel = req.selection.primary();
        if !sel.is_caret() || sel.focus.virt > 0 {
            return None;
        }
        let at = sel.focus.offset;
        let m = review::mark_at(self, at, |k| k == CriticKind::Comment)?;
        if !(m.content.start <= at && at <= m.content.end) {
            return None;
        }
        let text = self.text();
        let leaf = self.leaf(at)?;
        let ignore = || Some(Ok(Plan::select(req.selection.clone())));
        // single-line blocks (headings, table rows): a line break would break the block
        if !matches!(leaf.kind, LeafKind::Paragraph) {
            return ignore();
        }
        let line = leaf.lines.iter().find(|l| l.start <= at && at <= l.end)?;
        // never a blank or whitespace-only line inside the mark: both parts of the comment's
        // line around the caret must keep some text
        let blank = |s: &str| s.trim_matches([' ', '\t']).is_empty();
        let before = &text[line.start.max(m.content.start)..at];
        let after = &text[at..line.end.min(m.content.end).max(at)];
        if blank(before) || blank(after) {
            return ignore();
        }
        // the continuation prefix: a continuation line repeats its own prefix; the first line
        // of a list item gives quote markers plus the item's content indent (marker and its
        // spaces, a task box not included); a quote gives its markers
        let ls = crate::line_start(text, line.start);
        let first = leaf.lines.first().is_some_and(|f| f.start == line.start);
        let prefix: String = if !first {
            text[ls..line.start].to_string()
        } else {
            let item = self
                .containers_at(at)
                .into_iter()
                .rev()
                .find(|c| matches!(c.kind, markise_doc::ContainerKind::ListItem { .. }))
                .filter(|c| c.span.start >= ls && c.span.start <= line.start);
            let upto = match item {
                Some(c) => item_content_start(text.as_bytes(), c.span.start, line.start),
                None => line.start,
            };
            text[ls..upto]
                .chars()
                .map(|c| if c == '>' || c == '\t' { c } else { ' ' })
                .collect()
        };
        let insert = format!("{}{prefix}", self.line_ending().as_str());
        // the general check (review round 3): the block re-parsed with the line break must
        // still hold the same comment over both lines — text after the caret that starts block
        // syntax (`# `, `> `, `- `, a fence, a rule…) would end the paragraph
        if !self.comment_survives_break(at, &insert, &m) {
            return ignore();
        }
        let caret = at + insert.len();
        let ps = markise_doc::PatchSet::single_verbatim(at..at, insert);
        Some(Ok(Plan::single(PlanStep::patches(ps), Some(Selection::caret(caret)))))
    }

    /// Whether inserting `insert` at `at` keeps comment `m` one comment: the top-level block
    /// holding `at` (whole lines) is parsed alone with the insertion and must contain a
    /// comment at the same place, grown by the insertion.
    fn comment_survives_break(&self, at: usize, insert: &str, m: &ReviewMark) -> bool {
        let text = self.text();
        let i = self.top_partition_point(|t| t.end() < at);
        let Some(top) = self.top(i) else { return false };
        let a = crate::line_start(text, top.start().min(at));
        let (b, _) = crate::text::line_end(text, top.end().max(at));
        if !(a <= m.range.start && m.range.end <= b) {
            return false;
        }
        let mut block = String::with_capacity(b - a + insert.len());
        block.push_str(&text[a..at]);
        block.push_str(insert);
        block.push_str(&text[at..b]);
        let doc = Document::parse(block, self.options().clone());
        let (s, e) = (m.range.start - a, m.range.end - a + insert.len());
        review::all_marks(&doc)
            .iter()
            .any(|c| c.kind == CriticKind::Comment && c.range.start == s && c.range.end == e)
    }

    // ---------------------------------------------------------------- delete comment

    fn plan_delete_comment(&self, req: &PlanRequest) -> Result<Plan, PlanError> {
        self.review_on()?;
        let at = Self::review_at(req);
        let marks = review::marks_near(self, at);
        // a comment or highlight containing `at`; at the seam between a highlight and its
        // comment both qualify and mean the same thread
        let i = marks
            .iter()
            .position(|m| {
                matches!(m.kind, CriticKind::Comment | CriticKind::Highlight)
                    && m.range.start <= at
                    && at <= m.range.end
            })
            .ok_or(PlanError::NotApplicable("no comment here"))?;
        let text = self.text();
        let mut i = i;
        // a comment attached to a highlight: start from the highlight (both go)
        if marks[i].kind == CriticKind::Comment
            && marks[i].attached
            && i > 0
            && marks[i - 1].kind == CriticKind::Highlight
        {
            i -= 1;
        }
        let ch = remove_comment_change(text, &marks, i);
        single_change_plan(text, ch)
    }
}

/// The plan of one change, the caret after its result.
fn single_change_plan(text: &str, ch: Change) -> Result<Plan, PlanError> {
    let (patches, carets) = apply_in_order(text, std::slice::from_ref(&ch));
    let mut ed = Edits::new();
    for (r, t) in patches {
        ed.push(r, t);
    }
    let ps = ed.build().ok_or(PlanError::NotApplicable("overlap"))?;
    Ok(Plan::single(PlanStep::patches(ps), Some(Selection::caret(carets[0]))))
}

/// Where a list item's content starts: after its marker (`-`, `*`, `+`, `1.`, `1)`) and the
/// spaces after it (at most four), not after a task box; never past `limit`.
fn item_content_start(b: &[u8], marker: usize, limit: usize) -> usize {
    let mut i = marker;
    if matches!(b.get(i), Some(b'-' | b'*' | b'+')) {
        i += 1;
    } else {
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if matches!(b.get(i), Some(b'.' | b')')) {
            i += 1;
        }
    }
    let mut n = 0;
    while n < 4 && b.get(i) == Some(&b' ') {
        i += 1;
        n += 1;
    }
    i.min(limit)
}

/// Every node, depth-first.
fn flat<'a>(nodes: &'a [InlineNode], out: &mut Vec<&'a InlineNode>) {
    for n in nodes {
        out.push(n);
        flat(&n.children, out);
    }
}

/// The change resolving suggestion `marks[i]` (its attached comment goes with it).
fn resolve_change(text: &str, marks: &[ReviewMark], i: usize, how: Resolve) -> Change {
    let m = &marks[i];
    let raw = |r: &std::ops::Range<usize>| text[r.clone()].to_string();
    let keep = match (m.kind, how) {
        (CriticKind::Insert, Resolve::Accept) | (CriticKind::Delete, Resolve::Reject) => raw(&m.content),
        (CriticKind::Substitute, Resolve::Accept) => m.new.as_ref().map(raw).unwrap_or_default(),
        (CriticKind::Substitute, Resolve::Reject) => m.old.as_ref().map(raw).unwrap_or_default(),
        _ => String::new(),
    };
    let mut end = m.range.end;
    if let Some(c) = marks.get(i + 1)
        && c.kind == CriticKind::Comment
        && c.attached
    {
        end = c.range.end;
    }
    Change {
        range: m.range.start..end,
        insert: keep,
    }
}

/// The change removing comment or highlight `marks[i]`: a highlight keeps its text and takes
/// its attached comment with it; a comment goes away.
fn remove_comment_change(text: &str, marks: &[ReviewMark], i: usize) -> Change {
    let m = &marks[i];
    let mut end = m.range.end;
    let keep = if m.kind == CriticKind::Highlight {
        if let Some(c) = marks.get(i + 1)
            && c.kind == CriticKind::Comment
            && c.attached
        {
            end = c.range.end;
        }
        text[m.content.clone()].to_string()
    } else {
        String::new()
    };
    Change {
        range: m.range.start..end,
        insert: keep,
    }
}

/// Applies `changes` (disjoint, in document order) exactly as if they were applied one after
/// another, each with the whitespace rule of the module docs looking at the text as the
/// previous changes left it (review P2-4: Accept All equals accepting one by one). Changes
/// separated only by spaces or tabs form a run; each run becomes one patch over its window
/// (the run plus at most one space on each side). Returns the patches (original coordinates)
/// and, per change, the offset right after its result in the new text.
fn apply_in_order(text: &str, changes: &[Change]) -> (Vec<(std::ops::Range<usize>, String)>, Vec<usize>) {
    let b = text.as_bytes();
    let mut patches = Vec::new();
    let mut carets = Vec::with_capacity(changes.len());
    let mut delta: isize = 0;
    let mut i = 0;
    while i < changes.len() {
        // the run: changes whose gaps are only spaces or tabs
        let mut j = i + 1;
        while j < changes.len()
            && b[changes[j - 1].range.end..changes[j].range.start]
                .iter()
                .all(|c| matches!(c, b' ' | b'\t'))
        {
            j += 1;
        }
        let first = changes[i].range.start;
        let last = changes[j - 1].range.end;
        let ws = if first > 0 && b[first - 1] == b' ' {
            first - 1
        } else {
            first
        };
        let we = if b.get(last) == Some(&b' ') { last + 1 } else { last };
        let outside = if ws > 0 { Some(b[ws - 1]) } else { None };
        let new_ws = (ws as isize + delta) as usize;
        let mut out = String::new();
        let mut pos = ws;
        for c in &changes[i..j] {
            out.push_str(&text[pos..c.range.start]);
            let mut e = c.range.end;
            if c.insert.is_empty() {
                let before = out.as_bytes().last().copied().or(outside);
                let after = b.get(e).copied();
                let line_start = matches!(before, None | Some(b'\n' | b'\r'));
                let ends = matches!(after, None | Some(b'\n' | b'\r'))
                    || after.is_some_and(|c| matches!(c, b'.' | b',' | b';' | b':' | b'!' | b'?' | b')'));
                if after == Some(b' ') && (before == Some(b' ') || line_start) {
                    e += 1;
                } else if before == Some(b' ') && ends && !out.is_empty() {
                    out.pop();
                }
            }
            out.push_str(&c.insert);
            carets.push(new_ws + out.len());
            pos = e;
        }
        let we = we.max(pos);
        out.push_str(&text[pos..we]);
        delta += out.len() as isize - (we - ws) as isize;
        patches.push((ws..we, out));
        i = j;
    }
    (patches, carets)
}

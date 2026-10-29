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
        let mut taken = 0;
        let ch = resolve_change(text, &marks, i, how, &mut taken);
        let caret = ch.range.start + ch.insert.len();
        let mut ed = Edits::new();
        ed.push(ch.range, ch.insert);
        let ps = ed.build().ok_or(PlanError::NotApplicable("overlap"))?;
        Ok(Plan::single(PlanStep::patches(ps), Some(Selection::caret(caret))))
    }

    /// Accept or reject every suggestion (`clean`: also remove every comment and highlight,
    /// for a clean copy). The caret is mapped through the patches by the session.
    fn plan_resolve_all(&self, how: Resolve, clean: bool) -> Result<Plan, PlanError> {
        self.review_on()?;
        let marks = review::all_marks(self);
        let text = self.text();
        let mut ed = Edits::new();
        let mut taken = 0usize;
        let mut i = 0;
        let mut any = false;
        while i < marks.len() {
            let m = &marks[i];
            if m.range.start < taken {
                i += 1;
                continue;
            }
            if m.is_suggestion() {
                let ch = resolve_change(text, &marks, i, how, &mut taken);
                ed.push(ch.range, ch.insert);
                any = true;
            } else if clean {
                let ch = remove_comment_change(text, &marks, i, &mut taken);
                ed.push(ch.range, ch.insert);
                any = true;
            }
            i += 1;
        }
        if !any {
            return Err(PlanError::NotApplicable(if clean {
                "no comments or suggestions"
            } else {
                "no suggestions"
            }));
        }
        let ps = ed.build().ok_or(PlanError::NotApplicable("overlap"))?;
        Ok(Plan::single(PlanStep::patches(ps), None))
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
        let mut taken = 0;
        let mut ed = Edits::new();
        let mut i = i;
        // a comment attached to a highlight: start from the highlight (both go)
        if marks[i].kind == CriticKind::Comment
            && marks[i].attached
            && i > 0
            && marks[i - 1].kind == CriticKind::Highlight
        {
            i -= 1;
        }
        let ch = remove_comment_change(text, &marks, i, &mut taken);
        let caret = ch.range.start + ch.insert.len();
        ed.push(ch.range, ch.insert);
        let ps = ed.build().ok_or(PlanError::NotApplicable("overlap"))?;
        Ok(Plan::single(PlanStep::patches(ps), Some(Selection::caret(caret))))
    }
}

/// Every node, depth-first.
fn flat<'a>(nodes: &'a [InlineNode], out: &mut Vec<&'a InlineNode>) {
    for n in nodes {
        out.push(n);
        flat(&n.children, out);
    }
}

/// The change resolving suggestion `marks[i]` (its attached comment goes with it).
fn resolve_change(text: &str, marks: &[ReviewMark], i: usize, how: Resolve, taken: &mut usize) -> Change {
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
    replace_with_cleanup(text, m.range.start..end, keep, taken)
}

/// The change removing comment or highlight `marks[i]`: a highlight keeps its text and takes
/// its attached comment with it; a comment goes away.
fn remove_comment_change(text: &str, marks: &[ReviewMark], i: usize, taken: &mut usize) -> Change {
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
    replace_with_cleanup(text, m.range.start..end, keep, taken)
}

/// Replaces `range` with `keep`; when nothing is kept, one adjacent space goes too (see the
/// module docs). `taken`: the end of the previous change (a space it already took is not
/// taken again); updated.
fn replace_with_cleanup(text: &str, range: std::ops::Range<usize>, keep: String, taken: &mut usize) -> Change {
    let b = text.as_bytes();
    let (mut s, mut e) = (range.start, range.end);
    if keep.is_empty() {
        let before = if s > 0 { Some(b[s - 1]) } else { None };
        let after = b.get(e).copied();
        let line_start = matches!(before, None | Some(b'\n' | b'\r'));
        let ends = matches!(after, None | Some(b'\n' | b'\r'))
            || after.is_some_and(|c| matches!(c, b'.' | b',' | b';' | b':' | b'!' | b'?' | b')'));
        if after == Some(b' ') && (before == Some(b' ') || line_start) {
            e += 1;
        } else if before == Some(b' ') && ends && s > *taken {
            s -= 1;
        }
    }
    *taken = e;
    Change {
        range: s..e,
        insert: keep,
    }
}

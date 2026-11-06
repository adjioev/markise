//! Starting a section where the caret says (ADR 0026): as one undo step, at the document's level, with
//! every byte around it as it was. Only sections are known here; what one means is its site's.

use markise_doc::{DocPos, FormatDoc, HintSet, PatchSet, PlanStep, SectionAt, Selection, Span};
use markise_md::SECTION_METADATA;

use super::block_place::{InsertRefused, after_line, blank_line_at, blocks_of, line_for_block, separated};
use super::{EditError, EditorView, TxKind};
use crate::block_fields::{ids_in, new_anchor};

const INSERT_LABEL: &str = "Insert Section";

/// What a new section starts with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewSection {
    /// Its heading's text, one line.
    pub title: String,
    /// Its heading's id.
    pub id: String,
    /// Its metadata, in this order. None: it has no metadata block.
    pub metadata: Vec<(String, String)>,
}

/// One line, its runs of blanks as one: a title takes no line ending.
pub fn one_line(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn blank(t: &str) -> bool {
    t.chars().all(|c| matches!(c, ' ' | '\t' | '\r' | '\n'))
}

/// Whether the section `s` of the document has anything before `to` but blank lines and its metadata. A site
/// refuses a section with nothing in it or with only metadata.
pub(super) fn holds_more_than_metadata(doc: &dyn FormatDoc, s: SectionAt, to: usize) -> bool {
    let text = doc.text();
    let from = (doc.section_metadata(s.index))
        .filter(|m| m.closer.end <= to)
        .map_or(s.start, |m| m.closer.end);
    from < to && !blank(&text[from..to])
}

/// Whether the section that `line` ends has anything to end: what a boundary there would leave behind.
fn has_content_before(doc: &dyn FormatDoc, line: usize) -> bool {
    // on a boundary's own line that is the section before it
    let s = doc.section_at(line.saturating_sub(1));
    holds_more_than_metadata(doc, s, line)
}

/// What stands after `line`, blank lines passed over: whether anything does and whether a boundary is next.
fn after_it(doc: &dyn FormatDoc, line: usize) -> (bool, bool) {
    let text = doc.text();
    let mut at = line;
    while at < text.len() && blank_line_at(text, at) {
        at = after_line(text, at);
    }
    if at >= text.len() {
        return (false, false);
    }
    let boundary = doc.section_at(at).boundary.is_some_and(|b| b.start == at);
    (true, boundary)
}

impl EditorView {
    /// An id for a new section that is free on the page.
    pub fn free_section_id(&self) -> String {
        new_anchor(&ids_in(self.session.text()), None, &Default::default())
    }

    /// Starts a section after what the caret is in, or on the blank line it is on: never inside a block or a
    /// list. Its boundary stands before it when there is something to end; with nothing before it a boundary
    /// would make an empty section, or front matter of the first line, so it goes after it instead, if
    /// something other than a boundary follows. One undo step, the caret at the end of its title. Returns
    /// where the heading is.
    pub fn insert_section(&mut self, new: &NewSection, in_source: bool) -> Result<usize, InsertRefused> {
        if in_source {
            self.source_selection_to_session();
        }
        self.release();
        let doc = self.session.doc();
        let eol = doc.line_ending().as_str();
        let p = self.session.selection().primary();
        let at = if p.is_caret() {
            p.focus
        } else {
            DocPos::new(p.range().end)
        };
        let line = line_for_block(doc, at);
        let title = one_line(&new.title);
        let leading = has_content_before(doc, line);
        let (follows, boundary_next) = after_it(doc, line);
        let trailing = !leading && follows && !boundary_next;
        let holding = doc.section_at(line.saturating_sub(1));
        // metadata of its own in a section that has some already: a second block has no place there, and a
        // boundary would leave that one as it is
        let second_metadata = !leading && !new.metadata.is_empty() && doc.section_metadata(holding.index).is_some();
        // a boundary below it would leave what follows with only metadata
        let hollow_below = trailing
            && (doc.section_metadata(holding.index))
                .is_some_and(|m| m.opener.start >= line && blank(&doc.text()[m.closer.end..holding.end]));
        if second_metadata || hollow_below {
            if in_source {
                self.source_selection_from_session();
            }
            return Err(InsertRefused::WouldLeaveEmpty);
        }

        let mut body = String::new();
        if leading {
            body.push_str("---");
            body.push_str(eol);
            body.push_str(eol);
        }
        if !new.metadata.is_empty() {
            body.push_str(":::");
            body.push_str(SECTION_METADATA);
            body.push_str(eol);
            body.push_str(eol);
            for (key, value) in &new.metadata {
                body.push_str(&format!("{key}: {value}{eol}"));
            }
            body.push_str(eol);
            body.push_str(":::");
            body.push_str(eol);
            body.push_str(eol);
        }
        let heading_in_body = body.len();
        body.push_str(&format!("## {title} {{#{}}}", new.id));
        let (title_in_body, heading_end_in_body) = (heading_in_body + 3, body.len());
        if trailing {
            body.push_str(eol);
            body.push_str(eol);
            body.push_str("---");
        }

        let (insert, skip) = separated(doc.text(), line, &body, eol);
        let start = line + skip;
        let (heading, title_end, heading_end) = (
            start + heading_in_body,
            start + title_in_body + title.len(),
            start + heading_end_in_body,
        );
        let sections = doc.section_count() + usize::from(leading) + usize::from(trailing);
        let mut want = blocks_of(doc);
        if !new.metadata.is_empty() {
            want.push((3, SECTION_METADATA.to_string(), String::new()));
            want.sort();
        }
        let hints_after = (!doc.hints().is_empty()).then(HintSet::new);
        let done = self.session.transact(INSERT_LABEL, TxKind::Structural, |tx| {
            tx.step(PlanStep {
                patches: PatchSet::single_verbatim(line..line, &insert),
                hints_after,
            })?;
            let doc = tx.doc();
            let heading_stands = (doc.top_at(heading).and_then(|t| doc.top_span(t)))
                .is_some_and(|s| s.start == heading && s.end == heading_end);
            let opens = !leading || doc.section_at(start).boundary == Some(Span::new(start, start + 3));
            let own_id = doc.hidden_note(heading_end).is_some_and(|n| n.end == heading_end);
            if !heading_stands || !opens || !own_id || doc.section_count() != sections || blocks_of(doc) != want {
                return Err(EditError::Aborted(String::new()));
            }
            tx.set_selection(Selection::caret_at(DocPos::new(title_end)));
            Ok(())
        });
        if done.is_ok() {
            self.end_sticky();
        }
        self.sync();
        self.follow_block_modes();
        self.pin_caret();
        if in_source {
            self.source_selection_from_session();
        }
        match done {
            Ok(()) => Ok(heading),
            Err(EditError::Aborted(_)) => Err(InsertRefused::WouldNotStand),
            Err(e) => Err(InsertRefused::Edit(e)),
        }
    }
}

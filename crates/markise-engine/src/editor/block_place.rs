//! Where a block can be put and how it gets there (ADR 0022): what the document says about the
//! caret's place, for a host offering blocks to insert, and the insertion itself. The Markdown comes
//! from outside and is not trusted. No package and no block name is known here.

use std::fmt;

use markise_doc::{DocPos, FormatDoc, FormatOptions, HintSet, PatchSet, PlanStep, Selection};
use markise_layout::BlockViewMode;
use markise_md::Document;

use super::{EditError, EditorView, TxKind};
use crate::block_fields::{ids_in, new_anchor};

/// What undo calls an inserted block.
const INSERT_LABEL: &str = "Insert Block";

/// Why a block was not inserted. Nothing changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InsertRefused {
    /// The Markdown does not parse as exactly the block it says it is.
    NotItsBlock,
    /// In the document it would not stand as that block, or other blocks would pair differently.
    WouldNotStand,
    Edit(EditError),
}

impl fmt::Display for InsertRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InsertRefused::NotItsBlock => write!(f, "its Markdown is not exactly that block"),
            InsertRefused::WouldNotStand => write!(f, "it would change how the blocks around it pair"),
            InsertRefused::Edit(e) => write!(f, "{e}"),
        }
    }
}

/// Where the next line ending after `at` starts, or the end of the text. A line ends in LF, CRLF or a
/// lone CR, any of them in one file.
pub(super) fn line_end(text: &str, at: usize) -> usize {
    text.as_bytes()[at..]
        .iter()
        .position(|b| matches!(b, b'\r' | b'\n'))
        .map_or(text.len(), |i| at + i)
}

/// The start of the line after the one holding `at`, or the end of the text.
pub(super) fn after_line(text: &str, at: usize) -> usize {
    let end = line_end(text, at);
    match text.as_bytes().get(end) {
        Some(b'\r') if text.as_bytes().get(end + 1) == Some(&b'\n') => end + 2,
        Some(_) => end + 1,
        None => end,
    }
}

pub(super) fn line_start(text: &str, at: usize) -> usize {
    let bytes = text.as_bytes();
    // between the CR and the LF of one line ending: still the line before it
    let at = if at > 0 && bytes.get(at) == Some(&b'\n') && bytes[at - 1] == b'\r' {
        at - 1
    } else {
        at
    };
    bytes[..at]
        .iter()
        .rposition(|b| matches!(b, b'\r' | b'\n'))
        .map_or(0, |i| i + 1)
}

/// The line starting at `start` holds nothing but blanks.
pub(super) fn blank_line_at(text: &str, start: usize) -> bool {
    // what the parser calls blank: a no-break space is text
    text[start..line_end(text, start)].trim_matches([' ', '\t']).is_empty()
}

/// The text before `start`, which starts a line, without the line ending there: `None` with none.
fn before_line_ending(text: &str, start: usize) -> Option<&str> {
    let t = &text[..start];
    t.strip_suffix("\r\n")
        .or_else(|| t.strip_suffix('\n'))
        .or_else(|| t.strip_suffix('\r'))
}

/// The line before the one starting at `start` is blank, or there is none.
pub(super) fn blank_before(text: &str, start: usize) -> bool {
    let Some(before) = before_line_ending(text, start) else {
        return start == 0;
    };
    blank_line_at(text, line_start(before, before.len()))
}

/// The line a block goes to for the caret at `at`: the line itself when it is blank, in no block and
/// nothing else; otherwise the line after the outermost block or the top-level block holding it.
pub(super) fn line_for_block(doc: &dyn FormatDoc, at: DocPos) -> usize {
    let text = doc.text();
    // the virtual paragraph after a closer shares its end, and is outside the block
    let chain = doc.named_containers_at(at.offset);
    if let Some(c) = chain.iter().find(|c| !(at.virt > 0 && at.offset >= c.closer.end)) {
        return after_line(text, c.closer.end);
    }
    let top = doc.top_at(at.offset).and_then(|t| doc.top_span(t));
    match top {
        Some(s) if s.start <= at.offset && at.offset <= s.end => after_line(text, s.end),
        _ => {
            let start = line_start(text, at.offset);
            if blank_line_at(text, start) {
                start
            } else {
                after_line(text, at.offset)
            }
        }
    }
}

/// What is written at `line` so that `body` stands with a blank line on each side, reusing the blank
/// lines there are, and where in it the body starts. A file that ends without a line ending stays so.
pub(super) fn separated(text: &str, line: usize, body: &str, eol: &str) -> (String, usize) {
    let mut out = String::with_capacity(body.len() + 3 * eol.len());
    let complete = line == 0 || text[..line].ends_with(['\r', '\n']);
    if !complete {
        out.push_str(eol);
        out.push_str(eol);
    } else if !blank_before(text, line) {
        out.push_str(eol);
    }
    let start = out.len();
    out.push_str(body);
    if line < text.len() {
        out.push_str(eol);
        if !blank_line_at(text, line) {
            out.push_str(eol);
        }
    } else if text.is_empty() || text.ends_with(['\r', '\n']) {
        out.push_str(eol);
    }
    (out, start)
}

/// The trailing ids of a parsed text, as what `{#id}` reads there: by the one check every reader uses.
fn trailing_ids(doc: &dyn FormatDoc) -> Vec<(usize, usize)> {
    let text = doc.text();
    let mut out = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let next = after_line(text, start);
        // blanks after an id are no text of the line
        let end = start + text[start..next].trim_end_matches([' ', '\t', '\r', '\n']).len();
        if let Some(note) = doc.hidden_note(end)
            && note.end == end
            && note.start >= start
            && let Some(open) = text[note.start..end].find("{#")
        {
            out.push((note.start + open + 2, end - 1));
        }
        start = next;
    }
    out
}

/// `markdown` with every trailing id free in the document: an id still free stays, another gets the
/// field model's next one (ADR 0020).
fn with_free_ids(preset: &dyn FormatDoc, in_document: &str) -> String {
    let mut taken = ids_in(in_document);
    let text = preset.text();
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    for (start, end) in trailing_ids(preset) {
        let id = new_anchor(&taken, Some(&text[start..end]), &Default::default());
        out.push_str(&text[from..start]);
        out.push_str(&id);
        taken.insert(id);
        from = end;
    }
    out.push_str(&text[from..]);
    out
}

/// Every block of a document, as what must stay the same around an edit.
/// Of [`blocks_of`], the blocks whose openers stand in these bytes.
pub(super) fn blocks_in(doc: &dyn FormatDoc, range: &std::ops::Range<usize>) -> Vec<(usize, String, String)> {
    let Some(d) = doc.as_any().downcast_ref::<Document>() else {
        return Vec::new();
    };
    let text = doc.text();
    (d.directives().iter())
        .filter(|b| range.contains(&b.opener.start))
        .map(|b| {
            (
                b.fence_len,
                b.name.slice(text).to_string(),
                b.args.slice(text).to_string(),
            )
        })
        .collect()
}

pub(super) fn blocks_of(doc: &dyn FormatDoc) -> Vec<(usize, String, String)> {
    let Some(d) = doc.as_any().downcast_ref::<Document>() else {
        return Vec::new();
    };
    let text = doc.text();
    let mut out: Vec<_> = (d.directives().iter())
        .map(|b| {
            (
                b.fence_len,
                b.name.slice(text).to_string(),
                b.args.slice(text).to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

fn with_line_ending(text: &str, eol: &str) -> String {
    let unified = text.replace("\r\n", "\n").replace('\r', "\n");
    if eol == "\n" {
        unified
    } else {
        unified.replace('\n', eol)
    }
}

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
        blank_line_at(text, line_start(text, at.offset))
            && doc.leaf(at.offset).is_none_or(|l| l.span.is_empty())
            && doc.containers_at(at.offset).is_empty()
    }

    /// Inserts `markdown`, which has to be exactly a block named `block`, where the caret says, as one
    /// undo step, with new ids where they would repeat. It shows natively, with the caret in its first
    /// field. `in_source`: the caret is the source pane's. Returns where the block's opener is.
    pub fn insert_block(&mut self, block: &str, markdown: &str, in_source: bool) -> Result<usize, InsertRefused> {
        let body = markdown.trim_end();
        let format = self.session.format().clone();
        let preset = format.parse(body.to_string(), &FormatOptions::default());
        let whole = preset.named_containers_at(0).into_iter().next();
        if !whole.is_some_and(|c| c.opener.start == 0 && c.closer.end == body.len() && &*c.name == block) {
            return Err(InsertRefused::NotItsBlock);
        }
        if in_source {
            self.source_selection_to_session();
        }
        self.release();
        let doc = self.session.doc();
        let eol = doc.line_ending().as_str();
        let body = with_line_ending(&with_free_ids(preset.as_ref(), doc.text()), eol);
        let p = self.session.selection().primary();
        let at = if p.is_caret() {
            p.focus
        } else {
            DocPos::new(p.range().end)
        };
        let line = line_for_block(doc, at);
        let (insert, skip) = separated(doc.text(), line, &body, eol);
        let opener = line + skip;
        let mut want = blocks_of(doc);
        want.extend(blocks_of(preset.as_ref()));
        want.sort();
        let fields = self.place.fields.clone();
        let hints_after = (!doc.hints().is_empty()).then(HintSet::new);
        let body_len = body.len();
        let done = self.session.transact(INSERT_LABEL, TxKind::Structural, |tx| {
            tx.step(PlanStep {
                patches: PatchSet::single_verbatim(line..line, &insert),
                hints_after,
            })?;
            let doc = tx.doc();
            let stands = (doc.named_containers_at(opener).into_iter().next())
                .is_some_and(|c| c.opener.start == opener && c.closer.end == opener + body_len);
            if !stands || blocks_of(doc) != want {
                return Err(EditError::Aborted(String::new()));
            }
            let caret = first_field(doc, opener, fields.as_deref());
            tx.set_selection(Selection::caret_at(caret));
            Ok(())
        });
        if done.is_ok() {
            if self.place.mode != BlockViewMode::Native {
                self.add_own_mode(opener, BlockViewMode::Native);
            }
            // it may not fit its schema until its fields are filled: it stays native while it is edited
            // the mark goes over from a block edited before this one, which is left now
            self.end_sticky();
            if self.view_of_block_at(opener) {
                self.begin_sticky(opener);
            }
        }
        self.sync();
        if in_source {
            self.source_selection_from_session();
        }
        match done {
            Ok(()) => Ok(opener),
            Err(EditError::Aborted(_)) => Err(InsertRefused::WouldNotStand),
            Err(e) => Err(InsertRefused::Edit(e)),
        }
    }

    /// A package has a view for the block whose opener stands at `opener`.
    fn view_of_block_at(&self, opener: usize) -> bool {
        let Some(fields) = &self.place.fields else { return false };
        let doc = self.session.doc();
        let Some(c) = doc.named_containers_at(opener).pop() else {
            return false;
        };
        let Some(source) = doc.text().get(c.opener.start..c.closer.end) else {
            return false;
        };
        fields.fields_of(&c.name, &c.args, source, c.opener.start).is_some()
    }
}

/// Where the caret goes in the block at `opener`: the end of its first place to type, else the end of
/// the first thing in it, else the place inside an empty block.
fn first_field(doc: &dyn FormatDoc, opener: usize, fields: Option<&crate::block_view::PackageViews>) -> DocPos {
    let Some(c) = doc.named_containers_at(opener).pop() else {
        return DocPos::new(opener);
    };
    let source = &doc.text()[c.opener.start..c.closer.end];
    let slot = fields
        .and_then(|f| f.fields_of(&c.name, &c.args, source, c.opener.start))
        .and_then(|m| m.slots().into_iter().find(|s| s.is_plain()).map(|s| s.range().end));
    if let Some(end) = slot {
        return DocPos::new(c.opener.start + end);
    }
    let next = doc.top_at(c.opener.start).and_then(|t| doc.top_span(t + 1));
    match next.filter(|s| s.start < c.closer.start) {
        Some(s) => DocPos::new(doc.leaf(s.start).map_or(s.start, |l| l.content().end)),
        None => DocPos::with_virt(c.opener.end, 1),
    }
}

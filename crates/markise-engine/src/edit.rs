//! Applying a whole-text edit (source-code mode buffer) to a `markise-md` document as a set of
//! minimal patches.
//!
//! Only the regions that actually changed are replaced: a line diff finds the changed hunks,
//! a character diff inside each hunk narrows them down. Unchanged bytes — including line
//! terminators of untouched lines in a mixed-ending file — are never part of a replacement, so
//! they survive byte-identically (spec 01 R1.2, decision C23). Inserted text goes through
//! `Document::edit`, which converts *newly typed* line breaks to the document's line ending.

use std::time::{Duration, Instant};

use markise_md::Document;
use similar::{Algorithm, DiffOp, TextDiff};

/// One replacement in old-text byte coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Patch {
    pub start: usize,
    pub end: usize,
    pub insert: String,
    /// `insert` contains copied old text (wholesale fallback for huge hunks): apply it
    /// byte-for-byte so its existing line terminators survive. Otherwise `insert` is only
    /// newly typed text and its line breaks take the document's line ending.
    pub verbatim: bool,
}

/// Minimal patches turning `old` into `new` (non-overlapping, ascending).
pub fn patches(old: &str, new: &str) -> Vec<Patch> {
    if old == new {
        return Vec::new();
    }
    // common prefix / suffix first (cheap, and keeps the diffs small)
    let (ob, nb) = (old.as_bytes(), new.as_bytes());
    let mut p = ob.iter().zip(nb).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(p) || !new.is_char_boundary(p) {
        p -= 1;
    }
    let max_s = (ob.len() - p).min(nb.len() - p);
    let mut s = ob
        .iter()
        .rev()
        .zip(nb.iter().rev())
        .take(max_s)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(ob.len() - s) || !new.is_char_boundary(nb.len() - s) {
        s -= 1;
    }
    let (o, n) = (&old[p..ob.len() - s], &new[p..nb.len() - s]);
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(200);
    let lines = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .deadline(deadline)
        .diff_lines(o, n);
    let (mut oo, mut no) = (0usize, 0usize); // byte offsets in o / n
    for op in lines.ops() {
        let olen: usize = op.old_range().map(|i| lines.old_slice(i).map_or(0, |l| l.len())).sum();
        let nlen: usize = op.new_range().map(|i| lines.new_slice(i).map_or(0, |l| l.len())).sum();
        if !matches!(op, DiffOp::Equal { .. }) {
            refine(&o[oo..oo + olen], &n[no..no + nlen], p + oo, deadline, &mut out);
        }
        oo += olen;
        no += nlen;
    }
    out
}

/// Hunks larger than this (old + new bytes) are not refined by a character diff; they are
/// replaced wholesale, verbatim.
pub const REFINE_LIMIT: usize = 256 * 1024;

/// Character-level diff inside one changed hunk.
fn refine(o: &str, n: &str, base: usize, deadline: Instant, out: &mut Vec<Patch>) {
    // very large hunks (pastes) are replaced as a whole
    if o.len() + n.len() > REFINE_LIMIT || Instant::now() > deadline {
        out.push(Patch {
            start: base,
            end: base + o.len(),
            insert: n.to_string(),
            verbatim: true,
        });
        return;
    }
    let d = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .deadline(deadline)
        .diff_chars(o, n);
    let (mut oo, mut no) = (0usize, 0usize);
    let mut pending: Option<Patch> = None;
    for op in d.ops() {
        let olen: usize = op.old_range().map(|i| d.old_slice(i).map_or(0, |c| c.len())).sum();
        let nlen: usize = op.new_range().map(|i| d.new_slice(i).map_or(0, |c| c.len())).sum();
        if matches!(op, DiffOp::Equal { .. }) {
            if let Some(pp) = pending.take() {
                out.push(pp);
            }
        } else {
            let ins = &n[no..no + nlen];
            match &mut pending {
                Some(pp) if pp.end == base + oo => {
                    pp.end = base + oo + olen;
                    pp.insert.push_str(ins);
                }
                _ => {
                    if let Some(pp) = pending.take() {
                        out.push(pp);
                    }
                    pending = Some(Patch {
                        start: base + oo,
                        end: base + oo + olen,
                        insert: ins.to_string(),
                        verbatim: false,
                    });
                }
            }
        }
        oo += olen;
        no += nlen;
    }
    if let Some(pp) = pending {
        out.push(pp);
    }
}

/// Applies `new` (the full desired text) to `doc` as minimal edits; returns `true` if the text
/// changed. Patches are applied back to front so earlier offsets stay valid.
pub fn apply_text(doc: &mut Document, new: &str) -> bool {
    let ps = patches(doc.text(), new);
    for pch in ps.iter().rev() {
        if pch.verbatim {
            doc.edit_verbatim(pch.start..pch.end, &pch.insert);
        } else {
            doc.edit(pch.start..pch.end, &pch.insert);
        }
    }
    !ps.is_empty()
}

/// Format-neutral [`apply_text_verbatim`]: the minimal patches as one verbatim patch set
/// (inserts byte for byte, no line-ending conversion; the UI hands over the document text,
/// 99 C58): afterwards `doc.text() == new`. Returns `true` if the text changed.
pub fn apply_text_doc(doc: &mut dyn markise_doc::FormatDoc, new: &str) -> bool {
    let ps = patches(doc.text(), new);
    if ps.is_empty() {
        return false;
    }
    let set: Vec<markise_doc::Patch> = ps
        .into_iter()
        .map(|p| markise_doc::Patch::verbatim(p.start..p.end, p.insert))
        .collect();
    match markise_doc::PatchSet::new(set) {
        Ok(set) => {
            doc.apply(&set);
            true
        }
        Err(_) => false,
    }
}

/// Applies `new` to `doc` as minimal patches whose inserts are taken **byte for byte** (no
/// line-ending conversion of typed breaks): afterwards `doc.text() == new`. For an editor
/// whose text already is the document text (the UI converts a uniformly CRLF document's
/// `\n` back before handing text over), so preview, save and the editor never disagree
/// (99 C58). Returns `true` if the text changed.
pub fn apply_text_verbatim(doc: &mut Document, new: &str) -> bool {
    let ps = patches(doc.text(), new);
    for pch in ps.iter().rev() {
        doc.edit_verbatim(pch.start..pch.end, &pch.insert);
    }
    !ps.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_application_keeps_typed_breaks_as_typed() {
        let mut d = markise_md::parse("a\r\nmiddle\nz\r\n");
        assert!(apply_text_verbatim(&mut d, "a\r\nnew\nmiddle\nz\r\n"));
        assert_eq!(d.text(), "a\r\nnew\nmiddle\nz\r\n");
        assert_eq!(d.serialize(), "a\r\nnew\nmiddle\nz\r\n");
        assert!(!apply_text_verbatim(&mut d, "a\r\nnew\nmiddle\nz\r\n"));
    }

    #[test]
    fn untouched_mixed_line_endings_survive_c23() {
        // review item: edit `a` and `z`; `middle`'s LF must stay LF
        let mut d = markise_md::parse("a\r\nmiddle\nz\r\n");
        assert!(apply_text(&mut d, "A\r\nmiddle\nZ\r\n"));
        assert_eq!(d.serialize(), "A\r\nmiddle\nZ\r\n");
    }

    #[test]
    fn newly_typed_breaks_use_the_document_ending() {
        let mut d = markise_md::parse("a\r\nmiddle\nz\r\n");
        // the user typed a new line (TextEdit inserts `\n`) after `a`
        assert!(apply_text(&mut d, "a\r\nnew\nmiddle\nz\r\n"));
        // inserted "new\n" → the `\n` is new text: converted to the detected CRLF (C23)
        assert_eq!(d.serialize(), "a\r\nnew\r\nmiddle\nz\r\n");
        // a break typed at the end of a CRLF line, and one splitting a line
        let mut d = markise_md::parse("one\r\ntwo\r\n");
        assert!(apply_text(&mut d, "one\nX\r\ntw\no\r\n"));
        assert_eq!(d.serialize(), "one\r\nX\r\ntw\r\no\r\n");
    }

    #[test]
    fn many_scattered_edits_touch_only_their_bytes() {
        let old: String = (0..200)
            .map(|i| format!("line {i}{}", if i % 3 == 0 { "\n" } else { "\r\n" }))
            .collect();
        let new = old
            .replace("line 10\r\n", "LINE ten\r\n")
            .replace("line 150\n", "line 150 more\n");
        let mut d = markise_md::parse(&old);
        assert!(apply_text(&mut d, &new));
        assert_eq!(d.serialize(), new);
        let ps = patches(&old, &new);
        assert!(ps.iter().all(|p| p.end - p.start < 10), "{ps:?}");
    }

    #[test]
    fn huge_hunk_fallback_keeps_existing_terminators() {
        // second review, item 8: LF-dominant document, a 150k line ending in CRLF and another
        // 150k line; change the first char of both lines → one hunk > REFINE_LIMIT
        let l1 = "a".repeat(150_000);
        let l2 = "b".repeat(150_000);
        let old = format!("x\ny\nz\n{l1}\r\n{l2}\nend\n");
        let new = format!("x\ny\nz\nA{}\r\nB{}\nend\n", &l1[1..], &l2[1..]);
        let ps = patches(&old, &new);
        assert!(ps.iter().any(|p| p.verbatim), "the fallback path is exercised");
        let mut d = markise_md::parse(&old);
        assert!(apply_text(&mut d, &new));
        assert_eq!(d.serialize(), new, "the CRLF inside the replaced hunk survives");
    }

    #[test]
    fn multibyte_boundaries() {
        let mut d = markise_md::parse("héllo wörld\n");
        assert!(apply_text(&mut d, "héllo wérld\n"));
        assert_eq!(d.text(), "héllo wérld\n");
        assert!(apply_text(&mut d, "日本héllo wérld\n"));
        assert_eq!(d.text(), "日本héllo wérld\n");
        assert!(!apply_text(&mut d, "日本héllo wérld\n"));
    }
}

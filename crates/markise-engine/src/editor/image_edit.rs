//! Changing a Markdown image in place: its destination and its description, by what the rendered view
//! knows of it, its source. Nothing else of its bytes changes, a title and attributes included.

use std::fmt;
use std::ops::Range;

use markise_doc::{HintSet, Patch, PatchSet, PlanStep};

use super::block_place::blocks_of;
use super::{EditError, EditorView, TxKind};

const REPLACE_LABEL: &str = "Replace Image";
const EDIT_LABEL: &str = "Edit Image";

/// Why an image did not change. Nothing changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageRefused {
    /// The document is read-only.
    ReadOnly,
    /// That is no inline image with a destination of its own: a reference, or the text moved on.
    NoImage,
    /// With these bytes it would no longer be that image, or other blocks would pair differently.
    WouldNotStand,
    Edit(EditError),
}

impl fmt::Display for ImageRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageRefused::ReadOnly => write!(f, "the document is read-only"),
            ImageRefused::NoImage => write!(f, "it has no path of its own to change"),
            ImageRefused::WouldNotStand => write!(f, "that text would not stay one image"),
            ImageRefused::Edit(e) => write!(f, "{e}"),
        }
    }
}

/// The parts of `![description](destination "title")`, in the document's bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageParts {
    pub alt: Range<usize>,
    /// The destination as written, the angle brackets of `<a b.png>` included.
    pub dest: Range<usize>,
}

/// The parts of the inline image at `at`, which starts at its `!`. `None` for anything else, a
/// reference image too, whose path stands elsewhere.
pub fn image_parts_in(text: &str, at: &Range<usize>) -> Option<ImageParts> {
    let bytes = text.as_bytes();
    let end = at.end.min(bytes.len());
    if bytes.get(at.start..at.start + 2)? != b"![" {
        return None;
    }
    let alt_start = at.start + 2;
    let mut i = alt_start;
    let mut depth = 0usize;
    let alt_end = loop {
        match *bytes.get(i).filter(|_| i < end)? {
            b'\\' => i += 1,
            b'[' => depth += 1,
            b']' if depth == 0 => break i,
            b']' => depth -= 1,
            _ => {}
        }
        i += 1;
    };
    if bytes.get(alt_end + 1) != Some(&b'(') {
        return None;
    }
    let mut d = alt_end + 2;
    while d < end && matches!(bytes[d], b' ' | b'\t' | b'\r' | b'\n') {
        d += 1;
    }
    let dest_start = d;
    if bytes.get(d) == Some(&b'<') {
        d += 1;
        loop {
            match *bytes.get(d).filter(|_| d < end)? {
                b'\\' => d += 1,
                b'>' => break,
                b'\n' => return None,
                _ => {}
            }
            d += 1;
        }
        d += 1;
    } else {
        let mut parens = 0usize;
        while d < end {
            match bytes[d] {
                b'\\' => d += 1,
                b'(' => parens += 1,
                b')' if parens == 0 => break,
                b')' => parens -= 1,
                b' ' | b'\t' | b'\r' | b'\n' => break,
                _ => {}
            }
            d += 1;
        }
    }
    (d < end).then_some(ImageParts {
        alt: alt_start..alt_end,
        dest: dest_start..d,
    })
}

/// A destination as written for what a person typed: as it is when it reads as one, else in angle
/// brackets. `None` when no destination can hold it.
pub fn image_dest_for(typed: &str) -> Option<String> {
    let typed = typed.trim();
    if typed.contains(['\n', '\r']) {
        return None;
    }
    let inner = typed.strip_prefix('<').and_then(|t| t.strip_suffix('>'));
    if let Some(inner) = inner
        && !inner.contains(['<', '>'])
    {
        return Some(typed.to_string());
    }
    let mut parens = 0i32;
    let plain = !typed.is_empty()
        && !typed.starts_with('<')
        && typed.chars().all(|c| {
            match c {
                '(' => parens += 1,
                ')' => parens -= 1,
                _ => {}
            }
            parens >= 0 && !c.is_whitespace()
        })
        && parens == 0;
    if plain {
        return Some(typed.to_string());
    }
    (!typed.contains(['<', '>'])).then(|| format!("<{typed}>"))
}

impl EditorView {
    /// The parts of the image whose source is `at`.
    pub fn image_parts(&self, at: Range<usize>) -> Option<ImageParts> {
        image_parts_in(self.session.text(), &at)
    }

    /// Puts `dest`, as written, in the place of the image's destination: one undo step.
    pub fn replace_image_dest(&mut self, at: Range<usize>, dest: &str) -> Result<bool, ImageRefused> {
        self.change_image(at, None, dest, REPLACE_LABEL)
    }

    /// Changes the image's description and destination, both as written: one undo step.
    pub fn edit_image(&mut self, at: Range<usize>, alt: &str, dest: &str) -> Result<bool, ImageRefused> {
        if alt.contains(['\n', '\r']) {
            return Err(ImageRefused::WouldNotStand);
        }
        self.change_image(at, Some(alt), dest, EDIT_LABEL)
    }

    fn change_image(
        &mut self,
        at: Range<usize>,
        alt: Option<&str>,
        dest: &str,
        label: &'static str,
    ) -> Result<bool, ImageRefused> {
        if self.session.read_only().is_some() {
            return Err(ImageRefused::ReadOnly);
        }
        let parts = self.image_parts(at.clone()).ok_or(ImageRefused::NoImage)?;
        self.release();
        let doc = self.session.doc();
        let text = doc.text();
        let same_alt = alt.is_none_or(|a| a == &text[parts.alt.clone()]);
        if same_alt && dest == &text[parts.dest.clone()] {
            return Ok(false);
        }
        let mut patches = Vec::new();
        if let Some(a) = alt.filter(|_| !same_alt) {
            patches.push(Patch::verbatim(parts.alt.clone(), a.to_string()));
        }
        patches.push(Patch::verbatim(parts.dest.clone(), dest.to_string()));
        let patches = PatchSet::new(patches).map_err(|e| ImageRefused::Edit(EditError::Patch(e)))?;
        let want = blocks_of(doc);
        let hints_after = (!doc.hints().is_empty()).then(HintSet::new);
        // the same image, with these bytes
        let new_alt = alt.unwrap_or(&text[parts.alt.clone()]).to_string();
        let new_end = at.end as isize + dest.len() as isize - parts.dest.len() as isize + new_alt.len() as isize
            - parts.alt.len() as isize;
        let new_at = at.start..new_end.max(0) as usize;
        let start = at.start;
        let dest_owned = dest.to_string();
        let done = self.session.transact(label, TxKind::Structural, |tx| {
            tx.step(PlanStep { patches, hints_after })?;
            let doc = tx.doc();
            let text = doc.text();
            let stands = image_parts_in(text, &new_at).is_some_and(|p| {
                text[p.alt.clone()] == new_alt && text[p.dest] == dest_owned && p.alt.start == start + 2
            });
            if !stands || blocks_of(doc) != want {
                return Err(EditError::Aborted(String::new()));
            }
            Ok(())
        });
        self.sync();
        match done {
            Ok(()) => Ok(true),
            Err(EditError::Aborted(_)) => Err(ImageRefused::WouldNotStand),
            Err(e) => Err(ImageRefused::Edit(e)),
        }
    }
}

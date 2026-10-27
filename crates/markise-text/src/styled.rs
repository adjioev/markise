//! Styled paragraph input (ADR 0005 §4.1 `styled.rs`).

use std::ops::Range;

use crate::style::TextStyle;

/// Index into [`StyledText::styles`].
pub type StyleId = u16;

/// Paragraph base direction (UAX #9 P2/P3 or explicit).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BaseDir {
    /// First strong character decides (UAX #9 P2–P3); LTR when there is none.
    #[default]
    Auto,
    Ltr,
    Rtl,
}

/// An inline object (image, widget) occupying a box in the line, anchored at a byte offset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InlineObject {
    pub id: u64,
    /// Byte offset in the text before which the box is placed.
    pub index: usize,
    pub width: f32,
    pub height: f32,
}

/// Text plus style runs. Runs need not be sorted or complete: bytes not covered by a run use
/// style 0, later runs override earlier ones.
///
/// Malformed input is normalised, never rejected or panicked on: reversed run ranges are
/// swapped, ends clamped to the text and floored to `char` boundaries, runs naming a missing
/// style ignored; an empty `styles` list means one default [`TextStyle`]; non-finite or
/// non-positive sizes become 16, non-finite spacing 0, non-finite line heights "normal",
/// non-finite weight/stretch/angles their CSS initial values (see `matching::Request::normalized`);
/// inline objects get indices clamped to a char boundary and non-finite/negative sizes 0.
#[derive(Clone, Debug, PartialEq)]
pub struct StyledText {
    pub text: String,
    pub styles: Vec<TextStyle>,
    pub runs: Vec<(Range<usize>, StyleId)>,
    pub base_dir: BaseDir,
    pub inline_objects: Vec<InlineObject>,
}

impl StyledText {
    /// One style for the whole text.
    pub fn plain(text: impl Into<String>, style: TextStyle) -> Self {
        StyledText {
            text: text.into(),
            styles: vec![style],
            runs: Vec::new(),
            base_dir: BaseDir::Auto,
            inline_objects: Vec::new(),
        }
    }

    pub fn with_base_dir(mut self, dir: BaseDir) -> Self {
        self.base_dir = dir;
        self
    }

    /// Adds a style and applies it to `range`.
    pub fn push_run(&mut self, range: Range<usize>, style: TextStyle) -> StyleId {
        let id = self.styles.len() as StyleId;
        self.styles.push(style);
        self.runs.push((range, id));
        id
    }

    /// Per-byte style ids resolved into sorted, non-overlapping runs covering the text.
    pub(crate) fn resolved_runs(&self) -> Vec<(Range<usize>, StyleId)> {
        let len = self.text.len();
        let mut ids = vec![0 as StyleId; len];
        for (r, id) in &self.runs {
            // reversed ranges are swapped, ends clamped to the text and floored to char
            // boundaries (documented normalisation; never panics)
            let (a, b) = (r.start.min(r.end), r.start.max(r.end));
            let s = floor_char(&self.text, a.min(len));
            let e = floor_char(&self.text, b.min(len));
            if (*id as usize) < self.styles.len() {
                ids[s..e].iter_mut().for_each(|x| *x = *id);
            }
        }
        let mut out: Vec<(Range<usize>, StyleId)> = Vec::new();
        for (i, id) in ids.into_iter().enumerate() {
            match out.last_mut() {
                Some((r, last)) if *last == id => r.end = i + 1,
                _ => out.push((i..i + 1, id)),
            }
        }
        out
    }
}

pub(crate) fn floor_char(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn finite(v: f32, d: f32) -> f32 {
    if v.is_finite() { v } else { d }
}

impl TextStyle {
    pub(crate) fn sanitized(&self) -> TextStyle {
        let mut s = self.clone();
        s.size = if self.size.is_finite() && self.size > 0.0 {
            self.size
        } else {
            16.0
        };
        s.letter_spacing = finite(self.letter_spacing, 0.0);
        s.word_spacing = finite(self.word_spacing, 0.0);
        s.line_height = self.line_height.filter(|h| h.is_finite() && *h > 0.0);
        let r = crate::matching::Request {
            weight: self.weight,
            slope: self.slope,
            stretch: self.stretch,
        }
        .normalized();
        (s.weight, s.slope, s.stretch) = (r.weight, r.slope, r.stretch);
        s
    }

    fn is_sane(&self) -> bool {
        self.sanitized() == *self
    }
}

impl StyledText {
    /// Does layout need a normalised copy (see the type docs)?
    pub(crate) fn needs_normalizing(&self) -> bool {
        self.styles.is_empty()
            || !self.styles.iter().all(TextStyle::is_sane)
            || self.inline_objects.iter().any(|o| {
                o.index > self.text.len()
                    || !self.text.is_char_boundary(o.index.min(self.text.len()))
                    || !(o.width.is_finite() && o.width >= 0.0 && o.height.is_finite() && o.height >= 0.0)
            })
    }

    /// Normalised copy (see the type docs).
    pub(crate) fn normalized(&self) -> StyledText {
        let mut t = self.clone();
        if t.styles.is_empty() {
            t.styles.push(TextStyle::default());
        }
        t.styles = t.styles.iter().map(TextStyle::sanitized).collect();
        for o in &mut t.inline_objects {
            o.index = floor_char(&t.text, o.index.min(t.text.len()));
            let ok = |v: f32| if v.is_finite() && v >= 0.0 { v } else { 0.0 };
            o.width = ok(o.width);
            o.height = ok(o.height);
        }
        t
    }
}

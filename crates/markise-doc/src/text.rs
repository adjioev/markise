//! Byte spans and line endings (moved here from `markise-md`, which re-exports them).

use std::fmt;

/// A half-open byte range `start..end` into the document text.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    #[inline]
    pub const fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }
    #[inline]
    pub const fn len(&self) -> usize {
        self.end - self.start
    }
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.end == self.start
    }
    #[inline]
    pub fn slice<'a>(&self, text: &'a str) -> &'a str {
        &text[self.start..self.end]
    }
    #[inline]
    pub fn contains(&self, pos: usize) -> bool {
        self.start <= pos && pos < self.end
    }
    /// Moves both ends by `delta` bytes (span shifting after an edit before the span).
    #[inline]
    pub fn shift(&mut self, delta: isize) {
        self.start = (self.start as isize + delta) as usize;
        self.end = (self.end as isize + delta) as usize;
    }
    #[inline]
    pub fn range(&self) -> std::ops::Range<usize> {
        self.start..self.end
    }
}

impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

impl From<std::ops::Range<usize>> for Span {
    fn from(r: std::ops::Range<usize>) -> Self {
        Span::new(r.start, r.end)
    }
}

/// The line ending a document is written with (spec 01 R2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
    /// Classic Mac OS line ending; only chosen when a file contains CR but neither CRLF nor LF
    /// (decision C23 in `docs/spec/99-open-questions.md`).
    Cr,
}

impl LineEnding {
    pub const fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }

    /// Detection on open (R2.2): any CRLF → CRLF; else any LF → LF; else any lone CR → CR;
    /// else `None` (caller applies the configured default).
    pub fn detect(text: &str) -> Option<LineEnding> {
        let b = text.as_bytes();
        let mut saw_lf = false;
        let mut saw_cr = false;
        for i in memchr::memchr2_iter(b'\n', b'\r', b) {
            if b[i] == b'\r' {
                if b.get(i + 1) == Some(&b'\n') {
                    return Some(LineEnding::CrLf);
                }
                saw_cr = true;
            } else {
                saw_lf = true;
            }
        }
        if saw_lf {
            Some(LineEnding::Lf)
        } else if saw_cr {
            Some(LineEnding::Cr)
        } else {
            None
        }
    }
}

/// Replaces every CRLF, CR and LF in `s` with `eol`. Formats with further terminators (e.g.
/// Markdown's U+2424, R2.4) override [`crate::FormatDoc::canonicalize`].
pub fn normalize_breaks(s: &str, eol: LineEnding) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len() + 8);
    let mut last = 0;
    let mut it = memchr::memchr2_iter(b'\n', b'\r', b).peekable();
    while let Some(i) = it.next() {
        if i < last {
            continue; // the LF of a CRLF already handled
        }
        out.push_str(&s[last..i]);
        out.push_str(eol.as_str());
        last = if b[i] == b'\r' && b.get(i + 1) == Some(&b'\n') {
            i + 2
        } else {
            i + 1
        };
        while it.peek().is_some_and(|&j| j < last) {
            it.next();
        }
    }
    out.push_str(&s[last..]);
    out
}

/// `true` when `s` contains a CR or LF.
#[inline]
pub fn has_breaks(s: &str) -> bool {
    memchr::memchr2(b'\n', b'\r', s.as_bytes()).is_some()
}

/// `true` when `offset` starts a physical line (start of text, or after CR / LF).
#[inline]
pub fn is_line_start(text: &str, offset: usize) -> bool {
    let b = text.as_bytes();
    if offset == 0 {
        return true;
    }
    if offset > b.len() {
        return false;
    }
    match b[offset - 1] {
        b'\n' => true,
        // between the CR and LF of a CRLF is not a line start
        b'\r' => b.get(offset) != Some(&b'\n'),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_breaks_converts_every_ending() {
        assert_eq!(normalize_breaks("a\nb\r\nc\rd", LineEnding::CrLf), "a\r\nb\r\nc\r\nd");
        assert_eq!(normalize_breaks("a\r\n\r\n", LineEnding::Lf), "a\n\n");
        assert_eq!(normalize_breaks("\r\r\n\n", LineEnding::Cr), "\r\r\r");
        assert_eq!(normalize_breaks("plain", LineEnding::CrLf), "plain");
    }

    #[test]
    fn line_starts() {
        let t = "ab\r\ncd\ne";
        assert!(is_line_start(t, 0));
        assert!(!is_line_start(t, 1));
        assert!(is_line_start(t, 4));
        assert!(is_line_start(t, 7));
        assert!(!is_line_start(t, 3), "between CR and LF is not a line start");
    }
}

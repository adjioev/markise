//! Low-level text utilities: byte spans, line endings, line scanning, display width.

pub use markise_doc::text::{LineEnding, Span};

/// U+2424 SYMBOL FOR NEWLINE, accepted as a line separator when parsing (R2.4).
pub const SYMBOL_FOR_NEWLINE: &str = "\u{2424}";

/// One physical line: `start..content_end` is the content, `content_end..next` the terminator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub start: usize,
    pub content_end: usize,
    pub next: usize,
}

impl Line {
    #[inline]
    pub fn has_terminator(&self) -> bool {
        self.next > self.content_end
    }
}

/// Finds the end of the line starting at `start`. Returns `(content_end, next_line_start)`.
/// Terminators: CRLF, CR, LF, U+2424 (R2.4).
#[inline]
pub fn line_end(text: &str, start: usize) -> (usize, usize) {
    let b = text.as_bytes();
    let mut from = start;
    loop {
        match memchr::memchr3(b'\n', b'\r', 0xE2, &b[from..]) {
            None => return (b.len(), b.len()),
            Some(off) => {
                let i = from + off;
                match b[i] {
                    b'\n' => return (i, i + 1),
                    b'\r' => {
                        if b.get(i + 1) == Some(&b'\n') {
                            return (i, i + 2);
                        }
                        return (i, i + 1);
                    }
                    _ => {
                        if b.get(i + 1) == Some(&0x90) && b.get(i + 2) == Some(&0xA4) {
                            return (i, i + 3);
                        }
                        from = i + 1;
                    }
                }
            }
        }
    }
}

/// Iterator over the physical lines of `text[from..]`.
pub struct Lines<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Lines<'a> {
    pub fn new(text: &'a str, from: usize) -> Self {
        Lines { text, pos: from }
    }
}

impl Iterator for Lines<'_> {
    type Item = Line;
    #[inline]
    fn next(&mut self) -> Option<Line> {
        if self.pos >= self.text.len() {
            return None;
        }
        let start = self.pos;
        let (content_end, next) = line_end(self.text, start);
        self.pos = next;
        Some(Line {
            start,
            content_end,
            next,
        })
    }
}

/// Counts line terminators in `s` (CRLF counts once).
pub fn count_terminators(s: &str) -> usize {
    let mut n = 0;
    let mut pos = 0;
    while pos < s.len() {
        let (ce, next) = line_end(s, pos);
        if next > ce {
            n += 1;
        }
        pos = next;
    }
    n
}

/// Replaces every line terminator in `s` with `eol`.
pub fn normalize_eol(s: &str, eol: LineEnding) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pos = 0;
    while pos < s.len() {
        let (ce, next) = line_end(s, pos);
        out.push_str(&s[pos..ce]);
        if next > ce {
            out.push_str(eol.as_str());
        }
        pos = next;
    }
    out
}

/// Display width in terminal columns; East-Asian wide characters count as 2 (R3.2.3, R3.8.4).
pub fn display_width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
}

#[inline]
pub fn is_blank(s: &str) -> bool {
    s.bytes().all(|c| c == b' ' || c == b'\t')
}

/// Unicode punctuation (general category P*) — used for slugs (R7.1) and emphasis flanking.
pub fn is_punctuation(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_punctuation();
    }
    use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};
    c.general_category_group() == GeneralCategoryGroup::Punctuation
}

/// Unicode punctuation or symbol — CommonMark's notion of "punctuation character" for flanking.
pub fn is_punct_or_symbol(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_punctuation();
    }
    use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};
    matches!(
        c.general_category_group(),
        GeneralCategoryGroup::Punctuation | GeneralCategoryGroup::Symbol
    )
}

/// The content end of the line before the line starting at `ls` (`ls > 0`): `ls` minus its
/// preceding terminator (CRLF, LF, CR or U+2424).
pub(crate) fn prev_line_content_end(text: &str, ls: usize) -> usize {
    let before = &text[..ls];
    if before.ends_with("\r\n") {
        ls - 2
    } else if before.ends_with(SYMBOL_FOR_NEWLINE) {
        ls - SYMBOL_FOR_NEWLINE.len()
    } else {
        ls.saturating_sub(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_eol() {
        assert_eq!(LineEnding::detect("a\r\nb\n"), Some(LineEnding::CrLf));
        assert_eq!(LineEnding::detect("a\nb"), Some(LineEnding::Lf));
        assert_eq!(LineEnding::detect("a\rb"), Some(LineEnding::Cr));
        assert_eq!(LineEnding::detect("abc"), None);
    }

    #[test]
    fn line_terminators() {
        let t = "a\r\nb\rc\nd\u{2424}e";
        let lines: Vec<_> = Lines::new(t, 0).map(|l| &t[l.start..l.content_end]).collect();
        assert_eq!(lines, ["a", "b", "c", "d", "e"]);
        assert_eq!(count_terminators(t), 4);
        assert_eq!(normalize_eol(t, LineEnding::Lf), "a\nb\nc\nd\ne");
        // A non-newline U+24xx char must not split.
        let t = "a\u{2423}b";
        assert_eq!(Lines::new(t, 0).count(), 1);
    }

    #[test]
    fn widths() {
        assert_eq!(display_width("日本"), 4);
        assert_eq!(display_width("Hi"), 2);
    }
}

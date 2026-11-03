//! The first part of a large document for progressive opening (issue #126): a prefix of the
//! text, cut at the start of a top-level block, parsed on its own, that the editor shows while
//! the whole text is parsed.
//!
//! Block structure in Markdown is decided line by line from the top, so every top-level block
//! of a prefix that is followed by another block in that prefix comes out the same in the
//! whole text; only the last one can still grow (an open fence, a list, a paragraph that a
//! later `===` turns into a heading, a table). The cut is therefore made at the start of the
//! last top-level block of a parse of about `want` bytes. What does depend on text after the
//! cut is inline: reference definitions and footnotes defined later, the outline and `[toc]`
//! (and disclosures paired across the cut, which are separate blocks): the prefix renders them
//! as they would be without the rest, and the whole document replaces them.
//!
//! Front matter is the exception: its opening line is front matter only if a closing line
//! follows anywhere later. A document whose front matter does not close within the parsed
//! part is not opened progressively.

use crate::text::Lines;
use crate::{Document, ParseOptions};

/// The prefix of `text` to show first: `(cut, parsed text[..cut])`, with `cut` near `want`
/// at the start of a line and of a top-level block, or `None` when there is no useful cut
/// (the text is not much longer than `want`, has no line break after `want`, or its front
/// matter does not close in time).
pub fn progressive_prefix(text: &str, want: usize, opts: &ParseOptions) -> Option<(usize, Document)> {
    let bom = crate::bom_len(text);
    if want == 0 || text.len() <= want.saturating_mul(2) {
        return None;
    }
    // the end of the line holding `want`
    let lim = Lines::new(text, line_start(text, want)).next()?.next;
    if lim <= bom || lim >= text.len() {
        return None;
    }
    let doc = Document::parse(&text[..lim], opts.clone());
    if front_matter_cut(text, bom, &doc, opts) {
        return None;
    }
    // the start of the last top-level block (a block that may still grow), unless it starts
    // so early that most of the part would go (one very large block: cut it at the line end,
    // its first part is shown as such until the whole document replaces it)
    let n = doc.top_len();
    let last = (n >= 2)
        .then(|| doc.top(n - 1))
        .flatten()
        .map(|t| line_start(text, t.start()))
        .filter(|&s| s > bom && s >= want / 2 && doc.top(n - 2).is_some_and(|p| p.start() < s));
    match last {
        Some(cut) if cut < lim => Some((cut, Document::parse(&text[..cut], opts.clone()))),
        _ => Some((lim, doc)),
    }
}

/// The text starts with a front matter opener that the prefix `doc` did not take as front
/// matter (its closing line is after the prefix, or missing).
fn front_matter_cut(text: &str, bom: usize, doc: &Document, opts: &ParseOptions) -> bool {
    let Some(first) = Lines::new(text, bom).next() else {
        return false;
    };
    crate::scan::fm_open(&text[first.start..first.content_end], opts.remap_unicode_punctuation)
        && !doc
            .top(0)
            .is_some_and(|t| matches!(t.block.kind, crate::BlockKind::FrontMatter { .. }))
}

/// The start of the line holding byte `at` (line terminators: LF, CR, CRLF, U+2424).
fn line_start(text: &str, at: usize) -> usize {
    let b = text.as_bytes();
    let mut i = at.min(b.len());
    while i > 0 {
        if matches!(b[i - 1], b'\n' | b'\r') || (text.is_char_boundary(i) && text[..i].ends_with('\u{2424}')) {
            break;
        }
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> ParseOptions {
        ParseOptions::default()
    }

    /// Every top-level block of the prefix equals the whole document's block at that position.
    fn assert_stable(text: &str, cut: usize, prefix: &Document) {
        assert_eq!(prefix.text(), &text[..cut]);
        let full = Document::parse(text, opts());
        for (i, t) in prefix.tops().enumerate() {
            let f = full.top(i).expect("block in the whole document");
            assert_eq!(t.span(), f.span(), "top {i}");
            assert_eq!(t.to_abs(), f.to_abs(), "top {i}");
        }
    }

    #[test]
    fn cuts_at_the_start_of_the_last_block() {
        let mut text = String::new();
        for i in 0..200 {
            text.push_str(&format!("# H{i}\n\nPara {i} with *text*.\n\n- a\n- b\n\n"));
        }
        let (cut, doc) = progressive_prefix(&text, 1000, &opts()).unwrap();
        assert!((500..=1100).contains(&cut), "{cut}");
        assert!(text[..cut].ends_with("\n\n") || text[..cut].ends_with('\n'));
        assert_stable(&text, cut, &doc);
    }

    #[test]
    fn open_fence_or_list_at_the_cut_is_left_for_later() {
        // a fence spanning the wanted position: the cut is before it
        let mut text = "para\n\n".repeat(100);
        let fence_at = text.len();
        text.push_str("```\n");
        text.push_str(&"code\n\ncode\n".repeat(100));
        text.push_str("```\n\n");
        text.push_str(&"tail\n\n".repeat(400));
        let (cut, doc) = progressive_prefix(&text, fence_at + 50, &opts()).unwrap();
        assert_eq!(cut, fence_at);
        assert_stable(&text, cut, &doc);
        // a loose list spanning it
        let mut text = "para\n\n".repeat(100);
        let list_at = text.len();
        text.push_str(&"- item\n\n  more\n\n".repeat(100));
        text.push_str(&"tail\n\n".repeat(400));
        let (cut, doc) = progressive_prefix(&text, list_at + 50, &opts()).unwrap();
        assert_eq!(cut, list_at);
        assert_stable(&text, cut, &doc);
    }

    #[test]
    fn crlf_cr_and_bom() {
        for eol in ["\r\n", "\r", "\n"] {
            let mut text = String::from("\u{FEFF}");
            for i in 0..300 {
                text.push_str(&format!("Para {i}{eol}{eol}"));
            }
            let (cut, doc) = progressive_prefix(&text, 500, &opts()).unwrap();
            assert!(text.is_char_boundary(cut));
            assert!(
                !(text[..cut].ends_with('\r') && text[cut..].starts_with('\n')),
                "CRLF kept whole"
            );
            assert_stable(&text, cut, &doc);
            assert!(doc.has_bom());
        }
    }

    #[test]
    fn front_matter_longer_than_the_prefix_is_not_cut() {
        let mut text = String::from("---\n");
        for i in 0..200 {
            text.push_str(&format!("key{i}: value\n\n"));
        }
        text.push_str("---\n\n");
        text.push_str(&"para\n\n".repeat(1000));
        assert!(progressive_prefix(&text, 500, &opts()).is_none());
        // a short front matter is inside the prefix
        let text = format!("---\ntitle: x\n---\n\n{}", "para\n\n".repeat(1000));
        let (cut, doc) = progressive_prefix(&text, 500, &opts()).unwrap();
        assert!(matches!(
            doc.top(0).unwrap().block.kind,
            crate::BlockKind::FrontMatter { .. }
        ));
        assert_stable(&text, cut, &doc);
    }

    #[test]
    fn corpus_prefixes_parse_as_the_whole_document() {
        use markise_corpus::{Profile, generate_profile};
        for (seed, profile, crlf) in [
            (1, Profile::Mixed, false),
            (2, Profile::Mixed, true),
            (3, Profile::Tables, false),
            (4, Profile::Math, false),
            (5, Profile::Prose, false),
            (6, Profile::Log, true),
        ] {
            let text = generate_profile(96 << 10, seed, profile, crlf);
            for want in [3000, 10_000, 30_000] {
                let (cut, doc) = progressive_prefix(&text, want, &opts()).expect("a cut");
                assert_stable(&text, cut, &doc);
            }
        }
    }

    #[test]
    fn no_cut_without_room() {
        assert!(progressive_prefix(&"para\n\n".repeat(10), 50, &opts()).is_none());
        // one long line
        assert!(progressive_prefix(&"x".repeat(10_000), 100, &opts()).is_none());
    }

    #[test]
    fn one_large_block_is_cut_at_a_line_end() {
        let text = "- item\n".repeat(5000);
        let (cut, doc) = progressive_prefix(&text, 1000, &opts()).unwrap();
        assert!((1000..1010).contains(&cut), "{cut}");
        assert_eq!(doc.text(), &text[..cut]);
    }
}

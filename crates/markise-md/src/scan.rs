//! Line-level recognisers used by the block parser. All functions work on a byte slice of one
//! line (`b[p..end]`, terminator excluded) and return absolute positions.

use crate::block::{Align, MathDelim};
use crate::text::Span;
use crate::ticks::TickRuns;

#[inline]
pub(crate) fn is_ws(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

#[inline]
pub(crate) fn skip_ws(b: &[u8], mut p: usize, end: usize) -> usize {
    while p < end && is_ws(b[p]) {
        p += 1;
    }
    p
}

#[inline]
pub(crate) fn trim_end_ws(b: &[u8], start: usize, mut end: usize) -> usize {
    while end > start && is_ws(b[end - 1]) {
        end -= 1;
    }
    end
}

#[inline]
pub(crate) fn rest_blank(b: &[u8], p: usize, end: usize) -> bool {
    skip_ws(b, p, end) == end
}

/// ATX heading (R3.2.1). Returns `(level, text span, closed)`.
pub(crate) fn atx_heading(b: &[u8], p: usize, end: usize, strict: bool) -> Option<(u8, Span, bool)> {
    let mut q = p;
    while q < end && b[q] == b'#' {
        q += 1;
    }
    let level = q - p;
    if level == 0 || level > 6 {
        return None;
    }
    if q < end && !is_ws(b[q]) && strict {
        return None;
    }
    let start = skip_ws(b, q, end);
    let mut cend = trim_end_ws(b, start, end);
    let mut closed = false;
    // Closing sequence: trailing #'s preceded by whitespace (or the whole content).
    let mut h = cend;
    while h > start && b[h - 1] == b'#' {
        h -= 1;
    }
    if h < cend && (h == start || is_ws(b[h - 1])) {
        // Avoid treating an escaped `\#` as a closing sequence.
        if !(h > start && h >= 1 && b[h - 1] == b'\\') {
            closed = true;
            cend = trim_end_ws(b, start, h);
        }
    }
    Some((level as u8, Span::new(start, cend.max(start)), closed))
}

/// Opening code fence (R3.5.1). Returns `(char, len, info span)`.
pub(crate) fn fence_open(b: &[u8], p: usize, end: usize) -> Option<(u8, usize, Span)> {
    let ch = *b.get(p)?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let mut q = p;
    while q < end && b[q] == ch {
        q += 1;
    }
    let len = q - p;
    if len < 3 {
        return None;
    }
    let is = skip_ws(b, q, end);
    let ie = trim_end_ws(b, is, end);
    if ch == b'`' && b[is..ie].contains(&b'`') {
        return None;
    }
    Some((ch, len, Span::new(is, ie)))
}

/// Closing code fence: same char, at least `len` long, only whitespace after.
pub(crate) fn fence_close(b: &[u8], p: usize, end: usize, ch: u8, len: usize) -> Option<Span> {
    let mut q = p;
    while q < end && b[q] == ch {
        q += 1;
    }
    if q - p >= len && rest_blank(b, q, end) {
        Some(Span::new(p, q))
    } else {
        None
    }
}

/// Em/en dash run used as remapped `---` (R4.5.2, decision C26).
fn dash_lookalike_line(s: &str) -> bool {
    let t = s.trim_matches([' ', '\t']);
    !t.is_empty() && t.chars().all(|c| c == '—' || c == '–')
}

/// Thematic break (R3.9.1).
pub(crate) fn thematic_break(b: &[u8], p: usize, end: usize, remap: bool) -> bool {
    let Some(&ch) = b.get(p) else { return false };
    if ch == b'-' || ch == b'*' || ch == b'_' {
        let mut n = 0;
        for &c in &b[p..end] {
            if c == ch {
                n += 1;
            } else if !is_ws(c) {
                return false;
            }
        }
        return n >= 3;
    }
    remap && dash_lookalike_line(std::str::from_utf8(&b[p..end]).unwrap_or(""))
}

/// Setext underline (R3.2.2 / C16: any length ≥ 1). Returns the level.
pub(crate) fn setext_underline(b: &[u8], p: usize, end: usize) -> Option<u8> {
    let ch = *b.get(p)?;
    if ch != b'=' && ch != b'-' {
        return None;
    }
    let mut q = p;
    while q < end && b[q] == ch {
        q += 1;
    }
    if !rest_blank(b, q, end) {
        return None;
    }
    Some(if ch == b'=' { 1 } else { 2 })
}

/// How an HTML block ends (CommonMark kinds 1–6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HtmlEnd {
    Tag(&'static str),
    Comment,
    Pi,
    Decl,
    Cdata,
    Blank,
}

const RAW_TAGS: [&str; 4] = ["script", "pre", "style", "textarea"];

/// Block-level element names (R3.14.1), excluding the raw ones handled as kind 1.
const BLOCK_TAGS: &[&str] = &[
    "div",
    "p",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "table",
    "figure",
    "details",
    "left",
    "right",
    "center",
    "footer",
    "header",
    "nav",
    "ol",
    "ul",
    "article",
    "address",
    "dl",
    "applet",
    "map",
    "aside",
    "audio",
    "bdo",
    "blockquote",
    "form",
    "select",
    "dialog",
    "picture",
    "embed",
    "object",
    "iframe",
    "main",
    "section",
    "svg",
    "video",
    "template",
    "head",
    "canvas",
    "base",
    "datalist",
    "meta",
    "noscript",
    "title",
    "menu",
    "hr",
];

/// HTML block start (R3.14.1). Kind 7 of CommonMark is intentionally not supported (C27).
pub(crate) fn html_block_start(b: &[u8], p: usize, end: usize) -> Option<HtmlEnd> {
    if b.get(p) != Some(&b'<') {
        return None;
    }
    let s = &b[p..end];
    if s.starts_with(b"<!--") {
        return Some(HtmlEnd::Comment);
    }
    if s.starts_with(b"<?") {
        return Some(HtmlEnd::Pi);
    }
    if s.starts_with(b"<![CDATA[") {
        return Some(HtmlEnd::Cdata);
    }
    if s.len() > 2 && s[1] == b'!' && s[2].is_ascii_alphabetic() {
        return Some(HtmlEnd::Decl);
    }
    let (closing, name_start) = if s.get(1) == Some(&b'/') { (true, 2) } else { (false, 1) };
    let mut q = name_start;
    while q < s.len() && s[q].is_ascii_alphanumeric() {
        q += 1;
    }
    if q == name_start {
        return None;
    }
    let name = std::str::from_utf8(&s[name_start..q]).ok()?.to_ascii_lowercase();
    let after = s.get(q).copied();
    let delim_ok = |allow_slash: bool| match after {
        None => true,
        Some(c) => is_ws(c) || c == b'>' || (allow_slash && c == b'/' && s.get(q + 1) == Some(&b'>')),
    };
    if !closing {
        for t in RAW_TAGS {
            if name == t && delim_ok(false) {
                return Some(HtmlEnd::Tag(match t {
                    "script" => "</script>",
                    "pre" => "</pre>",
                    "style" => "</style>",
                    _ => "</textarea>",
                }));
            }
        }
    }
    if BLOCK_TAGS.contains(&name.as_str()) && delim_ok(true) {
        return Some(HtmlEnd::Blank);
    }
    None
}

/// Whether the HTML end condition is met on this line.
pub(crate) fn html_end_met(line: &[u8], end: HtmlEnd) -> bool {
    match end {
        HtmlEnd::Blank => false,
        HtmlEnd::Comment => memchr::memmem::find(line, b"-->").is_some(),
        HtmlEnd::Pi => memchr::memmem::find(line, b"?>").is_some(),
        HtmlEnd::Decl => line.contains(&b'>'),
        HtmlEnd::Cdata => memchr::memmem::find(line, b"]]>").is_some(),
        HtmlEnd::Tag(t) => {
            let lower = line.to_ascii_lowercase();
            memchr::memmem::find(&lower, t.as_bytes()).is_some()
        }
    }
}

/// Parsed list marker.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ListMarker {
    pub ordered: bool,
    /// Bullet char or ordered delimiter.
    pub ch: u8,
    pub number: u64,
    pub marker: Span,
}

pub(crate) fn list_marker(b: &[u8], p: usize, end: usize) -> Option<ListMarker> {
    let c = *b.get(p)?;
    if p >= end {
        return None;
    }
    if c == b'-' || c == b'+' || c == b'*' {
        if p + 1 < end && !is_ws(b[p + 1]) {
            return None;
        }
        return Some(ListMarker {
            ordered: false,
            ch: c,
            number: 0,
            marker: Span::new(p, p + 1),
        });
    }
    let mut q = p;
    while q < end && b[q].is_ascii_digit() && q - p < 10 {
        q += 1;
    }
    let digits = q - p;
    if digits == 0 || digits > 9 || q >= end {
        return None;
    }
    let d = b[q];
    if d != b'.' && d != b')' {
        return None;
    }
    if q + 1 < end && !is_ws(b[q + 1]) {
        return None;
    }
    let number = std::str::from_utf8(&b[p..q]).ok()?.parse().ok()?;
    Some(ListMarker {
        ordered: true,
        ch: d,
        number,
        marker: Span::new(p, q + 1),
    })
}

/// Task box right at `p`: `[ ]`, `[x]`, `[X]` followed by whitespace or end of line (R3.4.2).
pub(crate) fn task_box(b: &[u8], p: usize, end: usize) -> Option<(bool, Span)> {
    if p + 3 > end || b[p] != b'[' || b[p + 2] != b']' {
        return None;
    }
    let checked = match b[p + 1] {
        b' ' => false,
        b'x' | b'X' => true,
        _ => return None,
    };
    if p + 3 < end && !is_ws(b[p + 3]) {
        return None;
    }
    Some((checked, Span::new(p, p + 3)))
}

/// `[^label]:` at `p` (R3.12.1). Returns `(label span, position after the colon)`.
pub(crate) fn footnote_def_start(b: &[u8], p: usize, end: usize) -> Option<(Span, usize)> {
    if !b[p..end].starts_with(b"[^") {
        return None;
    }
    let ls = p + 2;
    let mut q = ls;
    while q < end && b[q] != b']' {
        if b[q] == b'[' || is_ws(b[q]) {
            return None;
        }
        if b[q] == b'\\' {
            q += 1;
        }
        q += 1;
    }
    if q >= end || q == ls || b.get(q + 1) != Some(&b':') {
        return None;
    }
    Some((Span::new(ls, q), q + 2))
}

/// `[toc]` and friends (R3.11.1); `remap` also accepts `【toc】` (R4.5.2).
pub(crate) fn is_toc_marker(s: &str, remap: bool) -> bool {
    let t = s.trim_matches([' ', '\t']);
    if t.eq_ignore_ascii_case("[toc]") || t.eq_ignore_ascii_case("[[_toc_]]") {
        return true;
    }
    if remap
        && let Some(inner) = t.strip_prefix('【').and_then(|r| r.strip_suffix('】'))
        && inner.eq_ignore_ascii_case("toc")
    {
        return true;
    }
    // kramdown: {:toc} or {: toc …}
    if let Some(inner) = t.strip_prefix("{:").and_then(|r| r.strip_suffix('}')) {
        let inner = inner.trim_start_matches([' ', '\t']);
        if inner.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("toc")) {
            let rest = &inner[3..];
            return rest.is_empty() || rest.starts_with([' ', '\t']);
        }
    }
    false
}

/// Math block opener on its own line (R3.7.1/R3.7.2).
pub(crate) fn math_opener(s: &str, latex: bool) -> Option<MathDelim> {
    let t = s.trim_matches([' ', '\t']);
    if let Some(rest) = t.strip_prefix("$$") {
        let rest = rest.trim_start_matches([' ', '\t']);
        if rest.is_empty() || (rest.starts_with('{') && rest.ends_with('}')) {
            return Some(MathDelim::Dollars);
        }
        return None;
    }
    if latex && t == "\\[" {
        return Some(MathDelim::Brackets);
    }
    None
}

pub(crate) fn math_closer(s: &str, delim: MathDelim) -> bool {
    let t = s.trim_matches([' ', '\t']);
    match delim {
        MathDelim::Dollars => {
            if let Some(rest) = t.strip_prefix("$$") {
                let rest = rest.trim_start_matches([' ', '\t']);
                rest.is_empty() || (rest.starts_with('{') && rest.ends_with('}'))
            } else {
                false
            }
        }
        MathDelim::Brackets => t == "\\]",
    }
}

/// Front-matter delimiter lines (R3.10.1). `remap` accepts dash look-alikes (C26).
pub(crate) fn fm_open(s: &str, remap: bool) -> bool {
    s == "---" || (remap && dash_lookalike_line(s))
}

pub(crate) fn fm_close(s: &str, remap: bool) -> bool {
    s == "---" || s == "..." || (remap && dash_lookalike_line(s))
}

/// Splits a table row into cell spans (absolute), honouring `\|`, code spans and inline math
/// (R3.8.1). Leading/trailing pipes are optional; cell spans are trimmed.
pub(crate) fn split_table_row(b: &[u8], start: usize, end: usize) -> Vec<Span> {
    let mut cells = Vec::new();
    let s = skip_ws(b, start, end);
    let e = trim_end_ws(b, s, end);
    let mut p = s;
    if p < e && b[p] == b'|' {
        p += 1;
    }
    let mut cell_start = p;
    let mut trailing_pipe = false;
    // memo of searches that already ran to the end of the row (keeps long rows linear)
    let mut dollar_fail = false;
    let mut ticks: Option<TickRuns> = None;
    while p < e {
        match b[p] {
            b'\\' => p += 2,
            b'`' => {
                let mut q = p;
                while q < e && b[q] == b'`' {
                    q += 1;
                }
                let n = q - p;
                p = match crate::ticks::find_run(b, s, q, e, n, 256, &mut ticks) {
                    Some(r) => r + n,
                    None => q,
                };
            }
            b'$' => {
                // inline math: skip to the next unescaped `$` when it looks like math
                let mut q = p + 1;
                let dbl = q < e && b[q] == b'$';
                if dbl {
                    q += 1;
                }
                if q < e && !is_ws(b[q]) && !dollar_fail {
                    let mut r = q;
                    let mut found = None;
                    while r < e {
                        if b[r] == b'\\' {
                            r += 2;
                            continue;
                        }
                        if b[r] == b'$' && !is_ws(b[r - 1]) {
                            if dbl {
                                if r + 1 < e && b[r + 1] == b'$' {
                                    found = Some(r + 2);
                                }
                            } else {
                                found = Some(r + 1);
                            }
                            break;
                        }
                        r += 1;
                    }
                    if r >= e {
                        dollar_fail = true;
                    }
                    p = found.unwrap_or(p + 1);
                } else if dollar_fail {
                    p += 1;
                } else {
                    p = q;
                }
            }
            b'|' => {
                cells.push(trim_span(b, cell_start, p));
                p += 1;
                cell_start = p;
                trailing_pipe = p >= e;
            }
            _ => p += 1,
        }
    }
    if !trailing_pipe {
        cells.push(trim_span(b, cell_start.min(e), e));
    }
    cells
}

fn trim_span(b: &[u8], s: usize, e: usize) -> Span {
    let s2 = skip_ws(b, s, e);
    Span::new(s2, trim_end_ws(b, s2, e))
}

/// Delimiter row (R3.8.1). Returns the alignments.
pub(crate) fn table_delim_row(b: &[u8], start: usize, end: usize) -> Option<Vec<Align>> {
    let s = skip_ws(b, start, end);
    let e = trim_end_ws(b, s, end);
    if s >= e {
        return None;
    }
    let has_pipe = b[s..e].contains(&b'|');
    let mut aligns = Vec::new();
    for cell in split_table_row(b, s, e) {
        let c = &b[cell.start..cell.end];
        if c.is_empty() {
            return None;
        }
        let left = c[0] == b':';
        let right = c.len() > 1 && c[c.len() - 1] == b':';
        let inner = &c[left as usize..c.len() - right as usize];
        if inner.is_empty() || !inner.iter().all(|&x| x == b'-') {
            return None;
        }
        aligns.push(match (left, right) {
            (true, true) => Align::Center,
            (true, false) => Align::Left,
            (false, true) => Align::Right,
            _ => Align::None,
        });
    }
    if aligns.is_empty() || (!has_pipe && aligns.len() == 1 && !b[s..e].contains(&b':')) {
        return None;
    }
    Some(aligns)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(s: &str) -> Vec<&str> {
        split_table_row(s.as_bytes(), 0, s.len())
            .into_iter()
            .map(|sp| &s[sp.start..sp.end])
            .collect()
    }

    #[test]
    fn table_cells() {
        assert_eq!(cells("| a | b |"), ["a", "b"]);
        assert_eq!(cells("a | b"), ["a", "b"]);
        assert_eq!(cells("| a\\|b | `x|y` | $p|q$ |"), ["a\\|b", "`x|y`", "$p|q$"]);
        assert_eq!(cells("| a |  |"), ["a", ""]);
        assert_eq!(cells("|a|"), ["a"]);
    }

    #[test]
    fn delim_rows() {
        let d = |s: &str| table_delim_row(s.as_bytes(), 0, s.len());
        assert_eq!(d("|---|:--|--:|:-:|").unwrap().len(), 4);
        assert!(d("---").is_none());
        assert!(d("| a |").is_none());
    }

    #[test]
    fn toc() {
        for t in ["[toc]", "[TOC]", " {:toc} ", "{: toc levels=2}", "[[_TOC_]]"] {
            assert!(is_toc_marker(t, false), "{t}");
        }
        assert!(!is_toc_marker("【toc】", false));
        assert!(is_toc_marker("【toc】", true));
        assert!(!is_toc_marker("[tocx]", false));
    }
}

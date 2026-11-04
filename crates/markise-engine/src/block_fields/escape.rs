//! Keeps markup typed at the start of a line as text, so a value cannot break the block's
//! structure (ADR 0020). The value holds what was typed (`# x`), the source the escaped form
//! (`\# x`). Decoding removes exactly what encoding adds, so unchanged lines round trip.

use markise_md::{BlockKind, Document, ParseOptions};

/// The line with its one block-level escape removed, if it has that form.
fn candidate(line: &str) -> Option<String> {
    if let Some(rest) = line.strip_prefix('\\') {
        return Some(rest.to_string());
    }
    let d = line.bytes().take_while(u8::is_ascii_digit).count();
    let rest = line[d..].strip_prefix('\\')?;
    (d > 0 && (rest.starts_with('.') || rest.starts_with(')'))).then(|| format!("{}{rest}", &line[..d]))
}

/// A line that alone is nothing but a paragraph, and stays one under any other line.
fn is_plain(line: &str, opts: &ParseOptions) -> bool {
    let t = line.trim_start();
    if t.starts_with(":::") || t.starts_with("$$") {
        return false;
    }
    // an underline or a table's delimiter row only shows next to another line
    if t.contains(['-', '=']) && t.chars().all(|c| matches!(c, '-' | '=' | ':' | '|' | ' ' | '\t')) {
        return false;
    }
    let tree = Document::parse(line, opts.clone()).tree();
    matches!(&tree[..], [b] if b.kind == BlockKind::Paragraph)
}

fn needs_escape(line: &str, opts: &ParseOptions) -> bool {
    if line.trim().is_empty() {
        return false;
    }
    !is_plain(line, opts) || candidate(line).is_some_and(|c| needs_escape(&c, opts))
}

/// The source form of a new line of a value.
pub(super) fn escape_line(line: &str, opts: &ParseOptions) -> String {
    let line = line.trim_start();
    if !needs_escape(line, opts) {
        return line.to_string();
    }
    let d = line.bytes().take_while(u8::is_ascii_digit).count();
    if d > 0 && line[d..].starts_with(['.', ')']) {
        format!("{}\\{}", &line[..d], &line[d..])
    } else {
        format!("\\{line}")
    }
}

/// The value form of a source line.
pub(super) fn unescape_line(line: &str, opts: &ParseOptions) -> String {
    match candidate(line) {
        Some(c) if needs_escape(&c, opts) => c,
        _ => line.to_string(),
    }
}

/// Where a trailing `{#id}` starts in `s`.
pub(super) fn anchor_start(s: &str) -> Option<usize> {
    let body = s.strip_suffix('}')?;
    let at = body.rfind("{#")?;
    let id = &body[at + 2..];
    (!id.is_empty() && !id.contains(char::is_whitespace) && !id.contains('{')).then_some(at)
}

/// Is this usable as the id inside `{#…}`?
pub(super) fn valid_anchor(id: &str) -> bool {
    anchor_start(&format!("{{#{id}}}")) == Some(0)
}

/// Where a line with a hidden `{#id}` keeps a typed one as text.
pub(super) fn escape_anchor_tail(s: &str) -> String {
    match anchor_start(s) {
        Some(at) => format!("{}\\{}", &s[..at], &s[at..]),
        None => s.to_string(),
    }
}

pub(super) fn unescape_anchor_tail(s: &str) -> String {
    match anchor_start(s) {
        Some(at) if s[..at].ends_with('\\') => format!("{}{}", &s[..at - 1], &s[at..]),
        _ => s.to_string(),
    }
}

/// The start of a trailing `#` run that is, or with its backslashes removed would be, a closing
/// sequence of an ATX heading.
fn closing_run(s: &str) -> Option<usize> {
    let t = s.trim_end();
    let at = t.trim_end_matches('#').len();
    let before = t[..at].trim_end_matches('\\');
    (at < t.len() && (before.is_empty() || before.ends_with([' ', '\t']))).then_some(at)
}

pub(super) fn escape_heading_tail(s: &str) -> String {
    match closing_run(s) {
        Some(at) => format!("{}\\{}", &s[..at], &s[at..]),
        None => s.to_string(),
    }
}

pub(super) fn unescape_heading_tail(s: &str) -> String {
    match closing_run(s) {
        Some(at) if s[..at].ends_with('\\') => format!("{}{}", &s[..at - 1], &s[at..]),
        _ => s.to_string(),
    }
}

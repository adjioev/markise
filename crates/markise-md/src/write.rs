//! Generators for blocks created fresh in the editor (R1.3: preferences apply only here).
//! All functions return Markdown with `\n` line terminators and no trailing terminator;
//! [`crate::ops`] converts to the document's line ending when inserting.

use crate::block::{Align, MathDelim};
use crate::options::{HeadingStyle, IndentSize, OrderedStyle, WriteOptions};
use crate::table::{self, PrettyStyle, TableModel};
use crate::text::display_width;

const ZWSP: char = '\u{200B}';

/// New heading (R3.2.3). Setext is never used inside list items.
pub fn heading(text: &str, level: u8, opts: &WriteOptions, in_list_item: bool) -> String {
    let level = level.clamp(1, 6);
    let hashes = "#".repeat(level as usize);
    let setext_ok = level <= 2 && !in_list_item;
    match opts.heading_style {
        HeadingStyle::Setext if setext_ok => {
            let ch = if level == 1 { "=" } else { "-" };
            format!("{text}\n{}", ch.repeat(3))
        }
        HeadingStyle::VariableSetext if setext_ok => {
            let ch = if level == 1 { "=" } else { "-" };
            format!("{text}\n{}", ch.repeat(display_width(text).max(1)))
        }
        HeadingStyle::AtxClosed => format!("{hashes} {text} {hashes}"),
        _ => format!("{hashes} {text}"),
    }
}

/// New paragraph; lines starting with 4+ spaces or a tab get a U+200B prefix so other parsers
/// do not read them as indented code (R3.1.4).
pub fn paragraph(text: &str) -> String {
    text.split('\n')
        .map(|l| {
            if l.starts_with("    ") || l.starts_with('\t') {
                format!("{ZWSP}{l}")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Longest run of `ch` at the start of any line of `content` (after ≤3 spaces).
fn longest_fence_line(content: &str, ch: char) -> usize {
    content
        .split('\n')
        .map(|l| {
            let t = l.trim_start_matches(' ');
            if l.len() - t.len() > 3 {
                0
            } else {
                t.chars().take_while(|&c| c == ch).count()
            }
        })
        .max()
        .unwrap_or(0)
}

/// Fence for a *new* code block (R3.5.3): backticks unless a content line starts with three
/// backticks, then tildes; lengthened past any fence-like content line.
pub fn new_fence(content: &str) -> (char, usize) {
    let ch = if longest_fence_line(content, '`') >= 3 {
        '~'
    } else {
        '`'
    };
    fence_for_pattern(content, ch, 3)
}

/// Fence for an existing block with pattern `(ch, len)` whose content changed (R3.5.3, T13).
pub fn fence_for_pattern(content: &str, ch: char, len: usize) -> (char, usize) {
    let longest = longest_fence_line(content, ch);
    if longest >= len { (ch, longest + 1) } else { (ch, len) }
}

/// New fenced code block.
pub fn fenced_code(info: &str, content: &str) -> String {
    let (ch, len) = new_fence(content);
    let fence: String = std::iter::repeat_n(ch, len).collect();
    if content.is_empty() {
        format!("{fence}{info}\n{fence}")
    } else {
        format!("{fence}{info}\n{content}\n{fence}")
    }
}

/// New display-math block.
pub fn math_block(content: &str, delim: MathDelim) -> String {
    match delim {
        MathDelim::Dollars => format!("$$\n{content}\n$$"),
        MathDelim::Brackets => format!("\\[\n{content}\n\\]"),
    }
}

/// New horizontal rule (R3.9.1).
pub fn thematic_break() -> String {
    "------".to_string()
}

/// New table of contents (R3.11.1).
pub fn toc() -> String {
    "[TOC]".to_string()
}

/// Front matter (R3.10.2/3): opening `---`, content lines starting with `---` get a U+200B
/// prefix, closing line as given (`---` or `...`).
pub fn front_matter(content: &str, closer: &str) -> String {
    let body = content
        .split('\n')
        .map(|l| {
            if l.starts_with("---") {
                format!("{ZWSP}{l}")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("---\n{body}\n{closer}")
}

/// New link reference definition (R3.13.1): `[label]: url` + TAB + `"title"`.
pub fn link_def(label: &str, url: &str, title: Option<&str>) -> String {
    match title {
        Some(t) => format!("[{label}]: {url}\t\"{t}\""),
        None => format!("[{label}]: {url}"),
    }
}

/// New footnote definition (R3.12.1).
pub fn footnote_def(label: &str, text: &str) -> String {
    format!("[^{label}]: {text}")
}

/// Quote prefix for new quotes (R3.3.2, §5 pretty indentation).
pub fn quote_prefix(opts: &WriteOptions) -> String {
    if opts.pretty_indentation {
        let n = opts.indent_cols().clamp(2, 5);
        format!(">{}", " ".repeat(n - 1))
    } else {
        "> ".to_string()
    }
}

/// New blockquote around already-serialized content.
pub fn blockquote(content: &str, opts: &WriteOptions) -> String {
    let prefix = quote_prefix(opts);
    content
        .split('\n')
        .map(|l| {
            if l.is_empty() {
                ">".to_string()
            } else {
                format!("{prefix}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// One new list item. `marker` is the bullet or `N.`/`N)`. Continuation lines are indented by
/// `max(marker width, indent size)` (R3.4.9).
pub fn list_item(marker: &str, task: Option<bool>, content: &str, opts: &WriteOptions) -> String {
    let width = marker.len() + 1;
    let (gap, cont) = match opts.indent_size {
        IndentSize::Tab => (" ".to_string(), "\t".to_string()),
        IndentSize::Spaces(n) => {
            let ind = width.max(n as usize);
            let gap = if opts.pretty_indentation {
                " ".repeat((ind - marker.len()).clamp(1, 4))
            } else {
                " ".to_string()
            };
            let cont = " ".repeat(if opts.pretty_indentation {
                marker.len() + gap.len()
            } else {
                ind
            });
            (gap, cont)
        }
    };
    let task = match task {
        Some(true) => "[x] ",
        Some(false) => "[ ] ",
        None => "",
    };
    let mut out = String::new();
    for (i, l) in content.split('\n').enumerate() {
        if i == 0 {
            out.push_str(marker);
            if !(l.is_empty() && task.is_empty()) {
                out.push_str(&gap);
            }
            out.push_str(task);
            out.push_str(l);
        } else {
            out.push('\n');
            if !l.is_empty() {
                out.push_str(&cont);
                out.push_str(l);
            }
        }
    }
    out
}

/// A new list. `items` are (task state, content).
pub fn list(ordered: bool, items: &[(Option<bool>, &str)], opts: &WriteOptions) -> String {
    items
        .iter()
        .enumerate()
        .map(|(i, (task, content))| {
            let marker = if ordered {
                let n = match opts.ordered_style {
                    OrderedStyle::Incrementing => i + 1,
                    OrderedStyle::Repeated => 1,
                };
                format!("{n}.")
            } else {
                opts.unordered_bullet.to_string()
            };
            list_item(&marker, if ordered { None } else { *task }, content, opts)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A new table (pretty-printed, R3.8.4).
pub fn new_table(aligns: &[Align], rows: &[Vec<String>]) -> String {
    let m = TableModel {
        aligns: aligns.to_vec(),
        rows: rows.to_vec(),
    };
    table::pretty_print(&m, &PrettyStyle::default()).join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vs() -> WriteOptions {
        WriteOptions {
            heading_style: HeadingStyle::VariableSetext,
            ..Default::default()
        }
    }

    #[test]
    fn t5_t6_variable_setext() {
        assert_eq!(heading("Hi", 1, &vs(), false), "Hi\n==");
        assert_eq!(heading("日本", 2, &vs(), false), "日本\n----");
        assert_eq!(heading("Hi", 3, &vs(), false), "### Hi");
        assert_eq!(heading("Hi", 1, &vs(), true), "# Hi");
    }

    #[test]
    fn heading_styles() {
        let mut o = WriteOptions::default();
        assert_eq!(heading("A", 2, &o, false), "## A");
        o.heading_style = HeadingStyle::AtxClosed;
        assert_eq!(heading("A", 3, &o, false), "### A ###");
        o.heading_style = HeadingStyle::Setext;
        assert_eq!(heading("Long text", 1, &o, false), "Long text\n===");
    }

    #[test]
    fn t12_t13_fences() {
        assert_eq!(new_fence("a\n```\nb"), ('~', 3));
        assert_eq!(new_fence("plain"), ('`', 3));
        assert_eq!(fence_for_pattern("x\n````\ny", '`', 3), ('`', 5));
        assert_eq!(fenced_code("rust", "fn x()"), "```rust\nfn x()\n```");
        assert_eq!(fenced_code("", ""), "```\n```");
    }

    #[test]
    fn t28_zwsp_paragraph() {
        assert_eq!(paragraph("    code-like\nnext"), "\u{200B}    code-like\nnext");
    }

    #[test]
    fn t23_front_matter() {
        assert_eq!(
            front_matter("title: x\n--- note", "---"),
            "---\ntitle: x\n\u{200B}--- note\n---"
        );
    }

    #[test]
    fn t37_new_link_def() {
        assert_eq!(link_def("id", "http://a.com", Some("T")), "[id]: http://a.com\t\"T\"");
    }

    #[test]
    fn lists() {
        let o = WriteOptions::default();
        assert_eq!(list(false, &[(None, "a"), (Some(true), "b")], &o), "- a\n- [x] b");
        assert_eq!(list(true, &[(None, "a"), (None, "b\nc")], &o), "1. a\n2. b\n   c");
        assert_eq!(blockquote("a\n\nb", &o), "> a\n>\n> b");
    }
}

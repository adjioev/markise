//! GFM tables: cell access and pretty-printing of new/edited tables (R3.8).

use crate::block::{Align, Block, BlockKind};
use crate::scan;
use crate::text::{Span, display_width};
use crate::ticks::TickRuns;

/// Cell spans of a table block: `rows[0]` is the header, the delimiter row is skipped.
pub fn cell_spans(text: &str, table: &Block) -> Vec<Vec<Span>> {
    let b = text.as_bytes();
    table
        .lines
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 1)
        .map(|(_, l)| scan::split_table_row(b, l.start, l.end))
        .collect()
}

/// Cell spans of one table row line (one entry of a table block's `lines`, not the delimiter
/// row). Lets callers address a few rows of a large table without splitting all of them.
pub fn row_cells(text: &str, line: Span) -> Vec<Span> {
    scan::split_table_row(text.as_bytes(), line.start, line.end)
}

/// A table as plain data (raw Markdown per cell).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TableModel {
    pub aligns: Vec<Align>,
    /// `rows[0]` is the header row.
    pub rows: Vec<Vec<String>>,
}

impl TableModel {
    pub fn from_block(text: &str, table: &Block) -> Self {
        let aligns = match &table.kind {
            BlockKind::Table { aligns } => aligns.clone(),
            _ => Vec::new(),
        };
        let rows = cell_spans(text, table)
            .into_iter()
            .map(|r| r.into_iter().map(|s| s.slice(text).to_string()).collect())
            .collect();
        TableModel { aligns, rows }
    }

    pub fn columns(&self) -> usize {
        self.aligns.len()
    }
}

/// Escapes unescaped `|` outside code spans and inline math; removes newlines (R3.8.4).
pub fn escape_cell(s: &str) -> String {
    let s: String = s.replace("\r\n", " ").replace(['\n', '\r'], " ");
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len() + 4);
    let mut i = 0;
    let mut last = 0;
    let mut ticks: Option<TickRuns> = None;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'`' => {
                let mut q = i;
                while q < b.len() && b[q] == b'`' {
                    q += 1;
                }
                let n = q - i;
                i = match crate::ticks::find_run(b, 0, q, b.len(), n, 256, &mut ticks) {
                    Some(r) => r + n,
                    None => q,
                };
            }
            b'$' => {
                let close = s[i + 1..].find('$').map(|p| i + 1 + p);
                match close {
                    Some(c) if c > i + 1 && !matches!(b[i + 1], b' ' | b'\t') => i = c + 1,
                    _ => i += 1,
                }
            }
            b'|' => {
                out.push_str(&s[last..i]);
                out.push_str("\\|");
                i += 1;
                last = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&s[last.min(s.len())..]);
    out
}

/// Formatting choices carried over from an existing table (R3.8.4).
#[derive(Clone, Debug, Default)]
pub struct PrettyStyle {
    /// Omit outer pipes (only honoured with more than one column).
    pub no_outer_pipes: bool,
}

const MIN_WIDTH: usize = 4;
const MAX_WIDTH: usize = 60;

fn pad(cell: &str, width: usize, align: Align) -> String {
    let w = display_width(cell);
    let extra = width.saturating_sub(w);
    match align {
        Align::Right => format!("{}{}", " ".repeat(extra), cell),
        Align::Center => {
            let l = extra / 2;
            format!("{}{}{}", " ".repeat(l), cell, " ".repeat(extra - l))
        }
        _ => format!("{}{}", cell, " ".repeat(extra)),
    }
}

fn delim(width: usize, align: Align) -> String {
    let w = width.max(3);
    match align {
        Align::Left => format!(":{}", "-".repeat(w - 1)),
        Align::Right => format!("{}:", "-".repeat(w - 1)),
        Align::Center => format!(":{}:", "-".repeat(w.saturating_sub(2).max(1))),
        Align::None => "-".repeat(w),
    }
}

/// Column widths for pretty-printing (start at 4, max with cell widths, cap at 60).
pub fn column_widths(model: &TableModel) -> Vec<usize> {
    let mut widths = vec![MIN_WIDTH; model.columns()];
    for row in &model.rows {
        for (i, c) in row.iter().enumerate().take(widths.len()) {
            widths[i] = widths[i].max(display_width(c).min(MAX_WIDTH));
        }
    }
    widths
}

fn join_row(cells: Vec<String>, style: &PrettyStyle, ncols: usize) -> String {
    let line = if style.no_outer_pipes && ncols > 1 {
        cells.join(" | ")
    } else {
        let mut s = String::from("|");
        for c in cells {
            s.push(' ');
            s.push_str(&c);
            s.push_str(" |");
        }
        s
    };
    line.trim_end().to_string()
}

/// Pretty-prints a table (R3.8.4). Cells are escaped with [`escape_cell`]. Returns lines
/// without terminators.
pub fn pretty_print(model: &TableModel, style: &PrettyStyle) -> Vec<String> {
    let n = model.columns();
    let escaped = TableModel {
        aligns: model.aligns.clone(),
        rows: model
            .rows
            .iter()
            .map(|r| {
                (0..n)
                    .map(|i| escape_cell(r.get(i).map(String::as_str).unwrap_or("")))
                    .collect()
            })
            .collect(),
    };
    let widths = column_widths(&escaped);
    let mut out = Vec::with_capacity(escaped.rows.len() + 1);
    for (ri, row) in escaped.rows.iter().enumerate() {
        let cells = (0..n).map(|i| pad(&row[i], widths[i], escaped.aligns[i])).collect();
        out.push(join_row(cells, style, n));
        if ri == 0 {
            let d = (0..n).map(|i| delim(widths[i], escaped.aligns[i])).collect();
            out.push(join_row(d, style, n));
        }
    }
    out
}

/// Renders a single row in a "minimal" style (one space margins), used when editing a table
/// that is not pretty-printed (A8).
pub fn plain_row(cells: &[String], style: &PrettyStyle) -> String {
    let cells: Vec<String> = cells.iter().map(|c| escape_cell(c)).collect();
    let n = cells.len();
    join_row(cells, style, n)
}

/// `true` when every row of the table already has the pretty-printed layout (A8).
pub fn is_pretty(text: &str, table: &Block) -> bool {
    let model = TableModel::from_block(text, table);
    let style = PrettyStyle {
        no_outer_pipes: !has_outer_pipes(text, table),
    };
    let lines = pretty_print(&model, &style);
    lines.len() == table.lines.len() && lines.iter().zip(&table.lines).all(|(a, l)| a == l.slice(text))
}

pub fn has_outer_pipes(text: &str, table: &Block) -> bool {
    table
        .lines
        .first()
        .is_some_and(|l| l.slice(text).trim_start().starts_with('|'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t26_pretty() {
        let m = TableModel {
            aligns: vec![Align::Left, Align::Right],
            rows: vec![vec!["a".into(), "b".into()], vec!["xx".into(), "yy".into()]],
        };
        let lines = pretty_print(&m, &PrettyStyle::default());
        assert_eq!(lines, ["| a    |    b |", "| :--- | ---: |", "| xx   |   yy |"]);
    }

    #[test]
    fn escape_cell_multibyte_after_dollar() {
        // regression: slicing inside a multibyte char after `$`
        assert_eq!(escape_cell("$é$"), "$é$");
        assert_eq!(escape_cell("$é|"), "$é\\|");
        assert_eq!(escape_cell("é|`ü|`|"), "é\\|`ü|`\\|");
    }

    #[test]
    fn t27_escape() {
        assert_eq!(escape_cell("a|b"), "a\\|b");
        assert_eq!(escape_cell("`a|b`"), "`a|b`");
        assert_eq!(escape_cell("a\\|b"), "a\\|b");
        assert_eq!(escape_cell("$a|b$"), "$a|b$");
    }

    #[test]
    fn wide_and_center() {
        let m = TableModel {
            aligns: vec![Align::Center, Align::None],
            rows: vec![vec!["日本".into(), "h".into()], vec!["x".into(), "".into()]],
        };
        let lines = pretty_print(&m, &PrettyStyle { no_outer_pipes: true });
        assert_eq!(lines, ["日本 | h", ":--: | ----", " x   |"]);
    }
}

//! Explicit table operations (spec 02 §6): one transaction, keeping container prefixes.
use crate::{
    Align, Document,
    table::{self, PrettyStyle, TableModel},
};
use markise_doc::{
    ColumnAlign, FormatDoc, LeafKind, PatchSet, Plan, PlanError, PlanRequest, PlanStep, Selection, TableView,
};

impl Document {
    fn table_context(&self, req: &PlanRequest) -> Option<(TableView, usize, usize)> {
        let at = req.selection.primary().focus.offset;
        let leaf = self.leaf(at)?;
        let t = self.table(leaf.span.start)?;
        let line = t.rows.iter().rposition(|l| l.start <= at).unwrap_or(0);
        let row = line.saturating_sub(1);
        let cells = self.table_row_cells(t.rows[if line == 1 { 0 } else { line }]);
        let col = cells
            .iter()
            .rposition(|c| c.start <= at)
            .unwrap_or(0)
            .min(t.aligns.len().checked_sub(1)?);
        Some((t, row, col))
    }
    pub(crate) fn can_table_op(&self, id: &str, req: &PlanRequest) -> bool {
        if id == "table.insert" {
            return self.insert_table_refusal(req).is_none();
        }
        let at = req.selection.primary().focus.offset;
        let Some((b, origin)) = self.leaf_rel(at) else {
            return false;
        };
        let crate::BlockKind::Table { aligns } = &b.kind else {
            return false;
        };
        let line = b.lines.partition_point(|l| l.start + origin <= at).saturating_sub(1);
        let row = line.saturating_sub(1);
        let l = b.lines[if line == 1 { 0 } else { line }];
        let cells = table::row_cells(self.text(), markise_doc::Span::new(l.start + origin, l.end + origin));
        let col = cells
            .partition_point(|c| c.start <= at)
            .saturating_sub(1)
            .min(aligns.len().saturating_sub(1));
        match id {
            "table.deleteRow" => row > 0 && b.lines.len() > 3,
            "table.deleteColumn" => aligns.len() > 1,
            "table.moveRowUp" => row > 1,
            "table.moveRowDown" => row > 0 && row + 2 < b.lines.len(),
            "table.moveColumnLeft" => col > 0,
            "table.moveColumnRight" => col + 1 < aligns.len(),
            "table.addRowAbove"
            | "table.addRowBelow"
            | "table.addColumnBefore"
            | "table.addColumnAfter"
            | "table.alignLeft"
            | "table.alignCenter"
            | "table.alignRight" => true,
            _ => false,
        }
    }
    pub(crate) fn plan_table_op(&self, id: &str, req: &PlanRequest) -> Result<Plan, PlanError> {
        if id == "table.insert" {
            return self.plan_insert_table(req);
        }
        if !self.can_table_op(id, req) {
            return Err(PlanError::NotApplicable("table boundary"));
        }
        let (t, mut row, mut col) = self.table_context(req).unwrap();
        let n = t.aligns.len();
        let mut m = TableModel {
            aligns: t
                .aligns
                .iter()
                .map(|a| match a {
                    ColumnAlign::None => Align::None,
                    ColumnAlign::Left => Align::Left,
                    ColumnAlign::Center => Align::Center,
                    ColumnAlign::Right => Align::Right,
                })
                .collect(),
            rows: t
                .rows
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != 1)
                .map(|(_, l)| {
                    let mut r: Vec<String> = self
                        .table_row_cells(*l)
                        .iter()
                        .take(n)
                        .map(|s| s.slice(self.text()).to_owned())
                        .collect();
                    r.resize(n, String::new());
                    r
                })
                .collect(),
        };
        match id {
            "table.addRowAbove" | "table.addRowBelow" => {
                row = if id.ends_with("Below") { row + 1 } else { row.max(1) };
                m.rows.insert(row, vec![String::new(); n]);
            }
            "table.deleteRow" => {
                m.rows.remove(row);
                row = row.min(m.rows.len() - 1);
            }
            "table.moveRowUp" | "table.moveRowDown" => {
                let to = if id.ends_with("Up") { row - 1 } else { row + 1 };
                m.rows.swap(row, to);
                row = to;
            }
            "table.addColumnBefore" | "table.addColumnAfter" => {
                if id.ends_with("After") {
                    col += 1;
                }
                m.aligns.insert(col, Align::None);
                for r in &mut m.rows {
                    r.insert(col, String::new());
                }
            }
            "table.deleteColumn" => {
                m.aligns.remove(col);
                for r in &mut m.rows {
                    r.remove(col);
                }
                col = col.min(m.aligns.len() - 1);
            }
            "table.moveColumnLeft" | "table.moveColumnRight" => {
                let to = if id.ends_with("Left") { col - 1 } else { col + 1 };
                m.aligns.swap(col, to);
                for r in &mut m.rows {
                    r.swap(col, to);
                }
                col = to;
            }
            "table.alignLeft" => m.aligns[col] = Align::Left,
            "table.alignCenter" => m.aligns[col] = Align::Center,
            "table.alignRight" => m.aligns[col] = Align::Right,
            _ => return Err(PlanError::Unsupported),
        }
        let lines = table::pretty_print(&m, &PrettyStyle::default());
        let first = t.rows[0].start;
        let last = t.rows.last().unwrap();
        let line_start = self.text()[..last.start].rfind('\n').map_or(0, |i| i + 1);
        let continuation = &self.text()[line_start..last.start];
        let mut text = String::new();
        let mut caret = first;
        for (i, l) in lines.iter().enumerate() {
            if i > 0 {
                text.push_str(self.line_ending().as_str());
                text.push_str(continuation);
            }
            if i == if row == 0 { 0 } else { row + 1 } {
                let cells = table::row_cells(l, markise_doc::Span::new(0, l.len()));
                caret = first + text.len() + cells[col].start;
            }
            text.push_str(l);
        }
        Ok(Plan::single(
            PlanStep::patches(PatchSet::single_verbatim(first..last.end, text)),
            Some(Selection::caret(caret)),
        ))
    }
}

/// Default size of Insert Table (spec 99 C2): columns, body rows.
pub const INSERT_TABLE_DEFAULT: (usize, usize) = (3, 3);
/// Largest table Insert Table makes: columns, body rows.
pub const INSERT_TABLE_MAX: (usize, usize) = (64, 1000);

impl Document {
    /// Insert Table (spec 02 §6, issue #164): a new table of `columns` × `rows` empty body
    /// cells (params; defaults [`INSERT_TABLE_DEFAULT`]) plus an empty header row, as a new
    /// block after the caret's block (on a blank line: there), in the caret's containers;
    /// blank lines keep it apart from its neighbours. The caret goes into the first header
    /// cell. One patch, so one undo step.
    /// Why Insert Table cannot run at the caret (PR #165 review): inside a code block, math
    /// block, raw (HTML) block or front matter the table would be code, not a table (cheap
    /// check). From the editor-only paragraph after such a block, the planned insertion is
    /// verified by re-parsing ([`Document::insert_table_parses`]): an unclosed fence or HTML
    /// block would swallow it.
    fn insert_table_refusal(&self, req: &PlanRequest) -> Option<&'static str> {
        let focus = req.selection.primary().focus;
        let leaf = self.leaf(focus.offset.min(self.text().len()))?;
        let opaque = matches!(
            leaf.kind,
            LeafKind::CodeBlock { .. } | LeafKind::Math | LeafKind::RawBlock | LeafKind::Metadata
        );
        if !opaque {
            return None;
        }
        if focus.virt == 0 {
            return Some("a table cannot go inside a code, math or HTML block or the front matter");
        }
        self.insert_table_edit(req).err()
    }

    fn plan_insert_table(&self, req: &PlanRequest) -> Result<Plan, PlanError> {
        let focus = req.selection.primary().focus;
        if focus.virt == 0
            && self.leaf(focus.offset.min(self.text().len())).is_some_and(|l| {
                matches!(
                    l.kind,
                    LeafKind::CodeBlock { .. } | LeafKind::Math | LeafKind::RawBlock | LeafKind::Metadata
                )
            })
        {
            return Err(PlanError::NotApplicable(
                "a table cannot go inside a code, math or HTML block or the front matter",
            ));
        }
        let (range, ins, caret) = self.insert_table_edit(req).map_err(PlanError::NotApplicable)?;
        Ok(Plan::single(
            PlanStep::patches(PatchSet::single_verbatim(range, ins)),
            Some(Selection::caret(caret)),
        )
        .labeled("Insert Table"))
    }

    /// The patch of Insert Table (range, replacement, caret), verified: the region re-parsed
    /// with it holds exactly the new table there (Codex review of PR #165: no heuristics
    /// about fences or HTML blocks that could be fooled).
    fn insert_table_edit(&self, req: &PlanRequest) -> Result<(std::ops::Range<usize>, String, usize), &'static str> {
        let size = |name: &str, default: usize, max: usize| {
            req.params
                .get_int(name)
                .map_or(default, |n| n.clamp(1, max as i64) as usize)
        };
        let cols = size("columns", INSERT_TABLE_DEFAULT.0, INSERT_TABLE_MAX.0);
        let rows = size("rows", INSERT_TABLE_DEFAULT.1, INSERT_TABLE_MAX.1);
        let m = TableModel {
            aligns: vec![Align::None; cols],
            rows: vec![vec![String::new(); cols]; rows + 1],
        };
        let lines = table::pretty_print(&m, &PrettyStyle::default());
        let text = self.text();
        let eol = self.line_ending().as_str();
        let at = req.selection.primary().focus.offset.min(text.len());
        let blank_line = |start: usize| {
            let (end, _) = crate::text::line_end(text, start);
            text[start..end].bytes().all(|b| matches!(b, b'>' | b' ' | b'\t'))
        };
        // where the table goes (`range` is replaced) and the container prefix of its lines
        let (range, prefix, before) = match self.leaf(at) {
            Some(leaf) => {
                // the containers' prefix of the block's first line (not a heading's marker)
                let ls = crate::line_start(text, leaf.span.start);
                let mut prefix = super::blocks::cont_prefix(text, ls, leaf.span.start);
                // after indented code (the caret in the paragraph after it): its 4-column
                // indentation is the code's, not a container's
                if matches!(leaf.kind, LeafKind::CodeBlock { .. }) && self.code_fence(leaf.span.start).is_none() {
                    prefix = super::blocks::strip_code_indent(&prefix);
                }
                let end = leaf.span.end;
                (end..end, prefix.clone(), format!("{eol}{}{eol}", prefix.trim_end()))
            }
            None => {
                // a blank line (or an empty document): the table replaces it
                let ls = crate::line_start(text, at);
                let (le, _) = crate::text::line_end(text, ls);
                let line = &text[ls..le];
                let prefix = if line.contains('>') {
                    format!("{} ", line.trim_end())
                } else {
                    String::new()
                };
                let before = if ls > 0 && !blank_line(crate::line_start(text, ls - 1)) {
                    format!("{eol}{}", prefix.trim_end())
                } else {
                    String::new()
                };
                // `before` ends the previous line's block: a blank line, then this one
                let before = if before.is_empty() {
                    before
                } else {
                    format!("{before}{eol}")
                };
                (ls..le, prefix, before)
            }
        };
        let mut ins = before;
        let mut caret = 0;
        for (i, l) in lines.iter().enumerate() {
            if i > 0 {
                ins.push_str(eol);
            }
            ins.push_str(&prefix);
            if i == 0 {
                let cells = table::row_cells(l, markise_doc::Span::new(0, l.len()));
                caret = range.start + ins.len() + cells[0].start;
            }
            ins.push_str(l);
        }
        // the next line is text: a blank line ends the table first (it would become a row)
        let (_, next) = crate::text::line_end(text, range.end);
        if next > range.end && next < text.len() && !blank_line(next) {
            ins.push_str(eol);
            ins.push_str(prefix.trim_end());
        }
        if !self.insert_table_parses(&range, &ins, caret, cols, rows) {
            return Err("the new table would not stay a table here");
        }
        Ok((range, ins, caret))
    }

    /// The top-level block around `range` (whole lines, and the line after it) parsed alone
    /// with `ins` in place of `range` has a table of `cols` columns and `rows` body rows
    /// whose first header cell starts at `caret`.
    fn insert_table_parses(
        &self,
        range: &std::ops::Range<usize>,
        ins: &str,
        caret: usize,
        cols: usize,
        rows: usize,
    ) -> bool {
        let text = self.text();
        let i = self.top_partition_point(|t| t.end() < range.start);
        let a = crate::line_start(text, self.top(i).map_or(range.start, |t| t.start().min(range.start)));
        let (_, next) = crate::text::line_end(text, range.end);
        let (b, _) = crate::text::line_end(text, next);
        let mut block = String::with_capacity(b - a + ins.len());
        block.push_str(&text[a..range.start]);
        block.push_str(ins);
        block.push_str(&text[range.end..b.max(range.end)]);
        let doc = Document::parse(block, self.options().clone());
        let at = caret - a;
        let Some(leaf) = doc.leaf(at) else { return false };
        if !matches!(leaf.kind, LeafKind::Table { columns } if columns as usize == cols) {
            return false;
        }
        let Some(t) = doc.table(leaf.span.start) else {
            return false;
        };
        t.rows.len() == rows + 2 && doc.table_row_cells(t.rows[0]).first().is_some_and(|c| c.start == at)
    }
}

//! Explicit table operations (spec 02 §6): one transaction, keeping container prefixes.
use crate::{
    Align, Document,
    table::{self, PrettyStyle, TableModel},
};
use markise_doc::{ColumnAlign, FormatDoc, PatchSet, Plan, PlanError, PlanRequest, PlanStep, Selection, TableView};

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

//! Paragraph → Table → Insert Table… (spec 02 §6, issue #164): a small dialog asks for the
//! size — a grid picker (spec 99 C2: up to 6 columns × 10 rows, the top row is the header)
//! and number fields for larger tables — then runs the registry command `table.insert` with
//! `columns` and body `rows`. The menu item, the toolbar, the context menu and the shortcut
//! all open it; the command itself (without a size: 3 × 3) is one undo step.

use egui::{Color32, Context, Key, RichText, Sense, Stroke, Ui, vec2};
use markise_doc::{ParamValue, Params};
use markise_md::{INSERT_TABLE_DEFAULT, INSERT_TABLE_MAX};

use super::MarkiseApp;

/// Columns of the grid picker.
pub const PICKER_COLUMNS: usize = 6;
/// Rows of the grid picker, the header row included.
pub const PICKER_ROWS: usize = 10;
const CELL: f32 = 20.0;
const GAP: f32 = 3.0;

/// The open Insert Table dialog: the size it inserts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InsertTable {
    pub columns: usize,
    /// Body rows (the header row is always added).
    pub rows: usize,
}

impl Default for InsertTable {
    fn default() -> Self {
        InsertTable {
            columns: INSERT_TABLE_DEFAULT.0,
            rows: INSERT_TABLE_DEFAULT.1,
        }
    }
}

/// What the user did in the dialog this frame.
enum Act {
    Insert,
    Cancel,
}

impl MarkiseApp {
    /// Opens the Insert Table dialog when the command can run at the caret; `false` otherwise.
    pub(super) fn open_insert_table(&mut self) -> bool {
        if self.source_focused() {
            return false;
        }
        let Some(d) = self.doc.as_ref() else { return false };
        if d.viewer.card_edit().is_some() || !d.viewer.command_enabled("table.insert") {
            return false;
        }
        self.insert_table = Some(InsertTable::default());
        true
    }

    /// The Insert Table dialog's size, while it is open (tests).
    pub fn insert_table_dialog(&self) -> Option<InsertTable> {
        self.insert_table
    }

    /// Sets the open dialog's size (tests; clamped like the fields).
    pub fn set_insert_table_size(&mut self, columns: usize, rows: usize) {
        if let Some(t) = self.insert_table.as_mut() {
            t.columns = columns.clamp(1, INSERT_TABLE_MAX.0);
            t.rows = rows.clamp(1, INSERT_TABLE_MAX.1);
        }
    }

    /// Inserts the table of the open dialog and closes it (its Insert button).
    pub fn confirm_insert_table(&mut self, ctx: &Context) -> bool {
        let Some(t) = self.insert_table.take() else {
            return false;
        };
        let Some(d) = self.doc.as_mut() else { return false };
        let params = Params::new()
            .with("columns", ParamValue::Int(t.columns as i64))
            .with("rows", ParamValue::Int(t.rows as i64));
        let ok = d.viewer.run(ctx, self.parley.as_mut(), "table.insert", &params).is_ok();
        if ok {
            d.sync();
            d.viewer.request_refocus();
        }
        ctx.request_repaint();
        ok
    }

    /// Closes the open dialog without inserting (Cancel, Esc).
    pub fn cancel_insert_table(&mut self) {
        self.insert_table = None;
        // the document takes the keyboard back from the dialog
        if let Some(d) = self.doc.as_mut() {
            d.viewer.request_refocus();
        }
    }

    /// Draws the open dialog (a modal); Enter inserts, Esc or a click outside cancels.
    pub(super) fn insert_table_window(&mut self, ctx: &Context) {
        let Some(mut t) = self.insert_table else { return };
        use crate::dialog::{Width, footer, modal, primary, secondary, title};
        let (act, dismissed) = modal(ctx, "markise-insert-table", Width::Narrow, |ui, look| {
            title(ui, look, "Insert Table");
            let mut act = None;
            if picker(ui, &mut t, look.accent, look.border, look.weak) {
                act = Some(Act::Insert);
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label("Columns");
                ui.add(egui::DragValue::new(&mut t.columns).range(1..=INSERT_TABLE_MAX.0));
                ui.add_space(12.0);
                ui.label("Rows");
                ui.add(egui::DragValue::new(&mut t.rows).range(1..=INSERT_TABLE_MAX.1));
                ui.label(RichText::new("plus the header row").color(look.weak));
            });
            footer(ui, |ui| {
                if primary(ui, look, "Insert").clicked() {
                    act = Some(Act::Insert);
                }
                if secondary(ui, look, "Cancel").clicked() {
                    act = Some(Act::Cancel);
                }
            });
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                act = Some(Act::Insert);
            }
            act
        });
        self.insert_table = Some(t);
        match act {
            Some(Act::Insert) => {
                self.confirm_insert_table(ctx);
            }
            Some(Act::Cancel) => self.cancel_insert_table(),
            None if dismissed => self.cancel_insert_table(),
            None => {}
        }
    }
}

/// The grid picker: hovering a cell sizes the table to it (columns × rows below the shaded
/// header row), a click inserts. Returns `true` on a click.
fn picker(ui: &mut Ui, t: &mut InsertTable, accent: Color32, border: Color32, weak: Color32) -> bool {
    let size = vec2(
        PICKER_COLUMNS as f32 * (CELL + GAP) - GAP,
        PICKER_ROWS as f32 * (CELL + GAP) - GAP,
    );
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let cell_at = |p: egui::Pos2| {
        let c = ((p.x - rect.left()) / (CELL + GAP)).floor() as usize;
        let r = ((p.y - rect.top()) / (CELL + GAP)).floor() as usize;
        (c.min(PICKER_COLUMNS - 1), r.min(PICKER_ROWS - 1))
    };
    if let Some(p) = resp.hover_pos() {
        let (c, r) = cell_at(p);
        t.columns = c + 1;
        t.rows = r.max(1); // the top row is the header: at least one body row
    }
    let painter = ui.painter_at(rect);
    for r in 0..PICKER_ROWS {
        for c in 0..PICKER_COLUMNS {
            let min = rect.min + vec2(c as f32 * (CELL + GAP), r as f32 * (CELL + GAP));
            let cell = egui::Rect::from_min_size(min, vec2(CELL, CELL));
            let on = c < t.columns && r <= t.rows;
            let fill = match (on, r == 0) {
                (true, true) => accent,
                (true, false) => accent.gamma_multiply(0.45),
                (false, true) => weak.gamma_multiply(0.25),
                (false, false) => Color32::TRANSPARENT,
            };
            painter.rect(cell, 3.0, fill, Stroke::new(1.0, border), egui::StrokeKind::Inside);
        }
    }
    ui.label(format!("{} columns × {} rows + header", t.columns, t.rows));
    resp.clicked()
}

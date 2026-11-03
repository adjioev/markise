//! Spec 02 §6 / issue #54: structural commands retain cell data, containers and undo.
use crate::common;
use common::T;
use markise_layout::Paging;

#[test]
fn table_operations_are_single_undo_steps_in_containers_and_crlf() {
    for prefix in ["", "> ", "  "] {
        for eol in ["\n", "\r\n"] {
            let table = ["| H | J |", "| --- | ---: |", "| a\\|b | `x` |", "| c | d |"]
                .map(|l| format!("{prefix}{l}{eol}"))
                .join("");
            let src = if prefix == "  " {
                format!("- intro{eol}{eol}{table}")
            } else {
                table
            };
            for cmd in [
                "table.addRowAbove",
                "table.addRowBelow",
                "table.deleteRow",
                "table.addColumnBefore",
                "table.addColumnAfter",
                "table.deleteColumn",
                "table.moveRowDown",
                "table.moveColumnRight",
                "table.alignCenter",
            ] {
                let mut t = T::new(&src);
                t.set(src.find("a\\|b").unwrap());
                assert!(t.ed.registry.enabled(&t.ed.session, cmd).is_ok(), "{cmd} {src:?}");
                t.run(cmd);
                assert_ne!(t.text(), src, "{cmd}");
                t.check_model();
                assert!(t.ed.session.doc().leaf(t.caret().offset).is_some());
                if eol == "\r\n" {
                    assert!(!t.text().replace("\r\n", "").contains('\n'));
                }
                t.run("edit.undo");
                assert_eq!(t.text(), src, "{cmd}");
            }
        }
    }
}
#[test]
fn moving_columns_carries_alignment_and_escaped_content() {
    let mut t = T::at("| H | J |\n| --- | ---: |\n| ‸a\\|b | c |\n| d | e |\n");
    t.run("table.moveColumnRight");
    let table = t.ed.session.doc().table(0).unwrap();
    assert_eq!(
        table.aligns,
        vec![markise_doc::ColumnAlign::Right, markise_doc::ColumnAlign::None]
    );
    let cells = t.ed.session.doc().table_row_cells(table.rows[2]);
    assert_eq!(cells[1].slice(t.text()), "a\\|b");
    assert_eq!(t.caret().offset, cells[1].start);
}
#[test]
fn boundaries_keep_header_and_minimum_body_and_column() {
    let t = T::at("| H |\n| --- |\n| ‸a |\n");
    for cmd in [
        "table.deleteRow",
        "table.deleteColumn",
        "table.moveRowUp",
        "table.moveRowDown",
        "table.moveColumnLeft",
        "table.moveColumnRight",
    ] {
        assert!(t.ed.registry.enabled(&t.ed.session, cmd).is_err(), "{cmd}");
    }
    let t = T::at("| ‸H |\n| --- |\n| a |\n| b |\n");
    assert!(t.ed.registry.enabled(&t.ed.session, "table.deleteRow").is_err());
}

/// The cell span at table line `line` (0 = header, 1 = delimiter, 2.. = body), column `col`.
fn cell(t: &T, line: usize, col: usize) -> markise_doc::Span {
    let doc = t.ed.session.doc();
    let start = t.text().find('|').unwrap();
    let table = doc.table(start).unwrap();
    doc.table_row_cells(table.rows[line])[col]
}

/// The trimmed cell texts of table line `line`.
fn row(t: &T, line: usize) -> Vec<String> {
    let doc = t.ed.session.doc();
    let start = t.text().find('|').unwrap();
    let table = doc.table(start).unwrap();
    doc.table_row_cells(table.rows[line])
        .iter()
        .map(|c| c.slice(t.text()).to_owned())
        .collect()
}

/// Issue #163: after Add Row Above/Below (menu or ⌘↩) the caret is in the new row's cell of
/// the same column, on a caret stop (the viewer's per-frame fix keeps it there), and typing,
/// Tab and Shift+Tab work there — the full and paged models, first and last rows.
#[test]
fn typing_into_a_freshly_added_row_issue_163() {
    let src = "para\n\n| H | J |\n| --- | --- |\n| a | b |\n| c | d |\n\nafter\n";
    // (command, caret's cell text, column, the new row's table line)
    let cases = [
        ("table.addRowBelow", "a", 0, 3),
        ("table.addRowBelow", "d", 1, 4),
        ("table.addRowAbove", "a", 0, 2),
        ("table.addRowAbove", "c", 0, 3),
        ("table.addRowAbove", "J", 1, 2),
        ("table.addRowBelow", "H", 0, 2),
    ];
    for paging in [Paging::Full, Paging::Paged { tops: 1 }] {
        for (cmd, at, col, line) in cases {
            let what = format!("{paging:?} {cmd} at {at}");
            let mut t = T::with_paging(src, paging);
            t.fix_selection = true;
            t.set(src.find(&format!("| {at} ")).unwrap() + 2);
            t.run(cmd);
            assert_eq!(row(&t, line), ["", ""], "{what}: {}", t.shown());
            assert_eq!(t.caret().offset, cell(&t, line, col).start, "{what}: {}", t.shown());
            assert!(t.selection_on_stops(), "{what}: caret on a stop");
            assert!(t.caret_visible(), "{what}");
            t.typ("xy");
            let mut want = vec![String::new(), String::new()];
            want[col] = "xy".into();
            assert_eq!(row(&t, line), want, "{what}: {}", t.shown());
            t.check_model();
            // Tab / Shift+Tab move into the other (empty) cell of the new row; typing works there
            let other = 1 - col;
            t.run(if other > col {
                "edit.indentOrNextCell"
            } else {
                "edit.outdentOrPrevCell"
            });
            assert_eq!(
                t.caret().offset,
                cell(&t, line, other).start,
                "{what}: Tab {}",
                t.shown()
            );
            t.typ("z");
            want[other] = "z".into();
            assert_eq!(row(&t, line), want, "{what}: {}", t.shown());
            t.check_model();
        }
    }
}

/// Issue #163: every empty cell is a caret stop (it was skipped, so the caret snapped out
/// of the cell or the table): a table whose body is all empty cells, header cells, padded
/// and unpadded empty cells.
#[test]
fn empty_cells_are_caret_stops_issue_163() {
    for src in [
        "| H | J |\n| --- | --- |\n|   |   |\n",
        "| H | J |\n|---|---|\n|||\n",
        "|   | J |\n| --- | --- |\n| a |   |\n",
        "> | H | J |\n> | --- | --- |\n> |   |   |\n",
    ] {
        let mut t = T::new(src);
        t.fix_selection = true;
        let start = src.find('|').unwrap();
        let table = t.ed.session.doc().table(start).unwrap();
        for line in [0, 2] {
            for col in 0..2 {
                let c = t.ed.session.doc().table_row_cells(table.rows[line])[col];
                if !c.is_empty() {
                    continue;
                }
                t.set(c.start);
                assert_eq!(t.caret().offset, c.start, "{src:?} {line}/{col}");
                assert!(t.caret_visible(), "{src:?} {line}/{col}");
            }
        }
        // the new row of an all-empty table takes text too
        let c = cell(&t, 2, 1);
        t.set(c.start);
        t.run("table.addRowBelow");
        assert_eq!(t.caret().offset, cell(&t, 3, 1).start, "{src:?}: {}", t.shown());
        t.typ("q");
        assert_eq!(row(&t, 3), ["", "q"], "{src:?}: {}", t.shown());
        t.check_model();
    }
}

/// Issue #163: after undo and redo of Add Row the caret is back in the new row and typing
/// goes into it.
#[test]
fn typing_into_an_added_row_after_undo_redo_issue_163() {
    for paging in [Paging::Full, Paging::Paged { tops: 1 }] {
        let src = "| H | J |\n| --- | --- |\n| a | b |\n\nafter\n";
        let mut t = T::with_paging(src, paging);
        t.fix_selection = true;
        t.set(src.find("| b").unwrap() + 2);
        t.run("table.addRowBelow");
        let added = t.text().to_owned();
        t.run("edit.undo");
        assert_eq!(t.text(), src);
        t.run("edit.redo");
        assert_eq!(t.text(), added);
        assert_eq!(t.caret().offset, cell(&t, 3, 1).start, "{paging:?}: {}", t.shown());
        t.typ("r");
        assert_eq!(row(&t, 3), ["", "r"], "{paging:?}: {}", t.shown());
        t.check_model();
    }
}

/// Issue #163: column commands put the caret into the new (empty) column of the caret's row.
#[test]
fn caret_lands_in_a_new_column_issue_163() {
    for (cmd, col) in [("table.addColumnBefore", 0), ("table.addColumnAfter", 1)] {
        let src = "| H |\n| --- |\n| a |\n";
        let mut t = T::new(src);
        t.fix_selection = true;
        t.set(src.find("| a").unwrap() + 2);
        t.run(cmd);
        assert_eq!(t.caret().offset, cell(&t, 2, col).start, "{cmd}: {}", t.shown());
        t.typ("n");
        assert_eq!(row(&t, 2)[col], "n", "{cmd}: {}", t.shown());
    }
}

fn insert_table(t: &mut T, cols: i64, rows: i64) {
    t.run_p(
        "table.insert",
        markise_doc::Params::new()
            .with("columns", markise_doc::ParamValue::Int(cols))
            .with("rows", markise_doc::ParamValue::Int(rows)),
    );
}

/// Issue #164 / spec 02 §6: Insert Table puts a header, the delimiter row and empty body rows
/// as a new block after the caret's block, the caret in the first header cell (typing goes
/// there), one undo step; default 3 × 3.
#[test]
fn insert_table_after_the_caret_block_issue_164() {
    for paging in [Paging::Full, Paging::Paged { tops: 1 }] {
        let src = "Intro text\n\nAfter\n";
        let mut t = T::with_paging(src, paging);
        t.fix_selection = true;
        t.set(3);
        t.run("table.insert");
        let table = t.ed.session.doc().table(t.text().find('|').unwrap()).unwrap();
        assert_eq!(table.aligns.len(), 3, "{}", t.shown());
        assert_eq!(table.rows.len(), 5, "header, delimiter, 3 body rows: {}", t.shown());
        assert!(t.text().starts_with("Intro text\n\n|"), "{}", t.shown());
        assert!(t.text().ends_with("|\n\nAfter\n"), "{}", t.shown());
        assert_eq!(t.caret().offset, cell(&t, 0, 0).start, "{}", t.shown());
        assert!(t.selection_on_stops());
        t.check_model();
        t.typ("Name");
        assert_eq!(row(&t, 0), ["Name", "", ""], "{}", t.shown());
        t.run("edit.undo");
        t.run("edit.undo");
        assert_eq!(t.text(), src, "{paging:?}: one undo step for the table");
    }
}

#[test]
fn insert_table_sizes_and_neighbours_issue_164() {
    // columns × body rows; the next block right after the caret's gets a blank line first
    let mut t = T::at("# Ti‸tle\nNext para\n");
    insert_table(&mut t, 2, 1);
    assert_eq!(
        t.text(),
        "# Title\n\n|      |      |\n| ---- | ---- |\n|      |      |\n\nNext para\n"
    );
    assert_eq!(t.caret().offset, cell(&t, 0, 0).start);
    // bounds: at least 1 × 1, at most 64 columns
    let mut t = T::at("a‸\n");
    insert_table(&mut t, 0, 0);
    assert_eq!(t.text(), "a\n\n|      |\n| ---- |\n|      |\n");
    let mut t = T::at("a‸\n");
    insert_table(&mut t, 500, 2);
    let table = t.ed.session.doc().table(3).unwrap();
    assert_eq!((table.aligns.len(), table.rows.len()), (64, 4));
    // an empty document and a blank line between blocks: the table goes there
    let mut t = T::new("");
    insert_table(&mut t, 1, 1);
    assert_eq!(t.text(), "|      |\n| ---- |\n|      |");
    assert_eq!(t.caret().offset, cell(&t, 0, 0).start);
    t.typ("h");
    assert_eq!(row(&t, 0), ["h"]);
    // CRLF documents keep their line endings
    let mut t = T::at("a‸\r\n");
    insert_table(&mut t, 1, 1);
    assert_eq!(t.text(), "a\r\n\r\n|      |\r\n| ---- |\r\n|      |\r\n");
}

/// Issue #164: in a quote and a list item the table keeps the container (prefixes).
#[test]
fn insert_table_in_containers_issue_164() {
    for (src, want) in [
        (
            "> quo‸ted\n",
            "> quoted\n>\n> |      |      |\n> | ---- | ---- |\n> |      |      |\n",
        ),
        (
            "- ite‸m\n- two\n",
            "- item\n\n  |      |      |\n  | ---- | ---- |\n  |      |      |\n\n- two\n",
        ),
    ] {
        let mut t = T::at(src);
        t.fix_selection = true;
        insert_table(&mut t, 2, 1);
        assert_eq!(t.text(), want);
        let leaf = t.ed.session.doc().leaf(t.caret().offset).unwrap();
        assert!(
            matches!(leaf.kind, markise_doc::LeafKind::Table { .. }),
            "{src:?}: a table in the container: {}",
            t.shown()
        );
        assert!(!leaf.containers.is_empty(), "{src:?}");
        t.typ("H");
        assert_eq!(row(&t, 0), ["H", ""], "{src:?}: {}", t.shown());
        t.check_model();
    }
}

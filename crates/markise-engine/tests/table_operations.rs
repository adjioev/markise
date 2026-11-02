//! Spec 02 §6 / issue #54: structural commands retain cell data, containers and undo.
mod common;
use common::T;

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

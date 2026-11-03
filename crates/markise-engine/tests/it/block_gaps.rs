//! The caret between two adjacent blocks (issue #166): the virtual empty paragraph in the gap
//! between blocks where no paragraph can be reached otherwise (two tables, code, math,
//! diagrams, images, rules, HTML, the end of a quote or list), reached with Down / Up /
//! Left / Right and with Enter at the end of a table's last row; typing there writes a real
//! paragraph with the blank lines that keep both neighbours intact, leaving it writes
//! nothing, undo restores the source and the caret.

use crate::common;

use common::T;
use markise_doc::{DocPos, LeafKind};
use markise_layout::Paging;

/// A block kind: its Markdown, where the caret goes on its last and on its first line
/// (`None`: no caret stop, e.g. a rule) and the `virt` of the slot after it before a
/// top-level block (1 + the containers it leaves).
struct Kind {
    name: &'static str,
    md: &'static str,
    last: Option<&'static str>,
    first: Option<&'static str>,
    virt: u16,
}

const fn k(
    name: &'static str,
    md: &'static str,
    last: Option<&'static str>,
    first: Option<&'static str>,
    virt: u16,
) -> Kind {
    Kind {
        name,
        md,
        last,
        first,
        virt,
    }
}

/// The caret marks are the text right after the caret (first match in the snippet).
const BLOCKS: &[Kind] = &[
    k("table", "| a | b |\n|---|---|\n| 1 | 2 |", Some("1 |"), Some("a |"), 1),
    k("fenced code", "```\ncode\n```", Some("de\n"), Some("de\n"), 1),
    k("indented code", "    code", Some("de"), Some("de"), 1),
    k("math", "$$\nx^2\n$$", Some("^2"), Some("^2"), 1),
    k("diagram", "```mermaid\ngraph TD\n```", Some(" TD"), Some(" TD"), 1),
    k("image", "![alt](img.png)", None, None, 1),
    k("rule", "***", None, None, 1),
    k("html", "<div>\nhtml\n</div>", Some("/div"), Some("div>"), 1),
];

/// Containers whose end is a gap before a block outside them.
const ENDS: &[Kind] = &[
    k("quote", "> quoted", Some("ed"), Some("ed"), 2),
    k("list", "- item", Some("em"), Some("em"), 3),
];

fn caret_in(src: &str, from: usize, mark: &str) -> usize {
    from + src[from..].find(mark).unwrap_or_else(|| panic!("{mark:?} in {src:?}"))
}

/// Every pair of blocks: Down from the first block's last line and Up from the second's
/// first line reach the gap (no source change, nothing in the history); typing writes a
/// paragraph between them; undo restores the source and the virtual caret; leaving the gap
/// without typing writes nothing.
#[test]
fn every_block_pair_has_a_reachable_gap_issue_166() {
    let firsts: Vec<&Kind> = BLOCKS.iter().chain(ENDS).collect();
    let seconds: Vec<&Kind> = BLOCKS.iter().chain(&ENDS[..1]).collect();
    for a in &firsts {
        for b in &seconds {
            if b.name == "indented code" && matches!(a.name, "indented code" | "list") {
                continue; // one code block / the item's paragraph (indentation continues it)
            }
            let what = format!("{} then {}", a.name, b.name);
            let src = format!("{}\n\n{}\n", a.md, b.md);
            let a_end = src.len() - b.md.len() - 3;
            let b_start = a_end + 2;
            let gap = DocPos::with_virt(a_end, a.virt);
            let mut t = T::new(&src);
            let undo = t.ed.session.history().undo_len();
            if let Some(m) = a.last {
                t.set(caret_in(&src, 0, m));
                t.run("caret.down");
                assert_eq!(t.caret(), gap, "{what}: Down into the gap: {}", t.shown());
                assert!(
                    t.ed.view.focus().is_some_and(|(_, f)| f.virtual_after),
                    "{what}: the empty line is shown"
                );
            }
            if let Some(m) = b.first {
                t.set(caret_in(&src, b_start, m));
                t.run("caret.up");
                assert_eq!(t.caret(), gap, "{what}: Up into the gap: {}", t.shown());
            }
            if a.last.is_none() && b.first.is_none() {
                continue; // reached by clicking (UI tests)
            }
            assert_eq!(t.text(), src, "{what}: nothing written yet");
            assert_eq!(t.ed.session.history().undo_len(), undo, "{what}: no history entry");
            // leaving without typing writes nothing; coming back with the arrows
            t.run("caret.down");
            // (into the second block, or past it when it has no caret stop)
            assert_ne!(t.caret(), gap, "{what}: Down leaves the gap: {}", t.shown());
            assert_eq!(t.text(), src, "{what}: leaving writes nothing");
            if b.first.is_some() {
                t.run("caret.up");
            } else {
                t.set(caret_in(&src, 0, a.last.unwrap()));
                t.run("caret.down");
            }
            assert_eq!(t.caret(), gap, "{what}: back in the gap: {}", t.shown());
            // typing: a paragraph between the blocks, both kept
            t.typ("x");
            let want = format!("{}\n\nx\n\n{}\n", a.md, b.md);
            assert_eq!(t.text(), want, "{what}: typed");
            assert_eq!(t.caret(), DocPos::new(a_end + 3), "{what}: caret after the text");
            let leaf = t.ed.session.doc().leaf(a_end + 2).unwrap();
            assert_eq!(leaf.kind, LeafKind::Paragraph, "{what}: a paragraph");
            assert_eq!(leaf.span.len(), 1, "{what}: holding just the typed text");
            assert_eq!(leaf.containers.len(), 0, "{what}: at the top level");
            t.check_model();
            t.run("edit.undo");
            assert_eq!(t.text(), src, "{what}: undo");
            assert_eq!(t.caret(), gap, "{what}: undo restores the virtual caret");
            t.run("edit.redo");
            assert_eq!(t.text(), want, "{what}: redo");
        }
    }
}

/// No gap where a paragraph or heading is next to it (Enter or typing at its edge reaches
/// that line already), nor between two items of one list.
#[test]
fn no_gap_next_to_text_or_between_list_items_issue_166() {
    let table = "| a |\n|---|\n| 1 |";
    for (src, from, want_after) in [
        (format!("para\n\n{table}\n"), "ra", "a |"),
        (format!("# Head\n\n{table}\n"), "ad", "a |"),
        (format!("{table}\n\npara\n"), "1 |", "ra"),
        (format!("{table}\n\n# Head\n"), "1 |", "ad"),
        ("> quoted\n\npara\n".to_string(), "ed", "ra"),
        ("- a\n\n  ```\n  c\n  ```\n- b\n".to_string(), "c\n", "b\n"),
    ] {
        let mut t = T::new(&src);
        t.set(caret_in(&src, 0, from));
        t.run("caret.down");
        // (the goal column may put it anywhere on that first line)
        let next = caret_in(&src, 0, want_after);
        let line = src[..next].rfind('\n').map_or(0, |n| n + 1);
        assert!(
            t.caret().virt == 0
                && t.caret().offset >= line
                && t.caret().offset <= src[line..].find('\n').unwrap() + line,
            "{src:?}: Down goes straight on: {}",
            t.shown()
        );
    }
}

/// Blocks without a blank line between them (a fence, then a table): the typed paragraph
/// gets a blank line on both sides, or the table would swallow it as a row.
#[test]
fn typing_in_a_gap_without_blank_lines_issue_166() {
    let src = "```\nc\n```\n| a |\n|---|\n| 1 |\n";
    let mut t = T::new(src);
    t.set(4);
    t.run("caret.down");
    assert_eq!(t.caret(), DocPos::with_virt(9, 1), "{}", t.shown());
    t.typ("x");
    assert_eq!(t.text(), "```\nc\n```\n\nx\n\n| a |\n|---|\n| 1 |\n");
    assert!(
        t.ed.session.doc().debug_tree().contains("table"),
        "{}",
        t.ed.session.doc().debug_tree()
    );
    t.check_model();
    // inside a quote: the blank lines keep the quote's prefix
    let src = "> | a |\n> |---|\n> | 1 |\n>\n> | b |\n> |---|\n> | 2 |\n";
    let mut t = T::new(src);
    t.set(src.find("1 |").unwrap());
    t.run("caret.down");
    let gap = src.find("1 |").unwrap() + 3;
    assert_eq!(t.caret(), DocPos::with_virt(gap, 1), "{}", t.shown());
    t.typ("x");
    assert_eq!(
        t.text(),
        "> | a |\n> |---|\n> | 1 |\n>\n> x\n>\n> | b |\n> |---|\n> | 2 |\n"
    );
    assert_eq!(t.ed.session.doc().debug_tree().matches("table").count(), 2);
    t.check_model();
}

/// Left / Right stop in the gap too, and Backspace / Delete at the blocks' edges keep their
/// old stops (they do not delete the blank line between the tables).
#[test]
fn horizontal_moves_through_the_gap_issue_166() {
    let src = "| a |\n|---|\n| 1 |\n\n| b |\n|---|\n| 2 |\n";
    let end1 = src.find("1 |").unwrap() + 1;
    let gap = DocPos::with_virt(src.find("\n\n").unwrap(), 1);
    let mut t = T::new(src);
    t.set(end1);
    t.run("caret.right");
    assert_eq!(t.caret(), gap, "{}", t.shown());
    t.run("caret.right");
    assert_eq!(t.caret().offset, src.find("b |").unwrap(), "{}", t.shown());
    t.run("caret.left");
    assert_eq!(t.caret(), gap, "{}", t.shown());
    t.run("caret.left");
    assert_eq!(t.caret().offset, end1, "{}", t.shown());
    for (at, key) in [
        (src.find("b |").unwrap(), "edit.deleteBackward"),
        (end1, "edit.deleteForward"),
    ] {
        let mut t = T::new(src);
        t.set(at);
        t.run(key);
        assert_eq!(t.text(), src, "{key} at {at}");
    }
}

/// Enter at the end of a table's last row goes to the virtual paragraph after the table;
/// elsewhere in the table Enter still does nothing, and Cmd+Enter still adds a row.
#[test]
fn enter_at_the_end_of_a_tables_last_row_issue_166() {
    let src = "| a | b |\n|---|---|\n| 1 | 2 |\n\n| c |\n|---|\n| 3 |\n";
    let end = src.find("2 |").unwrap() + 1;
    let gap = DocPos::with_virt(src.find("\n\n").unwrap(), 1);
    let mut t = T::new(src);
    let undo = t.ed.session.history().undo_len();
    t.set(end);
    t.run("edit.newline");
    assert_eq!(t.caret(), gap, "{}", t.shown());
    assert_eq!(t.text(), src);
    assert_eq!(t.ed.session.history().undo_len(), undo, "no history entry");
    t.typ("p");
    assert_eq!(
        t.text(),
        "| a | b |\n|---|---|\n| 1 | 2 |\n\np\n\n| c |\n|---|\n| 3 |\n"
    );
    // not at the end of the last row, or in an earlier row: nothing
    for at in [src.find("1 |").unwrap() + 1, src.find("b |").unwrap() + 1] {
        let mut t = T::new(src);
        t.set(at);
        t.run("edit.newline");
        assert_eq!((t.text(), t.caret().virt), (src, 0), "Enter at {at}");
    }
    // the header of a table without body rows is its last row
    let mut t = T::new("| h |\n|---|\n\n```\nc\n```\n");
    t.set(3);
    t.run("edit.newline");
    assert_eq!(t.caret(), DocPos::with_virt(11, 1), "{}", t.shown());
    // Cmd+Enter adds a row below
    let mut t = T::new(src);
    t.set(end);
    t.run("table.addRowBelow");
    assert_eq!(
        t.text().lines().filter(|l| l.starts_with('|')).count(),
        7,
        "{}",
        t.text()
    );
}

/// Enter at the start of a paragraph after a table, math or HTML opens the empty paragraph
/// above it (as after code and rules), and typing keeps the paragraph below apart.
#[test]
fn enter_at_the_start_of_a_paragraph_after_a_block_issue_166() {
    for block in ["| a |\n|---|\n| 1 |", "$$\nx\n$$", "<div>\nh\n</div>", "```\nc\n```"] {
        let src = format!("{block}\n\npara\n");
        let mut t = T::new(&src);
        t.set(block.len() + 2);
        t.run("edit.newline");
        assert_eq!(t.caret(), DocPos::with_virt(block.len(), 1), "{block:?}: {}", t.shown());
        t.typ("z");
        assert_eq!(t.text(), format!("{block}\n\nz\n\npara\n"), "{block:?}");
    }
    // directly after a fence the typed paragraph does not run into the next line's text
    let src = "```\nc\n```\npara\n";
    let mut t = T::new(src);
    t.set(src.find("para").unwrap());
    t.run("edit.newline");
    assert_eq!(t.caret(), DocPos::with_virt(9, 1), "{}", t.shown());
    t.typ("z");
    assert_eq!(t.text(), "```\nc\n```\n\nz\n\npara\n");
}

/// Indented code: the typed paragraph does not take the code's indentation (it would
/// continue the code block).
#[test]
fn gap_after_indented_code_issue_166() {
    let src = "    code\n\n***\n";
    let mut t = T::new(src);
    t.set(6);
    t.run("caret.down");
    assert_eq!(t.caret(), DocPos::with_virt(8, 1), "{}", t.shown());
    t.typ("x");
    assert_eq!(t.text(), "    code\n\nx\n\n***\n");
}

/// Gaps at page boundaries of a paged model (issue #125): reaching, typing and undo agree
/// with a full model, and the paged model stays equal to a full build.
#[test]
fn gaps_at_page_boundaries_issue_166() {
    let mut src = String::new();
    for i in 0..30 {
        src.push_str(&format!("| t{i} |\n|---|\n| r{i} |\n\n"));
    }
    for tops in [2usize, 3] {
        for k in [tops - 1, tops, 2 * tops - 1] {
            let row = src.find(&format!("r{k} |")).unwrap();
            let end = row + format!("r{k} |").len();
            let mut runs = Vec::new();
            for paging in [Paging::Paged { tops }, Paging::Full] {
                let mut t = T::with_paging(&src, paging);
                t.fix_selection = true;
                t.set(row + 1);
                t.reveal();
                t.run("caret.down");
                assert_eq!(
                    t.caret(),
                    DocPos::with_virt(end, 1),
                    "{paging:?} k={k}: {}",
                    t.caret().offset
                );
                t.run("caret.down");
                assert_eq!(t.caret().virt, 0, "{paging:?} k={k}: into the next table");
                t.run("caret.up");
                assert_eq!(t.caret(), DocPos::with_virt(end, 1), "{paging:?} k={k}: Up");
                t.typ("p");
                t.check_model();
                let typed = t.text().to_string();
                t.run("edit.undo");
                assert_eq!(t.text(), src, "{paging:?} k={k}: undo");
                assert_eq!(t.caret(), DocPos::with_virt(end, 1), "{paging:?} k={k}: undo caret");
                t.check_model();
                runs.push(typed);
            }
            assert_eq!(runs[0], runs[1], "paged == full (k={k})");
            assert!(
                runs[0].contains(&format!("| r{k} |\n\np\n\n| t{} |", k + 1)),
                "{}",
                runs[0]
            );
        }
    }
}

/// Front matter before a table: Up from the table's header reaches the gap after it.
#[test]
fn gap_after_front_matter_issue_166() {
    let src = "---\na: 1\n---\n\n| a |\n|---|\n| 1 |\n";
    let mut t = T::new(src);
    t.set(src.find("a |").unwrap());
    t.run("caret.up");
    assert_eq!(t.caret(), DocPos::with_virt(12, 1), "{}", t.shown());
    t.typ("x");
    assert_eq!(t.text(), "---\na: 1\n---\n\nx\n\n| a |\n|---|\n| 1 |\n");
    // no blank line after the front matter
    let src = "---\na: 1\n---\n| a |\n|---|\n| 1 |\n";
    let mut t = T::new(src);
    t.set(src.find("a |").unwrap());
    t.run("caret.up");
    assert_eq!(t.caret(), DocPos::with_virt(12, 1), "{}", t.shown());
    t.typ("x");
    assert_eq!(t.text(), "---\na: 1\n---\n\nx\n\n| a |\n|---|\n| 1 |\n");
}

/// CRLF files keep their line endings in the typed paragraph's separators.
#[test]
fn gap_in_a_crlf_file_issue_166() {
    let src = "| a |\r\n|---|\r\n| 1 |\r\n```\r\nc\r\n```\r\n";
    let mut t = T::new(src);
    t.set(src.find("1 |").unwrap());
    t.run("caret.down");
    assert_eq!(
        t.caret(),
        DocPos::with_virt(src.find("1 |").unwrap() + 3, 1),
        "{}",
        t.shown()
    );
    t.typ("x");
    assert_eq!(t.text(), "| a |\r\n|---|\r\n| 1 |\r\n\r\nx\r\n\r\n```\r\nc\r\n```\r\n");
}

/// Codex review of PR #167: indented code whose indentation holds a tab, inside a list item —
/// the typed paragraph takes the item's two columns (the code's four are measured in
/// columns), so it and the rule after it stay in the item.
#[test]
fn gap_after_tab_indented_code_in_a_list_item() {
    let src = "- a\n\n  \t  code\n\n  ***\n";
    let mut t = T::new(src);
    t.set(src.find("code").unwrap() + 1);
    t.run("caret.down");
    let end = src.find("code").unwrap() + 4;
    assert_eq!(t.caret(), DocPos::with_virt(end, 1), "{}", t.shown());
    t.typ("x");
    assert_eq!(t.text(), "- a\n\n  \t  code\n\n  x\n\n  ***\n");
    let doc = t.ed.session.doc();
    for (what, at) in [("x", src.len() - 4), ("rule", t.text().find("***").unwrap())] {
        let leaf = doc.leaf(at).unwrap();
        assert_eq!(
            leaf.containers.len(),
            2,
            "{what} stays in the item: {}",
            doc.debug_tree()
        );
    }
}

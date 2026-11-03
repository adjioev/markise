//! Table editing on the real app (spec 02 §6): rows added from the Paragraph → Table menu
//! and with ⌘↩ take a caret, clicks, Tab and typing (issue #163), in the rendered view and in
//! Side by Side; Insert Table (issue #164).

use crate::common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use markise_doc::DocPos;
use markise_ui_egui::{AppOptions, MarkiseApp, ViewMode, menu::MenuAction};

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-tables-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn open(name: &str, src: &str) -> Harness<'static, MarkiseApp> {
    let p = tmpdir(name).join("t.md");
    std::fs::write(&p, src).unwrap();
    let app = MarkiseApp::new(AppOptions {
        open: vec![p],
        ..common::no_watch()
    });
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app);
    let t = Instant::now();
    while h.state().model().is_none() || h.state().is_loading() {
        assert!(t.elapsed() < common::HANG_GUARD, "timed out waiting for the load");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
    h
}

fn cmd() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND
    } else {
        Modifiers::CTRL | Modifiers::COMMAND
    }
}

fn text(h: &Harness<'static, MarkiseApp>) -> String {
    h.state().document_text().unwrap()
}

fn caret(h: &mut Harness<'static, MarkiseApp>) -> usize {
    let sel = h.state_mut().viewer().unwrap().ed.session.selection().primary();
    assert!(sel.is_caret(), "a caret: {sel:?}");
    sel.focus.offset
}

fn set_caret(h: &mut Harness<'static, MarkiseApp>, at: usize) {
    h.state_mut().viewer().unwrap().ed.set_caret(DocPos::new(at), false);
    h.run_steps(2);
}

fn typ(h: &mut Harness<'static, MarkiseApp>, s: &str) {
    for c in s.chars() {
        h.event(Event::Text(c.to_string()));
        h.step();
    }
}

fn key(h: &mut Harness<'static, MarkiseApp>, m: Modifiers, k: Key) {
    h.key_press_modifiers(m, k);
    h.run_steps(2);
}

fn click(h: &mut Harness<'static, MarkiseApp>, p: egui::Pos2) {
    h.hover_at(p);
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    h.run_steps(2);
}

/// The caret rectangle the app reports to the platform (IME cursor area) this frame.
fn caret_screen(h: &mut Harness<'static, MarkiseApp>) -> egui::Rect {
    h.step();
    h.output().platform_output.ime.expect("a caret is shown").cursor_rect
}

fn menu(h: &mut Harness<'static, MarkiseApp>, id: &'static str) {
    let ctx = h.ctx.clone();
    assert!(h.state_mut().menu_action(&ctx, &MenuAction::Command(id)), "{id} runs");
    h.run_steps(2);
}

/// Trimmed cells of the first table's line `line` (0 = header, 1 = delimiter, 2.. = body).
fn row(h: &Harness<'static, MarkiseApp>, line: usize) -> Vec<String> {
    let t = text(h);
    let l = t
        .lines()
        .filter(|l| l.trim_start_matches("> ").starts_with('|'))
        .nth(line)
        .unwrap();
    let l = l.trim_start_matches("> ").trim();
    l[1..l.len() - 1].split('|').map(|c| c.trim().to_owned()).collect()
}

/// The start of cell `col` of table line `line` (empty cells: before their closing pipe).
fn cell_start(h: &Harness<'static, MarkiseApp>, line: usize, col: usize) -> usize {
    let t = text(h);
    let mut off = 0;
    let mut n = 0;
    for l in t.split_inclusive('\n') {
        if l.trim_start_matches("> ").starts_with('|') {
            if n == line {
                let pipes: Vec<usize> = l.match_indices('|').map(|(i, _)| i).collect();
                let (a, b) = (pipes[col] + 1, pipes[col + 1]);
                let cell = &l[a..b];
                let lead = cell.len() - cell.trim_start().len();
                return off + if cell.trim().is_empty() { b } else { a + lead };
            }
            n += 1;
        }
        off += l.len();
    }
    panic!("no table line {line}");
}

/// Issue #163 (owner report): Add Row Above/Below from the Paragraph → Table menu and ⌘↩ put
/// the caret into the new row (same column, not out of the table); typing there, Tab into
/// the next new cell, and clicking into a new cell after leaving it all edit the new row.
#[test]
fn added_rows_take_caret_typing_tab_and_clicks_issue_163() {
    let src = "Intro\n\n| Head | Col |\n| ---- | --- |\n| a1 | b1 |\n| a2 | b2 |\n\nAfter\n";
    // (how, caret's cell text, the new row's table line)
    let cases: [(&str, &str, usize); 4] = [
        ("table.addRowBelow", "a2", 4),
        ("table.addRowAbove", "a1", 2),
        ("cmd-enter", "a1", 3),
        ("cmd-enter", "a2", 4),
    ];
    for (how, at, line) in cases {
        let what = format!("{how} at {at}");
        let mut h = open("rows", src);
        click(&mut h, egui::pos2(600.0, 200.0));
        set_caret(&mut h, src.find(at).unwrap());
        if how == "cmd-enter" {
            key(&mut h, cmd(), Key::Enter);
        } else {
            menu(&mut h, how);
        }
        assert_eq!(row(&h, line), ["", ""], "{what}: {}", text(&h));
        assert_eq!(caret(&mut h), cell_start(&h, line, 0), "{what}: caret in the new row");
        let first = caret_screen(&mut h);
        typ(&mut h, "x");
        assert_eq!(row(&h, line), ["x", ""], "{what}: {}", text(&h));
        // Tab into the second new cell, type there
        key(&mut h, Modifiers::NONE, Key::Tab);
        assert_eq!(caret(&mut h), cell_start(&h, line, 1), "{what}: Tab");
        let second = caret_screen(&mut h);
        typ(&mut h, "y");
        assert_eq!(row(&h, line), ["x", "y"], "{what}: {}", text(&h));
        // a click into the new row's first cell (right of its text) puts the caret there
        let after = text(&h).find("After").unwrap();
        set_caret(&mut h, after);
        click(&mut h, egui::pos2(first.max.x + 12.0, first.center().y));
        typ(&mut h, "z");
        assert_eq!(row(&h, line), ["xz", "y"], "{what}: click: {}", text(&h));
        assert!(second.min.x > first.min.x, "{what}: columns side by side");
    }
}

/// Issue #163: a click into an empty cell of a freshly added row (no text in it yet) and
/// typing there; after undo and redo of the row the new row edits as well.
#[test]
fn clicking_an_empty_new_cell_and_undo_redo_issue_163() {
    let src = "| H | J |\n| --- | --- |\n| a | b |\n\nAfter\n";
    let mut h = open("click-empty", src);
    click(&mut h, egui::pos2(600.0, 200.0));
    set_caret(&mut h, src.find("b |").unwrap());
    key(&mut h, cmd(), Key::Enter);
    assert_eq!(caret(&mut h), cell_start(&h, 3, 1), "same column: {}", text(&h));
    let r = caret_screen(&mut h);
    let added = text(&h);
    key(&mut h, cmd(), Key::Z);
    assert_eq!(text(&h), src);
    key(&mut h, cmd() | Modifiers::SHIFT, Key::Z);
    assert_eq!(text(&h), added);
    let after = text(&h).find("After").unwrap();
    set_caret(&mut h, after);
    click(&mut h, egui::pos2(r.center().x + 6.0, r.center().y));
    assert_eq!(caret(&mut h), cell_start(&h, 3, 1), "clicked into the empty cell");
    typ(&mut h, "q");
    assert_eq!(row(&h, 3), ["", "q"], "{}", text(&h));
}

/// Issue #163 in Side by Side (the rendered pane takes the caret and commands; typing goes to
/// the source pane): Add Row from the rendered pane leaves the caret in the new row over the
/// next frames (it was snapped out of the table), both panes show the row, and in the
/// rendered view typing goes into it.
#[test]
fn added_rows_in_side_by_side_issue_163() {
    let src = "| H | J |\n| --- | --- |\n| a | b |\n\nAfter\n";
    let mut h = open("sbs", src);
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    h.run_steps(2);
    // focus the rendered pane, then the caret in `b`
    click(&mut h, egui::pos2(800.0, 100.0));
    set_caret(&mut h, src.find("b |").unwrap());
    menu(&mut h, "table.addRowBelow");
    h.run_steps(3);
    assert_eq!(caret(&mut h), cell_start(&h, 3, 1), "{}", text(&h));
    assert_eq!(h.state().source_buffer(), Some(text(&h).as_str()), "both panes");
    h.state_mut().set_view_mode(ViewMode::Rendered);
    h.run_steps(3);
    // focus the rendered view (leaving Side by Side takes the source pane's caret), then
    // into the new cell
    click(&mut h, egui::pos2(600.0, 400.0));
    let at = cell_start(&h, 3, 1);
    set_caret(&mut h, at);
    typ(&mut h, "s");
    assert_eq!(row(&h, 3), ["", "s"], "{}", text(&h));
}

fn insert_table_chord() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND | Modifiers::ALT
    } else {
        Modifiers::CTRL | Modifiers::COMMAND | Modifiers::SHIFT
    }
}

/// Issue #164: Paragraph → Table → Insert Table… asks for the size (3 × 3 by default);
/// Enter inserts a header, the delimiter row and the body rows after the caret's block, the
/// caret in the first header cell; typing goes there; one undo step removes the table.
#[test]
fn insert_table_from_the_menu_issue_164() {
    let src = "Intro\n\nAfter\n";
    let mut h = open("insert-menu", src);
    click(&mut h, egui::pos2(600.0, 200.0));
    set_caret(&mut h, 2);
    // the menus: in Paragraph → Table (first), not on Paragraph itself
    let bar = h.state().menu_model();
    let para = bar.menu("Paragraph").unwrap();
    let table = para
        .entries
        .iter()
        .find_map(|e| match e {
            markise_ui_egui::menu::Entry::Submenu(m) if m.title == "Table" => Some(m),
            _ => None,
        })
        .unwrap();
    let markise_ui_egui::menu::Entry::Item(first) = &table.entries[0] else {
        panic!("an item first")
    };
    assert_eq!(
        (first.key.as_str(), first.label.as_str()),
        ("table.insert", "Insert Table…")
    );
    assert!(first.enabled);
    assert!(
        para.entries
            .iter()
            .all(|e| !matches!(e, markise_ui_egui::menu::Entry::Item(i) if i.key == "table.insert"))
    );
    menu(&mut h, "table.insert");
    let size = h.state().insert_table_dialog().expect("the dialog is open");
    assert_eq!((size.columns, size.rows), (3, 3));
    assert_eq!(text(&h), src, "nothing inserted yet");
    h.state_mut().set_insert_table_size(2, 2);
    key(&mut h, Modifiers::NONE, Key::Enter);
    assert!(h.state().insert_table_dialog().is_none(), "closed");
    assert_eq!(
        text(&h),
        "Intro\n\n|      |      |\n| ---- | ---- |\n|      |      |\n|      |      |\n\nAfter\n"
    );
    assert_eq!(caret(&mut h), cell_start(&h, 0, 0), "caret in the first header cell");
    let inserted = text(&h);
    key(&mut h, cmd(), Key::Z);
    assert_eq!(text(&h), src, "one undo step");
    key(&mut h, cmd() | Modifiers::SHIFT, Key::Z);
    assert_eq!(text(&h), inserted);
    // typing and Tab fill the header
    let at = cell_start(&h, 0, 0);
    set_caret(&mut h, at);
    typ(&mut h, "Name");
    key(&mut h, Modifiers::NONE, Key::Tab);
    typ(&mut h, "Age");
    assert_eq!(row(&h, 0), ["Name", "Age"], "{}", text(&h));
}

/// Issue #164: the shortcut (⌥⌘T, Ctrl+Shift+T) opens the dialog too; Esc cancels it; the
/// toolbar's More menu and the cheat sheet have the command.
#[test]
fn insert_table_shortcut_toolbar_and_cheat_sheet_issue_164() {
    let src = "Intro\n";
    let mut h = open("insert-key", src);
    click(&mut h, egui::pos2(600.0, 200.0));
    set_caret(&mut h, 2);
    key(&mut h, insert_table_chord(), Key::T);
    assert!(
        h.state().insert_table_dialog().is_some(),
        "the shortcut opens the dialog"
    );
    key(&mut h, Modifiers::NONE, Key::Escape);
    assert!(h.state().insert_table_dialog().is_none(), "Esc closes it");
    assert_eq!(text(&h), src, "nothing inserted");
    // the Insert button inserts the default 3 × 3
    key(&mut h, insert_table_chord(), Key::T);
    let ctx = h.ctx.clone();
    assert!(h.state_mut().confirm_insert_table(&ctx));
    h.run_steps(2);
    assert_eq!(row(&h, 0).len(), 3);
    assert_eq!(
        text(&h).lines().filter(|l| l.starts_with('|')).count(),
        5,
        "{}",
        text(&h)
    );
    assert!(
        h.state().toolbar_state().unwrap().get("table.insert").is_some(),
        "in the toolbar"
    );
    let sheet = h.state().shortcut_groups();
    let table = sheet.iter().find(|g| g.title == "Table").unwrap();
    assert!(table.rows.iter().any(|r| r.id == "table.insert"), "{table:?}");
}

/// PR #165 review: the Insert Table dialog is modal — File → New does not run under it —
/// and it never inserts into another document than the one it was opened for (or after the
/// document changed).
#[test]
fn insert_table_dialog_stays_with_its_document() {
    let src = "Intro\n";
    let mut h = open("insert-origin", src);
    click(&mut h, egui::pos2(600.0, 200.0));
    set_caret(&mut h, 2);
    menu(&mut h, "table.insert");
    assert!(h.state().insert_table_dialog().is_some());
    let ctx = h.ctx.clone();
    assert!(
        !h.state_mut().menu_action(&ctx, &MenuAction::Command("file.new")),
        "no command under the dialog"
    );
    assert_eq!(text(&h), src);
    // another document anyway (e.g. a reload): the dialog closes, nothing is inserted
    h.state_mut().new_document();
    h.run_steps(2);
    assert!(h.state().insert_table_dialog().is_none(), "closed for another document");
    assert!(!h.state_mut().confirm_insert_table(&ctx));
    assert_eq!(text(&h), "", "the new document stays empty");
    // an edit of its own document while it is open closes it too
    let mut h = open("insert-edit", src);
    click(&mut h, egui::pos2(600.0, 200.0));
    set_caret(&mut h, 2);
    menu(&mut h, "table.insert");
    h.state_mut().viewer().unwrap().ed.session.insert_text("x").unwrap();
    h.run_steps(2);
    assert!(h.state().insert_table_dialog().is_none(), "closed after an edit");
    assert_eq!(text(&h), "Inxtro\n");
}

/// Codex review 2 of PR #165: Close (and Quit) still work while the Insert Table dialog is
/// open — the dialog goes first; losing the window focus dismisses it too.
#[test]
fn close_and_focus_loss_dismiss_the_insert_table_dialog() {
    let mut h = open("insert-close", "Intro\n");
    click(&mut h, egui::pos2(600.0, 200.0));
    set_caret(&mut h, 2);
    menu(&mut h, "table.insert");
    assert!(h.state().insert_table_dialog().is_some());
    let ctx = h.ctx.clone();
    assert!(
        h.state_mut().menu_action(&ctx, &MenuAction::Command("file.close")),
        "Close runs"
    );
    assert!(h.state().insert_table_dialog().is_none(), "the dialog went first");
    // focus loss
    let mut h = open("insert-blur", "Intro\n");
    click(&mut h, egui::pos2(600.0, 200.0));
    set_caret(&mut h, 2);
    menu(&mut h, "table.insert");
    h.event(Event::WindowFocused(false));
    h.run_steps(2);
    assert!(h.state().insert_table_dialog().is_none(), "dismissed on focus loss");
    assert_eq!(text(&h), "Intro\n");
}

//! Review comments and suggestions in the real app (ADR 0012, issue #69): Add Comment with
//! ⌥⌘M on a selection, the comments panel (list, jump, Accept / Reject / Accept All), the
//! document's context menu, View → Comments & Suggestions off (plain text, bytes unchanged),
//! File → Save Copy Without Comments, and the AI review loop end to end.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_doc::{CriticKind, Selection};
use markise_ui_egui::confirm::{Choice, Confirm, Kind, Scripted};
use markise_ui_egui::menu::MenuAction;
use markise_ui_egui::sidebar::Tab;
use markise_ui_egui::{AppOptions, MarkiseApp};

type H = Harness<'static, MarkiseApp>;

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-review-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn open_with(name: &str, text: &str, confirm: Option<&Arc<Scripted>>) -> (H, PathBuf) {
    let dir = tmpdir(name);
    let p = dir.join("draft.md");
    std::fs::write(&p, text).unwrap();
    let mut opts = AppOptions {
        open: vec![p.clone()],
        ..Default::default()
    };
    if let Some(s) = confirm {
        opts.confirm = s.clone() as Arc<dyn Confirm>;
    }
    let mut h = Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), MarkiseApp::new(opts));
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    (h, p)
}

fn open(name: &str, text: &str) -> (H, PathBuf) {
    open_with(name, text, None)
}

fn wait_until(h: &mut H, what: &str, f: impl Fn(&MarkiseApp) -> bool) {
    let t = Instant::now();
    while !f(h.state()) {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.step();
}

fn cmd() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers {
            mac_cmd: true,
            command: true,
            ..Default::default()
        }
    } else {
        Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        }
    }
}

/// ⌥⌘M on macOS, Ctrl+Alt+M elsewhere.
fn add_comment_chord() -> Modifiers {
    Modifiers { alt: true, ..cmd() }
}

fn typ(h: &mut H, s: &str) {
    for c in s.chars() {
        let key = if c == ' ' {
            Some(Key::Space)
        } else {
            Key::from_name(&c.to_string())
        };
        let ev = |k: Key, pressed: bool| Event::Key {
            key: k,
            physical_key: Some(k),
            pressed,
            repeat: false,
            modifiers: Modifiers::default(),
        };
        if let Some(k) = key {
            h.event(ev(k, true));
        }
        h.event(Event::Text(c.to_string()));
        if let Some(k) = key {
            h.event(ev(k, false));
        }
        h.step();
    }
}

fn text(h: &H) -> String {
    h.state().document_text().unwrap()
}

fn select(h: &mut H, a: usize, b: usize) {
    let v = h.state_mut().viewer().unwrap();
    v.ed.edit(|s| s.set_selection(Selection::range(a, b)));
    v.request_refocus();
    h.run_steps(2);
}

fn caret(h: &mut H) -> usize {
    h.state_mut()
        .viewer()
        .unwrap()
        .ed
        .session
        .selection()
        .primary()
        .focus
        .offset
}

fn save_and_read(h: &mut H, p: &Path) -> String {
    h.key_press_modifiers(cmd(), Key::S);
    wait_until(h, "save", |a| !a.is_busy() && !a.is_dirty());
    std::fs::read_to_string(p).unwrap()
}

fn undo(h: &mut H) {
    h.key_press_modifiers(cmd(), Key::Z);
    h.step();
}

/// Clicks the first widget labelled `label` (panel buttons repeat per row).
fn click_first(h: &mut H, label: &str) {
    h.query_all_by_label(label)
        .next()
        .unwrap_or_else(|| panic!("no “{label}”"))
        .click();
    h.run_steps(3);
}

fn show_panel(h: &mut H) {
    h.state_mut().set_sidebar_tab(Tab::Comments);
    h.run_steps(3);
}

const REVIEWED: &str = "# Plan\n\nWe ship {++the beta ++}on Friday.\n\nThe team {--really --}likes it.\n\n\
It {~~is~>was~~}{>>@ai: past tense<<} fine. {==Costs==}{>>numbers?<<} stay low.\n";

#[test]
fn add_comment_on_a_selection_with_the_shortcut() {
    let (mut h, p) = open("add", "The quick fox jumps.\n");
    select(&mut h, 4, 13);
    h.key_press_modifiers(add_comment_chord(), Key::M);
    h.run_steps(2);
    assert_eq!(text(&h), "The {==quick fox==}{>><<} jumps.\n");
    assert_eq!(caret(&mut h), 22, "the caret is in the empty comment");
    typ(&mut h, "too vague");
    assert_eq!(text(&h), "The {==quick fox==}{>>too vague<<} jumps.\n");
    assert_eq!(save_and_read(&mut h, &p), "The {==quick fox==}{>>too vague<<} jumps.\n");
    // undo the typing (word by word), then the command in one step
    for _ in 0..4 {
        if text(&h) == "The {==quick fox==}{>><<} jumps.\n" {
            break;
        }
        undo(&mut h);
    }
    assert_eq!(text(&h), "The {==quick fox==}{>><<} jumps.\n");
    undo(&mut h);
    assert_eq!(text(&h), "The quick fox jumps.\n");
    // the menu item is there with its shortcut
    let bar = h.state().menu_model();
    let item = bar
        .item("review.addComment")
        .expect("Edit → Comments & Suggestions → Add Comment");
    assert!(item.chord.is_some());
}

#[test]
fn panel_lists_marks_and_jumps_to_them() {
    let (mut h, _p) = open("panel", REVIEWED);
    show_panel(&mut h);
    let marks = h.state().review_marks();
    let kinds: Vec<CriticKind> = marks.iter().map(|m| m.kind).collect();
    assert_eq!(
        kinds,
        [
            CriticKind::Insert,
            CriticKind::Delete,
            CriticKind::Substitute,
            CriticKind::Comment,
            CriticKind::Highlight,
            CriticKind::Comment
        ]
    );
    assert!(h.query_by_label("2 comments · 3 suggestions").is_some());
    assert!(h.query_by_label("@ai").is_some(), "the author of a comment");
    assert!(h.query_by_label("past tense").is_some());
    // a click on an item's text puts the caret at the mark
    h.get_by_label("numbers?").click();
    h.run_steps(3);
    let hl = marks.iter().find(|m| m.kind == CriticKind::Highlight).unwrap();
    assert_eq!(caret(&mut h), hl.range.start);
}

#[test]
fn accept_reject_and_accept_all_from_the_panel() {
    let (mut h, p) = open("accept", REVIEWED);
    show_panel(&mut h);
    click_first(&mut h, "Accept"); // the insertion
    assert!(text(&h).contains("We ship the beta on Friday."), "{}", text(&h));
    click_first(&mut h, "Reject"); // the deletion
    assert!(text(&h).contains("The team really likes it."), "{}", text(&h));
    click_first(&mut h, "Accept All"); // the substitution and its comment
    assert_eq!(
        text(&h),
        "# Plan\n\nWe ship the beta on Friday.\n\nThe team really likes it.\n\n\
It was fine. {==Costs==}{>>numbers?<<} stay low.\n"
    );
    // each command is one undo step
    undo(&mut h);
    assert!(text(&h).contains("It {~~is~>was~~}{>>@ai: past tense<<} fine."));
    undo(&mut h);
    assert!(text(&h).contains("The team {--really --}likes it."));
    // Delete on the remaining comment thread keeps the highlighted text
    for _ in 0..2 {
        h.key_press_modifiers(Modifiers { shift: true, ..cmd() }, Key::Z);
        h.step();
    }
    h.run_steps(2);
    click_first(&mut h, "Delete");
    let want = "# Plan\n\nWe ship the beta on Friday.\n\nThe team really likes it.\n\nIt was fine. Costs stay low.\n";
    assert_eq!(text(&h), want);
    assert_eq!(save_and_read(&mut h, &p), want);
}

#[test]
fn context_menu_accepts_the_suggestion_under_the_pointer() {
    let (mut h, _p) = open("context", "One {++two ++}three.\n");
    let r = h.state_mut().viewer().unwrap().item_screen_rect(0).expect("visible");
    // right-click into the inserted word
    let p = egui::pos2(r.left() + 48.0, r.top() + 10.0);
    h.hover_at(p);
    h.run_steps(2);
    h.event(Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Secondary,
        pressed: true,
        modifiers: Modifiers::default(),
    });
    h.step();
    h.event(Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Secondary,
        pressed: false,
        modifiers: Modifiers::default(),
    });
    h.run_steps(3);
    let c = caret(&mut h);
    assert!(
        (4..=14).contains(&c),
        "the right click moved the caret into the mark: {c}"
    );
    h.get_by_label("Accept Suggestion").click();
    h.run_steps(3);
    assert_eq!(text(&h), "One two three.\n");
}

#[test]
fn hover_bar_rejects_the_suggestion_under_the_pointer() {
    let (mut h, _p) = open("hover", "One {--two --}three.\n");
    let r = h.state_mut().viewer().unwrap().item_screen_rect(0).expect("visible");
    h.hover_at(egui::pos2(r.left() + 48.0, r.top() + 10.0));
    h.run_steps(3);
    // the bar shows Accept and Reject above the deleted word
    assert!(h.query_by_label("Accept").is_some());
    h.get_by_label("Reject").click();
    h.run_steps(3);
    assert_eq!(text(&h), "One two three.\n");
    // the pointer over plain text: no bar
    h.hover_at(egui::pos2(r.left() + 2.0, r.top() + 10.0));
    h.run_steps(3);
    assert!(h.query_by_label("Accept").is_none());
}

#[test]
fn switching_off_shows_plain_text_and_keeps_the_bytes() {
    let (mut h, _p) = open("toggle", REVIEWED);
    show_panel(&mut h);
    assert!(h.state().critic_markup());
    assert!(h.state().menu_model().item("view.criticMarkup").is_some());
    h.state_mut().set_critic_markup(false);
    wait_until(&mut h, "re-parse", |a| !a.is_reparsing());
    h.run_steps(3);
    assert!(!h.state().critic_markup());
    assert!(h.state().review_marks().is_empty(), "no marks: plain text");
    assert!(h.query_by_label("Comments and suggestions are off.").is_some());
    assert_eq!(text(&h), REVIEWED, "byte-identical");
    assert!(!h.state().is_dirty());
    // Add Comment is not available while off
    select(&mut h, 2, 6);
    h.key_press_modifiers(add_comment_chord(), Key::M);
    h.run_steps(2);
    assert_eq!(text(&h), REVIEWED);
    h.state_mut().set_critic_markup(true);
    wait_until(&mut h, "re-parse", |a| !a.is_reparsing());
    h.run_steps(2);
    assert_eq!(h.state().review_marks().len(), 6);
    assert!(!h.state().is_dirty());
}

#[test]
fn save_copy_without_comments_asks_and_writes_a_new_file() {
    let s = Scripted::new();
    let (mut h, p) = open_with("copy", REVIEWED, Some(&s));
    let dir = p.parent().unwrap().to_path_buf();
    // accept
    s.answer(Choice::AcceptAll);
    h.state_mut().queue_save_dialog_answer(Some(dir.join("accepted.md")));
    let ctx = h.ctx.clone();
    h.state_mut()
        .menu_action(&ctx, &MenuAction::Command("file.saveCopyClean"));
    h.run_steps(3);
    wait_until(&mut h, "copy", |a| !a.is_saving_copy());
    assert_eq!(s.last().unwrap().kind, Kind::CleanCopy);
    assert_eq!(
        std::fs::read_to_string(dir.join("accepted.md")).unwrap(),
        "# Plan\n\nWe ship the beta on Friday.\n\nThe team likes it.\n\nIt was fine. Costs stay low.\n"
    );
    // reject
    s.answer(Choice::RejectAll);
    h.state_mut().queue_save_dialog_answer(Some(dir.join("rejected.md")));
    h.state_mut().save_copy_without_comments();
    h.run_steps(3);
    wait_until(&mut h, "copy", |a| !a.is_saving_copy());
    assert_eq!(
        std::fs::read_to_string(dir.join("rejected.md")).unwrap(),
        "# Plan\n\nWe ship on Friday.\n\nThe team really likes it.\n\nIt is fine. Costs stay low.\n"
    );
    // the document itself is untouched, in the editor and on disk
    assert_eq!(text(&h), REVIEWED);
    assert!(!h.state().is_dirty());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), REVIEWED);
    // the copy never replaces the document's own file
    s.answer(Choice::AcceptAll);
    h.state_mut().queue_save_dialog_answer(Some(p.clone()));
    h.state_mut().save_copy_without_comments();
    h.run_steps(3);
    assert!(!h.state().is_saving_copy());
    assert!(h.state().banner_text().unwrap_or_default().contains("cannot replace"));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), REVIEWED);
    // Cancel: nothing written
    s.answer(Choice::Cancel);
    h.state_mut().queue_save_dialog_answer(Some(dir.join("never.md")));
    h.state_mut().save_copy_without_comments();
    h.run_steps(3);
    assert!(!dir.join("never.md").exists());
}

/// The motivating flow of issue #69, end to end: an AI tool writes a Markdown file; the user
/// opens it in Markise and comments on passages; the file goes back to the AI tool, which
/// answers with CriticMarkup suggestions (and comments of its own); the user accepts some,
/// rejects others, deletes the answered comments, and saves. The saved bytes are checked
/// exactly, and the file round-trips through both steps without touching any other byte.
#[test]
fn ai_review_loop_end_to_end() {
    // 1. the AI's first draft
    let draft = "# Launch plan\n\n\
Our product launches on March 3. The marketing team will publish a blog post and a\n\
newsletter on the same day.\n\n\
- Budget: $40,000 for ads\n\
- Owner: Dana\n\n\
Risks are low because the beta had no major bugs.\n";
    let (mut h, p) = open("ailoop", draft);
    // 2. the user comments on two passages
    let a = draft.find("March 3").unwrap();
    select(&mut h, a, a + "March 3".len());
    h.key_press_modifiers(add_comment_chord(), Key::M);
    h.run_steps(2);
    typ(&mut h, "is this date confirmed");
    let t = text(&h);
    let b = t.find("Risks are low").unwrap();
    let e = t.find("major bugs.").unwrap() + "major bugs.".len();
    select(&mut h, b, e);
    h.key_press_modifiers(add_comment_chord(), Key::M);
    h.run_steps(2);
    typ(&mut h, "too optimistic");
    let commented = save_and_read(&mut h, &p);
    assert_eq!(
        commented,
        "# Launch plan\n\n\
Our product launches on {==March 3==}{>>is this date confirmed<<}. The marketing team will publish a blog post and a\n\
newsletter on the same day.\n\n\
- Budget: $40,000 for ads\n\
- Owner: Dana\n\n\
{==Risks are low because the beta had no major bugs.==}{>>too optimistic<<}\n"
    );

    // 3. the AI tool revises the file with suggestions and replies (as such tools write it)
    let revised = "# Launch plan\n\n\
Our product launches on {~~March 3~>March 10~~}{>>@ai: moved, the date was not confirmed<<}. The marketing team will publish a blog post{-- and a\n\
newsletter--} on the same day.\n\n\
- Budget: {~~$40,000~>$45,000~~} for ads\n\
- Owner: Dana{++ (backup: Lee)++}\n\n\
{~~Risks are low because the beta had no major bugs.~>Risks are moderate: the beta found two \
major bugs, both fixed.~~}{>>@ai: rewritten after your comment<<}\n";
    std::fs::write(&p, revised).unwrap();
    h.state_mut().open(p.clone());
    wait_until(&mut h, "reload", |a| {
        a.document_text().as_deref() == Some(revised) && !a.is_busy()
    });
    show_panel(&mut h);
    let marks = h.state().review_marks();
    assert_eq!(marks.iter().filter(|m| m.is_suggestion()).count(), 5, "{marks:#?}");

    // 4. the user decides: accept the new date (its reply goes with it), reject the
    //    newsletter cut, reject the budget change, accept the backup owner and the rewrite
    let at = |h: &H, needle: &str| h.state().document_text().unwrap().find(needle).unwrap() + 3;
    let ctx = h.ctx.clone();
    let steps = [
        ("review.accept", "{~~March 3"),
        ("review.reject", "{-- and a"),
        ("review.reject", "{~~$40,000"),
        ("review.accept", "{++ (backup"),
        ("review.accept", "{~~Risks are low"),
    ];
    for (id, needle) in steps {
        let off = at(&h, needle);
        assert!(h.state_mut().run_review(&ctx, id, Some(off)), "{id} at {needle}");
        h.run_steps(2);
    }
    let saved = save_and_read(&mut h, &p);
    assert_eq!(
        saved,
        "# Launch plan\n\n\
Our product launches on March 10. The marketing team will publish a blog post and a\n\
newsletter on the same day.\n\n\
- Budget: $40,000 for ads\n\
- Owner: Dana (backup: Lee)\n\n\
Risks are moderate: the beta found two major bugs, both fixed.\n"
    );
    assert!(h.state().review_marks().is_empty());
}

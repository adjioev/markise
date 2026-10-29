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
use markise_ui_egui::prefs::Section;
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
    // panel rows act only on a current listing (older ones are disabled)
    if h.state().sidebar_tab() == Tab::Comments {
        wait_until(h, "panel list", |a| a.comments_panel_ready());
        h.run_steps(2);
    }
    assert!(has_text(h, label), "{label}");
    h.query_all_by_label(label)
        .next()
        .unwrap_or_else(|| panic!("no “{label}”"))
        .click();
    h.run_steps(3);
}

/// A widget or text with exactly this label or text (labels that label another widget too).
/// Steps a few frames until it shows (fonts arriving on a busy machine re-layout the UI).
fn has_text(h: &mut H, s: &str) -> bool {
    let t = Instant::now();
    loop {
        if h.query_all_by(|n| text_is(n, s)).next().is_some() {
            return true;
        }
        if t.elapsed() > Duration::from_secs(5) {
            return false;
        }
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn text_is(n: &egui_kittest::kittest::AccessKitNode<'_>, s: &str) -> bool {
    n.label().as_deref() == Some(s) || n.value().as_deref() == Some(s)
}

fn click_text(h: &mut H, s: &str) {
    assert!(has_text(h, s), "{s}");
    h.get_by(|n| text_is(n, s)).click();
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
    assert!(has_text(&mut h, "2 comments · 3 suggestions"));
    assert!(has_text(&mut h, "@ai"), "the author of a comment");
    assert!(has_text(&mut h, "past tense"));
    // a click on an item's text puts the caret at the mark
    click_text(&mut h, "numbers?");
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

/// Review rounds 1 and 4: a row listed at an older revision is disabled (no-op) until the
/// next listing — with two identical `{++A++}` and the first deleted, the stale row must not
/// act on the remaining one; after the new listing it does.
#[test]
fn stale_panel_rows_are_disabled_until_the_next_listing() {
    let src = "{++A++} {++A++}\n";
    let (mut h, _p) = open("stale", src);
    // the list must stay old until the test releases it, however slow the machine
    h.state_mut().set_comments_debounce(Duration::from_secs(3600));
    show_panel(&mut h);
    assert!(has_text(&mut h, "+ A"));
    assert!(h.state().comments_panel_ready());
    {
        let v = h.state_mut().viewer().unwrap();
        v.ed.edit(|s| {
            s.apply_external(&markise_doc::PatchSet::delete(0..8), "Test Edit", false)
                .unwrap()
        });
    }
    h.run_steps(2);
    assert!(!h.state().comments_panel_ready(), "the list is still the old one");
    let accept = h.query_all_by_label("Accept").next().expect("a stale row");
    assert!(
        egui_kittest::kittest::NodeT::accesskit_node(&accept).is_disabled(),
        "stale rows are disabled"
    );
    accept.click();
    h.run_steps(3);
    assert_eq!(text(&h), "{++A++}\n", "a click on a stale row does nothing");
    // the next listing arrives: the row acts on its mark
    h.state_mut().set_comments_debounce(Duration::ZERO);
    wait_until(&mut h, "panel list", |a| a.comments_panel_ready());
    h.run_steps(2);
    click_first(&mut h, "Accept");
    assert_eq!(text(&h), "A\n");
}

/// Review rounds 4 and 5: the hover bar never acts on an obsolete or a different mark —
/// hover `two`, move onto the bar, delete that mark so the adjacent `four` slides under the
/// point, click Reject: nothing happens (any edit hides the bar).
#[test]
fn hover_bar_hides_after_an_edit() {
    let src = "One {--two--}{--four--} end.\n";
    let (mut h, _p) = open("hover-stale", src);
    let r = h.state_mut().viewer().unwrap().item_screen_rect(0).expect("visible");
    h.hover_at(egui::pos2(r.left() + 48.0, r.top() + 10.0));
    h.run_steps(3);
    let bar = h
        .state_mut()
        .viewer()
        .unwrap()
        .hover_bar_rect()
        .expect("the bar over two");
    // onto the bar (it stays), then the first mark goes away
    h.hover_at(bar.center());
    h.run_steps(2);
    assert!(h.state_mut().viewer().unwrap().hover_bar_rect().is_some(), "on the bar");
    {
        let v = h.state_mut().viewer().unwrap();
        v.ed.edit(|s| {
            s.apply_external(&markise_doc::PatchSet::delete(4..13), "Test Edit", false)
                .unwrap()
        });
    }
    h.run_steps(3);
    assert_eq!(text(&h), "One {--four--} end.\n");
    assert!(
        h.state_mut().viewer().unwrap().hover_bar_rect().is_none(),
        "any edit hides the bar"
    );
    if let Some(n) = h.query_all_by_label("Reject").next() {
        n.click();
    }
    h.run_steps(3);
    assert_eq!(text(&h), "One {--four--} end.\n", "four stays");
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
    // the switch lives in Preferences → Markdown
    let ctx = h.ctx.clone();
    assert!(h.state_mut().menu_action(&ctx, &MenuAction::Command("app.preferences")));
    h.run_steps(3);
    h.get_by_role_and_label(egui::Role::Button, Section::Markdown.label())
        .click();
    h.run_steps(2);
    let label = "Comments & suggestions (CriticMarkup)";
    h.get_by_role_and_label(egui::Role::Switch, label).focus();
    h.run_steps(1);
    h.key_press(Key::Space);
    h.run_steps(2);
    h.key_press(Key::Escape);
    h.run_steps(2);
    wait_until(&mut h, "re-parse", |a| !a.is_reparsing());
    h.run_steps(3);
    assert!(!h.state().critic_markup());
    assert!(!h.state().preference_values().critic_markup);
    assert!(h.state().review_marks().is_empty(), "no marks: plain text");
    assert!(has_text(&mut h, "Comments and suggestions are off."));
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

// ------------------------------------------------------------------ comment display

use markise_engine::settings::CommentDisplay;

/// A document whose column leaves room for margin cards (sidebar hidden, narrow column).
fn open_roomy(name: &str, text: &str) -> (H, PathBuf) {
    let (mut h, p) = open(name, text);
    let ctx = h.ctx.clone();
    h.state_mut()
        .menu_action(&ctx, &MenuAction::Command("view.toggleSidebar"));
    h.state_mut().set_overrides(markise_theme::Overrides {
        content_width: Some(480.0),
        ..Default::default()
    });
    h.run_steps(4);
    (h, p)
}

fn click_at(h: &mut H, p: egui::Pos2) {
    h.hover_at(p);
    h.step();
    h.drag_at(p);
    h.step();
    h.drop_at(p);
    h.step();
}

fn visible_text(h: &mut H) -> String {
    h.state_mut().viewer().unwrap().visible_text()
}

fn comments(h: &mut H) -> Vec<markise_ui_egui::viewer::ShownComment> {
    h.state_mut().viewer().unwrap().shown_comments().to_vec()
}

fn caret_at(h: &mut H, at: usize) {
    let v = h.state_mut().viewer().unwrap();
    v.ed.edit(|s| s.set_selection(Selection::caret(at)));
    v.ed.refresh_focus();
    v.request_refocus();
    h.run_steps(3);
}

const NOTE: &str = "Note {==this==}{>>check **this** `x`<<} end.\n";

/// Owner feedback: the comment's markers are not painted unless the caret is inside its text
/// (not merely before or after the mark).
#[test]
fn comment_markers_show_only_with_the_caret_inside() {
    let (mut h, _p) = open("markers", NOTE);
    let start = NOTE.find("{>>").unwrap();
    let end = NOTE.find("<<}").unwrap() + 3;
    for at in [0, start, end, NOTE.len() - 1] {
        caret_at(&mut h, at);
        let t = visible_text(&mut h);
        assert!(!t.contains("{>>") && !t.contains("<<}"), "caret at {at}: {t:?}");
    }
    caret_at(&mut h, start + 5);
    assert!(visible_text(&mut h).contains("{>>check **this** `x`<<}"));
}

/// Margin cards: the text as written (plain, with its line breaks) beside its line; a click
/// on the card puts the caret in the comment (its source shows).
#[test]
fn margin_cards_show_the_text_and_open_for_editing() {
    let src = "Plan {==date==}{>>first line\nsecond **line** [l](u)<<} ok.\n\nMore {>>solo<<} text.\n";
    let (mut h, _p) = open_roomy("margin", src);
    assert_eq!(h.state().comment_display(), CommentDisplay::Margin);
    let cs = comments(&mut h);
    assert_eq!(cs.len(), 2, "{cs:#?}");
    assert_eq!(
        &*cs[0].text, "first line\nsecond **line** [l](u)",
        "lines and literal Markdown"
    );
    assert_eq!(&*cs[1].text, "solo", "a standalone comment has a card too");
    let card = cs[0].card.expect("a margin card");
    assert!(card.left() > cs[0].glyph.right(), "in the margin");
    assert!((card.top() - cs[0].glyph.top()).abs() < 12.0, "aligned with its line");
    click_at(&mut h, card.center());
    h.run_steps(3);
    let c = caret(&mut h);
    assert!(
        cs[0].content.start <= c && c <= cs[0].content.end,
        "caret in the comment: {c}"
    );
    assert!(visible_text(&mut h).contains("{>>first line"));
}

/// Bubbles: hovering the glyph opens a popover after a short rest, leaving closes it,
/// clicking the glyph opens the comment for editing and pins the popover; Esc closes it.
#[test]
fn bubbles_show_a_popover_on_hover() {
    let (mut h, _p) = open_roomy("bubbles", NOTE);
    h.state_mut().set_comment_display(CommentDisplay::Bubbles);
    h.run_steps(3);
    let cs = comments(&mut h);
    assert!(cs[0].card.is_none(), "no card until hovered");
    let glyph = cs[0].glyph;
    h.hover_at(glyph.center());
    let t = Instant::now();
    while !h.state_mut().viewer().unwrap().popover_open() {
        assert!(t.elapsed() < Duration::from_secs(5), "popover did not open");
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(&*comments(&mut h)[0].text, "check **this** `x`");
    // the pointer leaves: closed
    h.hover_at(glyph.center() + egui::vec2(0.0, 200.0));
    h.run_steps(3);
    assert!(!h.state_mut().viewer().unwrap().popover_open());
    // a click on the glyph: editing, pinned
    click_at(&mut h, glyph.center());
    h.run_steps(3);
    let c = caret(&mut h);
    assert!(cs[0].content.start <= c && c <= cs[0].content.end);
    h.hover_at(glyph.center() + egui::vec2(0.0, 200.0));
    h.run_steps(3);
    assert!(h.state_mut().viewer().unwrap().popover_open(), "pinned");
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(!h.state_mut().viewer().unwrap().popover_open());
}

/// The setting switches the rendering; the panel's switch writes it to settings.json.
#[test]
fn panel_switch_changes_the_setting() {
    let dir = tmpdir("display-setting");
    let doc = dir.join("note.md");
    std::fs::write(&doc, NOTE).unwrap();
    let cfg = dir.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .allow_missing_glyphs()
        .build_ui_state(
            |ui, app: &mut MarkiseApp| app.ui(ui),
            MarkiseApp::new(AppOptions {
                open: vec![doc],
                config: markise_ui_egui::ConfigDir::Dir(cfg.clone()),
                ..Default::default()
            }),
        );
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    show_panel(&mut h);
    let ctx = h.ctx.clone();
    h.state_mut()
        .menu_action(&ctx, &MenuAction::Command("view.toggleSidebar"));
    h.state_mut()
        .menu_action(&ctx, &MenuAction::Command("view.toggleSidebar"));
    h.state_mut().set_overrides(markise_theme::Overrides {
        content_width: Some(420.0),
        ..Default::default()
    });
    h.run_steps(4);
    assert!(comments(&mut h)[0].card.is_some(), "margin card");
    h.get_by_label("Show comments as bubbles").click();
    h.run_steps(3);
    assert_eq!(h.state().comment_display(), CommentDisplay::Bubbles);
    assert!(comments(&mut h)[0].card.is_none(), "bubbles: no card until hovered");
    let settings = cfg.join("markise").join("settings.json");
    wait_until(&mut h, "settings.json", move |_| {
        std::fs::read_to_string(&settings).is_ok_and(|s| s.contains("\"commentDisplay\": \"bubbles\""))
    });
    h.get_by_label("Show comments in the margin").click();
    h.run_steps(3);
    assert_eq!(h.state().comment_display(), CommentDisplay::Margin);
}

// ------------------------------------------------------------------ multi-line comments

/// Owner decision: Enter inside a comment adds a line break inside it (keeping the quote or
/// list prefix), a second Enter adds nothing; the comment round-trips.
#[test]
fn enter_inside_a_comment_makes_a_multi_line_comment() {
    for (name, src, want) in [
        ("para", "A {>>onetwo<<} b\n", "A {>>one\ntwo<<} b\n"),
        ("quote", "> A {>>onetwo<<} b\n", "> A {>>one\n> two<<} b\n"),
        ("item", "- A {>>onetwo<<} b\n", "- A {>>one\n  two<<} b\n"),
    ] {
        let (mut h, p) = open(&format!("ml-{name}"), src);
        let at = src.find("two<<}").unwrap();
        caret_at(&mut h, at);
        h.key_press(Key::Enter);
        h.step();
        h.key_press(Key::Enter);
        h.step();
        assert_eq!(text(&h), want, "{name}");
        let saved = save_and_read(&mut h, &p);
        assert_eq!(saved, want);
        let marks = h.state().review_marks();
        assert_eq!(marks.len(), 1, "{name}: one comment");
        assert_eq!(marks[0].kind, CriticKind::Comment);
    }
}

// ------------------------------------------------------------------ cards stay on screen (#91)

/// A comment at the start of a long line and one at the end of it, so the cards' anchors span
/// the whole column.
const WIDE: &str = "Intro {>>Card text that is long enough to wrap onto a few lines in the margin<<} \
the rest of a fairly long paragraph line that fills the whole column width and wraps at least \
once, ending here {>>@bot: Ca done<<}.\n\nSecond paragraph.\n";

/// Checks the shown comments of the current frame: each margin card lies fully inside the
/// window's visible area and the document surface (not under the sidebar, not past the right
/// edge, not over the scrollbar); returns the problems found.
fn check_cards_on_screen(h: &mut H, what: &str) -> Vec<String> {
    let screen = h.ctx.content_rect();
    let surface = h.state_mut().viewer().unwrap().surface_rect().expect("surface");
    let mut bad = Vec::new();
    if surface.right() > screen.right() + 0.5 {
        bad.push(format!("{what}: surface {surface:?} past the window {screen:?}"));
    }
    // the document area without the scrollbar (10 pt)
    let area = Rect::from_min_max(surface.min, egui::pos2(surface.right() - 10.0, surface.bottom()));
    let cs = comments(h);
    if cs.len() != 2 {
        bad.push(format!("{what}: {} comments shown", cs.len()));
    }
    for c in &cs {
        if let Some(card) = c.card
            && !(area.expand(0.5).contains_rect(card) && screen.expand(0.5).contains_rect(card))
        {
            bad.push(format!("{what}: card {card:?} outside {area:?} / window {screen:?}"));
        }
    }
    bad
}

use egui::Rect;

/// Bubbles: hovers each shown glyph until its popover opens and checks that the popover lies
/// inside the window and the document surface; moves the pointer away afterwards.
fn check_popovers_on_screen(h: &mut H, what: &str) -> Vec<String> {
    let screen = h.ctx.content_rect();
    let surface = h.state_mut().viewer().unwrap().surface_rect().expect("surface");
    let mut bad = Vec::new();
    let n = comments(h).len();
    for i in 0..n {
        let g = comments(h)[i].glyph;
        if !surface.contains(g.center()) {
            continue;
        }
        h.hover_at(g.center());
        let t = Instant::now();
        while comments(h)[i].card.is_none() && t.elapsed() < Duration::from_secs(5) {
            h.step();
            std::thread::sleep(Duration::from_millis(20));
        }
        match comments(h)[i].card {
            None => bad.push(format!("{what}: popover {i} did not open")),
            Some(card) => {
                if !(surface.expand(0.5).contains_rect(card) && screen.expand(0.5).contains_rect(card)) {
                    bad.push(format!(
                        "{what}: popover {card:?} outside {surface:?} / window {screen:?}"
                    ));
                }
            }
        }
        h.hover_at(egui::pos2(surface.center().x, surface.bottom() - 2.0));
        h.run_steps(3);
    }
    bad
}

/// Issue #91: in margin mode a comment card is drawn inside the visible area, or the comment
/// falls back to a bubble (no card) when the right margin is too narrow — for every built-in
/// style and CSS theme, several window widths and zoom levels, with the sidebar shown.
#[test]
fn margin_cards_stay_inside_the_window_in_every_style_and_theme() {
    // a Retina display (2 px per point), like the owner's
    let dir = tmpdir("on-screen");
    let doc = dir.join("draft.md");
    std::fs::write(&doc, WIDE).unwrap();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .with_pixels_per_point(2.0)
        .allow_missing_glyphs()
        .build_ui_state(
            |ui, app: &mut MarkiseApp| app.ui(ui),
            MarkiseApp::new(AppOptions {
                open: vec![doc],
                ..Default::default()
            }),
        );
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    assert_eq!(h.state().comment_display(), CommentDisplay::Margin);
    type Apply = Box<dyn Fn(&mut MarkiseApp)>;
    let mut looks: Vec<(String, Apply)> = Vec::new();
    for s in markise_theme::style::BUILTIN_STYLES.iter().map(|s| s.id) {
        looks.push((
            format!("style {s}"),
            Box::new(move |a: &mut MarkiseApp| a.select_style(markise_theme::catalog::StyleId::Builtin(s.into()))),
        ));
    }
    for t in markise_theme::builtin::BUILTIN_THEMES.iter().map(|t| t.id) {
        looks.push((
            format!("theme {t}"),
            Box::new(move |a: &mut MarkiseApp| a.select_theme(markise_theme::catalog::ThemeId::Builtin(t.into()))),
        ));
    }
    assert!(looks.len() >= 8, "registry");
    let mut bad = Vec::new();
    let mut margin_seen = 0;
    let mut bubbles_seen = 0;
    for (name, apply) in &looks {
        let before = h.state().theme().clone();
        apply(h.state_mut());
        let t = Instant::now();
        while *h.state().theme() == before && t.elapsed() < Duration::from_secs(10) {
            h.step();
            std::thread::sleep(Duration::from_millis(5));
        }
        for sidebar in [true, false] {
            for zoom in [1.0, 1.5] {
                for width in [900.0, 1100.0, 1400.0, 1900.0] {
                    // the zoom first (egui applies it at the next pass), then the window size in
                    // points (kittest's size is in points; a zoomed window has fewer)
                    h.state_mut().set_zoom(zoom);
                    h.run_steps(2);
                    h.set_size(egui::vec2(width / zoom, 760.0 / zoom));
                    h.run_steps(4);
                    let screen = h.ctx.content_rect();
                    assert!(
                        (screen.width() - width / zoom).abs() < 1.0,
                        "window {screen:?} at {width} / {zoom}"
                    );
                    let what = format!("{name} sidebar={sidebar} zoom={zoom} width={width}");
                    bad.extend(check_cards_on_screen(&mut h, &what));
                    if comments(&mut h).iter().any(|c| c.card.is_some()) {
                        margin_seen += 1;
                    } else {
                        bubbles_seen += 1;
                        bad.extend(check_popovers_on_screen(&mut h, &what));
                    }
                }
            }
            let ctx = h.ctx.clone();
            h.state_mut()
                .menu_action(&ctx, &MenuAction::Command("view.toggleSidebar"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    assert!(
        margin_seen > 0 && bubbles_seen > 0,
        "margin {margin_seen}, bubbles {bubbles_seen}"
    );
}

/// Issue #91, the mechanism: when the document surface is laid out wider than what is visible
/// (here the whole app UI is given 420 pt more than the window, so its right part is clipped
/// away), margin cards must not go into the invisible part: they stay in the visible margin
/// or the comments fall back to bubbles, whose popovers stay on screen too.
#[test]
fn cards_stay_visible_when_the_surface_is_wider_than_the_window() {
    let dir = tmpdir("wider-surface");
    let doc = dir.join("draft.md");
    std::fs::write(&doc, WIDE).unwrap();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1500.0, 760.0))
        .allow_missing_glyphs()
        .build_ui_state(
            |ui, app: &mut MarkiseApp| {
                let r = ui.max_rect();
                let wide = Rect::from_min_size(r.min, egui::vec2(r.width() + 420.0, r.height()));
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(wide));
                app.ui(&mut child)
            },
            MarkiseApp::new(AppOptions {
                open: vec![doc],
                ..Default::default()
            }),
        );
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    let ctx = h.ctx.clone();
    h.state_mut()
        .menu_action(&ctx, &MenuAction::Command("view.toggleSidebar"));
    let screen = h.ctx.content_rect();
    let mut bad = Vec::new();
    for width in [None, Some(480.0), Some(700.0)] {
        h.state_mut().set_overrides(markise_theme::Overrides {
            content_width: width,
            ..Default::default()
        });
        h.run_steps(4);
        let surface = h.state_mut().viewer().unwrap().surface_rect().unwrap();
        assert!(
            surface.right() > screen.right() + 300.0,
            "the surface is wider: {surface:?}"
        );
        let cs = comments(&mut h);
        assert_eq!(cs.len(), 2);
        for c in &cs {
            if let Some(card) = c.card
                && !screen.contains_rect(card)
            {
                bad.push(format!(
                    "content width {width:?}: card {card:?} outside the window {screen:?}"
                ));
            }
        }
        if cs.iter().all(|c| c.card.is_none()) {
            // bubbles: each popover on screen
            for (i, c) in cs.iter().enumerate() {
                h.hover_at(c.glyph.center());
                let t = Instant::now();
                while comments(&mut h)[i].card.is_none() && t.elapsed() < Duration::from_secs(5) {
                    h.step();
                    std::thread::sleep(Duration::from_millis(20));
                }
                let card = comments(&mut h)[i].card.expect("popover");
                if !screen.contains_rect(card) {
                    bad.push(format!(
                        "content width {width:?}: popover {card:?} outside the window {screen:?}"
                    ));
                }
                h.hover_at(egui::pos2(100.0, 700.0));
                h.run_steps(3);
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    // with a narrow column the visible margin holds cards
    h.state_mut().set_overrides(markise_theme::Overrides {
        content_width: Some(480.0),
        ..Default::default()
    });
    h.run_steps(4);
    assert!(
        comments(&mut h).iter().all(|c| c.card.is_some()),
        "margin cards in the visible margin"
    );
}

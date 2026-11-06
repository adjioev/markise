//! Practice mode on the real app (ADR 0024), on a small neutral package: what hides, what is off, and
//! that nothing done here reaches the file.

use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use markise_doc::Selection;
use markise_engine::editor::command::Registry;
use markise_engine::editor::keymap::{Keymap, Platform, reserved_chords};
use markise_layout::BlockViewMode;
use markise_md::MarkdownFormat;
use markise_ui_egui::MarkiseApp;
use markise_ui_egui::menu::{APP_COMMANDS, MenuAction, app_command, descriptor_chord};

use crate::block_views::{Fixture, caret, shown, text};
use crate::common;

type H = Harness<'static, MarkiseApp>;

const MANIFEST: &str = r#"{"format":"markise-block-package/2","library":"sample","version":"1.0.0","blocks":["question","exercise","hint","remark","tip"]}"#;
const QUESTION: &str = r#"{"label": "Question", "variants": {"": "one", "many": "several"}, "fields": [
    {"name": "prompt", "type": "text"},
    {"name": "answers", "type": "choices", "minItems": 2, "correct": {"": "exactly-one", "many": "at-least-one"}},
    {"name": "explanation", "type": "quote", "min": 0}
]}"#;
const EXERCISE: &str = r#"{"label": "Exercise", "fields": [
    {"name": "task", "type": "text"},
    {"name": "solution", "type": "quote", "min": 0}
]}"#;
const TIP: &str = r#"{"label": "Tip", "fields": [{"name": "body", "type": "text"}]}"#;
const VIEWS: &str = r#"{
    "question": {"kind": "choices", "accent": "purple", "practice": {"reveal": ["explanation"]}},
    "exercise": {"kind": "card", "accent": "teal", "practice": {"reveal": ["solution"]}},
    "hint": {"kind": "callout", "accent": "amber", "practice": {"reveal": ["solution"]}},
    "remark": {"kind": "choices", "practice": {"reveal": ["solution"]}},
    "tip": {"kind": "callout", "accent": "green"}
}"#;
const FILES: &[(&str, &str)] = &[
    ("manifest.json", MANIFEST),
    ("components/question/question.json", QUESTION),
    ("components/exercise/exercise.json", EXERCISE),
    ("components/hint/hint.json", EXERCISE),
    ("components/remark/remark.json", EXERCISE),
    ("components/tip/tip.json", TIP),
    ("editor/palette.json", r#"{"format": 1}"#),
    ("editor/views.json", VIEWS),
];

pub(crate) const DOC: &str = "# Doc\n\nBefore.\n\n:::question\n\n**1.** A prompt?\n\n- [ ] A) one {#a1}\n- [x] B) two {#a2}\n\n> Explained at length.\n\n:::\n\n:::question many\n\nSeveral?\n\n- [x] First {#b1}\n- [ ] Second {#b2}\n- [x] Third {#b3}\n\n> Because of reasons.\n\n:::\n\n:::exercise\n\nWork this out.\n\n> The worked solution.\n\n:::\n\n:::hint\n\nA hint's task.\n\n> The hint's solution.\n\n:::\n\n:::remark\n\nA remark's task.\n\n> The remark's solution.\n\n:::\n\n:::tip\n\nJust a tip.\n\n:::\n\nAfter.\n";

fn fixture(doc: &str) -> Fixture {
    Fixture::with_package(doc, FILES)
}

fn command(h: &mut H, id: &'static str) -> bool {
    let ctx = h.ctx.clone();
    let done = h.state_mut().menu_action(&ctx, &MenuAction::Command(id));
    common::run(h);
    done
}

fn mode_of(h: &mut H, needle: &str) -> BlockViewMode {
    let at = text(h).find(needle).unwrap();
    let v = &h.state_mut().viewer().unwrap().ed.view;
    let c = v.practised_around(at);
    c.map_or(BlockViewMode::Native, |_| BlockViewMode::Practice)
}

/// The document is saved and clean, and has no undo step, and says just this.
fn untouched(h: &mut H, f: &Fixture, doc: &str) {
    assert_eq!(text(h), doc, "the text");
    let session = &h.state_mut().viewer().unwrap().ed.session;
    assert!(!session.is_dirty(), "dirty");
    assert!(!session.history().can_undo(), "an undo step");
    assert_eq!(std::fs::read_to_string(&f.doc).unwrap(), doc, "the file");
}

const SPOILERS: [&str; 5] = [
    "Explained",
    "Because of",
    "worked solution",
    "hint's solution",
    "remark's solution",
];

#[test]
fn a_document_in_practice_hides_what_tells_the_answers() {
    let f = fixture(DOC);
    let mut h = f.ready();
    let s = shown(&mut h);
    assert!(s.contains("Explained at length"), "shown outside Practice: {s}");
    assert!(command(&mut h, "view.practiceAll"));
    let s = shown(&mut h);
    for spoiler in SPOILERS {
        assert!(!s.contains(spoiler), "{spoiler}: {s}");
    }
    for stays in [
        "A prompt?",
        "one",
        "two",
        "Several?",
        "Work this out",
        "Just a tip",
        "A hint's task",
    ] {
        assert!(s.contains(stays), "{stays}: {s}");
    }
    // no mark in any painted answer: both answers of the first question draw alike
    let view = &mut h.state_mut().viewer().unwrap().ed.view;
    let filled: usize = (0..view.len())
        .filter_map(|i| view.cache_peek(i).cloned())
        .flat_map(|l| l.prims.clone())
        .filter(|p| match p {
            markise_layout::Prim::Checkbox { checked, .. } => *checked,
            // a radio's dot has no ring
            markise_layout::Prim::Circle { stroke, .. } => stroke.width == 0.0,
            _ => false,
        })
        .count();
    assert_eq!(filled, 0, "a mark shows");
    untouched(&mut h, &f, DOC);
    // and back, with nothing changed
    assert!(command(&mut h, "view.practiceAll"));
    assert!(shown(&mut h).contains("Explained at length"));
    untouched(&mut h, &f, DOC);
}

#[test]
fn the_switch_goes_through_practice_for_a_block_that_has_it_and_past_it_for_one_that_has_not() {
    let f = fixture(DOC);
    let mut h = f.ready();
    assert!(
        h.query_all_by_label("Practice: question").next().is_some(),
        "the block's switch offers it"
    );
    assert!(h.query_by_label("Practice: exercise").is_some(), "with no answers too");
    assert!(h.query_by_label("Practice: tip").is_none());
    assert!(h.query_by_label("Show Markdown: tip").is_some());
    h.get_all_by_label("Practice: question").next().unwrap().click();
    common::run(&mut h);
    assert_eq!(mode_of(&mut h, "A prompt?"), BlockViewMode::Practice);
    assert_eq!(mode_of(&mut h, "Several?"), BlockViewMode::Native, "per block");
    let s = shown(&mut h);
    assert!(!s.contains("Explained") && s.contains("Because of"), "{s}");
    untouched(&mut h, &f, DOC);
}

#[test]
fn a_block_with_no_config_has_no_practice_in_a_document_in_practice() {
    let f = fixture(DOC);
    let mut h = f.ready();
    command(&mut h, "view.practiceAll");
    assert_eq!(mode_of(&mut h, "Just a tip"), BlockViewMode::Native);
    // it edits in place as ever
    let at = DOC.find("Just a tip").unwrap() + 4;
    caret(&mut h, at);
    h.event(Event::Text("x".into()));
    common::run(&mut h);
    assert!(text(&h).contains("Justx a tip"), "{}", text(&h));
}

#[test]
fn every_way_of_changing_a_practised_block_is_off() {
    let f = fixture(DOC);
    let mut h = f.ready();
    command(&mut h, "view.practiceAll");
    let inside = DOC.find("one {#a1}").unwrap() + 1;
    caret(&mut h, inside);
    h.event(Event::Text("typed".into()));
    common::run(&mut h);
    let cmd = if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD
    } else {
        Modifiers::CTRL
    };
    for (m, k) in [
        (Modifiers::NONE, Key::Backspace),
        (Modifiers::NONE, Key::Delete),
        (Modifiers::NONE, Key::Tab),
        (Modifiers::SHIFT, Key::Enter),
        (cmd, Key::B),
    ] {
        h.key_press_modifiers(m, k);
        common::run(&mut h);
    }
    h.event(Event::Paste("pasted".into()));
    common::run(&mut h);
    for id in ["block.moveUp", "block.moveDown", "block.delete", "block.editForm"] {
        command(&mut h, id);
    }
    assert!(h.query_by_label("Edit Block: question").is_none());
    assert!(h.query_by_label("Delete Block: question").is_none());
    assert!(h.state().block_form().is_none());
    // a press on an answer's mark is not an edit either
    let v = h.state_mut().viewer().unwrap();
    assert!(!v.ed.press_answer_mark(inside));
    // a selection from outside over it holds it whole, and does not take it away
    let (a, b) = (DOC.find("Before").unwrap(), DOC.find("After").unwrap());
    v.ed.set_selection(Selection::single(
        markise_doc::DocPos::new(a),
        markise_doc::DocPos::new(b),
    ));
    v.request_refocus();
    common::run(&mut h);
    h.event(Event::Text("z".into()));
    common::run(&mut h);
    untouched(&mut h, &f, DOC);
}

#[test]
fn a_caret_that_lands_on_what_is_hidden_reveals_nothing_and_goes_after_the_block() {
    let f = fixture(DOC);
    let mut h = f.ready();
    command(&mut h, "view.practiceAll");
    let hidden = DOC.find("Explained").unwrap() + 2;
    caret(&mut h, hidden);
    assert!(!shown(&mut h).contains("Explained"));
    let at = h
        .state_mut()
        .viewer()
        .unwrap()
        .ed
        .session
        .selection()
        .primary()
        .focus
        .offset;
    assert!(at > DOC.find(":::\n\n:::question many").unwrap(), "{at}");
    // nor by the session's own route, which goes round the selection's rules
    let v = h.state_mut().viewer().unwrap();
    v.ed.edit(|s| s.set_selection(Selection::caret(hidden)));
    v.ed.refresh_focus();
    common::run(&mut h);
    assert!(!shown(&mut h).contains("Explained"));
    untouched(&mut h, &f, DOC);
}

#[test]
fn entering_practice_with_the_caret_on_what_hides_moves_it() {
    let f = fixture(DOC);
    let mut h = f.ready();
    caret(&mut h, DOC.find("Explained").unwrap() + 2);
    command(&mut h, "view.practiceAll");
    let at = h
        .state_mut()
        .viewer()
        .unwrap()
        .ed
        .session
        .selection()
        .primary()
        .focus
        .offset;
    assert!(at > DOC.find("Explained").unwrap() + 2, "{at}");
    assert!(!shown(&mut h).contains("Explained"));
}

#[test]
fn a_block_that_cannot_be_practised_shows_its_first_row_and_a_reason_that_tells_nothing() {
    // broken in each way a block can be, each of which says something where it would be shown otherwise
    let doc = ":::question\n\nUnmarked?\n\n- [ ] Answer one {#a1}\n- [ ] Answer two {#a2}\n\n> Secret one.\n\n:::\n\n:::question\n\nTwo marks?\n\n- [x] Answer three {#b1}\n- [x] Answer four {#b2}\n\n> Secret two.\n\n:::\n\n:::question\n\nExtra?\n\n- [x] Answer five {#c1}\n- [ ] Answer six {#c2}\n\n```\nsecret code\n```\n\n> Secret three.\n\n:::\n\nAfter.\n";
    let f = fixture(doc);
    let mut h = f.ready();
    command(&mut h, "view.practiceAll");
    let s = shown(&mut h);
    for spoiler in [
        "Secret",
        "secret code",
        "Answer",
        "Unmarked",
        "Two marks",
        "Mark exactly",
        "In no field",
    ] {
        assert!(!s.contains(spoiler), "{spoiler}: {s}");
    }
    assert_eq!(s.matches("Not ready to practice").count(), 3, "{s}");
    assert_eq!(
        h.query_all_by_label("Practice: Not ready to practice").count(),
        3,
        "for a screen reader, the same"
    );
    for label in ["Not filled in yet", "Outside its schema"] {
        assert!(h.query_all_by_label_contains(label).next().is_none(), "{label}");
    }
    untouched(&mut h, &f, doc);
}

#[test]
fn a_block_too_large_to_read_tells_nothing_in_practice_either() {
    let doc = format!(
        ":::question\n\n{}\n\n- [x] Right {{#a1}}\n- [ ] Wrong {{#a2}}\n\n> Secret.\n\n:::\n",
        "word ".repeat(14_000)
    );
    let f = fixture(&doc);
    let mut h = f.ready();
    command(&mut h, "view.practiceAll");
    let s = shown(&mut h);
    assert!(
        !s.contains("Secret") && !s.contains("Right"),
        "{}",
        &s[..s.len().min(300)]
    );
    assert!(s.contains("Not ready to practice"));
}

#[test]
fn find_does_not_go_to_what_practice_hides() {
    let f = fixture(DOC);
    let mut h = f.ready();
    command(&mut h, "view.practiceAll");
    h.state_mut().open_find(false);
    h.state_mut().set_find_query(markise_engine::search::Query {
        text: "Explained".into(),
        ..Default::default()
    });
    let start = std::time::Instant::now();
    while h.state().find_results().is_none() {
        assert!(start.elapsed() < common::HANG_GUARD, "find worker timed out");
        h.step();
    }
    assert_eq!(
        h.state().find_results().unwrap().matches.len(),
        0,
        "a match in what hides"
    );
    // what shows is found as ever
    h.state_mut().set_find_query(markise_engine::search::Query {
        text: "prompt".into(),
        ..Default::default()
    });
    let start = std::time::Instant::now();
    while h.state().find_results().is_none_or(|r| r.matches.is_empty()) {
        assert!(start.elapsed() < common::HANG_GUARD, "find worker timed out");
        h.step();
    }
}

#[test]
fn the_shortcut_of_practice_clashes_with_nothing() {
    for platform in [Platform::Mac, Platform::Other] {
        let keymap = Keymap::new(&Registry::for_format(&MarkdownFormat::default()), platform);
        let reserved = reserved_chords(platform);
        let d = app_command("view.practiceAll").unwrap();
        let c = descriptor_chord(d, platform).expect("a chord");
        assert!(keymap.lookup(&c).is_empty(), "{platform:?}: {:?}", keymap.lookup(&c));
        assert!(!reserved.contains(&c), "{platform:?}");
        for other in APP_COMMANDS.iter().filter(|o| o.id != d.id) {
            assert_ne!(
                descriptor_chord(other, platform).as_ref(),
                Some(&c),
                "{platform:?} {}",
                other.id
            );
        }
    }
}

// ---- answering

fn press(h: &mut H, label: &str) {
    h.get_by_label(label).click();
    common::run(h);
}

fn is_selected(h: &H, label: &str) -> bool {
    use egui::accesskit::Toggled;
    let node = h.get_by_label(label);
    let node = node.accesskit_node();
    node.is_selected() == Some(true) || node.toggled() == Some(Toggled::True)
}

/// All that a screen reader can read.
fn all_labels(h: &H) -> String {
    (h.query_all(egui_kittest::kittest::by()))
        .filter_map(|n| n.accesskit_node().label().or(n.accesskit_node().value()))
        .collect::<Vec<_>>()
        .join(
            "
",
        )
}

fn a_doc_with_one_question() -> &'static str {
    ":::question\n\nA prompt?\n\n- [ ] Wrong one {#a1}\n- [x] Right one {#a2}\n- [ ] Wrong two {#a3}\n\n> Explained here.\n\n:::\n\nAfter.\n"
}

fn practising(doc: &str) -> (Fixture, H) {
    let f = fixture(doc);
    let mut h = f.ready();
    command(&mut h, "view.practiceAll");
    (f, h)
}

/// The tick and cross lines an item with this text draws, as how many answers say right and wrong.
fn verdict_counts(h: &mut H) -> (usize, usize) {
    let view = &mut h.state_mut().viewer().unwrap().ed.view;
    let (mut right, mut wrong) = (0, 0);
    for i in 0..view.len() {
        if let Some(markise_layout::PracticeItem::Answer { correct: Some(c), .. }) = view.practice_item_of(i) {
            if c {
                right += 1;
            } else {
                wrong += 1;
            }
        }
    }
    (right, wrong)
}

#[test]
fn a_whole_practice_session_leaves_the_bytes_and_the_dirty_state_as_they_were() {
    let doc = DOC;
    let (f, mut h) = practising(doc);
    let bytes = std::fs::read(&f.doc).unwrap();
    let state = h.state_mut().viewer().unwrap().ed.session.state_id();
    let check = |h: &mut H, when: &str| {
        assert_eq!(text(h).as_bytes(), bytes, "{when}: the text");
        let session = &h.state_mut().viewer().unwrap().ed.session;
        assert!(!session.is_dirty(), "{when}: dirty");
        assert!(!session.history().can_undo(), "{when}: an undo step");
        assert_eq!(session.state_id(), state, "{when}: another state of the document");
        assert_eq!(std::fs::read(&f.doc).unwrap(), bytes, "{when}: the file");
    };
    press(&mut h, "A) one");
    check(&mut h, "a click");
    press(&mut h, "B) two");
    check(&mut h, "a second click");
    // the keyboard, on the other question
    caret(&mut h, doc.find("Second").unwrap());
    h.event(Event::Text(" ".into()));
    common::run(&mut h);
    check(&mut h, "Space");
    for key in [Key::ArrowDown, Key::ArrowUp, Key::ArrowRight, Key::ArrowLeft] {
        h.key_press(key);
        common::run(&mut h);
    }
    check(&mut h, "arrows");
    h.event(Event::Text("not a space".into()));
    h.key_press(Key::Backspace);
    common::run(&mut h);
    check(&mut h, "typing");
    h.get_all_by_label("Check")
        .find(|b| !b.accesskit_node().is_disabled())
        .unwrap()
        .click();
    common::run(&mut h);
    check(&mut h, "Check");
    h.get_all_by_label("Show answer").next().unwrap().click();
    common::run(&mut h);
    check(&mut h, "Show answer");
    for _ in 0..3 {
        let Some(b) = h.query_all_by_label("Reset").next() else {
            break;
        };
        b.click();
        common::run(&mut h);
    }
    check(&mut h, "Reset");
    command(&mut h, "view.practiceAll");
    check(&mut h, "out of Practice");
}

#[test]
fn check_says_right_or_wrong_shows_the_correct_answer_and_reveals_and_reset_takes_it_back() {
    let doc = a_doc_with_one_question();
    let (f, mut h) = practising(doc);
    assert!(
        h.get_by_label("Check").accesskit_node().is_disabled(),
        "nothing to check yet"
    );
    press(&mut h, "Wrong one");
    assert!(is_selected(&h, "Wrong one"));
    press(&mut h, "Wrong two");
    assert!(
        is_selected(&h, "Wrong two") && !is_selected(&h, "Wrong one"),
        "one for single correctness"
    );
    assert!(!shown(&mut h).contains("Explained"));
    h.get_by_label("Check").click();
    common::run(&mut h);
    assert!(shown(&mut h).contains("Explained here"), "revealed");
    assert_eq!(verdict_counts(&mut h), (1, 2));
    assert!(
        h.query_by_label("Right one: correct").is_some(),
        "in words for a screen reader"
    );
    assert!(h.query_by_label("Wrong two: wrong").is_some());
    // what is checked stays until Reset
    press(&mut h, "Right one: correct");
    assert!(is_selected(&h, "Wrong two: wrong"));
    h.get_by_label("Reset").click();
    common::run(&mut h);
    assert!(!shown(&mut h).contains("Explained"));
    assert_eq!(verdict_counts(&mut h), (0, 0));
    assert!(!is_selected(&h, "Wrong two"));
    assert!(h.query_by_label("Check").is_some());
    untouched(&mut h, &f, doc);
}

#[test]
fn several_answers_can_be_selected_when_several_can_be_right() {
    let doc = ":::question many\n\nWhich?\n\n- [x] Aa {#a1}\n- [ ] Bb {#a2}\n- [x] Cc {#a3}\n\n:::\n";
    let (f, mut h) = practising(doc);
    press(&mut h, "Aa");
    press(&mut h, "Bb");
    press(&mut h, "Cc");
    assert!(is_selected(&h, "Aa") && is_selected(&h, "Bb") && is_selected(&h, "Cc"));
    press(&mut h, "Bb");
    assert!(!is_selected(&h, "Bb"), "and taken back");
    h.get_by_label("Check").click();
    common::run(&mut h);
    assert_eq!(verdict_counts(&mut h), (2, 1));
    untouched(&mut h, &f, doc);
}

#[test]
fn the_keyboard_alone_goes_through_a_question() {
    let doc = a_doc_with_one_question();
    let (f, mut h) = practising(doc);
    caret(&mut h, doc.find("Wrong one").unwrap());
    h.key_press(Key::ArrowDown);
    common::run(&mut h);
    let at = h
        .state_mut()
        .viewer()
        .unwrap()
        .ed
        .session
        .selection()
        .primary()
        .focus
        .offset;
    assert!(
        at >= doc.find("Right one").unwrap() && at <= doc.find("Wrong two").unwrap(),
        "the next answer: {at}"
    );
    h.event(Event::Text(" ".into()));
    common::run(&mut h);
    assert!(is_selected(&h, "Right one"));
    h.key_press(Key::Enter);
    common::run(&mut h);
    assert!(shown(&mut h).contains("Explained"), "Enter checks");
    assert_eq!(verdict_counts(&mut h), (1, 2));
    h.key_press(Key::Enter);
    common::run(&mut h);
    assert!(!shown(&mut h).contains("Explained"), "and on a checked block resets");
    untouched(&mut h, &f, doc);
}

#[test]
fn nothing_for_a_screen_reader_tells_an_answer_before_it_is_checked() {
    let (_f, h) = practising(a_doc_with_one_question());
    let labels = all_labels(&h);
    for telling in ["Explained", "correct", "wrong", "Mark exactly"] {
        assert!(!labels.contains(telling), "{telling}: {labels}");
    }
    for n in ["Wrong one", "Right one", "Wrong two"] {
        assert!(!is_selected(&h, n), "{n}");
    }
}

#[test]
fn a_block_with_no_answers_reveals_behind_show_answer_in_any_view_kind() {
    for (block, task, solution) in [
        ("exercise", "Work this out", "The worked solution"),
        ("hint", "A hint's task", "The hint's solution"),
        ("remark", "A remark's task", "The remark's solution"),
    ] {
        let doc = format!(":::{block}\n\n{task}.\n\n> {solution}.\n\n:::\n");
        let (f, mut h) = practising(&doc);
        let s = shown(&mut h);
        assert!(s.contains(task) && !s.contains(solution), "{block}: {s}");
        assert!(!all_labels(&h).contains(solution), "{block}: for a screen reader");
        assert!(h.query_by_label("Check").is_none(), "{block}");
        let before = h.state_mut().viewer().unwrap().ed.view.total_height();
        h.get_by_label("Show answer").click();
        common::run(&mut h);
        assert!(shown(&mut h).contains(solution), "{block}");
        let open = h.state_mut().viewer().unwrap().ed.view.total_height();
        assert!(open > before + 10.0, "{block}: {before} to {open}");
        h.get_by_label("Reset").click();
        common::run(&mut h);
        assert!(!shown(&mut h).contains(solution), "{block}");
        assert!(h.query_by_label("Show answer").is_some(), "{block}");
        let closed = h.state_mut().viewer().unwrap().ed.view.total_height();
        assert!((closed - before).abs() < 0.5, "{block}: {before} to {closed}");
        untouched(&mut h, &f, &doc);
    }
}

#[test]
fn enter_reveals_a_block_with_no_answers_too() {
    let doc = ":::exercise\n\nWork this out.\n\n> The worked solution.\n\n:::\n";
    let (f, mut h) = practising(doc);
    caret(&mut h, doc.find("Work").unwrap() + 2);
    h.key_press(Key::Enter);
    common::run(&mut h);
    assert!(shown(&mut h).contains("worked solution"));
    h.key_press(Key::Enter);
    common::run(&mut h);
    assert!(!shown(&mut h).contains("worked solution"));
    untouched(&mut h, &f, doc);
}

#[test]
fn an_answered_block_keeps_its_state_through_an_edit_elsewhere_and_loses_it_with_its_own() {
    let doc = format!(
        "{}\n{}",
        a_doc_with_one_question(),
        ":::tip\n\nA tip.\n\n:::\n\nThe end.\n"
    );
    let (_f, mut h) = practising(&doc);
    press(&mut h, "Right one");
    // above and below it, as anywhere
    caret(&mut h, doc.find("The end").unwrap());
    h.event(Event::Text("x".into()));
    common::run(&mut h);
    assert!(is_selected(&h, "Right one"), "after an edit below");
    caret(&mut h, 0);
    h.event(Event::Text("Before.\n\n".into()));
    common::run(&mut h);
    assert!(is_selected(&h, "Right one"), "after an edit above");
    // a change from outside: through the same door as any patch
    let t = text(&h);
    let at = t.find("Explained").unwrap();
    let v = h.state_mut().viewer().unwrap();
    let ps = markise_doc::PatchSet::single_verbatim(at..at, "Well ");
    v.ed.edit(|s| s.apply_external(&ps, "edit", false)).unwrap();
    common::run(&mut h);
    assert!(!is_selected(&h, "Right one"), "the block changed");
    assert!(
        h.state_mut()
            .viewer()
            .unwrap()
            .ed
            .practice_show(t.find(":::question").unwrap())
            .is_none()
    );
}

#[test]
fn two_blocks_with_the_same_text_are_two_blocks() {
    let q = a_doc_with_one_question();
    let doc = format!("{q}\n{q}");
    let (f, mut h) = practising(&doc);
    let first = doc.find(":::question").unwrap();
    let second = doc.rfind(":::question").unwrap();
    let v = h.state_mut().viewer().unwrap();
    assert!(v.ed.practice_press(doc[first..].find("Right one").unwrap() + first));
    common::run(&mut h);
    let v = h.state_mut().viewer().unwrap();
    assert!(v.ed.practice_show(first).is_some());
    assert!(v.ed.practice_show(second).is_none());
    untouched(&mut h, &f, &doc);
}

#[test]
fn replacing_the_document_ends_all_of_it() {
    let doc = a_doc_with_one_question();
    let (_f, mut h) = practising(doc);
    press(&mut h, "Right one");
    let v = h.state_mut().viewer().unwrap();
    let opener = 0;
    assert!(v.ed.practice_show(opener).is_some());
    let next = doc.replace("Explained", "Told");
    let format = v.ed.session.format().clone();
    let replaced = format.parse(next, &markise_doc::FormatOptions::default());
    v.ed.edit(|s| {
        s.replace_doc(
            replaced.into(),
            "Reload",
            markise_engine::editor::TxKind::Structural,
            false,
        )
    })
    .unwrap();
    common::run(&mut h);
    assert!(h.state_mut().viewer().unwrap().ed.practice_show(opener).is_none());
}

// ---- the document as a whole

fn score_label(h: &H) -> Option<String> {
    (h.query_all(egui_kittest::kittest::by()))
        .filter_map(|n| n.accesskit_node().label().or(n.accesskit_node().value()))
        .find(|l| {
            let w: Vec<&str> = l.split(' ').collect();
            w.len() == 3 && w[1] == "of" && w[0].parse::<u32>().is_ok() && w[2].parse::<u32>().is_ok()
        })
}

#[test]
fn the_score_counts_the_blocks_checked_and_reset_all_starts_over() {
    let q = |n: u32| {
        format!(":::question\n\nAsked {n}?\n\n- [ ] No {n} {{#a1}}\n- [x] Yes {n} {{#a2}}\n\n> Told {n}.\n\n:::\n\n")
    };
    let doc = format!("{}{}{}:::exercise\n\nWork.\n\n> Worked.\n\n:::\n", q(1), q(2), q(3));
    let (f, mut h) = practising(&doc);
    assert_eq!(score_label(&h), None, "nothing yet");
    assert!(h.query_by_label("Reset All").is_none());
    press(&mut h, "Yes 1");
    assert_eq!(score_label(&h), None, "selected, not checked");
    assert!(h.query_by_label("Reset All").is_some());
    let check = |h: &mut H| {
        h.get_all_by_label("Check")
            .find(|b| !b.accesskit_node().is_disabled())
            .unwrap()
            .click();
        common::run(h);
    };
    check(&mut h);
    assert_eq!(score_label(&h).as_deref(), Some("1 of 1"));
    press(&mut h, "No 2");
    check(&mut h);
    assert_eq!(score_label(&h).as_deref(), Some("1 of 2"));
    // a block with no answers counts for nothing
    h.get_by_label("Show answer").click();
    common::run(&mut h);
    assert_eq!(score_label(&h).as_deref(), Some("1 of 2"));
    let before = h.state_mut().viewer().unwrap().ed.view.total_height();
    h.get_by_label("Reset All").click();
    common::run(&mut h);
    assert_eq!(score_label(&h), None);
    assert!(h.query_by_label("Reset All").is_none());
    let s = shown(&mut h);
    for telling in ["Told 1", "Told 2", "Worked"] {
        assert!(!s.contains(telling), "{telling}: {s}");
    }
    assert!(h.state_mut().viewer().unwrap().ed.view.total_height() < before);
    untouched(&mut h, &f, &doc);
}

#[test]
fn the_example_package_can_be_practised_through_the_real_path() {
    let f = Fixture::example();
    let mut h = f.ready();
    let source = text(&h);
    assert!(shown(&mut h).contains("Yeast feeds") || source.contains("Yeast feeds"));
    command(&mut h, "view.practiceAll");
    h.state_mut().viewer().unwrap().ed.view.scroll_to_y(f64::MAX);
    common::run(&mut h);
    let s = shown(&mut h);
    assert!(s.contains("What makes the dough rise?"), "{s}");
    assert!(!s.contains("Yeast feeds"), "{s}");
    press(&mut h, "The salt");
    h.get_by_label("Check").click();
    common::run(&mut h);
    h.state_mut().viewer().unwrap().ed.view.scroll_to_y(f64::MAX);
    common::run(&mut h);
    assert!(shown(&mut h).contains("Yeast feeds"));
    assert_eq!(score_label(&h).as_deref(), Some("0 of 1"));
    assert_eq!(text(&h), source);
    assert!(!h.state_mut().viewer().unwrap().ed.session.is_dirty());
}

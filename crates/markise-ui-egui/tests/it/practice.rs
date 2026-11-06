//! Practice mode on the real app (ADR 0024), on a small neutral package: what hides, what is off, and
//! that nothing done here reaches the file.

use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
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
    Fixture::with_files(doc, FILES)
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

//! A package's text follows the document's language (ADR 0019), on a small neutral package with
//! English and Russian labels: on each surface, by the fallback order, as `lang` is edited, and
//! with no `lang` at all. The app's own words stay English.

use std::sync::Arc;
use std::time::Duration;

use egui::{Event, Key};
use egui_kittest::kittest::Queryable;
use markise_doc::{Patch, PatchSet, Selection};
use markise_ui_egui::{AppOptions, ConfigDir, MarkiseApp, ViewMode};

use crate::block_library::frames;
use crate::block_views::{Fixture, shown, text};
use crate::common;

type H = egui_kittest::Harness<'static, MarkiseApp>;

const MANIFEST: &str = r#"{"format":"markise-block-package/2","library":"sample","version":"1.0.0","blocks":["tip","only","memo","plate"]}"#;
const TIP: &str = r#"{"label": {"en": "Tip", "ru": "Совет"},
    "variants": {"": {"en": "plain", "ru": "обычный"}, "big": {"en": "large", "ru": "большой"}},
    "fields": [
        {"name": "title", "type": "heading", "level": 3, "min": 0, "label": {"en": "Title", "ru": "Заголовок"}},
        {"name": "body", "type": "text", "min": 1, "label": {"en": "Body", "ru": "Содержание"}}
    ]}"#;
// English alone: stays English whatever the document says
const ONLY: &str = r#"{"label": {"en": "Only English"}, "fields": [{"name": "body", "type": "text", "min": 1}]}"#;
// no English: the first language there is
const MEMO: &str = r#"{"label": {"ru": "Заметка"}, "fields": [{"name": "body", "type": "text", "min": 1}]}"#;
const PLATE: &str = r#"{"label": {"en": "Plate", "ru": "Табличка"},
    "argument": {"name": "set", "pattern": "^[a-z]+/[a-z]+$",
        "label": {"en": "Image set", "ru": "Набор картинок"}, "help": {"en": "Where they are", "ru": "Где они лежат"}},
    "fields": [{"name": "body", "type": "text", "min": 0}]}"#;
const VIEWS: &str = r#"{"tip": {"kind": "callout"}, "only": {"kind": "callout"}, "memo": {"kind": "callout"}, "plate": {"kind": "card"}}"#;
const PALETTE: &str = r#"{"format": 1,
    "groups": [{"id": "g", "label": {"en": "Teaching", "ru": "Обучение"}, "blocks": ["tip"]}],
    "blocks": {"tip": {"description": {"en": "A short hint", "ru": "Короткая подсказка"}}},
    "presets": [
        {"id": "tip", "block": "tip", "label": {"en": "Plain tip", "ru": "Простой совет"}, "markdown": ":::tip\n\nHello\n\n:::\n"},
        {"id": "only", "block": "only", "markdown": ":::only\n\nHello\n\n:::\n"}
    ]}"#;

const BLOCKS: &str = ":::tip\n\n### Heading\n\nHello\n\n:::\n\n:::only\n\nHello\n\n:::\n\n:::memo\n\nHello\n\n:::\n";

fn doc(lang: Option<&str>) -> String {
    match lang {
        Some(l) => format!("---\ntitle: T\nlang: {l}\n---\n\n{BLOCKS}\nAfter.\n"),
        None => format!("{BLOCKS}\nAfter.\n"),
    }
}

fn fixture(text: &str) -> Fixture {
    Fixture::with_package(
        text,
        &[
            ("manifest.json", MANIFEST),
            ("components/tip/tip.json", TIP),
            ("components/only/only.json", ONLY),
            ("components/memo/memo.json", MEMO),
            ("components/plate/plate.json", PLATE),
            ("editor/views.json", VIEWS),
            ("editor/palette.json", PALETTE),
        ],
    )
}

fn replace(h: &mut H, at: std::ops::Range<usize>, insert: &str) {
    let patches = PatchSet::new(vec![Patch::verbatim(at, insert.to_string())]).unwrap();
    let v = h.state_mut().viewer().unwrap();
    v.ed.edit(|s| s.apply_external(&patches, "Edit", false)).unwrap();
    common::run(h);
}

/// Changes what `lang` says.
fn set_lang(h: &mut H, value: &str) {
    let d = text(h);
    let start = d.find("lang: ").unwrap() + "lang: ".len();
    let end = start + d[start..].find('\n').unwrap();
    replace(h, start..end, value);
}

fn open_form(h: &mut H, needle: &str) {
    let at = text(h).find(needle).unwrap();
    assert!(h.state_mut().open_block_form(at));
    common::run(h);
}

fn labels_are(h: &mut H, tip: &str, only: &str, memo: &str) {
    let s = shown(h);
    for label in [tip, only, memo] {
        assert!(s.contains(label), "{label} in {s}");
    }
}

#[test]
fn the_card_label_is_in_the_document_s_language() {
    let f = fixture(&doc(Some("ru")));
    let mut h = f.ready();
    labels_are(&mut h, "Совет", "Only English", "Заметка");
    assert!(!shown(&mut h).contains("Tip"));
}

#[test]
fn no_lang_is_english_and_nothing_else_changes() {
    let f = fixture(&doc(None));
    let mut h = f.ready();
    labels_are(&mut h, "Tip", "Only English", "Заметка");
    open_form(&mut h, "Heading");
    assert_eq!(h.state().block_form().unwrap().title(), "Tip");
    assert!(h.query_by_label("Body").is_some());
}

#[test]
fn the_fallback_order_shows_on_screen() {
    for (lang, tip) in [
        ("ru", "Совет"),
        ("ru-RU", "Совет"),
        ("RU", "Совет"),
        ("de", "Tip"),
        ("de-DE", "Tip"),
        // not a language tag: as if there were none
        ("\"a b\"", "Tip"),
    ] {
        let f = fixture(&doc(Some(lang)));
        let mut h = f.ready();
        let s = shown(&mut h);
        assert!(s.contains(tip), "{lang}: {tip} in {s}");
        assert!(s.contains("Заметка"), "{lang}: the first there is, in {s}");
    }
}

#[test]
fn the_form_has_its_title_note_and_fields_in_the_language() {
    let f = fixture(&doc(Some("ru-RU")));
    let mut h = f.ready();
    open_form(&mut h, "Heading");
    assert_eq!(h.state().block_form().unwrap().title(), "Совет");
    assert!(h.query_by_label("обычный").is_some(), "the variant in words");
    assert!(h.query_by_label("Заголовок").is_some());
    assert!(h.query_by_label("Содержание").is_some());
    assert!(h.query_by_label("Save").is_some(), "the app's words stay English");
    assert!(h.query_by_label("Cancel").is_some());
}

#[test]
fn a_reason_names_the_field_in_the_language_in_the_app_s_words() {
    let d = "---\nlang: ru\n---\n\n:::tip\n\n### Heading\n\n:::\n";
    let f = fixture(d);
    let mut h = f.ready();
    let s = shown(&mut h);
    assert!(s.contains("Совет"), "{s}");
    // the field is the package's, what is wrong with it the app's
    assert!(s.contains("Содержание: Required"), "{s}");
}

#[test]
fn the_argument_field_and_its_reason_follow_the_language() {
    let d = "---\nlang: ru\n---\n\n:::plate\n\nHello\n\n:::\n";
    let f = fixture(d);
    let mut h = f.ready();
    // the argument's label is the package's, what is wrong with it the app's
    let s = shown(&mut h);
    assert!(s.contains("Набор картинок: Required"), "{s}");
    open_form(&mut h, "Hello");
    assert!(h.query_by_label("Набор картинок").is_some());
    assert!(h.query_by_label("Image set").is_none());
    assert!(h.query_by_label_contains("Где они лежат").is_some(), "its help");
    // live, with the form open and what was typed into the argument kept
    h.get_by_label("Набор картинок").click();
    common::run(&mut h);
    h.get_by_label("Набор картинок").type_text("some/set");
    common::run(&mut h);
    let draft = h.state().block_form().unwrap().values().unwrap().clone();
    set_lang(&mut h, "en");
    assert!(h.query_by_label("Image set").is_some());
    assert!(h.query_by_label("Набор картинок").is_none());
    assert!(h.query_by_label_contains("Where they are").is_some());
    assert_eq!(h.state().block_form().unwrap().values(), Some(&draft));
    let s = shown(&mut h);
    assert!(s.contains("Image set: Required"), "{s}");
    assert_eq!(h.state().block_form().unwrap().title(), "Plate");
}

#[test]
fn the_library_shows_the_language_and_finds_every_language() {
    let d = "---\nlang: ru\n---\n\nBefore.\n";
    let f = fixture(d);
    let mut h = f.ready();
    let end = d.find("Before.").unwrap() + "Before.".len();
    let v = h.state_mut().viewer().unwrap();
    v.ed.set_selection(Selection::caret_at(markise_doc::DocPos::with_virt(end, 1)));
    v.ed.refresh_focus();
    v.request_refocus();
    common::run(&mut h);
    h.event(Event::Text("/".to_string()));
    frames(&mut h);
    let titles = |h: &H| -> Vec<String> {
        let open = h.state().block_library().expect("the library is open");
        open.titles().into_iter().map(str::to_string).collect()
    };
    assert_eq!(titles(&h), ["Простой совет", "Only English"]);
    assert!(h.query_by_label("Обучение").is_some(), "the group");
    h.event(Event::Text("plain".to_string()));
    frames(&mut h);
    assert_eq!(titles(&h), ["Простой совет"], "an English word finds it");
    for _ in 0.."plain".len() {
        h.key_press(Key::Backspace);
    }
    h.event(Event::Text("подсказка".to_string()));
    frames(&mut h);
    assert_eq!(titles(&h), ["Простой совет"], "and so does a Russian one");
}

#[test]
fn the_label_of_a_dragged_block_is_in_the_language() {
    let d = "---\nlang: ru\n---\n\nBefore.\n\n:::tip\n\nHello\n\n:::\n";
    let f = fixture(d);
    let mut h = f.ready();
    crate::block_library::moves::pick_up(&mut h, "Hello");
    let drag = h
        .state_mut()
        .viewer()
        .unwrap()
        .block_drag_shown()
        .cloned()
        .expect("a drag shows");
    assert_eq!(
        drag.label, "Совет stays here",
        "the package's name in the app's sentence"
    );
}

#[test]
fn editing_lang_changes_every_label_with_no_reload() {
    let f = fixture(&doc(Some("en")));
    let mut h = f.ready();
    labels_are(&mut h, "Tip", "Only English", "Заметка");
    set_lang(&mut h, "ru");
    labels_are(&mut h, "Совет", "Only English", "Заметка");
    assert!(!shown(&mut h).contains("Tip\n"), "the old label is gone");
    set_lang(&mut h, "de");
    labels_are(&mut h, "Tip", "Only English", "Заметка");
    set_lang(&mut h, "ru-RU");
    labels_are(&mut h, "Совет", "Only English", "Заметка");
    // undo is an edit like any other
    let v = h.state_mut().viewer().unwrap();
    assert!(v.ed.edit(|s| s.undo()));
    common::run(&mut h);
    labels_are(&mut h, "Tip", "Only English", "Заметка");
    // the whole key going away
    let d = text(&h);
    let (start, end) = (d.find("lang: ").unwrap(), d.find("\n---").unwrap());
    replace(&mut h, start..end, "");
    assert_eq!(h.state_mut().viewer().unwrap().block_language(), "en");
}

#[test]
fn an_open_form_follows_and_keeps_its_draft_and_the_keyboard() {
    let f = fixture(&doc(Some("en")));
    let mut h = f.ready();
    open_form(&mut h, "Heading");
    h.get_by_label("Body").click();
    common::run(&mut h);
    h.get_by_label("Body").type_text("!");
    common::run(&mut h);
    let draft = h.state().block_form().unwrap().values().unwrap().clone();
    assert!(h.state().block_form().unwrap().is_dirty());
    set_lang(&mut h, "ru");
    let form = h.state().block_form().expect("the form stays");
    assert_eq!(form.title(), "Совет");
    assert_eq!(form.values(), Some(&draft), "nothing typed is lost");
    assert!(form.is_dirty());
    assert!(h.query_by_label("Body").is_none());
    assert!(h.query_by_label("Содержание").is_some());
    assert!(h.state().block_form().unwrap().message().is_none());
    h.get_by_label("Save").click();
    common::run(&mut h);
    assert!(text(&h).contains("Hello!"), "the draft still saves: {}", text(&h));
}

#[test]
fn a_keystroke_in_lang_asks_for_the_frame_that_shows_it() {
    let f = fixture(&doc(Some("en")));
    let mut h = f.ready();
    let d = text(&h);
    let start = d.find("lang: ").unwrap() + "lang: ".len();
    replace(&mut h, start..start + 2, "r");
    // the front matter takes a caret in source mode (qa_sweep a13 for the rendered view)
    h.state_mut().set_view_mode(ViewMode::Source);
    common::run(&mut h);
    assert_eq!(h.state_mut().viewer().unwrap().block_language(), "r");
    while h.state().source_caret().unwrap() > start + 1 {
        h.key_press(Key::ArrowLeft);
        h.step();
    }
    while h.state().source_caret().unwrap() < start + 1 {
        h.key_press(Key::ArrowRight);
        h.step();
    }
    h.event(Event::Text("u".to_string()));
    h.step();
    assert_eq!(text(&h)[start..start + 2], *"ru", "typed");
    // what a real event loop does: nothing more comes unless this frame asked for it
    assert!(h.ctx.has_requested_repaint(), "the edit asks for the next frame");
    h.step();
    assert_eq!(h.state_mut().viewer().unwrap().block_language(), "ru");
    h.state_mut().set_view_mode(ViewMode::Rendered);
    common::run(&mut h);
    labels_are(&mut h, "Совет", "Only English", "Заметка");
}

#[test]
fn an_edit_elsewhere_keeps_the_views_they_are_not_read_again() {
    let f = fixture(&doc(Some("ru")));
    let mut h = f.ready();
    let views = |h: &mut H| h.state_mut().viewer().unwrap().block_fields().map(Arc::as_ptr);
    let before = views(&mut h);
    let at = text(&h).find("After.").unwrap();
    replace(&mut h, at..at, "x");
    assert_eq!(views(&mut h), before);
    set_lang(&mut h, "en");
    assert_ne!(views(&mut h), before);
}

#[test]
fn at_rest_a_document_with_a_language_asks_for_no_frames() {
    let f = fixture(&doc(Some("ru")));
    // the package is accepted: the next opening uses it at once
    drop(f.ready());
    let ctx = egui::Context::default();
    let mut app = MarkiseApp::new(AppOptions {
        open: vec![f.doc.clone()],
        config: ConfigDir::Dir(f.config.clone()),
        ..common::no_watch()
    });
    // time passes between frames, so that a card's fade-in ends as it does in a window
    let clock = std::cell::Cell::new(0.0);
    let mut frame = |app: &mut MarkiseApp| {
        clock.set(clock.get() + 1.0);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 780.0))),
            time: Some(clock.get()),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| app.ui(ui));
        out.textures_delta.clear();
        let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
        out.viewport_output[&egui::ViewportId::ROOT].repaint_delay
    };
    let t = std::time::Instant::now();
    loop {
        let delays = common::idle_delays(&mut app, &mut frame, 6);
        if app.block_package().is_some() && delays.iter().rev().take(3).all(|d| *d == Duration::MAX) {
            break;
        }
        assert!(t.elapsed().as_secs() < 30, "{delays:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(app.viewer().unwrap().block_language(), "ru");
}

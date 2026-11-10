//! Text a package writes, wherever the app shows it (ADR 0019, #178): each piece stays one line, and
//! nothing in it hides or turns the text around, so a package cannot forge lines that look like the
//! application's. The application's own separators stay, and other scripts stay readable.

use egui::{Event, Key};
use egui_kittest::kittest::{NodeT, Queryable, by};
use markise_doc::{DocPos, Selection};

use crate::block_library::frames;
use crate::block_views::{Fixture, shown};
use crate::common;

type H = egui_kittest::Harness<'static, markise_ui_egui::MarkiseApp>;

/// As JSON writes it: a line break that would start a line looking like the status bar's, and a
/// direction control that turns what follows around.
macro_rules! hostile {
    () => {
        r"x\nBlocks: other 9.9.9 \u202e"
    };
}
/// The same as it shows.
const SHOWN: &str = r"x\u{a}Blocks: other 9.9.9 \u{202e}";
/// Russian with a zero-width joiner, which some scripts need: it shows as written.
const RUSSIAN: &str = "Совет\u{200d}дня";

/// Every label and value a screen reader gets.
fn said(h: &H) -> Vec<String> {
    (h.query_all(by()))
        .flat_map(|n| {
            let a = n.accesskit_node();
            [a.label(), a.value()]
        })
        .flatten()
        .collect()
}

/// Nothing anywhere holds the raw text: no forged line, no direction control.
fn nothing_raw(h: &H, also: &str) {
    for s in said(h).iter().map(String::as_str).chain([also]) {
        assert!(!s.contains('\u{202e}'), "a direction control in {s:?}");
        assert!(!s.contains("\nBlocks: other"), "a forged line in {s:?}");
    }
}

fn says(h: &H, text: &str) -> bool {
    said(h).iter().any(|s| s.contains(text))
}

fn type_text(h: &mut H, t: &str) {
    h.event(Event::Text(t.to_string()));
    frames(h);
}

fn free_line(h: &mut H, end: usize) {
    let v = h.state_mut().viewer().unwrap();
    v.ed.set_selection(Selection::caret_at(DocPos::with_virt(end, 1)));
    v.ed.refresh_focus();
    v.request_refocus();
    common::run(h);
}

const MANIFEST: &str = r#"{"format":"markise-block-package/2","library":"sample","version":"1.0.0","blocks":["tip"],"authoring":"blocks-v3"}"#;

const TIP: &str = concat!(
    r#"{"label": ""#,
    hostile!(),
    r#"", "fields": [{"name": "body", "type": "text", "min": 1, "label": "Совет\u200dдня "#,
    hostile!(),
    r#"", "help": ""#,
    hostile!(),
    r#""}]}"#
);
const VIEWS: &str = r#"{"tip": {"kind": "callout", "accent": "green"}}"#;

/// The library's rows, groups and details, and the banner of a preset that is refused.
#[test]
fn the_library_and_a_refused_preset_show_a_package_s_words_as_data() {
    const PALETTE: &str = concat!(
        r#"{"format": 1, "groups": [{"id": "g", "label": ""#,
        hostile!(),
        r#"", "blocks": ["tip"]}], "blocks": {"tip": {"description": ""#,
        hostile!(),
        r#""}}, "presets": [{"id": "bad", "block": "tip", "label": "Совет\u200dдня "#,
        hostile!(),
        r#"", "markdown": ":::tip\n\nnever closed\n"}]}"#
    );
    let doc = "Before.\n";
    let files = [
        ("manifest.json", MANIFEST.replace(r#","authoring":"blocks-v3""#, "")),
        ("components/tip/tip.json", TIP.to_string()),
        ("editor/palette.json", PALETTE.to_string()),
    ];
    let files: Vec<(&str, &str)> = files.iter().map(|(n, c)| (*n, c.as_str())).collect();
    let f = Fixture::with_package(doc, &files);
    let mut h = f.ready();
    free_line(&mut h, 7);
    type_text(&mut h, "/");
    assert!(h.state().block_library().is_some());
    let row = format!("{RUSSIAN} {SHOWN}");
    assert!(
        h.query_by_label(&row).is_some(),
        "the row, readable and on one line: {:?}",
        said(&h)
    );
    assert!(says(&h, SHOWN), "the group");
    nothing_raw(&h, "");

    h.key_press(Key::Enter);
    frames(&mut h);
    let banner = h.state().banner_text().unwrap_or_default();
    assert!(banner.contains(&format!("“{row}” was not inserted")), "{banner}");
    nothing_raw(&h, &banner);
}

/// The block's native view, its warning, and its form: the title, a field's label and help, and the
/// question about a changed draft.
#[test]
fn a_block_s_view_warning_and_form_show_a_package_s_words_as_data() {
    let doc = "Before.\n\n:::tip\n\n:::\n";
    let files = [
        ("manifest.json", MANIFEST.replace(r#","authoring":"blocks-v3""#, "")),
        ("components/tip/tip.json", TIP.to_string()),
        ("editor/palette.json", r#"{"format": 1}"#.to_string()),
        ("editor/views.json", VIEWS.to_string()),
    ];
    let files: Vec<(&str, &str)> = files.iter().map(|(n, c)| (*n, c.as_str())).collect();
    let f = Fixture::with_package(doc, &files);
    let mut h = f.ready();
    let view = shown(&mut h);
    assert!(view.contains(SHOWN), "the view's label: {view}");
    // the application's separator between the field and what is wrong stays
    let warning = format!("Not filled in yet: {RUSSIAN} {SHOWN}: Required");
    assert!(h.query_by_label(&warning).is_some(), "the warning: {:?}", said(&h));
    nothing_raw(&h, &view);

    h.get_all_by_label("Edit Block: tip").next().unwrap().click();
    common::run(&mut h);
    let form = h.state().block_form().expect("the form opens");
    assert_eq!(form.title(), SHOWN);
    assert!(
        says(&h, &format!("{RUSSIAN} {SHOWN}")),
        "the field's label: {:?}",
        said(&h)
    );
    let title = h.state().block_form().unwrap().title().to_string();
    nothing_raw(&h, &title);
}

/// A package's word for a section: on the divider, in the status bar, and in a new section's form,
/// with its fields and their values.
#[test]
fn a_section_s_word_and_fields_show_a_package_s_words_as_data() {
    const WORDS: &str = concat!(
        r#"{"format": 1, "label": ""#,
        hostile!(),
        r#"", "fields": [{"key": "style", "label": ""#,
        hostile!(),
        r#"", "values": [{"value": "wide", "label": "Совет\u200dдня "#,
        hostile!(),
        r#""}]}]}"#
    );
    let doc = "One.\n\n---\n\nTwo.\n";
    let files = [
        ("manifest.json", MANIFEST.to_string()),
        ("components/tip/tip.json", TIP.to_string()),
        ("editor/palette.json", r#"{"format": 1}"#.to_string()),
        ("editor/sections.json", WORDS.to_string()),
    ];
    let files: Vec<(&str, &str)> = files.iter().map(|(n, c)| (*n, c.as_str())).collect();
    let f = Fixture::with_package(doc, &files);
    let mut h = f.ready();
    let (name, about) = h.state_mut().viewer().unwrap().section_status(false).unwrap();
    assert_eq!(name, format!("§ {SHOWN} 1"));
    nothing_raw(&h, &format!("{name}\n{about}"));

    free_line(&mut h, doc.len() - 1);
    type_text(&mut h, "/");
    assert_eq!(h.state().block_library().unwrap().titles().len(), 1);
    h.key_press(Key::Enter);
    frames(&mut h);
    let form = h.state().section_form().expect("the form opens");
    assert_eq!(form.heading(), SHOWN);
    assert!(says(&h, SHOWN), "the field's label: {:?}", said(&h));
    let heading = h.state().section_form().unwrap().heading().to_string();
    nothing_raw(&h, &heading);
}

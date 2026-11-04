use egui::accesskit::Role;
use egui::{Event, ImeEvent, Key, Modifiers};
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

use crate::common::{harness, harness_with, hints, schema, value_of, values};

fn texts(h: &egui_kittest::Harness<'_, crate::common::S>, key: &str) -> Vec<String> {
    let v = value_of(h, key);
    (v.as_array().expect("array").iter())
        .map(|e| e.as_str().or_else(|| e["text"].as_str()).expect("text").to_string())
        .collect()
}

#[test]
fn typing_changes_the_value_and_reports_once_per_change() {
    let mut h = harness("single");
    h.run();
    h.get_by_label("Title").click();
    h.run();
    h.get_by_label("Title").type_text("!");
    h.run();
    assert_eq!(value_of(&h, "title"), json!("A title!"));
    assert_eq!(h.state().changes, 1);
    h.run();
    assert_eq!(h.state().changes, 1, "no change without an edit");
}

#[test]
fn no_edit_reports_no_change_and_leaves_missing_keys_missing() {
    let mut v = values();
    v.as_object_mut().expect("object").remove("body");
    let mut h = harness_with(&schema(), &hints("single"), v.clone());
    h.run();
    assert_eq!(h.state().changes, 0);
    assert_eq!(h.state().form.values(), &v);
}

#[test]
fn reorder_by_buttons_moves_values_and_keeps_the_caret_in_its_row() {
    let mut h = harness("single");
    h.run();
    h.get_by_label("Items 1").click();
    h.run();
    h.get_by_label("Items 1").type_text("X");
    h.run();
    assert_eq!(texts(&h, "items"), ["oneX", "two"]);

    h.get_by_label("Move down: Items 1").click();
    h.run();
    assert_eq!(texts(&h, "items"), ["two", "oneX"]);
    // the row travelled: it holds its focus state, typing goes on in the moved text
    h.get_by_label("Items 2").click();
    h.run();
    h.get_by_label("Items 2").type_text("Y");
    h.run();
    assert_eq!(texts(&h, "items")[1].len(), 5);
    assert!(texts(&h, "items")[1].contains('Y'));
}

#[test]
fn alt_arrows_reorder_from_inside_the_row_with_caret_and_focus() {
    let mut h = harness("single");
    h.run();
    h.get_by_label("Items 1").click();
    h.run();
    h.get_by_label("Items 1").type_text("X");
    h.run();
    h.key_press_modifiers(Modifiers::ALT, Key::ArrowDown);
    h.run();
    assert_eq!(texts(&h, "items"), ["two", "oneX"]);
    assert!(h.get_by_label("Items 2").is_focused(), "focus went with the row");
    assert!(h.query_by_label("Moved to position 2 of 2").is_some(), "announced");
    // the caret stayed after the typed X
    h.get_by_label("Items 2").type_text("Y");
    h.run();
    assert_eq!(texts(&h, "items"), ["two", "oneXY"]);
    assert!(
        h.query_by_label("Moved to position 2 of 2").is_none(),
        "that was the last news"
    );
    h.key_press_modifiers(Modifiers::ALT, Key::ArrowUp);
    h.run();
    assert_eq!(texts(&h, "items"), ["oneXY", "two"]);
    // at the edge nothing moves
    h.key_press_modifiers(Modifiers::ALT, Key::ArrowUp);
    h.run();
    assert_eq!(texts(&h, "items"), ["oneXY", "two"]);
}

#[test]
fn keyboard_alone_reaches_and_presses_the_buttons() {
    let mut h = harness("single");
    h.run();
    h.get_by_label("Items 1").click();
    h.run();
    // Tab from the text field: up is disabled on the first row, so down comes next
    h.key_press(Key::Tab);
    h.run();
    let down = h.get_by_label("Move down: Items 1");
    assert!(down.is_focused());
    h.key_press(Key::Space);
    h.run();
    assert_eq!(texts(&h, "items"), ["two", "one"]);
    // its button went disabled at the edge: the focus passed to the other one of the row
    assert!(h.get_by_label("Move up: Items 2").is_focused());
    h.key_press(Key::Space);
    h.run();
    assert_eq!(texts(&h, "items"), ["one", "two"]);
    assert!(h.get_by_label("Move down: Items 1").is_focused());
}

#[test]
fn add_and_remove_respect_the_limits_and_a_new_row_takes_the_focus() {
    let mut h = harness("single");
    h.run();
    h.get_by_label("Add item").click();
    h.run();
    assert_eq!(texts(&h, "items"), ["one", "two", ""]);
    assert!(h.get_by_label("Items 3").is_focused());
    h.get_by_label("Items 3").type_text("three");
    h.run();
    assert_eq!(texts(&h, "items"), ["one", "two", "three"]);
    assert!(
        h.get_by_label("Add item").accesskit_node().is_disabled(),
        "the limit is 3"
    );

    h.get_by_label("Remove: Items 3").click();
    h.run();
    assert!(
        h.get_by_label("Remove: Items 2").is_focused(),
        "the focus goes to the next row left"
    );
    h.get_by_label("Remove: Items 2").click();
    h.run();
    assert_eq!(texts(&h, "items"), ["one"]);
    assert!(
        h.get_by_label("Remove: Items 1").accesskit_node().is_disabled(),
        "the limit is 1"
    );
}

#[test]
fn a_new_answer_has_no_id_of_its_own_and_marks_stay_exclusive() {
    let mut h = harness("single");
    h.run();
    h.get_by_label("Add answer").click();
    h.run();
    assert_eq!(value_of(&h, "answers")[3], json!({"text": "", "correct": false}));
    h.get_by_label("Marked: Answers 1").click();
    h.run();
    let marks: Vec<bool> = (value_of(&h, "answers").as_array().expect("array").iter())
        .map(|a| a["correct"].as_bool().expect("bool"))
        .collect();
    assert_eq!(marks, [true, false, false, false]);
}

#[test]
fn checkboxes_mark_independently() {
    let mut h = harness("multiple");
    h.run();
    h.get_by_label("Marked: Answers 1").click();
    h.run();
    let marks: Vec<bool> = (value_of(&h, "answers").as_array().expect("array").iter())
        .map(|a| a["correct"].as_bool().expect("bool"))
        .collect();
    assert_eq!(marks, [true, true, false]);
}

#[test]
fn a_pressed_button_keeps_the_focus_when_its_row_moves() {
    let mut h = harness("multiple");
    h.run();
    h.get_by_label("Answers 2").focus();
    h.run();
    h.key_press(Key::Tab); // from the text of row 2 to its up button
    h.run();
    assert!(h.get_by_label("Move up: Answers 2").is_focused());
    h.key_press(Key::Tab); // and to its down button
    h.run();
    h.key_press(Key::Space);
    h.run();
    assert_eq!(texts(&h, "answers"), ["Red", "Blue", "Green"]);
    // row 3 has no down: passed to its up
    assert!(h.get_by_label("Move up: Answers 3").is_focused());
    h.key_press(Key::Space);
    h.run();
    assert_eq!(texts(&h, "answers"), ["Red", "Green", "Blue"]);
    assert!(
        h.get_by_label("Move up: Answers 2").is_focused(),
        "stays on the button of the row it moved with"
    );
}

#[test]
fn reorder_keeps_ids_with_their_answers() {
    let mut h = harness("single");
    h.run();
    h.get_by_label("Move up: Answers 3").click();
    h.run();
    let ids: Vec<String> = (value_of(&h, "answers").as_array().expect("array").iter())
        .map(|a| a["id"].as_str().expect("id").to_string())
        .collect();
    assert_eq!(ids, ["a1", "a3", "a2"]);
}

#[test]
fn errors_are_announced_and_go_away() {
    let mut h = harness("single");
    h.run();
    assert!(h.query_all_by_role(Role::Alert).next().is_none(), "the values fit");
    h.get_by_label("Title").click();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.key_press(Key::Backspace);
    h.run();
    let alert = h.get_by_role_and_label(Role::Alert, "Required");
    assert_eq!(alert.accesskit_node().live(), egui::accesskit::Live::Polite);
    assert_eq!(
        h.get_by_label("Title").accesskit_node().description().as_deref(),
        Some("Required")
    );
    assert_eq!(h.state().form.errors().len(), 1);
    assert_eq!(h.state().form.errors()[0].path, "title");

    h.get_by_label("Title").type_text("back");
    h.run();
    assert!(h.query_all_by_role(Role::Alert).next().is_none());
    assert!(h.state().form.errors().is_empty());
}

#[test]
fn a_correctness_error_shows_on_a_block_that_opened_broken() {
    let mut v = values();
    v["answers"][1]["correct"] = json!(false);
    let mut h = harness_with(&schema(), &hints("single"), v);
    h.run();
    assert!(h.query_by_label("Mark exactly one").is_some());
    h.get_by_label("Marked: Answers 2").click();
    h.run();
    assert!(h.query_by_label("Mark exactly one").is_none());
}

#[test]
fn an_ime_composition_survives_redraws_and_a_reorder_and_reports_only_its_end() {
    let mut h = harness("single");
    h.run();
    h.get_by_label("Items 1").click();
    h.run();
    h.event(Event::Ime(ImeEvent::Preedit {
        text: "n".into(),
        active_range_chars: Some(0..1),
    }));
    h.run();
    assert!(h.state().form.is_composing());
    assert_eq!(h.state().changes, 0, "an unfinished composition is no change");
    assert_eq!(texts(&h, "items")[0], "onen");
    h.run_steps(5);

    // the row moves in the middle of the composition; it goes on where it is
    h.key_press_modifiers(Modifiers::ALT, Key::ArrowDown);
    h.run();
    assert_eq!(texts(&h, "items"), ["two", "onen"]);
    assert!(h.state().form.is_composing());
    h.event(Event::Ime(ImeEvent::Preedit {
        text: "ni".into(),
        active_range_chars: Some(0..2),
    }));
    h.run();
    assert_eq!(texts(&h, "items")[1], "oneni");
    h.event(Event::Ime(ImeEvent::Commit("Nihon".into())));
    h.run();
    assert!(!h.state().form.is_composing());
    assert_eq!(texts(&h, "items"), ["two", "oneNihon"]);
    assert!(h.state().changes >= 1);
}

#[test]
fn values_set_from_outside_wait_for_the_composition_and_keep_rows() {
    let mut h = harness("single");
    h.run();
    h.get_by_label("Items 1").click();
    h.run();
    h.event(Event::Ime(ImeEvent::Preedit {
        text: "x".into(),
        active_range_chars: None,
    }));
    h.run();
    h.state_mut().form.set_values(values());
    h.run();
    assert_eq!(texts(&h, "items")[0], "onex", "not during the composition");
    h.event(Event::Ime(ImeEvent::Commit("y".into())));
    h.run();
    assert_eq!(texts(&h, "items")[0], "one", "after it");
}

#[test]
fn outside_values_keep_a_rows_focus_through_a_reorder() {
    let mut h = harness("single");
    h.run();
    h.get_by_label("Answers 2").click();
    h.run();
    let mut v = values();
    v["answers"].as_array_mut().expect("array").swap(0, 1);
    v["answers"][1]["text"] = json!("Emerald");
    h.state_mut().form.set_values(v);
    h.run();
    assert!(
        h.get_by_label("Answers 1").is_focused(),
        "a2 moved up and kept its focus"
    );
    assert_eq!(h.get_by_label("Answers 1").value().as_deref(), Some("Green"));
    assert_eq!(h.get_by_label("Answers 2").value().as_deref(), Some("Emerald"));
}

#[test]
fn a_multiline_field_scrolls_to_its_caret() {
    let long = (0..60).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
    let mut v = values();
    v["body"] = json!(long);
    let mut h = harness_with(&schema(), &hints("single"), v);
    h.run();
    let top = h.get_by_label("Body").rect().top();
    h.get_by_label("Body").focus();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.key_press(Key::ArrowRight);
    h.run();
    h.get_by_label("Body").type_text("!");
    h.run();
    assert!(value_of(&h, "body").as_str().expect("text").ends_with('!'));
    assert!(
        h.get_by_label("Body").rect().top() < top - 100.0,
        "the text scrolled up inside its area"
    );
    // the fields under it stayed reachable
    assert!(h.get_by_label("Items 1").rect().top() < 900.0);
}

#[test]
fn a_wrong_type_is_shown_and_kept() {
    let mut v = values();
    v["title"] = json!(5);
    v["items"] = json!({"no": "list"});
    let mut h = harness_with(&schema(), &hints("single"), v.clone());
    h.run();
    assert!(h.query_all_by_label("This value cannot be edited here").count() == 2);
    assert_eq!(h.state().form.values(), &v);
    assert_eq!(h.state().changes, 0);
}

#[test]
fn nested_rows_keep_their_state_through_a_reorder() {
    let mut h = harness("single");
    h.run();
    h.get_by_label("Move down: Rows 1").click();
    h.run();
    assert_eq!(
        value_of(&h, "rows"),
        json!([{"k": "y", "on": true}, {"k": "x", "on": false}])
    );
    h.get_by_label("Add row").click();
    h.run();
    assert_eq!(value_of(&h, "rows")[2], json!({"k": "", "on": false}));
}

#[test]
fn it_follows_zoom_and_theme_without_leaving_its_width() {
    for (zoom, theme) in [(1.0, egui::Theme::Light), (2.0, egui::Theme::Dark)] {
        let mut h = harness("single");
        h.ctx.set_theme(theme);
        h.ctx.set_zoom_factor(zoom);
        h.run();
        let width = 480.0 / zoom;
        for label in [
            "Title",
            "Items 1",
            "Answers 1",
            "Move down: Answers 1",
            "Remove: Items 1",
        ] {
            let r = h.get_by_label(label).rect();
            assert!(r.right() <= width + 1.0, "{label} at zoom {zoom}: {r:?}");
        }
        assert_eq!(h.ctx.global_style().visuals.dark_mode, theme == egui::Theme::Dark);
    }
}

#[test]
fn a_composition_belongs_to_the_form_that_has_the_focus() {
    use egui_schema_form::SchemaForm;
    use egui_schema_form::schema_form_core::FormSpec;
    let spec = || FormSpec::parse(&schema(), &hints("single"), None).expect("in the subset");
    struct Two {
        a: SchemaForm,
        b: SchemaForm,
    }
    let state = Two {
        a: SchemaForm::new(spec(), values(), "form a"),
        b: SchemaForm::new(spec(), json!({"title": "Other"}), "form b"),
    };
    let mut h = egui_kittest::Harness::builder()
        .with_size(egui::vec2(900.0, 900.0))
        .build_ui_state(
            |ui, s: &mut Two| {
                ui.columns(2, |c| {
                    s.a.ui(&mut c[0]);
                    s.b.ui(&mut c[1]);
                });
            },
            state,
        );
    h.run();
    h.get_all_by_label("Items 1").next().expect("in form a").focus();
    h.run();
    h.event(Event::Ime(ImeEvent::Preedit {
        text: "n".into(),
        active_range_chars: None,
    }));
    h.run();
    assert!(h.state().a.is_composing());
    assert!(!h.state().b.is_composing(), "the other form hears nothing of it");
    // and values for the other form do not wait
    h.state_mut().b.set_values(json!({"title": "Now"}));
    assert_eq!(h.state().b.values()["title"], json!("Now"));
    assert_eq!(h.state().a.values()["items"][0], json!("onen"));
    // focus going elsewhere ends it
    h.get_all_by_label("Title").nth(1).expect("in form b").focus();
    h.run();
    assert!(!h.state().a.is_composing());
}

#[test]
fn removing_down_to_the_limit_lands_on_an_enabled_control() {
    let mut h = harness("single");
    h.run();
    h.get_by_label("Remove: Items 2").click();
    h.run();
    assert_eq!(texts(&h, "items"), ["one"]);
    assert!(h.get_by_label("Remove: Items 1").accesskit_node().is_disabled());
    assert!(
        h.get_by_label("Add item").is_focused(),
        "not on the button that went disabled"
    );
    // and with Add at its own limit first: its id stays the same as rows come and go
    h.get_by_label("Add item").click();
    h.run();
    h.get_by_label("Add item").click();
    h.run();
    assert!(h.get_by_label("Add item").accesskit_node().is_disabled());
    h.get_by_label("Remove: Items 3").click();
    h.run();
    assert!(h.get_by_label("Remove: Items 2").is_focused());
}

#[test]
fn a_new_row_focuses_its_first_real_control() {
    let schema = json!({"type": "object", "properties": {
        "flags": {"type": "array", "items": {"type": "object", "properties": {
            "on": {"type": "boolean"}, "name": {"type": "string"},
        }}},
        "groups": {"type": "array", "items": {"type": "object", "properties": {
            "g": {"type": "object", "properties": {"inner": {"type": "string"}}},
        }}},
        "lists": {"type": "array", "items": {"type": "object", "properties": {
            "tags": {"type": "array", "items": {"type": "string"}},
        }}},
        "empty": {"type": "array", "items": {"type": "object", "properties": {}}},
    }});
    let hints = json!({"version": 1, "fields": {
        "flags": {"add_label": "Add flag", "order": ["on", "name"], "fields": {"on": {"label": "Enabled"}}},
        "groups": {"add_label": "Add group", "fields": {"g": {"fields": {"inner": {"label": "Inner"}}}}},
        "lists": {"add_label": "Add list", "fields": {"tags": {"label": "Tags", "add_label": "Add tag"}}},
        "empty": {"label": "Bare", "add_label": "Add bare"},
    }});
    let mut h = harness_with(&schema, &hints, json!({}));
    h.run();
    h.get_by_label("Add flag").click();
    h.run();
    assert!(h.get_by_label("Enabled").is_focused(), "a checkbox");
    h.get_by_label("Add group").click();
    h.run();
    assert!(h.get_by_label("Inner").is_focused(), "inside a group");
    h.get_by_label("Add list").click();
    h.run();
    assert!(h.get_by_label("Add tag").is_focused(), "a nested array's add");
    h.get_by_label("Add bare").click();
    h.run();
    assert!(h.get_by_label("Remove: Bare 1").is_focused(), "nothing else in the row");
    // an answer's first real control is its text, not its mark
    let mut h = harness("single");
    h.run();
    h.get_by_label("Add answer").click();
    h.run();
    assert!(h.get_by_label("Answers 4").is_focused());
}

#[test]
fn an_entrys_reason_shows_and_a_wrong_type_inside_an_entry_is_kept() {
    let mut v = values();
    v["answers"][0]["text"] = json!(42);
    v["answers"][1]["correct"] = json!("yes");
    v["answers"][2]["text"] = json!("");
    let mut h = harness_with(&schema(), &hints("single"), v.clone());
    h.run();
    assert_eq!(h.query_all_by_label("This value cannot be edited here").count(), 2);
    assert!(h.query_by_label("Required").is_some(), "the empty text of answer 3");
    // marking another answer does not touch a mark it cannot read
    h.get_by_label("Marked: Answers 3").click();
    h.run();
    assert_eq!(value_of(&h, "answers")[0]["text"], json!(42));
    assert_eq!(value_of(&h, "answers")[1]["correct"], json!("yes"));
    assert_eq!(value_of(&h, "answers")[2]["correct"], json!(true));
}

#[test]
fn a_checkbox_over_a_wrong_type_keeps_it() {
    let mut v = values();
    v["rows"][0]["on"] = json!("yes");
    let mut h = harness_with(&schema(), &hints("single"), v);
    h.run();
    assert!(h.query_by_label("This value cannot be edited here").is_some());
    assert_eq!(value_of(&h, "rows")[0]["on"], json!("yes"));
}

#[test]
fn moving_to_another_field_ends_a_composition() {
    let mut h = harness("single");
    h.run();
    h.get_by_label("Items 1").focus();
    h.run();
    h.event(Event::Ime(ImeEvent::Preedit {
        text: "n".into(),
        active_range_chars: None,
    }));
    h.run();
    assert!(h.state().form.is_composing());
    h.get_by_label("Title").focus();
    h.run();
    assert!(
        !h.state().form.is_composing(),
        "another editor of the same form has the focus"
    );
    let reported = h.state().changes;
    assert!(reported >= 1, "what the composition left is reported");
    h.get_by_label("Title").type_text("!");
    h.run();
    assert_eq!(value_of(&h, "title"), json!("A title!"));
    assert_eq!(h.state().changes, reported + 1, "later edits are reported");
    // a new composition there is its own
    h.event(Event::Ime(ImeEvent::Preedit {
        text: "x".into(),
        active_range_chars: None,
    }));
    h.run();
    assert!(h.state().form.is_composing());
}

#[test]
fn a_hosts_accent_gives_its_fills_and_readable_text() {
    use egui_schema_form::Style;
    let light_accent = egui::Color32::from_rgb(0xf0, 0xe0, 0x60);
    let dark_accent = egui::Color32::from_rgb(0x10, 0x20, 0x60);
    let s = Style::DARK.with_accent(light_accent);
    assert_eq!(s.accent, light_accent);
    assert_eq!(s.on_accent, egui::Color32::BLACK);
    assert_ne!(s.accent_soft, Style::DARK.accent_soft);
    assert_ne!(s.accent_soft, Style::DARK.field, "a marked row stands out");
    assert_ne!(s.hover, s.accent_soft);
    assert_eq!(Style::LIGHT.with_accent(dark_accent).on_accent, egui::Color32::WHITE);
}

#[test]
fn a_checkbox_field_still_answers_to_mouse_and_keyboard() {
    let schema = json!({"type": "object", "properties": {"on": {"type": "boolean"}}});
    let hints = json!({"version": 2, "fields": {"on": {"label": "On"}}});
    let mut h = harness_with(&schema, &hints, json!({"on": false}));
    h.run();
    assert_eq!(h.get_by_label("On").accesskit_node().role(), Role::CheckBox);
    h.get_by_label("On").click();
    h.run();
    assert_eq!(value_of(&h, "on"), json!(true));
    h.get_by_label("On").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    assert_eq!(value_of(&h, "on"), json!(false));
}

fn long_text() -> String {
    "A rather long piece of text that cannot fit on one line of a narrow form at all ".repeat(4)
}

fn line_field_harness(text: &str) -> egui_kittest::Harness<'static, crate::common::S> {
    let schema = json!({"type": "object", "properties": {"title": {"type": "string"}}});
    let hints = json!({"version": 2, "fields": {"title": {"label": "Title"}}});
    harness_with(&schema, &hints, json!({"title": text}))
}

#[test]
fn a_one_line_field_grows_with_its_text() {
    let mut short = line_field_harness("Short");
    short.run();
    let low = short
        .get_by_label("Title")
        .accesskit_node()
        .bounding_box()
        .expect("box")
        .height();
    let mut long = line_field_harness(&long_text());
    long.run();
    let high = long
        .get_by_label("Title")
        .accesskit_node()
        .bounding_box()
        .expect("box")
        .height();
    assert!(high > low * 2.0, "{low} {high}");
}

#[test]
fn enter_in_a_one_line_field_keeps_it_one_line_and_leaves_the_field() {
    let mut h = line_field_harness("Short");
    h.run();
    h.get_by_label("Title").click();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(value_of(&h, "title"), json!("Short"), "no line break");
    assert!(
        !h.get_by_label("Title").accesskit_node().is_focused(),
        "as a one-line field does"
    );
}

#[test]
fn a_paste_into_a_one_line_field_has_no_line_breaks() {
    let mut h = line_field_harness("Short");
    h.run();
    h.get_by_label("Title").click();
    h.run();
    h.event(Event::Paste(" one\ntwo\r\nthree".into()));
    h.run();
    assert_eq!(value_of(&h, "title"), json!("Short one two  three"));
}

#[test]
fn a_long_row_wraps_and_keeps_its_caret_through_a_reorder() {
    let long = long_text();
    let v = json!({"title": "", "items": [long, "two"]});
    let mut h = harness_with(&schema(), &hints("single"), v);
    h.run();
    h.get_by_label("Items 1").click();
    h.run();
    h.get_by_label("Items 1").type_text("X");
    h.run();
    h.key_press_modifiers(Modifiers::ALT, Key::ArrowDown);
    h.run();
    h.get_by_label("Items 2").type_text("Y");
    h.run();
    assert!(texts(&h, "items")[1].contains('Y'), "the caret stayed in its row");
    assert!(texts(&h, "items")[1].contains('X'));
}

#[test]
fn a_long_checkbox_label_stays_inside_a_narrow_form() {
    let long = "A checkbox label far too long to fit on one line of this narrow form";
    let schema = json!({"type": "object", "properties": {"on": {"type": "boolean"}}});
    let hints = json!({"version": 2, "fields": {"on": {"label": long}}});
    let mut h = harness_with(&schema, &hints, json!({"on": false}));
    h.set_size(egui::vec2(160.0, 400.0));
    h.run();
    let b = h.get_by_label(long).accesskit_node().bounding_box().expect("box");
    assert!(b.x1 <= 160.0, "{b:?}");
    assert!(b.height() > 30.0, "it wrapped: {b:?}");
}

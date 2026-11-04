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
    assert_eq!(
        value_of(&h, "answers")[3],
        json!({"id": "", "text": "", "correct": false})
    );
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

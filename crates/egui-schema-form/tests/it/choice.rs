//! Choice buttons: icons and labels from data, exclusive options, keyboard and screen reader.

use egui::accesskit::{Role, Toggled};
use egui::{ColorImage, Key, TextureHandle, TextureOptions, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use egui_schema_form::schema_form_core::{FormSpec, SPEC_VERSION};
use egui_schema_form::{FileRequest, FormHost, SchemaForm, Thumbnail};
use serde_json::{Value, json};

#[derive(Default)]
struct Host {
    asked: Vec<String>,
    texture: Option<TextureHandle>,
}

impl FormHost for Host {
    fn request_file(&mut self, _: FileRequest, _: &str) {}
    fn thumbnail(&mut self, _: &str) -> Thumbnail {
        Thumbnail::None
    }
    fn icon(&mut self, name: &str) -> Thumbnail {
        self.asked.push(name.to_string());
        match (&self.texture, name) {
            (_, "broken") => Thumbnail::Failed("cannot read it".into()),
            (Some(t), _) => Thumbnail::Mask {
                texture: t.id(),
                size: vec2(8.0, 8.0),
            },
            _ => Thumbnail::Loading,
        }
    }
}

struct S {
    form: SchemaForm,
    host: Host,
    changes: usize,
}

fn spec(selection: &str) -> FormSpec {
    let schema = json!({"type": "object", "properties": {
        "where": {"type": "array", "minItems": 1, "items": {"type": "object", "properties": {
            "id": {"type": "string"}, "text": {"type": "string"}, "correct": {"type": "boolean"},
        }}},
        "tone": {"type": "string", "enum": ["soft", "hard"]},
        "tags": {"type": "array", "items": {"type": "string", "enum": ["x", "y", "off"]}},
    }});
    let hints = json!({"version": SPEC_VERSION, "fields": {
        "where": {"label": "Where", "widget": "buttons", "selection": selection, "columns": 2, "options": [
            {"value": "Left", "label": "To the left", "icon": "left"},
            {"value": "Right", "icon": "broken"},
            {"value": "Nowhere", "exclusive": true},
        ]},
        "tone": {"label": "Tone", "options": [{"value": "soft", "label": "Soft"}]},
        "tags": {"label": "Tags", "options": [{"value": "off", "label": "Off", "exclusive": true}]},
    }});
    FormSpec::parse(&schema, &hints, None).expect("in the subset")
}

fn values() -> Value {
    json!({
        "where": [
            {"id": "a1", "text": "Left", "correct": true},
            {"id": "a2", "text": "Right", "correct": false},
            {"id": "a3", "text": "Nowhere", "correct": false},
        ],
        "tone": "soft",
        "tags": ["x"],
    })
}

fn harness(spec: FormSpec, values: Value) -> Harness<'static, S> {
    let state = S {
        form: SchemaForm::new(spec, values, "choice"),
        host: Host::default(),
        changes: 0,
    };
    Harness::builder().with_size(vec2(420.0, 600.0)).build_ui_state(
        |ui, s: &mut S| {
            if s.host.texture.is_none() {
                let image = ColorImage::filled([8, 8], egui::Color32::WHITE);
                s.host.texture = Some(ui.ctx().load_texture("icon", image, TextureOptions::LINEAR));
            }
            s.changes += usize::from(s.form.ui_with(ui, &mut s.host).changed);
        },
        state,
    )
}

fn marks(h: &Harness<'_, S>) -> Vec<bool> {
    (h.state().form.values()["where"].as_array().expect("array").iter())
        .map(|e| e["correct"].as_bool().expect("bool"))
        .collect()
}

#[test]
fn the_buttons_carry_the_labels_and_icons_of_the_data() {
    let mut h = harness(spec("multiple"), values());
    h.run();
    assert_eq!(h.get_by_label("To the left").accesskit_node().role(), Role::CheckBox);
    assert_eq!(h.get_by_label("Right").accesskit_node().role(), Role::CheckBox);
    assert_eq!(h.get_by_label("Soft").accesskit_node().role(), Role::RadioButton);
    assert_eq!(h.get_by_label("hard").accesskit_node().role(), Role::RadioButton);
    let asked = &h.state().host.asked;
    assert!(asked.contains(&"left".to_string()) && asked.contains(&"broken".to_string()));
    assert_eq!(h.state().changes, 0);
}

#[test]
fn pressing_marks_and_an_exclusive_option_clears_the_others() {
    let mut h = harness(spec("multiple"), values());
    h.run();
    assert_eq!(
        h.get_by_label("To the left").accesskit_node().toggled(),
        Some(Toggled::True)
    );
    h.get_by_label("Right").click();
    h.run();
    assert_eq!(marks(&h), [true, true, false]);
    assert_eq!(h.state().changes, 1);
    assert!(h.state().form.errors().is_empty());

    h.get_by_label("Nowhere").click();
    h.run();
    assert_eq!(marks(&h), [false, false, true]);

    h.get_by_label("To the left").click();
    h.run();
    assert_eq!(marks(&h), [true, false, false]);
    assert_eq!(
        h.state().form.values()["where"][0]["id"],
        json!("a1"),
        "the row stays what it was"
    );
}

#[test]
fn one_selection_acts_as_radio_buttons() {
    let mut h = harness(spec("single"), values());
    h.run();
    assert_eq!(h.get_by_label("Right").accesskit_node().role(), Role::RadioButton);
    h.get_by_label("Right").click();
    h.run();
    assert_eq!(marks(&h), [false, true, false]);
    h.get_by_label("Right").click();
    h.run();
    assert_eq!(marks(&h), [false, true, false], "pressed again, it stays");
}

#[test]
fn strings_and_a_string_hold_the_choice_too() {
    let mut h = harness(spec("multiple"), values());
    h.run();
    h.get_by_label("hard").click();
    h.run();
    assert_eq!(h.state().form.values()["tone"], json!("hard"));
    h.get_by_label("y").click();
    h.run();
    assert_eq!(h.state().form.values()["tags"], json!(["x", "y"]));
    h.get_by_label("Off").click();
    h.run();
    assert_eq!(h.state().form.values()["tags"], json!(["off"]));
}

#[test]
fn the_keyboard_reaches_and_presses_a_button_and_the_focus_stays() {
    let mut h = harness(spec("multiple"), values());
    h.run();
    h.get_by_label("To the left").focus();
    h.run();
    h.key_press(Key::Tab);
    h.run();
    assert!(h.get_by_label("Right").accesskit_node().is_focused());
    h.key_press(Key::Space);
    h.run();
    assert_eq!(marks(&h), [true, true, false]);
    assert!(h.get_by_label("Right").accesskit_node().is_focused());
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(marks(&h), [true, false, false]);
    h.key_press(Key::Tab);
    h.run();
    assert!(h.get_by_label("Nowhere").accesskit_node().is_focused());
}

#[test]
fn a_row_of_no_option_is_shown_kept_and_can_be_removed() {
    let mut v = values();
    v["where"]
        .as_array_mut()
        .expect("array")
        .push(json!({"id": "a4", "text": "Sideways", "correct": true}));
    let mut h = harness(spec("multiple"), v);
    h.run();
    let reasons = h.state().form.errors().to_vec();
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert_eq!(reasons[0].path, "where/3");
    let _ = h.get_by_label("Sideways: Not one of the options");

    h.get_by_label("Right").click();
    h.run();
    assert_eq!(
        h.state().form.values()["where"][3]["text"],
        json!("Sideways"),
        "untouched"
    );

    h.get_by_label("Remove: Sideways").click();
    h.run();
    assert_eq!(h.state().form.values()["where"].as_array().expect("array").len(), 3);
    assert!(h.state().form.errors().is_empty());
}

#[test]
fn a_missing_option_gets_a_row_only_when_selected() {
    let v = json!({"where": [{"id": "a2", "text": "Right", "correct": true}]});
    let mut h = harness(spec("multiple"), v);
    h.run();
    h.get_by_label("To the left").click();
    h.run();
    assert_eq!(
        h.state().form.values()["where"],
        json!([{"text": "Left", "correct": true}, {"id": "a2", "text": "Right", "correct": true}])
    );
    assert!(h.state().form.errors().is_empty(), "the host gives it an id");
}

#[test]
fn an_exclusive_option_with_others_in_the_values_has_a_reason_and_the_form_mends_it() {
    let v = json!({"where": [
        {"id": "a1", "text": "Left", "correct": true},
        {"id": "a3", "text": "Nowhere", "correct": true},
    ]});
    let mut h = harness(spec("multiple"), v);
    h.run();
    let _ = h.get_by_label("`Nowhere` cannot go with the others");
    h.get_by_label("Nowhere").click();
    h.run();
    assert_eq!(h.state().form.values()["where"][1]["correct"], json!(false));
    assert!(h.state().form.errors().is_empty());
}

#[test]
fn a_value_of_another_type_is_kept() {
    let v = json!({"tone": 3, "tags": "x", "where": []});
    let mut h = harness(spec("multiple"), v.clone());
    h.run();
    assert_eq!(h.state().form.values(), &v);
    assert_eq!(h.state().changes, 0);
}

#[test]
fn a_narrow_panel_holds_fewer_buttons_per_row() {
    let spec = spec("multiple");
    let state = S {
        form: SchemaForm::new(spec, values(), "narrow"),
        host: Host::default(),
        changes: 0,
    };
    let mut h = Harness::builder().with_size(vec2(90.0, 600.0)).build_ui_state(
        |ui, s: &mut S| {
            s.form.ui_with(ui, &mut s.host);
        },
        state,
    );
    h.run();
    let left = h
        .get_by_label("To the left")
        .accesskit_node()
        .bounding_box()
        .expect("box");
    let right = h.get_by_label("Right").accesskit_node().bounding_box().expect("box");
    assert!(right.y0 >= left.y1, "one per row: {left:?} {right:?}");
}

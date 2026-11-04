//! Images and host widgets: everything slow or site-specific comes through the host.

use std::cell::RefCell;
use std::rc::Rc;

use egui::accesskit::Role;
use egui::{Ui, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use egui_schema_form::schema_form_core::FormSpec;
use egui_schema_form::{FileRequest, FormHost, FormOutput, HostWidget, SchemaForm, Thumbnail, WidgetContext};
use serde_json::{Value, json};

#[derive(Default)]
struct Host {
    requests: Vec<(FileRequest, String)>,
    asked: Vec<String>,
    answer: Option<Thumbnail>,
}

impl FormHost for Host {
    fn request_file(&mut self, request: FileRequest, field_label: &str) {
        self.requests.push((request, field_label.to_string()));
    }
    fn thumbnail(&mut self, path: &str) -> Thumbnail {
        self.asked.push(path.to_string());
        self.answer.clone().unwrap_or(Thumbnail::None)
    }
}

struct S {
    form: SchemaForm,
    host: Host,
    changes: usize,
}

fn image_spec() -> FormSpec {
    let schema = json!({
        "type": "object",
        "properties": {"pictures": {"type": "array", "maxItems": 3, "items": {"type": "object", "properties": {
            "alt": {"type": "string"},
            "path": {"type": "string"},
        }}}},
    });
    let hints = json!({"version": 1, "fields": {"pictures": {
        "label": "Pictures", "widget": "images", "add_label": "Add picture",
    }}});
    FormSpec::parse(&schema, &hints, None).expect("in the subset")
}

fn harness(spec: FormSpec, values: Value) -> Harness<'static, S> {
    let state = S {
        form: SchemaForm::new(spec, values, "media"),
        host: Host::default(),
        changes: 0,
    };
    Harness::builder().with_size(vec2(520.0, 700.0)).build_ui_state(
        |ui, s: &mut S| {
            let out: FormOutput = s.form.ui_with(ui, &mut s.host);
            s.changes += usize::from(out.changed);
        },
        state,
    )
}

fn two_pictures() -> Value {
    json!({"pictures": [{"alt": "first", "path": "a.png"}, {"alt": "second", "path": "b.png"}]})
}

#[test]
fn the_picker_only_asks_and_the_answer_goes_to_the_row_wherever_it_is() {
    let mut h = harness(image_spec(), two_pictures());
    h.run();
    h.get_by_label("Choose file: Pictures 1").click();
    h.run();
    assert_eq!(h.state().host.requests.len(), 1, "asked once");
    assert_eq!(h.state().host.requests[0].1, "Pictures 1");
    assert_eq!(h.state().changes, 0, "nothing changes until the answer");

    // the row moves while the dialog is open
    h.get_by_label("Move down: Pictures 1").click();
    h.run();
    assert_eq!(h.state().changes, 1);
    let request = h.state().host.requests[0].0;
    h.state_mut().form.deliver_file(request, Some("chosen.png".into()));
    h.run();
    assert_eq!(
        h.state().form.values(),
        &json!({"pictures": [{"alt": "second", "path": "b.png"}, {"alt": "first", "path": "chosen.png"}]})
    );
    assert_eq!(h.state().changes, 2, "the answer is a change");
}

#[test]
fn a_cancelled_or_late_answer_changes_nothing() {
    let mut h = harness(image_spec(), two_pictures());
    h.run();
    h.get_by_label("Choose file: Pictures 1").click();
    h.run();
    h.get_by_label("Choose file: Pictures 2").click();
    h.run();
    let (first, second) = (h.state().host.requests[0].0, h.state().host.requests[1].0);
    h.state_mut().form.deliver_file(first, None);
    h.run();
    assert_eq!(h.state().form.values(), &two_pictures());
    assert_eq!(h.state().changes, 0);

    h.get_by_label("Remove: Pictures 2").click();
    h.run();
    h.state_mut().form.deliver_file(second, Some("late.png".into()));
    h.run();
    assert_eq!(
        h.state().form.values(),
        &json!({"pictures": [{"alt": "first", "path": "a.png"}]})
    );
    assert_eq!(h.state().changes, 1, "only the removal");
    // the same token again means nothing
    h.state_mut().form.deliver_file(second, Some("again.png".into()));
    h.run();
    assert_eq!(h.state().changes, 1);
}

#[test]
fn alt_and_path_edit_as_text_and_a_new_picture_is_empty() {
    let mut h = harness(image_spec(), two_pictures());
    h.run();
    h.get_by_label("Alt text: Pictures 2").focus();
    h.run();
    h.get_by_label("Alt text: Pictures 2").type_text("!");
    h.run();
    assert_eq!(h.state().form.values()["pictures"][1]["alt"], json!("second!"));
    h.get_by_label("Path: Pictures 2").focus();
    h.run();
    h.get_by_label("Path: Pictures 2").type_text("x");
    h.run();
    assert_eq!(h.state().form.values()["pictures"][1]["path"], json!("b.pngx"));
    h.get_by_label("Add picture").click();
    h.run();
    assert_eq!(h.state().form.values()["pictures"][2], json!({"alt": "", "path": ""}));
    assert!(h.get_by_label("Alt text: Pictures 3").is_focused());
    assert!(
        h.get_by_label("Add picture").accesskit_node().is_disabled(),
        "the limit is 3"
    );
}

#[test]
fn thumbnails_come_from_the_host_in_all_their_states() {
    let mut v = two_pictures();
    v["pictures"]
        .as_array_mut()
        .expect("array")
        .push(json!({"alt": "no file yet", "path": ""}));
    let mut h = harness(image_spec(), v);
    h.state_mut().host.answer = Some(Thumbnail::Loading);
    // a spinner keeps repainting: steps, not a run to rest
    h.run_steps(2);
    assert!(h.state().host.asked.contains(&"a.png".to_string()));
    assert!(
        !h.state().host.asked.iter().any(String::is_empty),
        "no file, no question"
    );
    assert_eq!(
        h.query_all_by_role_and_label(Role::ProgressIndicator, "Loading")
            .count(),
        2
    );

    h.state_mut().host.answer = Some(Thumbnail::Failed("No such file".into()));
    h.run();
    assert_eq!(h.query_all_by_label("No such file").count(), 2);
    assert_eq!(h.state().changes, 0, "a failed picture is no edit, and the path stays");

    h.state_mut().host.answer = Some(Thumbnail::Ready {
        texture: egui::TextureId::default(),
        size: vec2(64.0, 32.0),
    });
    h.run();
    assert!(
        h.query_by_role_and_label(Role::Image, "first").is_some(),
        "the alt text names the picture"
    );
    assert_eq!(h.query_all_by_label("No such file").count(), 0);
}

#[test]
fn without_a_host_there_is_no_picker_and_the_row_still_edits() {
    let state = SchemaForm::new(image_spec(), two_pictures(), "plain");
    let mut h = Harness::builder().with_size(vec2(520.0, 700.0)).build_ui_state(
        |ui, f: &mut SchemaForm| {
            f.ui(ui);
        },
        state,
    );
    h.run();
    assert!(h.query_by_label("Choose file: Pictures 1").is_none());
    assert!(h.query_by_label("Alt text: Pictures 1").is_some());
}

/// The test's widget: only here, the crate has none.
struct Counter {
    drawn: Rc<RefCell<Vec<String>>>,
}

impl HostWidget for Counter {
    fn ui(&mut self, ui: &mut Ui, cx: WidgetContext<'_>) -> bool {
        self.drawn
            .borrow_mut()
            .push(format!("{}:{}", cx.label, cx.errors.join(",")));
        let n = cx.value.as_u64().unwrap_or(0);
        if ui.button(format!("Count {n} of {}", cx.label)).clicked() {
            *cx.value = json!(n + 1);
            return true;
        }
        false
    }
}

fn custom_spec(name: &str) -> FormSpec {
    let schema = json!({
        "type": "object",
        "required": ["count"],
        "properties": {"title": {"type": "string"}, "count": {}},
    });
    let hints = json!({"version": 1, "order": ["title", "count"], "fields": {
        "title": {"label": "Title"},
        "count": {"label": "Times", "widget": "custom", "custom": name},
    }});
    FormSpec::parse(&schema, &hints, None).expect("in the subset")
}

#[test]
fn a_registered_widget_is_drawn_by_name_and_owns_its_value() {
    let mut h = harness(custom_spec("counter"), json!({"title": "t", "count": 2}));
    let drawn = Rc::new(RefCell::new(Vec::new()));
    h.state_mut()
        .form
        .register_widget("counter", Counter { drawn: drawn.clone() });
    h.run();
    h.get_by_label("Count 2 of Times").click();
    h.run();
    assert_eq!(h.state().form.values(), &json!({"title": "t", "count": 3}));
    assert_eq!(h.state().changes, 1);
    assert_eq!(drawn.borrow().last().map(String::as_str), Some("Times:"));
    assert!(h.query_by_label("Count 3 of Times").is_some());
}

#[test]
fn a_host_widget_hears_its_reasons() {
    let mut h = harness(custom_spec("counter"), json!({"title": "t"}));
    let drawn = Rc::new(RefCell::new(Vec::new()));
    h.state_mut()
        .form
        .register_widget("counter", Counter { drawn: drawn.clone() });
    h.run();
    assert_eq!(drawn.borrow().last().map(String::as_str), Some("Times:Required"));
    assert!(h.query_by_label("Required").is_some());
    assert_eq!(
        h.state().form.values(),
        &json!({"title": "t"}),
        "not written until it changes"
    );
}

#[test]
fn an_unknown_widget_says_so_and_keeps_the_value() {
    let v = json!({"title": "t", "count": {"anything": [1, 2]}});
    let mut h = harness(custom_spec("nobody-registered-this"), v.clone());
    h.run();
    assert!(
        h.query_by_label("This field needs a widget the application does not have")
            .is_some()
    );
    h.get_by_label("Title").focus();
    h.run();
    h.get_by_label("Title").type_text("!");
    h.run();
    assert_eq!(
        h.state().form.values()["count"],
        v["count"],
        "another field's edit keeps it"
    );
}

#[test]
fn a_wrong_type_in_an_image_row_is_shown_and_kept() {
    let schema = json!({"type": "object", "properties": {"pictures": {"type": "array", "items": {
        "type": "object",
        "properties": {"alt": {"type": "string"}, "path": {"type": "string"}},
    }}}});
    let hints = json!({"version": 1, "fields": {"pictures": {"label": "Pictures", "widget": "images"}}});
    let spec = FormSpec::parse(&schema, &hints, None).expect("in the subset");
    let v = json!({"pictures": [{"alt": 7, "path": ["x"]}, {"alt": 9, "path": "ok.png"}]});
    let mut h = harness(spec, v.clone());
    h.run();
    assert_eq!(h.query_all_by_label("This value cannot be edited here").count(), 3);
    assert_eq!(
        h.query_all_by_label("The value has the wrong type").count(),
        3,
        "the reasons show"
    );
    assert!(
        h.query_by_label("Alt text: Pictures 1").is_none(),
        "no empty box to type over"
    );
    assert!(h.query_by_label("Path: Pictures 1").is_none());
    assert!(
        h.query_by_label("Choose file: Pictures 1").is_none(),
        "a file would overwrite it"
    );
    assert!(
        !h.state().host.asked.iter().any(String::is_empty),
        "nothing to show for it"
    );

    // the other part of a row still edits, and the bad one stays
    h.get_by_label("Path: Pictures 2").focus();
    h.run();
    h.get_by_label("Path: Pictures 2").type_text("x");
    h.run();
    assert_eq!(
        h.state().form.values(),
        &json!({"pictures": [{"alt": 7, "path": ["x"]}, {"alt": 9, "path": "ok.pngx"}]})
    );
}

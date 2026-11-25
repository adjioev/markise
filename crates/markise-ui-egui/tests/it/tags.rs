//! Tags in the real app (spec 06 §10, ADR 0027): the Tags view of the sidebar lists the
//! folder's tags with their notes, its field searches `tag:x`, a click on a note opens it,
//! and Cmd/Ctrl+click on a tag pill in the document shows that tag's notes.

use crate::common;

use std::path::{Path, PathBuf};

use egui::{Event, Key, Modifiers, PointerButton};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_ui_egui::{AppOptions, MarkiseApp};

type H = Harness<'static, MarkiseApp>;

fn folder(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-tags-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("Sub")).unwrap();
    let d = std::fs::canonicalize(&d).unwrap();
    std::fs::write(d.join("Home.md"), "# Home\n\nToday: #idea and #project/alpha.\n").unwrap();
    std::fs::write(
        d.join("Sub/Plan.md"),
        "---\ntags: [Idea, travel]\n---\n# Plan\n\nText.\n",
    )
    .unwrap();
    std::fs::write(d.join("Sub/Code.md"), "Only `#code` here.\n").unwrap();
    d
}

fn harness(dir: &Path) -> H {
    let opts = AppOptions {
        open: vec![dir.to_path_buf(), dir.join("Home.md")],
        ..common::no_watch()
    };
    Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), MarkiseApp::new(opts))
}

fn command() -> Modifiers {
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

fn indexed(h: &mut H) {
    common::wait_until(h, "the tag index", |a| {
        a.vault_index().is_some_and(|ix| ix.tags().is_complete()) && a.document_path().is_some()
    });
    common::settle(h);
}

#[test]
fn tag_list_search_and_open() {
    let dir = folder("list");
    let mut h = harness(&dir);
    indexed(&mut h);
    h.get_by_label("Tags").click();
    h.step();
    // every tag with its number of notes; nested tags under their parent; none from code
    common::wait_for_label(&mut h, "#idea (2 notes)");
    h.get_by_label("#project (1 note)");
    h.get_by_label("#project/alpha (1 note)");
    h.get_by_label("#travel (1 note)");
    assert!(h.query_by_label("#code (1 note)").is_none());
    // a click lists the notes with the tag; a click on one opens it
    h.get_by_label("#idea (2 notes)").click();
    h.step();
    assert_eq!(h.state().tags_view_state().0, Some("idea"));
    common::wait_for_label(&mut h, "Home.md");
    h.get_by_label("Sub/Plan.md").click();
    common::wait_until(&mut h, "Plan opened", |a| {
        a.document_path().is_some_and(|p| p.ends_with("Sub/Plan.md"))
    });
    // the field searches: `tag:#project` and Enter lists that tag's notes
    h.get_by_label("← All tags").click();
    common::wait_for_label(&mut h, "#travel (1 note)");
    h.get_by_label("Find").click();
    h.step();
    h.event(Event::Text("tag:#project".into()));
    h.step();
    assert!(h.query_by_label("#travel (1 note)").is_none(), "filtered");
    h.key_press(Key::Enter);
    h.step();
    assert_eq!(h.state().tags_view_state().0, Some("project"));
    common::wait_for_label(&mut h, "Home.md");
    assert!(h.query_by_label("Sub/Plan.md").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cmd_click_on_a_tag_pill_lists_its_notes() {
    let dir = folder("pill");
    let mut h = harness(&dir);
    indexed(&mut h);
    let target = format!("{}project/alpha", markise_layout::TAG_LINK_PREFIX);
    let pos = h.state().link_center(&target).expect("the tag is drawn as a pill");
    let m = command();
    h.hover_at(pos);
    h.event(Event::ModifiersChanged(m));
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: m,
        });
    }
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
    h.step();
    assert_eq!(h.state().tags_view_state().0, Some("project/alpha"));
    common::wait_for_label(&mut h, "Home.md");
    // the source is untouched
    assert_eq!(
        std::fs::read_to_string(dir.join("Home.md")).unwrap(),
        "# Home\n\nToday: #idea and #project/alpha.\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

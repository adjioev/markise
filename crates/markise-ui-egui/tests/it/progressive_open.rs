//! Progressive open (issue #126) through the real UI: a large file shows its first part at
//! once, the loading state is only in the status bar (the document never shifts), find and the
//! outline say they cover the loaded part, and when the whole document arrives the content on
//! screen stays where it is with the edits made meanwhile; then the word count runs on the
//! whole document.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};

use egui::{Event, Modifiers, MouseWheelSource, MouseWheelUnit, TouchPhase};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_engine::load::{ProgressiveOpts, Stage, StageHook};
use markise_engine::search::Query;
use markise_ui_egui::{AppOptions, MarkiseApp};

use crate::common;

type H = Harness<'static, MarkiseApp>;

/// Holds the loader's rest thread before it parses until released.
type Gate = Arc<(Mutex<bool>, Condvar)>;

fn hook(gate: &Gate) -> StageHook {
    let g = gate.clone();
    Arc::new(move |st, _| {
        if st == Stage::Parse && std::thread::current().name() == Some("markise-load-rest") {
            let (m, cv) = &*g;
            let mut held = m.lock().unwrap();
            while *held {
                held = cv.wait(held).unwrap();
            }
        }
    })
}

fn file() -> (PathBuf, String) {
    let d = std::env::temp_dir().join(format!("markise-progressive-open-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let p = std::fs::canonicalize(&d).unwrap().join("big.md");
    let text = markise_corpus::generate(48 << 10, 21);
    std::fs::write(&p, &text).unwrap();
    (p, text)
}

#[test]
fn the_first_part_is_shown_and_the_whole_document_replaces_it_in_place() {
    let (p, text) = file();
    let gate: Gate = Arc::new((Mutex::new(true), Condvar::new()));
    let mut h: H = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(
            |ui, app: &mut MarkiseApp| app.ui(ui),
            MarkiseApp::new(AppOptions {
                open: vec![p.clone()],
                progressive: Some(ProgressiveOpts {
                    min_bytes: 16 << 10,
                    prefix_bytes: 4 << 10,
                }),
                load_hook: Some(hook(&gate)),
                ..common::no_watch()
            }),
        );
    common::wait_until(&mut h, "the first part", |a| a.is_partial());
    let shown = h.state().document_text().unwrap();
    assert!(shown.len() < 16 << 10 && text.starts_with(&shown));
    assert!(
        h.query_by_label("Loading the rest of the file…").is_some(),
        "status bar note"
    );
    assert!(
        h.query_by_label_contains("words").is_none(),
        "no word count of the part"
    );
    let top = h.state_mut().viewer().unwrap().surface_rect().unwrap();

    // read and edit meanwhile: scroll a little, type near the top
    let r = top;
    h.input_mut().events.push(Event::PointerMoved(r.center()));
    for _ in 0..4 {
        h.input_mut().events.push(Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -80.0),
            modifiers: Modifiers::NONE,
            phase: TouchPhase::Move,
            source: MouseWheelSource::Unknown,
        });
        h.step();
    }
    let at = shown.find("\n\n").unwrap() + 2;
    {
        let v = h.state_mut().viewer().unwrap();
        v.ed.set_caret(markise_doc::DocPos::new(at), false);
        v.ed.edit(|s| s.insert_text("XYZ")).unwrap();
    }
    // find covers the loaded part, and says so
    h.state_mut().open_find(false);
    h.state_mut().set_find_query(Query {
        text: "XYZ".into(),
        ..Default::default()
    });
    common::wait_until(&mut h, "find results", |a| a.find_results().is_some());
    assert!(h.query_by_label("Loaded part").is_some());
    h.state_mut().close_find();
    for _ in 0..3 {
        h.step();
    }
    let before = h.state_mut().viewer().unwrap().painted().to_vec();
    let rect_before = h.state_mut().viewer().unwrap().surface_rect().unwrap();
    assert!(!before.is_empty());

    {
        let (m, cv) = &*gate;
        *m.lock().unwrap() = false;
        cv.notify_all();
    }
    common::wait_until(&mut h, "the whole document", |a| !a.is_loading());
    h.step();
    let after = h.state_mut().viewer().unwrap().painted().to_vec();
    assert_eq!(after.first(), before.first(), "the content on screen did not move");
    assert_eq!(
        h.state_mut().viewer().unwrap().surface_rect().unwrap(),
        rect_before,
        "the document area did not shift"
    );
    let mut want = text.clone();
    want.insert_str(at, "XYZ");
    assert_eq!(h.state().document_text().unwrap(), want, "the edit was carried over");
    assert!(h.state().is_dirty() && !h.state().is_partial());
    assert!(h.query_by_label("Loading the rest of the file…").is_none());
    // the word count starts on the whole document
    let t = std::time::Instant::now();
    while h.query_by_label_contains("words").is_none() {
        assert!(t.elapsed() < common::HANG_GUARD, "no word count");
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(
        h.query_by_label_contains("words").is_some(),
        "the word count of the whole document"
    );
    // the edit undoes
    let v = h.state_mut().viewer().unwrap();
    assert!(v.ed.edit(|s| s.undo()));
    assert_eq!(h.state().document_text().unwrap(), text);
}

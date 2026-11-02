//! Issue #133: Select All on a large document must not freeze the UI. The menus, the toolbar,
//! the context menu and shortcut dispatch ask every format command whether it is enabled (and
//! checked) every frame; that used to dry-run the command over the whole selection — a full
//! inline parse of the document per command per frame. Enablement is now memoised per
//! (document, selection) state and decided without planning for large selections.

use std::time::{Duration, Instant};

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use markise_ui_egui::{AppOptions, MarkiseApp};

type H = Harness<'static, MarkiseApp>;

fn open(name: &str, src: &str) -> H {
    let d = std::env::temp_dir().join(format!("markise-selectall-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let f = d.join("doc.md");
    std::fs::write(&f, src).unwrap();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(
            |ui, app: &mut MarkiseApp| app.ui(ui),
            MarkiseApp::new(AppOptions {
                open: vec![f],
                ..Default::default()
            }),
        );
    let t = Instant::now();
    while h.state().model().is_none() || h.state().is_busy() {
        assert!(t.elapsed() < Duration::from_secs(300), "document did not load");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(3);
    h
}

fn cmd() -> Modifiers {
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

fn planner_runs(h: &mut H) -> u64 {
    h.state_mut().viewer().unwrap().ed.session.plan_queries_run()
}

/// Selects everything with ⌘A / Ctrl+A, then checks that the following frames run no planner
/// (enablement and check state come from memory) and stay within `budget` each; returns the
/// slowest frame.
fn check_select_all(size: usize, budget: Duration) -> Duration {
    let big = markise_corpus::generate(size, 7);
    let mut h = open(&format!("{size}"), &big);
    // focus the document, then Select All
    let p = egui::pos2(500.0, 300.0);
    h.drag_at(p);
    h.step();
    h.drop_at(p);
    h.step();
    let t = Instant::now();
    h.key_press_modifiers(cmd(), Key::A);
    h.step();
    let first = t.elapsed();
    let sel = h.state_mut().viewer().unwrap().ed.session.selection().primary().range();
    // the whole document (the editable content: front matter and trailing blank lines aside)
    assert!(
        sel.len() + 256 >= big.len(),
        "Select All selected {sel:?} of {}",
        big.len()
    );
    h.run_steps(2);
    let runs = planner_runs(&mut h);
    let mut slowest = Duration::ZERO;
    for _ in 0..10 {
        let t = Instant::now();
        h.step();
        slowest = slowest.max(t.elapsed());
    }
    assert_eq!(
        planner_runs(&mut h),
        runs,
        "steady frames answer enablement from memory"
    );
    eprintln!(
        "select all in {} KiB: first frame {first:?}, slowest of 10 later frames {slowest:?}",
        big.len() >> 10
    );
    assert!(first <= budget, "the Select All frame took {first:?}");
    assert!(slowest <= budget, "a frame after Select All took {slowest:?}");
    // the menus: the inline format toggles are disabled above the format limit, Hyperlink
    // still works (it acts on the first block only)
    let bar = h.state().menu_model();
    let enabled = |id: &str| bar.item(id).unwrap_or_else(|| panic!("{id}")).enabled;
    let huge = big.len() > markise_md::FORMAT_SELECTION_LIMIT;
    assert_eq!(enabled("format.strong"), !huge);
    assert!(enabled("format.link"));
    if huge {
        // ⌘B / Ctrl+B is refused at once (the document is not changed)
        let t = Instant::now();
        h.key_press_modifiers(cmd(), Key::B);
        h.step();
        let bold = t.elapsed();
        assert_eq!(h.state_mut().viewer().unwrap().ed.session.text(), big, "Bold refused");
        assert!(bold <= budget, "the Bold frame took {bold:?}");
    }
    slowest
}

/// A selection above the exact-query limit (enablement decided without planning).
#[test]
fn select_all_answers_menu_enablement_from_memory() {
    // debug build, loaded CI machine: a generous bound; the release budget is the heavy test's
    check_select_all(4 * markise_md::EXACT_QUERY_LIMIT, Duration::from_secs(2));
}

/// Issue #133 goal: Select All in a multi-megabyte document keeps every frame within budget
/// (it took seconds per frame: every format command dry-ran over the whole selection).
#[test]
#[ignore = "heavy: large input or exhaustive matrix; cargo xtask heavy"]
fn heavy_suite_select_all_in_a_large_document_keeps_frames_within_budget() {
    // above the format limit (toggles refused) and below it (toggles planned on demand only)
    check_select_all(8 << 20, Duration::from_millis(100));
    check_select_all(3 << 20, Duration::from_millis(100));
}

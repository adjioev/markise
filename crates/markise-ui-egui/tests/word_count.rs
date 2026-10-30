//! Issue #53: the status bar shows the document's word count and reading time, the selection's
//! count ("12 of 3,456 words") while text is selected; a click cycles words / characters /
//! characters without spaces (persisted as `statusBar.count`); Preferences → General → "Show
//! word count" hides it (persisted as `statusBar.showWordCount`). Typing in a 10 MB document
//! never recounts the document on the UI thread.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use markise_doc::Selection;
use markise_engine::settings::CountMode;
use markise_ui_egui::{AppOptions, ConfigDir, MarkiseApp};

type H = Harness<'static, MarkiseApp>;

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-wordcount-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn harness(open: Vec<PathBuf>, config: Option<&Path>) -> H {
    Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(
            |ui, app: &mut MarkiseApp| app.ui(ui),
            MarkiseApp::new(AppOptions {
                open,
                config: config.map_or(ConfigDir::None, |c| ConfigDir::Dir(c.to_path_buf())),
                ..Default::default()
            }),
        )
}

fn wait_for(h: &mut H, what: &str, limit: Duration, f: impl Fn(&mut H) -> bool) {
    let t = Instant::now();
    while !f(h) {
        if t.elapsed() >= limit {
            let labels: Vec<String> = h
                .query_all_by_label_contains("·")
                .filter_map(|n| n.accesskit_node().value().or(n.accesskit_node().label()))
                .filter(|l| l.contains('·') || l.contains("Count"))
                .collect();
            panic!("timed out waiting for {what}; status bar labels: {labels:?}");
        }
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.step();
}

fn shows(h: &H, label: &str) -> bool {
    h.query_by_label(label).is_some()
}

fn wait_label(h: &mut H, label: &str) {
    let l = label.to_string();
    wait_for(h, label, Duration::from_secs(20), move |h| shows(h, &l));
}

fn open(name: &str, src: &str, config: Option<&Path>) -> H {
    let d = tmpdir(name);
    let f = d.join("doc.md");
    std::fs::write(&f, src).unwrap();
    let mut h = harness(vec![f], config);
    wait_for(&mut h, "load", Duration::from_secs(20), |h| {
        h.state().model().is_some() && !h.state().is_busy()
    });
    h
}

fn select(h: &mut H, a: usize, b: usize) {
    h.state_mut().viewer().unwrap().ed.set_selection(Selection::range(a, b));
    h.run_steps(2);
}

const DOC: &str = "# A title\n\nHello **brave** new [world](https://example.com).\n\n```\nnot counted as words\n```\n";

#[test]
fn status_bar_shows_document_and_selection_counts() {
    let mut h = open("text", DOC, None);
    // "A title" + "Hello brave new world." — the code block counts characters only
    wait_label(&mut h, "6 words · 1 min read");

    // a selection shows its own count: "brave new" (markers inside it are not counted)
    let a = DOC.find("brave").unwrap();
    let b = DOC.find(" [world").unwrap();
    select(&mut h, a, b);
    wait_label(&mut h, "2 of 6 words");
    // the whole document
    select(&mut h, 0, DOC.len());
    wait_label(&mut h, "6 of 6 words");
    // a caret again: the document count
    select(&mut h, 3, 3);
    wait_label(&mut h, "6 words · 1 min read");
}

#[test]
fn clicking_the_count_cycles_and_persists_it() {
    let cfg = tmpdir("cycle-config");
    let mut h = open("cycle", "One two three.\n", Some(&cfg));
    wait_label(&mut h, "3 words · 1 min read");
    h.get_by_label("3 words · 1 min read").click();
    h.run_steps(2);
    assert_eq!(h.state().count_mode(), CountMode::Characters);
    wait_label(&mut h, "14 characters · 1 min read");
    h.get_by_label("14 characters · 1 min read").click();
    h.run_steps(2);
    wait_label(&mut h, "12 characters without spaces · 1 min read");
    let settings = cfg.join("markise").join("settings.json");
    wait_for(&mut h, "statusBar.count saved", Duration::from_secs(20), |_| {
        std::fs::read_to_string(&settings).is_ok_and(|s| s.contains("\"statusBar.count\": \"charactersNoSpaces\""))
    });
    h.get_by_label("12 characters without spaces · 1 min read").click();
    h.run_steps(2);
    wait_label(&mut h, "3 words · 1 min read");
    assert_eq!(h.state().count_mode(), CountMode::Words);
    wait_for(&mut h, "statusBar.count saved", Duration::from_secs(20), |_| {
        std::fs::read_to_string(&settings).is_ok_and(|s| s.contains("\"statusBar.count\": \"words\""))
    });
    drop(h);

    // a new session reads the saved mode
    std::fs::write(
        &settings,
        std::fs::read_to_string(&settings)
            .unwrap()
            .replace("\"statusBar.count\": \"words\"", "\"statusBar.count\": \"characters\""),
    )
    .unwrap();
    let d = tmpdir("cycle2");
    let f = d.join("doc.md");
    std::fs::write(&f, "One two three.\n").unwrap();
    let mut h = harness(vec![f], Some(&cfg));
    wait_label(&mut h, "14 characters · 1 min read");
}

#[test]
fn show_word_count_switch_hides_it_and_persists() {
    let cfg = tmpdir("toggle-config");
    let mut h = open("toggle", "One two three.\n", Some(&cfg));
    assert!(h.state().show_word_count(), "on by default");
    assert!(h.state().preference_values().show_word_count);
    wait_label(&mut h, "3 words · 1 min read");

    h.state_mut().set_show_word_count(false);
    h.run_steps(2);
    assert!(!shows(&h, "3 words · 1 min read"), "hidden when switched off");
    assert!(h.query_by_label_contains("words").is_none());
    assert!(
        h.state().word_count_stats().is_none(),
        "nothing is counted while hidden"
    );
    assert!(!h.state().preference_values().show_word_count);
    let settings = cfg.join("markise").join("settings.json");
    wait_for(&mut h, "statusBar.showWordCount saved", Duration::from_secs(20), |_| {
        std::fs::read_to_string(&settings).is_ok_and(|s| s.contains("\"statusBar.showWordCount\": false"))
    });

    // edits while hidden are counted when it comes back
    h.state_mut().set_show_word_count(true);
    wait_label(&mut h, "3 words · 1 min read");
}

#[test]
fn preferences_general_has_the_show_word_count_switch() {
    let row = markise_ui_egui::prefs::ROWS
        .iter()
        .find(|r| r.label == "Show word count")
        .expect("Preferences row");
    assert_eq!(row.section, markise_ui_egui::prefs::Section::General);
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

/// Issue #29 goal: typing in a 10 MB document does not recount it on the UI thread. The first
/// count runs on a worker; after typing, only the edited items are counted (counted through
/// the counter's statistics seam) and no new worker job starts.
#[test]
fn typing_in_a_ten_megabyte_document_recounts_only_the_edited_items() {
    let big = markise_corpus::generate(10 << 20, 7);
    let mut h = open("big", &big, None);
    let t = Instant::now();
    wait_for(&mut h, "first count", Duration::from_secs(300), |h| {
        h.query_by_label_contains("min read").is_some()
    });
    eprintln!("10 MB: first count (worker) shown after {:?}", t.elapsed());
    let (before, jobs) = h.state().word_count_stats().expect("counting");
    assert_eq!(jobs, 1, "the first count of a large document runs on a worker");
    assert!(
        before.items_counted < 300,
        "the UI thread counted only a few items: {before:?}"
    );
    let label = |h: &H| {
        h.query_by_label_contains("min read")
            .and_then(|n| n.accesskit_node().value().or(n.accesskit_node().label()))
            .unwrap_or_default()
    };
    let shown = label(&h);

    // type a few words at the document end
    let p = egui::pos2(600.0, 300.0);
    h.drag_at(p);
    h.step();
    h.drop_at(p);
    h.step();
    if cfg!(target_os = "macos") {
        h.key_press_modifiers(cmd(), Key::ArrowDown);
    } else {
        h.key_press_modifiers(cmd(), Key::End);
    }
    h.step();
    let t = Instant::now();
    for c in "\n\nzebra quokka okapi".chars() {
        h.event(Event::Text(c.to_string()));
        h.step();
    }
    let typing = t.elapsed();
    let (during, _) = h.state().word_count_stats().unwrap();
    assert!(
        during.items_counted - before.items_counted <= 2,
        "no counting per keystroke: {before:?} → {during:?}"
    );
    // after the debounce, only the edited items are counted
    std::thread::sleep(Duration::from_millis(400));
    wait_for(&mut h, "updated count", Duration::from_secs(20), |h| label(h) != shown);
    let (after, jobs) = h.state().word_count_stats().unwrap();
    assert_eq!(jobs, 1, "no worker recount after typing");
    assert!(
        after.items_counted - before.items_counted <= 8,
        "only the edited items were counted: {before:?} → {after:?}"
    );
    eprintln!(
        "10 MB: typed 19 chars in {typing:?}; counted {} items after typing",
        after.items_counted - before.items_counted
    );
}

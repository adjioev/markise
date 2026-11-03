//! Update check (issue #70): the first-launch question, the setting (off = the client is never
//! called), the once-a-day launch check, the per-version dismissal and Check for Updates…,
//! all against a mocked GitHub client.

use crate::common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::{settle, wait_until};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_engine::update::{self, ReleaseClient, Response, TransportError};
use markise_ui_egui::menu::{MenuAction, Platform};
use markise_ui_egui::updates::{Message, UpdateSource};
use markise_ui_egui::{AppOptions, ConfigDir, MarkiseApp};

type H = Harness<'static, MarkiseApp>;

/// A mocked GitHub: counts checks (every check asks for the latest stable release once) and
/// answers per URL.
struct Mock {
    checks: AtomicUsize,
    /// (answer to the releases list, answer to the latest stable release)
    answers: Mutex<(Result<Response, TransportError>, Result<Response, TransportError>)>,
}

fn release_json(t: &str, pre: bool) -> String {
    format!(
        r#"{{"tag_name":"{t}","prerelease":{pre},"draft":false,"html_url":"https://github.com/adjioev/markise/releases/tag/{t}"}}"#
    )
}

/// GitHub's answers for these releases (newest first): the list and the latest stable one.
fn answers_for(tags: &[(&str, bool)]) -> (Result<Response, TransportError>, Result<Response, TransportError>) {
    let items: Vec<String> = tags.iter().map(|(t, pre)| release_json(t, *pre)).collect();
    let latest = tags.iter().find(|(_, pre)| !pre).map_or(
        Response {
            status: 404,
            body: Ok(String::new()),
        },
        |(t, _)| Response::ok(release_json(t, false)),
    );
    (Ok(Response::ok(format!("[{}]", items.join(",")))), Ok(latest))
}

impl Mock {
    fn releases(tags: &[(&str, bool)]) -> Arc<Mock> {
        Arc::new(Mock {
            checks: AtomicUsize::new(0),
            answers: Mutex::new(answers_for(tags)),
        })
    }

    fn set_releases(&self, tags: &[(&str, bool)]) {
        *self.answers.lock().unwrap() = answers_for(tags);
    }

    /// The same answer to every request.
    fn set(&self, answer: Result<Response, TransportError>) {
        *self.answers.lock().unwrap() = (answer.clone(), answer);
    }

    fn calls(&self) -> usize {
        self.checks.load(Ordering::SeqCst)
    }
}

impl ReleaseClient for Mock {
    fn get(&self, url: &str) -> Result<Response, TransportError> {
        let a = self.answers.lock().unwrap();
        if url == update::LATEST_URL {
            self.checks.fetch_add(1, Ordering::SeqCst);
            a.1.clone()
        } else {
            a.0.clone()
        }
    }
}

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-updates-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn settings_file(cfg: &Path) -> PathBuf {
    cfg.join("markise").join("settings.json")
}

fn write_settings(cfg: &Path, json: &str) {
    let f = settings_file(cfg);
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(f, json).unwrap();
}

fn read_settings(cfg: &Path) -> serde_json::Value {
    std::fs::read_to_string(settings_file(cfg))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null)
}

fn harness(cfg: &Path, mock: &Arc<Mock>) -> H {
    harness_with(cfg, UpdateSource::Client(mock.clone()))
}

fn harness_with(cfg: &Path, updates: UpdateSource) -> H {
    let opts = AppOptions {
        config: ConfigDir::Dir(cfg.to_path_buf()),
        updates,
        ..common::no_watch()
    };
    Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), MarkiseApp::new(opts))
}

fn launched(h: &mut H) {
    wait_until(h, "settings", MarkiseApp::settings_loaded);
    settle(h);
    for _ in 0..3 {
        h.step();
    }
}

fn notice_version(a: &MarkiseApp) -> Option<String> {
    a.update_state().notice.as_ref().map(|r| r.version.to_string())
}

fn check_now(h: &mut H) {
    let ctx = h.ctx.clone();
    assert!(
        h.state_mut()
            .menu_action(&ctx, &MenuAction::Command("app.checkUpdates"))
    );
    wait_until(h, "check done", |a| !a.is_checking_updates());
    settle(h);
}

#[test]
fn first_launch_asks_once_and_never_contacts_github_without_a_yes() {
    let cfg = tmpdir("ask").join("config");
    let mock = Mock::releases(&[("v99.0.0", false)]);
    {
        let mut h = harness(&cfg, &mock);
        launched(&mut h);
        assert!(h.state().update_state().asking, "the question is shown");
        assert_eq!(h.state().update_auto(), None);
        // a non-modal card: its buttons are there, nothing else is blocked
        h.get_by_label("Check Automatically");
        h.get_by_label("Not Now").click();
        h.run();
        assert!(!h.state().update_state().asking);
        assert_eq!(h.state().update_auto(), Some(false));
        h.state_mut().on_exit();
    }
    assert_eq!(read_settings(&cfg)["updates.autoCheck"], false);
    // the next launch neither asks nor checks
    let mut h = harness(&cfg, &mock);
    launched(&mut h);
    assert!(!h.state().update_state().asking);
    assert_eq!(mock.calls(), 0, "off: the client is never called");
    assert!(h.query_by_label("Check Automatically").is_none());
}

#[test]
fn an_unanswered_question_is_asked_again_and_checks_nothing() {
    let cfg = tmpdir("unanswered").join("config");
    let mock = Mock::releases(&[("v99.0.0", false)]);
    for _ in 0..2 {
        let mut h = harness(&cfg, &mock);
        launched(&mut h);
        assert!(h.state().update_state().asking);
        h.state_mut().on_exit();
    }
    assert_eq!(mock.calls(), 0);
}

#[test]
fn yes_checks_at_once_shows_the_notice_and_throttles_to_once_a_day() {
    let cfg = tmpdir("yes").join("config");
    let mock = Mock::releases(&[("v99.0.0", false), ("v0.0.1", false)]);
    {
        let mut h = harness(&cfg, &mock);
        launched(&mut h);
        h.get_by_label("Check Automatically").click();
        wait_until(&mut h, "check done", |a| {
            a.update_auto() == Some(true) && !a.is_checking_updates()
        });
        settle(&mut h);
        h.run();
        assert_eq!(mock.calls(), 1);
        assert_eq!(notice_version(h.state()).as_deref(), Some("99.0.0"));
        h.get_by_label("Markise 99.0.0 is available —");
        h.get_by_label("What's new");
        h.get_by_label("Download");
        assert!(h.state().update_settings().1.is_some(), "last check recorded");
        h.state_mut().on_exit();
    }
    let s = read_settings(&cfg);
    assert_eq!(s["updates.autoCheck"], true);
    assert!(s["updates.lastCheck"].as_u64().is_some());
    // within a day: the next launch does not check
    let mut h = harness(&cfg, &mock);
    launched(&mut h);
    assert_eq!(mock.calls(), 1, "at most once a day");
    assert!(notice_version(h.state()).is_none());
}

#[test]
fn a_due_launch_check_runs_and_dismissal_is_per_version() {
    let cfg = tmpdir("dismiss").join("config");
    write_settings(
        &cfg,
        r#"{"version":2,"updates.autoCheck":true,"updates.lastCheck":1000}"#,
    );
    let mock = Mock::releases(&[("v99.0.0", false)]);
    {
        let mut h = harness(&cfg, &mock);
        launched(&mut h);
        assert_eq!(mock.calls(), 1, "a day passed since the last check");
        assert_eq!(notice_version(h.state()).as_deref(), Some("99.0.0"));
        h.run();
        h.get_by_label("Dismiss").click();
        h.run();
        assert!(notice_version(h.state()).is_none());
        assert!(h.query_by_label("What's new").is_none());
        h.state_mut().on_exit();
    }
    assert_eq!(read_settings(&cfg)["updates.dismissed"], "99.0.0");
    // the same version stays dismissed on a later due check …
    let mut s = read_settings(&cfg);
    s["updates.lastCheck"] = 1000.into();
    write_settings(&cfg, &s.to_string());
    {
        let mut h = harness(&cfg, &mock);
        launched(&mut h);
        assert_eq!(mock.calls(), 2);
        assert!(notice_version(h.state()).is_none(), "dismissed version");
        h.state_mut().on_exit();
    }
    // … a newer one is announced again
    let mut s = read_settings(&cfg);
    s["updates.lastCheck"] = 1000.into();
    write_settings(&cfg, &s.to_string());
    mock.set_releases(&[("v99.1.0", false), ("v99.0.0", false)]);
    let mut h = harness(&cfg, &mock);
    launched(&mut h);
    assert_eq!(notice_version(h.state()).as_deref(), Some("99.1.0"));
}

#[test]
fn check_for_updates_works_with_automatic_checks_off_and_reports_every_result() {
    let cfg = tmpdir("manual").join("config");
    write_settings(
        &cfg,
        r#"{"version":2,"updates.autoCheck":false,"updates.dismissed":"99.0.0"}"#,
    );
    let mock = Mock::releases(&[("v0.0.1", false)]);
    let mut h = harness(&cfg, &mock);
    launched(&mut h);
    assert_eq!(mock.calls(), 0, "automatic checks are off");
    // older release: up to date
    check_now(&mut h);
    assert_eq!(mock.calls(), 1);
    let v = env!("CARGO_PKG_VERSION");
    assert_eq!(h.state().update_state().message, Some(Message::UpToDate(v.into())));
    h.run();
    h.get_by_label(&format!("Markise {v} is the latest version."));
    // a server error and rate limiting are reported
    mock.set(Ok(Response {
        status: 500,
        body: Ok(String::new()),
    }));
    check_now(&mut h);
    assert!(matches!(&h.state().update_state().message, Some(Message::Error(e)) if e.contains("500")));
    mock.set(Ok(Response {
        status: 403,
        body: Ok("{\"message\":\"API rate limit exceeded\"}".into()),
    }));
    check_now(&mut h);
    assert!(matches!(&h.state().update_state().message, Some(Message::Error(e)) if e.contains("limiting")));
    // offline
    mock.set(Err(TransportError("dns failure".into())));
    check_now(&mut h);
    assert!(matches!(&h.state().update_state().message, Some(Message::Error(e)) if e.contains("dns failure")));
    // malformed answer
    mock.set(Ok(Response {
        status: 200,
        body: Ok("<html>".into()),
    }));
    check_now(&mut h);
    assert!(matches!(&h.state().update_state().message, Some(Message::Error(_))));
    // a newer version is shown even when its notice was dismissed before
    mock.set_releases(&[("v99.0.0", false)]);
    check_now(&mut h);
    assert_eq!(notice_version(h.state()).as_deref(), Some("99.0.0"));
    assert_eq!(h.state().update_state().message, None);
    assert_eq!(mock.calls(), 6);
    assert_eq!(
        h.state().update_auto(),
        Some(false),
        "a manual check changes no setting"
    );
}

#[test]
fn turning_the_preference_on_checks_and_off_stops_launch_checks() {
    let cfg = tmpdir("pref").join("config");
    write_settings(&cfg, r#"{"version":2,"updates.autoCheck":false}"#);
    let mock = Mock::releases(&[("v99.0.0", false)]);
    {
        let mut h = harness(&cfg, &mock);
        launched(&mut h);
        let ctx = h.ctx.clone();
        h.state_mut()
            .apply_preference(&ctx, markise_ui_egui::prefs::Change::UpdateAuto(true));
        wait_until(&mut h, "check", |a| !a.is_checking_updates());
        assert_eq!(mock.calls(), 1);
        h.state_mut()
            .apply_preference(&ctx, markise_ui_egui::prefs::Change::UpdateAuto(false));
        h.state_mut().on_exit();
    }
    // a day later, but off: nothing
    let mut s = read_settings(&cfg);
    s["updates.lastCheck"] = 1000.into();
    write_settings(&cfg, &s.to_string());
    let mut h = harness(&cfg, &mock);
    launched(&mut h);
    assert_eq!(mock.calls(), 1);
    assert_eq!(h.state().preference_values().update_auto, Some(false));
}

#[test]
fn without_a_client_nothing_is_asked_and_check_for_updates_says_why() {
    let cfg = tmpdir("off").join("config");
    let mut h = harness_with(
        &cfg,
        UpdateSource::Off("Development build: update checks are off.".into()),
    );
    launched(&mut h);
    assert!(!h.state().update_state().asking);
    let ctx = h.ctx.clone();
    assert!(
        h.state_mut()
            .menu_action(&ctx, &MenuAction::Command("app.checkUpdates"))
    );
    h.run();
    assert_eq!(
        h.state().update_state().message,
        Some(Message::Unavailable("Development build: update checks are off.".into()))
    );
    h.get_by_label("Development build: update checks are off.");
    assert_eq!(h.state().update_state().started, 0);
    assert_eq!(
        h.state().preference_values().update_unavailable.as_deref(),
        Some("Development build: update checks are off.")
    );
}

#[test]
fn check_for_updates_is_in_the_app_menu_on_macos_and_in_help_elsewhere() {
    let mut app = MarkiseApp::new(common::no_watch());
    app.set_menu_platform(Platform::Mac);
    let bar = app.menu_model();
    let keys: Vec<String> = bar.menus[0].items().iter().map(|i| i.key.clone()).collect();
    assert_eq!(keys[..2], ["app.about", "app.checkUpdates"]);
    assert_eq!(bar.item("app.checkUpdates").unwrap().label, "Check for Updates…");
    app.set_menu_platform(Platform::Other);
    let bar = app.menu_model();
    let help: Vec<String> = bar
        .menu("Help")
        .unwrap()
        .items()
        .iter()
        .map(|i| i.key.clone())
        .collect();
    assert_eq!(help[..2], ["app.about", "app.checkUpdates"]);
}

/// Review of #146 (P2): an answer whose body could not be read (oversized, broken off) still
/// counts as contact, so the next launch does not ask again within the day; an offline launch
/// does not count.
#[test]
fn an_unreadable_answer_counts_for_throttling_and_offline_does_not() {
    let cfg = tmpdir("unreadable").join("config");
    write_settings(&cfg, r#"{"version":2,"updates.autoCheck":true}"#);
    let mock = Mock::releases(&[]);
    mock.set(Err(TransportError("offline".into())));
    {
        let mut h = harness(&cfg, &mock);
        launched(&mut h);
        assert_eq!(mock.calls(), 1);
        assert_eq!(h.state().update_settings().1, None, "offline: not counted");
        h.state_mut().on_exit();
    }
    mock.set(Ok(Response {
        status: 200,
        body: Err("response body exceeds the limit".into()),
    }));
    {
        let mut h = harness(&cfg, &mock);
        launched(&mut h);
        assert_eq!(mock.calls(), 2, "retried after the offline launch");
        assert!(h.state().update_settings().1.is_some(), "an answer counts");
        h.state_mut().on_exit();
    }
    let mut h = harness(&cfg, &mock);
    launched(&mut h);
    assert_eq!(mock.calls(), 2, "throttled");
}

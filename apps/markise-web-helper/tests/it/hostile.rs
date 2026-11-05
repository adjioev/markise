//! A page that tries everything it must not manage, and pages that try to hold the helper up.

use std::process::Command;
use std::time::{Duration, Instant};

use markise_web_render::proto::{Area, FailKind};

use crate::support::{Answer, Helper, pixel};

/// The helper's own self-test: the forbidden page, with a listener, and the fixture.
#[test]
fn the_page_reaches_nothing_it_must_not() {
    let out = Command::new(env!("CARGO_BIN_EXE_markise-web-helper"))
        .arg("--self-test")
        .output()
        .expect("the helper starts");
    let text = String::from_utf8_lossy(&out.stdout);
    println!("{text}");
    assert!(out.status.success(), "{text}");
    assert!(text.contains("stopped: fetch ") && text.contains("stopped: webrtc ") && text.ends_with("ok\n"));
}

/// What an older macOS does, forced: it has no proxy API, so this helper has no engine, and says so
/// rather than running a page less closed off. Nothing else about that system can be had here.
#[test]
fn a_system_without_what_it_takes_has_no_helper_and_says_so() {
    let out = Command::new(env!("CARGO_BIN_EXE_markise-web-helper"))
        .env("MARKISE_WEB_HELPER_NO_PROXY_API", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("the helper starts");
    assert!(out.status.success(), "{:?}", out.status);
    let (hello, _) = markise_web_render::proto::read_from_helper(&mut out.stdout.as_slice()).unwrap();
    assert_eq!(
        hello,
        markise_web_render::proto::FromHelper::Hello {
            protocol: markise_web_render::proto::PROTOCOL,
            available: false
        }
    );
}

/// Nothing that came with macOS 14 may be linked: on an older system the loader would end the process
/// before it could say anything. The symbols are looked up when it runs.
#[test]
fn nothing_newer_than_the_deployment_target_is_linked() {
    let out = Command::new("nm")
        .args(["-u", env!("CARGO_BIN_EXE_markise-web-helper")])
        .output()
        .expect("nm");
    let imports = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && imports.contains("_nw_endpoint_create_host"),
        "{imports}"
    );
    assert!(!imports.contains("nw_proxy_config"), "{imports}");
}

/// An attempt that runs out of its budget counts against the page, not for it.
#[test]
fn the_self_test_counts_what_did_not_finish_against_it() {
    let page = include_str!("../../src/mac/self_test.js");
    assert!(!page.contains("'timeout'"), "no way to pass by timing out");
    assert!(page.contains("'unfinished'"));
    let runner = include_str!("../../src/mac/self_test.rs");
    assert!(runner.contains("UNFINISHED") && runner.contains("wrong.push"));
}

#[test]
fn nothing_is_kept_between_sessions() {
    let mut h = Helper::start();
    // green only when nothing was left by the session before; red otherwise
    let probe = r#"export default function init(root) {
      let clean = true;
      try { clean = clean && localStorage.getItem('left') === null; localStorage.setItem('left', '1'); } catch {}
      try { clean = clean && sessionStorage.getItem('left') === null; sessionStorage.setItem('left', '1'); } catch {}
      clean = clean && !document.cookie.includes('left');
      document.cookie = 'left=1';
      root.style.cssText = 'height:10px;background:' + (clean ? 'rgb(0,255,0)' : 'rgb(255,0,0)');
    }"#;
    h.file(Area::Package, "p.js", probe);
    let color = |h: &mut Helper, session: &str| {
        let mut r = h.request("<section></section>", Some("p.js"), &[]);
        r.session = session.to_string();
        match h.render(r) {
            Answer::Image(m, b) => pixel(&m, &b, 2, 2),
            Answer::Failed(k, why) => panic!("{k:?}: {why}"),
        }
    };
    assert_eq!(color(&mut h, "first"), [0, 255, 0, 255]);
    assert_eq!(
        color(&mut h, "second"),
        [0, 255, 0, 255],
        "another package finds nothing"
    );
}

fn fails_with(js: &str, expect: FailKind) -> Helper {
    let mut h = Helper::start();
    h.file(Area::Package, "m.js", js);
    let mut r = h.request("<section></section>", Some("m.js"), &[]);
    // short for what has to run into the limit, and no reason to on a machine under load otherwise
    r.limit_ms = if expect == FailKind::Hung { 1000 } else { 20_000 };
    let t = Instant::now();
    match h.render(r) {
        Answer::Failed(k, _) => assert_eq!(k, expect),
        Answer::Image(m, _) => panic!("an image of {m:?}"),
    }
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
    h
}

#[test]
fn a_script_that_never_finishes_ends_the_helper_in_time() {
    let mut h = fails_with("export default function(){ for(;;){} }", FailKind::Hung);
    assert!(h.exits_within(Duration::from_secs(3)));
    let mut h = fails_with(
        "export default function(){ return new Promise(()=>{}); }",
        FailKind::Hung,
    );
    assert!(h.exits_within(Duration::from_secs(3)));
}

#[test]
fn a_script_that_throws_or_does_not_load_fails_quietly() {
    let mut h = fails_with("export default function(){ throw new Error('no'); }", FailKind::Script);
    assert!(!h.exits_within(Duration::from_millis(200)), "it goes on");
    let mut r = h.request("<section></section>", Some("none.js"), &[]);
    r.limit_ms = 20_000;
    assert!(matches!(h.render(r), Answer::Failed(FailKind::Script, _)));
    h.file(
        Area::Package,
        "s.js",
        "export default function(root){root.style.height='5px';}",
    );
    let r = h.request("<section></section>", Some("s.js"), &[]);
    assert!(matches!(h.render(r), Answer::Image(..)), "the next one draws");
}

#[test]
fn an_enormous_height_is_refused() {
    fails_with(
        "export default function(root){ root.style.height='100000000px'; }",
        FailKind::Size,
    );
}

#[test]
fn a_block_of_no_height_is_refused() {
    let mut h = Helper::start();
    let r = h.request("", None, &[]);
    assert!(matches!(h.render(r), Answer::Failed(FailKind::Size, _)));
}

#[test]
fn a_page_resizing_itself_gets_one_measure() {
    let mut h = Helper::start();
    h.file(
        Area::Package,
        "m.js",
        "export default function(root){ let n=10; root.style.height=n+'px'; new ResizeObserver(()=>{n+=10; root.style.height=n+'px';}).observe(root); setInterval(()=>{n+=10; root.style.height=n+'px'},1); }",
    );
    let r = h.request("<section></section>", Some("m.js"), &[]);
    assert!(matches!(h.render(r), Answer::Image(..)));
}

#[test]
fn dialogs_and_windows_hold_nothing_up() {
    let mut h = Helper::start();
    h.file(
        Area::Package,
        "m.js",
        "export default function(root){ for(let i=0;i<200;i++){alert('a');confirm('b');prompt('c');window.open('about:blank');} root.style.height='10px'; }",
    );
    let r = h.request("<section></section>", Some("m.js"), &[]);
    assert!(matches!(h.render(r), Answer::Image(..)));
}

#[test]
fn file_requests_are_bounded() {
    let mut h = Helper::start();
    h.file(
        Area::Package,
        "m.js",
        "export default async function(root){ await Promise.all([...Array(2000).keys()].map((i)=>fetch('flood-'+i+'.txt').catch(()=>0))); root.style.height='10px'; }",
    );
    let mut r = h.request("<section></section>", Some("m.js"), &[]);
    r.limit_ms = 20_000;
    let _ = h.render(r);
    assert!(h.asked.len() <= 513, "{}", h.asked.len());
}

#[test]
fn a_cancelled_render_answers_and_the_next_one_draws() {
    let mut h = Helper::start();
    h.file(
        Area::Package,
        "m.js",
        "export default function(){ return new Promise((r)=>setTimeout(r,300)); }",
    );
    let r = h.request("<section style='height:5px'></section>", Some("m.js"), &[]);
    let id = r.id;
    h.send(markise_web_render::proto::ToHelper::Render(r));
    h.send(markise_web_render::proto::ToHelper::Cancel { id });
    assert!(matches!(h.answer(id), Answer::Failed(FailKind::Cancelled, _)));
    let r = h.request("<section style='height:5px'></section>", None, &[]);
    assert!(matches!(h.render(r), Answer::Image(..)));
}

#[test]
fn the_client_going_away_ends_the_helper() {
    let mut h = Helper::start();
    let r = h.request("<section style='height:5px'></section>", None, &[]);
    assert!(matches!(h.render(r), Answer::Image(..)));
    h.close_input();
    assert!(h.exits_within(Duration::from_secs(3)));
}

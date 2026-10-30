//! macOS: hidden NSWindow + one WKWebView, requests rendered one at a time on the main thread.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::mpsc;
use std::time::Instant;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBitmapImageFileType, NSBitmapImageRep, NSImage,
    NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    NSActivityOptions, NSDictionary, NSError, NSNumber, NSObjectNSKeyValueCoding, NSObjectProtocol, NSPoint,
    NSProcessInfo, NSRect, NSSize, NSString, NSURL, ns_string,
};
use objc2_web_kit::{WKContentWorld, WKSnapshotConfiguration, WKWebView, WKWebViewConfiguration};
use serde_json::{Value, json};

use crate::proto;

/// The page every block is rendered into: site CSS goes into `#mk-css`, the block into
/// `#mk-root`. Animations and transitions are off so a snapshot never catches a mid-state.
const SHELL: &str = r#"<!doctype html><html><head><meta charset="utf-8">
<style>
html,body{margin:0;padding:0;background:transparent;overflow:hidden}
*,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent}
#mk-root{display:flow-root}
</style><style id="mk-css"></style><base id="mk-base"></head><body><div id="mk-root"></div></body></html>"#;

/// Runs in the page for every request (body of an async function, argument `req` = JSON).
/// Waits for the viewport width, installs CSS and HTML, runs the site's `decorate(block)` (the
/// explicit "decorated" signal is its promise resolving), waits for fonts, measures the wrapper.
const RENDER_JS: &str = r#"
const r = JSON.parse(req);
const t0 = performance.now();
const sleep = (ms) => new Promise(res => setTimeout(res, ms));
for (let i = 0; i < 200 && Math.abs(window.innerWidth - r.width) > 0.5; i++) await sleep(2);
const css = document.getElementById('mk-css');
if (css.textContent !== r.css) css.textContent = r.css;
const html = document.documentElement;
if (html.dataset.theme !== r.theme) html.dataset.theme = r.theme;
const style = (r.vars || '') + ';color-scheme:' + r.theme;
if (html.getAttribute('style') !== style) html.setAttribute('style', style);
const base = document.getElementById('mk-base');
if (r.base && base.getAttribute('href') !== r.base) base.setAttribute('href', r.base);
const root = document.getElementById('mk-root');
root.innerHTML = r.html;
const block = root.firstElementChild;
let decorated = null;
if (r.js && block) {
  if (window.__mkJs !== r.js) { (0, eval)(r.js); window.__mkJs = r.js; }
  const fn = window.markiseBlocks && window.markiseBlocks[block.dataset.component];
  if (fn) {
    decorated = await Promise.race([
      Promise.resolve().then(() => fn(block)).then(() => true, (e) => 'error: ' + e),
      sleep(2000).then(() => false),
    ]);
  }
}
// site assets (pictures) by relative paths: wait until each image loaded or failed
const imgs = [...root.querySelectorAll('img')].filter(i => !i.complete);
if (imgs.length) await Promise.race([
  Promise.all(imgs.map(i => new Promise(res => { i.addEventListener('load', res, { once: true }); i.addEventListener('error', res, { once: true }); }))),
  sleep(3000),
]);
await Promise.race([document.fonts.ready, sleep(1000)]);
const h = root.getBoundingClientRect().height;
return JSON.stringify({ height: h, decorated, inner_width: window.innerWidth, ms: performance.now() - t0 });
"#;

struct Req {
    id: u64,
    width: f64,
    payload: String,
    received: Instant,
}

struct State {
    window: Retained<NSWindow>,
    web: Retained<WKWebView>,
    queue: VecDeque<Req>,
    busy: bool,
    ready: bool,
    out: mpsc::Sender<(Value, Vec<u8>)>,
    selftest: Option<String>,
}

thread_local! {
    static ST: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    ST.with(|s| f(s.borrow_mut().as_mut().expect("state")))
}

pub fn main() {
    let started = Instant::now();
    let args: Vec<String> = std::env::args().collect();
    let selftest = (args.get(1).map(String::as_str) == Some("--selftest"))
        .then(|| args.get(2).cloned().unwrap_or_else(|| "selftest.png".into()));
    let self_w: f64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(600.0);

    let mtm = MainThreadMarker::new().expect("main thread");
    let app = NSApplication::sharedApplication(mtm);
    // no Dock icon, never activates
    let policy = match std::env::var("MARKISE_WR_POLICY").as_deref() {
        Ok("accessory") => NSApplicationActivationPolicy::Accessory,
        _ => NSApplicationActivationPolicy::Prohibited,
    };
    app.setActivationPolicy(policy);
    // keep timers and WebKit responsive while in background (no App Nap)
    let activity: Retained<ProtocolObject<dyn NSObjectProtocol>> = NSProcessInfo::processInfo()
        .beginActivityWithOptions_reason(
            NSActivityOptions::UserInitiated | NSActivityOptions::LatencyCritical,
            ns_string!("markise block rendering"),
        );
    std::mem::forget(activity);

    // ---- hidden window + web view
    let mode = std::env::var("MARKISE_WR_WINDOW").unwrap_or_else(|_| "offscreen".into());
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 100.0));
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setIgnoresMouseEvents(true);
    window.setExcludedFromWindowsMenu(true);
    let config = unsafe { WKWebViewConfiguration::new(mtm) };
    let web = unsafe { WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), rect, &config) };
    // transparent page background (KVC; public API has no switch for it on macOS)
    unsafe {
        let no = NSNumber::numberWithBool(false);
        web.setValue_forKey(Some(&no), ns_string!("drawsBackground"));
    }
    window.setContentView(Some(&web));
    match mode.as_str() {
        "none" => {}
        "alpha" => {
            window.setAlphaValue(0.0);
            window.setFrameOrigin(NSPoint::new(0.0, 0.0));
            window.orderBack(None);
        }
        _ => {
            // far outside every screen: never visible, still "in a window" for WebKit
            window.setFrameOrigin(NSPoint::new(-20000.0, -20000.0));
            window.orderFrontRegardless();
        }
    }
    // Site assets: with `MARKISE_WR_ROOT` (a folder: the site root) the shell is loaded from a
    // temporary file with read access to that folder, so pictures and fonts of the site load;
    // every request sets `<base href>` to the document's folder so relative picture paths
    // resolve like in the document.
    let root = std::env::var("MARKISE_WR_ROOT").ok().filter(|r| !r.is_empty());
    // WebKit only loads a file page inside its read-access folder, so the shell file lives in
    // the user's cache folder and read access is the common ancestor of it and the site root
    // (the home folder for sites under it).
    let cache = std::env::var_os("HOME")
        .map(|h| std::path::PathBuf::from(h).join("Library/Caches/markise-web-render"))
        .unwrap_or_else(std::env::temp_dir);
    let _ = std::fs::create_dir_all(&cache);
    let shell_file = cache.join(format!("shell-{}.html", std::process::id()));
    match root {
        Some(r) if std::fs::write(&shell_file, SHELL).is_ok() => unsafe {
            let file = NSURL::fileURLWithPath(&NSString::from_str(&shell_file.to_string_lossy()));
            let common: std::path::PathBuf = std::path::Path::new(&r)
                .components()
                .zip(shell_file.components())
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| a)
                .collect();
            let access = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&common.to_string_lossy()), true);
            let _ = web.loadFileURL_allowingReadAccessToURL(&file, &access);
        },
        _ => unsafe {
            let _ = web.loadHTMLString_baseURL(&NSString::from_str(SHELL), None);
        },
    }

    // ---- output thread (stdout carries protocol frames only)
    let (tx, rx) = mpsc::channel::<(Value, Vec<u8>)>();
    std::thread::spawn(move || {
        let mut out = std::io::stdout().lock();
        for (j, b) in rx {
            if proto::write_frame(&mut out, &j, &b).is_err() {
                std::process::exit(0);
            }
        }
    });

    ST.with(|s| {
        *s.borrow_mut() = Some(State {
            window,
            web,
            queue: VecDeque::new(),
            busy: false,
            ready: false,
            out: tx,
            selftest: selftest.clone(),
        })
    });

    if selftest.is_some() {
        let html = format!(
            r#"<section data-component="probe" style="font:16px -apple-system;padding:12px;border:1px solid #888;border-radius:8px"><h2>Feasibility probe</h2><p>Width {self_w} CSS px. Thin lines: </p><div style="height:1px;background:#000;margin:2px 0"></div><div style="height:0.5px;background:#000"></div></section>"#
        );
        enqueue(Req {
            id: 1,
            width: self_w,
            payload: json!({"width": self_w, "html": html, "css": "", "js": ""}).to_string(),
            received: Instant::now(),
        });
    } else {
        // ---- input thread: requests are handed to the main thread
        let shell_tmp = shell_file.clone();
        std::thread::spawn(move || {
            let mut input = std::io::stdin().lock();
            loop {
                match proto::read_frame(&mut input) {
                    Ok((v, _)) => {
                        let id = v["id"].as_u64().unwrap_or(0);
                        let width = v["width"].as_f64().unwrap_or(600.0).clamp(50.0, 4000.0);
                        let payload = json!({
                            "width": width,
                            "html": v["html"].as_str().unwrap_or(""),
                            "css": v["css"].as_str().unwrap_or(""),
                            "js": v["js"].as_str().unwrap_or(""),
                            "theme": if v["theme"] == "dark" { "dark" } else { "light" },
                            "vars": v["vars"].as_str().unwrap_or(""),
                            "base": v["base"].as_str().unwrap_or(""),
                        })
                        .to_string();
                        let req = Req {
                            id,
                            width,
                            payload,
                            received: Instant::now(),
                        };
                        DispatchQueue::main().exec_async(move || enqueue(req));
                    }
                    Err(_) => {
                        // Markise went away (EOF) or sent garbage
                        let _ = std::fs::remove_file(&shell_tmp);
                        DispatchQueue::main().exec_async(|| std::process::exit(0));
                        return;
                    }
                }
            }
        });
    }

    poll_ready(started);
    app.run();
}

/// Polls `document.readyState` of the shell page until it is complete.
fn poll_ready(started: Instant) {
    let web = with_state(|s| s.web.clone());
    let block = RcBlock::new(move |res: *mut AnyObject, _err: *mut NSError| {
        let done = unsafe { res.as_ref() }
            .and_then(|o| o.downcast_ref::<NSString>())
            .is_some_and(|s| s.to_string() == "complete");
        if done {
            let ms = started.elapsed().as_secs_f64() * 1e3;
            with_state(|s| {
                s.ready = true;
                if s.selftest.is_none() {
                    let _ = s.out.send((json!({"ready": true, "ms_startup": ms}), Vec::new()));
                } else {
                    eprintln!("ready after {ms:.1} ms");
                }
            });
            pump();
        } else {
            let _ = DispatchQueue::main().after(dispatch2::DispatchTime::NOW.time(5_000_000), move || {
                poll_ready(started)
            });
        }
    });
    unsafe {
        web.evaluateJavaScript_completionHandler(
            ns_string!("document.getElementById('mk-root') ? document.readyState : 'loading'"),
            Some(&block),
        )
    };
}

fn enqueue(req: Req) {
    with_state(|s| s.queue.push_back(req));
    pump();
}

fn respond(json: Value, png: Vec<u8>) {
    let selftest = with_state(|s| {
        s.busy = false;
        s.selftest.clone()
    });
    if let Some(path) = selftest {
        eprintln!("{json}");
        if !png.is_empty() {
            std::fs::write(&path, &png).expect("write png");
            eprintln!("wrote {path}");
        }
        std::process::exit(if json["ok"] == true { 0 } else { 1 });
    }
    with_state(|s| {
        let _ = s.out.send((json, png));
    });
    pump();
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// Starts the next queued request if idle.
fn pump() {
    let Some((req, web, window)) = with_state(|s| {
        if !s.ready || s.busy {
            return None;
        }
        let r = s.queue.pop_front()?;
        s.busy = true;
        Some((r, s.web.clone(), s.window.clone()))
    }) else {
        return;
    };
    let mtm = MainThreadMarker::new().expect("main thread");
    let ms_queue = ms(req.received);
    let t_js = Instant::now();
    // width first (the page waits until it sees it); keep the current height
    let cur_h = web.frame().size.height.max(1.0);
    let size = NSSize::new(req.width, cur_h);
    window.setContentSize(size);
    web.setFrameSize(size);

    let id = req.id;
    let width = req.width;
    let received = req.received;
    let arg = NSString::from_str(&req.payload);
    let args = NSDictionary::<NSString, AnyObject>::from_slices(&[ns_string!("req")], &[&*arg as &AnyObject]);
    let block = RcBlock::new(move |res: *mut AnyObject, err: *mut NSError| {
        let ms_js = ms(t_js);
        let text = unsafe { res.as_ref() }
            .and_then(|o| o.downcast_ref::<NSString>())
            .map(|s| s.to_string());
        let Some(m) = text.and_then(|t| serde_json::from_str::<Value>(&t).ok()) else {
            let e = unsafe { err.as_ref() }
                .map(|e| {
                    let msg = e
                        .userInfo()
                        .objectForKey(ns_string!("WKJavaScriptExceptionMessage"))
                        .and_then(|o| o.downcast_ref::<NSString>().map(|s| s.to_string()))
                        .unwrap_or_default();
                    format!("{} {msg}", e.localizedDescription())
                })
                .unwrap_or_else(|| "no result".into());
            respond(json!({"id": id, "ok": false, "error": format!("js: {e}")}), Vec::new());
            return;
        };
        let height = m["height"].as_f64().unwrap_or(0.0).ceil().max(1.0);
        snapshot(id, width, height, m, received, ms_queue, ms_js);
    });
    unsafe {
        web.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            ns_string!(RENDER_JS),
            Some(&args),
            None,
            &WKContentWorld::pageWorld(mtm),
            Some(&block),
        );
    }
}

fn snapshot(id: u64, width: f64, height: f64, m: Value, received: Instant, ms_queue: f64, ms_js: f64) {
    let mtm = MainThreadMarker::new().expect("main thread");
    let (web, window) = with_state(|s| (s.web.clone(), s.window.clone()));
    let size = NSSize::new(width, height);
    window.setContentSize(size);
    web.setFrameSize(size);
    let backing = window.backingScaleFactor();
    let t_snap = Instant::now();
    let cfg = unsafe { WKSnapshotConfiguration::new(mtm) };
    unsafe {
        cfg.setRect(NSRect::new(NSPoint::new(0.0, 0.0), size));
        cfg.setSnapshotWidth(Some(&NSNumber::numberWithDouble(width)));
        cfg.setAfterScreenUpdates(true);
    }
    let block = RcBlock::new(move |img: *mut NSImage, err: *mut NSError| {
        let ms_snap = ms(t_snap);
        let Some(img) = (unsafe { img.as_ref() }) else {
            let e = unsafe { err.as_ref() }
                .map(|e| e.localizedDescription().to_string())
                .unwrap_or_else(|| "no image".into());
            respond(
                json!({"id": id, "ok": false, "error": format!("snapshot: {e}")}),
                Vec::new(),
            );
            return;
        };
        let t_png = Instant::now();
        let Some(cg) = (unsafe { img.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }) else {
            respond(json!({"id": id, "ok": false, "error": "no CGImage"}), Vec::new());
            return;
        };
        let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
        let (px_w, px_h) = (rep.pixelsWide(), rep.pixelsHigh());
        let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }
            .map(|d| d.to_vec())
            .unwrap_or_default();
        let resp = json!({
            "id": id, "ok": !png.is_empty(), "height": height, "width": width,
            "px_w": px_w, "px_h": px_h, "backing": backing,
            "decorated": m["decorated"], "inner_width": m["inner_width"], "ms_page": m["ms"],
            "ms_queue": ms_queue, "ms_js": ms_js, "ms_snap": ms_snap, "ms_png": ms(t_png),
            "ms_total": ms(received), "png_bytes": png.len(),
        });
        respond(resp, png);
    });
    unsafe { web.takeSnapshotWithConfiguration_completionHandler(Some(&cfg), &block) };
}

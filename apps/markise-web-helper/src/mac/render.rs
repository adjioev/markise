//! One render at a time: load the page, prepare it, run the block's module, settle, measure,
//! snapshot, answer, and leave nothing running. Each step ends in the next one or in a failure, and
//! a timer that does not depend on the page bounds the whole. A callback from before (a page left
//! behind, a render cancelled) finds its generation gone and does nothing: authority belongs to
//! the render that asked.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::Sender;
use std::time::Instant;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchTime};
use markise_web_render::proto::{
    self, Area, FailKind, FromHelper, MAX_FILE, MAX_PIXELS, MAX_SIDE_PX, RenderRequest, Rendered, ToHelper,
};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSBackingStoreType, NSImage, NSWindow, NSWindowStyleMask};
use objc2_foundation::{
    NSData, NSDictionary, NSError, NSHTTPURLResponse, NSNumber, NSObjectNSKeyValueCoding, NSObjectProtocol, NSPoint,
    NSRect, NSSize, NSString, NSURL, NSURLRequest, ns_string,
};
use objc2_web_kit::{
    WKContentRuleList, WKContentRuleListStore, WKContentWorld, WKInactiveSchedulingPolicy, WKNavigation,
    WKSnapshotConfiguration, WKURLSchemeTask, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
};
use serde_json::json;

use super::delegates::{Delegate, url_text};
use super::{network, page};

/// Files one render may ask for.
const FILES_LIMIT: u32 = 512;
/// Requests waiting: the client sends one at a time, so more than this is a client gone wrong.
const QUEUE_LIMIT: usize = 64;
/// How long a page left behind has to stop: longer means its script is spinning.
const PARK_LIMIT_MS: i64 = 1500;

struct Current {
    req: RenderRequest,
    started: Instant,
    files: u32,
    cancelled: bool,
}

struct Page {
    session: String,
    web: Retained<WKWebView>,
    /// The web view's window, never ordered in: nothing to appear on any screen, whatever happens
    /// to the rest. A web view snapshots in it and takes its scale from it.
    window: Retained<NSWindow>,
    _delegate: Retained<Delegate>,
}

struct State {
    out: Sender<(FromHelper, Vec<u8>)>,
    queue: VecDeque<RenderRequest>,
    page: Option<Page>,
    current: Option<Current>,
    /// Compiled once, for every web view.
    rules: Option<Retained<WKContentRuleList>>,
    rules_failed: bool,
    /// The navigation of ours under way, by its URL until it starts and by itself after; any
    /// other is the page's, and refused.
    expect_url: Option<String>,
    navigation: Option<Retained<WKNavigation>>,
    /// Scheme tasks waiting for a file, by token.
    tasks: HashMap<u64, Retained<ProtocolObject<dyn WKURLSchemeTask>>>,
    next_token: u64,
    /// What a callback from before no longer matches.
    generation: u64,
    /// The page left behind is on its way out.
    parked: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(s.borrow_mut().as_mut().expect("the state")))
}

fn mtm() -> MainThreadMarker {
    MainThreadMarker::new().expect("the main thread")
}

/// Whether this system has what it takes to run a page closed off.
pub fn system_is_usable() -> bool {
    let mtm = mtm();
    let (store, config) = unsafe {
        (
            WKWebsiteDataStore::nonPersistentDataStore(mtm),
            WKWebViewConfiguration::new(mtm),
        )
    };
    let prefs = unsafe { config.preferences() };
    network::usable(&store, &prefs)
}

pub fn install(out: Sender<(FromHelper, Vec<u8>)>) {
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            out,
            queue: VecDeque::new(),
            page: None,
            current: None,
            rules: None,
            rules_failed: false,
            expect_url: None,
            navigation: None,
            tasks: HashMap::new(),
            next_token: 1,
            generation: 0,
            parked: false,
        });
    });
    compile_rules();
}

/// In a folder of its own that goes at once: the store keeps what it compiled nowhere else.
fn compile_rules() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let folder = std::env::temp_dir().join(format!("markise-web-helper-{}-{nanos}", std::process::id()));
    let made = {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(&folder)
    };
    let url = NSURL::fileURLWithPath(&NSString::from_str(&folder.to_string_lossy()));
    let store = made
        .ok()
        .and_then(|()| unsafe { WKContentRuleListStore::storeWithURL(Some(&url), mtm()) });
    let Some(store) = store else {
        rules_ready(None);
        return;
    };
    let done = RcBlock::new(move |list: *mut WKContentRuleList, _err: *mut NSError| {
        let _ = std::fs::remove_dir_all(&folder);
        rules_ready(unsafe { Retained::retain(list) });
    });
    unsafe {
        store.compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
            Some(ns_string!("markise-no-network")),
            Some(&NSString::from_str(page::RULES)),
            Some(&done),
        );
    }
}

/// Without its rules a page must not run at all.
fn rules_ready(list: Option<Retained<WKContentRuleList>>) {
    with(|s| {
        s.rules_failed = list.is_none();
        s.rules = list;
    });
    pump();
}

fn new_page(session: &str, rules: &WKContentRuleList) -> Page {
    let mtm = mtm();
    let delegate = Delegate::new(mtm);
    let config = unsafe { WKWebViewConfiguration::new(mtm) };
    unsafe {
        // nothing is kept, and nothing is shared with another session
        let store = WKWebsiteDataStore::nonPersistentDataStore(mtm);
        network::dead_proxy(&store);
        config.setWebsiteDataStore(&store);
        config.userContentController().addContentRuleList(rules);
        config.setURLSchemeHandler_forURLScheme(
            Some(ProtocolObject::from_ref(&*delegate)),
            &NSString::from_str(proto::SCHEME),
        );
        config.setUpgradeKnownHostsToHTTPS(false);
        config.setMediaTypesRequiringUserActionForPlayback(objc2_web_kit::WKAudiovisualMediaTypes::All);
        let prefs = config.preferences();
        prefs.setJavaScriptCanOpenWindowsAutomatically(false);
        network::no_peer_connections(&prefs);
        // safe browsing would ask the network about what loads
        prefs.setFraudulentWebsiteWarningEnabled(false);
        // not suspended for not being on screen
        if prefs.respondsToSelector(objc2::sel!(setInactiveSchedulingPolicy:)) {
            prefs.setInactiveSchedulingPolicy(WKInactiveSchedulingPolicy::None);
        }
    }
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 800.0));
    let web = unsafe { WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), rect, &config) };
    unsafe {
        web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        web.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        // a page's transparent parts stay so (there is no public switch on macOS)
        web.setValue_forKey(Some(&NSNumber::numberWithBool(false)), ns_string!("drawsBackground"));
    }
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
    window.setContentView(Some(&web));
    Page {
        session: session.to_string(),
        web,
        window,
        _delegate: delegate,
    }
}

pub fn received(message: ToHelper, bytes: Vec<u8>) {
    match message {
        ToHelper::Render(req) => {
            let over = with(|s| {
                if s.queue.len() >= QUEUE_LIMIT {
                    return true;
                }
                s.queue.push_back(req);
                false
            });
            if over {
                std::process::exit(2);
            }
            pump();
        }
        ToHelper::Cancel { id } => {
            let answer = with(|s| {
                if let Some(c) = s.current.as_mut().filter(|c| c.req.id == id) {
                    c.cancelled = true;
                    return false;
                }
                let before = s.queue.len();
                s.queue.retain(|r| r.id != id);
                s.queue.len() != before
            });
            if answer {
                send_failed(id, FailKind::Cancelled, "cancelled");
            }
        }
        ToHelper::File { token, found } => file_answered(token, found, bytes),
    }
}

fn send_failed(id: u64, kind: FailKind, reason: &str) {
    let mut reason = reason.to_string();
    reason.truncate(reason.floor_char_boundary(1024));
    with(|s| {
        let _ = s.out.send((FromHelper::Failed { id, kind, reason }, Vec::new()));
    });
}

fn shell_url(generation: u64) -> String {
    format!(
        "{}://{}{}?r={generation}",
        proto::SCHEME,
        proto::PACKAGE_HOST,
        proto::SHELL_PATH
    )
}

fn pump() {
    let next = with(|s| {
        if s.current.is_some() || s.rules.is_none() && !s.rules_failed {
            return None;
        }
        s.queue.pop_front()
    });
    let Some(req) = next else { return };
    if with(|s| s.rules_failed) {
        send_failed(req.id, FailKind::Snapshot, "the content rules did not compile");
        return pump();
    }
    begin(req);
}

/// Navigates the page to a fresh document for this generation.
fn navigate(web: &WKWebView, generation: u64) {
    let url = shell_url(generation);
    let ns = NSURL::URLWithString(&NSString::from_str(&url)).expect("the page's URL");
    with(|s| {
        s.expect_url = Some(url);
        s.navigation = None;
    });
    let navigation = unsafe { web.loadRequest(&NSURLRequest::requestWithURL(&ns)) };
    with(|s| s.navigation = navigation);
}

fn begin(req: RenderRequest) {
    let (web, window, generation) = with(|s| {
        // another session: a store of its own and nothing left of the last
        if s.page.as_ref().is_none_or(|p| p.session != req.session) {
            s.page = Some(new_page(&req.session, s.rules.as_ref().expect("the rules")));
        }
        s.generation += 1;
        s.parked = false;
        s.tasks.clear();
        let p = s.page.as_ref().expect("the page");
        (p.web.clone(), p.window.clone(), s.generation)
    });
    let size = NSSize::new(req.width as f64, 800.0);
    window.setContentSize(size);
    web.setFrameSize(size);
    let limit = req.limit_ms as i64;
    with(|s| {
        s.current = Some(Current {
            req,
            started: Instant::now(),
            files: 0,
            cancelled: false,
        });
    });
    let timer = DispatchTime::NOW.time(limit * 1_000_000);
    let _ = DispatchQueue::main().after(timer, move || timed_out(generation));
    // a fresh document every time: nothing of the last block's code goes on running in it
    navigate(&web, generation);
}

/// The page is not getting anywhere, for whatever reason. Its script may be spinning, in which case
/// no call into it will ever come back and its process has to go: this one exits, and the page's
/// process with it. The client starts another for what comes next.
fn timed_out(generation: u64) {
    let id = with(|s| {
        if s.generation != generation {
            return None;
        }
        s.current.as_ref().map(|c| c.req.id)
    });
    let Some(id) = id else { return };
    send_failed(id, FailKind::Hung, "the page did not finish in time");
    exit_soon();
}

/// Time for the writer to say what is said, then this ends everything.
fn exit_soon() {
    let timer = DispatchTime::NOW.time(100_000_000);
    let _ = DispatchQueue::main().after(timer, || std::process::exit(3));
}

fn finish(answer: Result<(Rendered, Vec<u8>), (FailKind, String)>) {
    let Some(current) = with(|s| s.current.take()) else {
        return;
    };
    with(|s| {
        s.generation += 1;
        s.tasks.clear();
    });
    match answer {
        Ok((rendered, bytes)) => with(|s| {
            let _ = s.out.send((FromHelper::Rendered(rendered), bytes));
        }),
        Err((kind, reason)) => send_failed(current.req.id, kind, &reason),
    }
    if with(|s| s.queue.is_empty()) {
        park();
    } else {
        pump();
    }
}

/// Nothing to do next: the page's code must not go on running, or the machine pays for a document
/// at rest. It goes with its document, unless it is spinning, and then with this process.
fn park() {
    let Some((web, generation)) = with(|s| {
        s.parked = true;
        s.page.as_ref().map(|p| (p.web.clone(), s.generation))
    }) else {
        return;
    };
    navigate(&web, generation);
    let timer = DispatchTime::NOW.time(PARK_LIMIT_MS * 1_000_000);
    let _ = DispatchQueue::main().after(timer, move || {
        if with(|s| s.parked && s.generation == generation) {
            exit_soon();
        }
    });
}

fn fail(kind: FailKind, reason: impl Into<String>) {
    finish(Err((kind, reason.into())));
}

/// Whether this callback's render is still the one going, and still wanted. If not wanted, it ends
/// here.
fn proceed(generation: u64) -> bool {
    let state = with(|s| {
        (s.generation == generation)
            .then(|| s.current.as_ref().map(|c| c.cancelled))
            .flatten()
    });
    match state {
        Some(false) => true,
        Some(true) => {
            fail(FailKind::Cancelled, "cancelled");
            false
        }
        None => false,
    }
}

pub fn navigation_allowed(url: &str) -> bool {
    with(|s| {
        if s.expect_url.as_deref() == Some(url) {
            s.expect_url = None;
            true
        } else {
            false
        }
    })
}

fn is_ours(navigation: Option<&WKNavigation>) -> bool {
    with(|s| match (&s.navigation, navigation) {
        (Some(a), Some(b)) => std::ptr::addr_eq(Retained::as_ptr(a), std::ptr::from_ref(b)),
        _ => false,
    })
}

pub fn page_loaded(navigation: Option<&WKNavigation>) {
    if !is_ours(navigation) {
        return;
    }
    let generation = with(|s| {
        s.navigation = None;
        s.parked = false;
        s.generation
    });
    if !proceed(generation) {
        return;
    }
    let (web, payload) = with(|s| {
        let c = s.current.as_ref().expect("the render");
        let payload = json!({
            "lang": c.req.lang, "dark": c.req.dark, "vars": c.req.vars, "html": c.req.html,
            "styles": c.req.styles.iter().map(|p| proto::file_url(Area::Package, p)).collect::<Vec<_>>(),
        });
        (s.page.as_ref().expect("the page").web.clone(), payload.to_string())
    });
    call(
        &web,
        page::PREPARE,
        "req",
        &payload,
        &isolated_world(),
        generation,
        move |answer| match answer {
            Ok(_) => run_module(generation),
            Err(e) => fail(FailKind::Script, format!("the page: {e}")),
        },
    );
}

fn isolated_world() -> Retained<WKContentWorld> {
    unsafe { WKContentWorld::worldWithName(ns_string!("markise-helper"), mtm()) }
}

pub fn page_failed(navigation: Option<&WKNavigation>, reason: &str) {
    if is_ours(navigation) && with(|s| s.current.is_some()) {
        fail(FailKind::Script, format!("the page did not load: {reason}"));
    }
}

/// The page's process is gone. The next render gets a new one.
pub fn page_crashed(web: &WKWebView) {
    let ours = with(|s| {
        s.page
            .as_ref()
            .is_some_and(|p| std::ptr::addr_eq(Retained::as_ptr(&p.web), std::ptr::from_ref(web)))
    });
    if !ours {
        return;
    }
    with(|s| {
        s.page = None;
        s.parked = false;
    });
    if with(|s| s.current.is_some()) {
        fail(FailKind::Crashed, "the page's process stopped");
    }
}

fn run_module(generation: u64) {
    if !proceed(generation) {
        return;
    }
    let (web, module) = with(|s| {
        let c = s.current.as_ref().expect("the render");
        let url = c.req.module.as_ref().map(|p| proto::file_url(Area::Package, p));
        (s.page.as_ref().expect("the page").web.clone(), url)
    });
    let Some(url) = module else {
        return settle(generation);
    };
    let world = unsafe { WKContentWorld::pageWorld(mtm()) };
    call(
        &web,
        page::RUN_MODULE,
        "url",
        &url,
        &world,
        generation,
        move |answer| match answer {
            Ok(_) => settle(generation),
            Err(e) => fail(FailKind::Script, format!("the block's code: {e}")),
        },
    );
}

fn settle(generation: u64) {
    if !proceed(generation) {
        return;
    }
    let web = with(|s| s.page.as_ref().expect("the page").web.clone());
    call(
        &web,
        page::SETTLE,
        "unused",
        "",
        &isolated_world(),
        generation,
        move |answer| {
            let height = answer.ok().and_then(|text| text.parse::<f64>().ok());
            match height {
                Some(h) if h.is_finite() && h >= 1.0 => snapshot(h.ceil(), generation),
                Some(_) => fail(FailKind::Size, "the block has no height"),
                None => fail(FailKind::Script, "the page gave no height"),
            }
        },
    );
}

/// Calls a script with one string argument; the answer comes as text, if its render still goes.
fn call(
    web: &WKWebView,
    body: &str,
    name: &str,
    value: &str,
    world: &WKContentWorld,
    generation: u64,
    then: impl FnOnce(Result<String, String>) + 'static,
) {
    let value = NSString::from_str(value);
    let key = NSString::from_str(name);
    let args = NSDictionary::<NSString, AnyObject>::from_slices(&[&*key], &[&*value as &AnyObject]);
    let then = RefCell::new(Some(then));
    let done = RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
        let Some(then) = then.borrow_mut().take() else { return };
        if !with(|s| s.generation == generation) {
            return;
        }
        if let Some(e) = unsafe { error.as_ref() } {
            let message = e
                .userInfo()
                .objectForKey(ns_string!("WKJavaScriptExceptionMessage"))
                .and_then(|o| o.downcast_ref::<NSString>().map(|s| s.to_string()))
                .unwrap_or_else(|| e.localizedDescription().to_string());
            return then(Err(message));
        }
        let text = unsafe { result.as_ref() }.map_or(String::new(), |o| {
            if let Some(s) = o.downcast_ref::<NSString>() {
                s.to_string()
            } else if let Some(n) = o.downcast_ref::<NSNumber>() {
                n.doubleValue().to_string()
            } else {
                String::new()
            }
        });
        then(Ok(text));
    });
    unsafe {
        web.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(body),
            Some(&args),
            None,
            world,
            Some(&done),
        );
    }
}

fn snapshot(height: f64, generation: u64) {
    if !proceed(generation) {
        return;
    }
    let (width, scale, web, window) = with(|s| {
        let c = s.current.as_ref().expect("the render");
        let p = s.page.as_ref().expect("the page");
        (c.req.width as f64, c.req.scale, p.web.clone(), p.window.clone())
    });
    let (px_w, px_h) = ((width * scale).ceil(), (height * scale).ceil());
    if px_w > MAX_SIDE_PX as f64 || px_h > MAX_SIDE_PX as f64 || px_w * px_h > MAX_PIXELS as f64 {
        return fail(FailKind::Size, format!("the block is {height} points tall"));
    }
    let size = NSSize::new(width, height);
    window.setContentSize(size);
    web.setFrameSize(size);
    let backing = window.backingScaleFactor();
    let config = unsafe { WKSnapshotConfiguration::new(mtm()) };
    unsafe {
        config.setRect(NSRect::new(NSPoint::new(0.0, 0.0), size));
        // the image's width in points of this window, whatever its screen's scale
        config.setSnapshotWidth(Some(&NSNumber::numberWithDouble(width * scale / backing)));
        config.setAfterScreenUpdates(true);
    }
    let done = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
        if !proceed(generation) {
            return;
        }
        let Some(image) = (unsafe { image.as_ref() }) else {
            let why =
                unsafe { error.as_ref() }.map_or("no image".to_string(), |e| e.localizedDescription().to_string());
            return fail(FailKind::Snapshot, why);
        };
        let Some(cg) = (unsafe { image.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }) else {
            return fail(FailKind::Snapshot, "no bitmap");
        };
        let Some((px_width, px_height, bytes)) = super::raster::rgba(&cg) else {
            return fail(FailKind::Snapshot, "no bytes within the bounds");
        };
        let (id, ms) = with(|s| {
            let c = s.current.as_ref().expect("the render");
            (c.req.id, c.started.elapsed().as_secs_f64() * 1e3)
        });
        let rendered = Rendered {
            id,
            css_height: height,
            px_width,
            px_height,
            ms,
        };
        finish(Ok((rendered, bytes)));
    });
    unsafe { web.takeSnapshotWithConfiguration_completionHandler(Some(&config), &done) };
}

pub fn start_task(task: Retained<ProtocolObject<dyn WKURLSchemeTask>>) {
    let request = unsafe { task.request() };
    let url = request.URL().map_or(String::new(), |u| url_text(&u));
    let shell_path = format!("{}://{}{}?", proto::SCHEME, proto::PACKAGE_HOST, proto::SHELL_PATH);
    if url.starts_with(&shell_path) {
        respond(&task, 200, &url, Some("text/html"), page::shell().as_bytes());
        return;
    }
    // a file for this render's own page, or for nobody
    let document = request.mainDocumentURL().map_or(String::new(), |u| url_text(&u));
    let parsed = proto::parse_file_url(&url);
    let asked = parsed.and_then(|(area, path)| {
        with(|s| {
            let generation = s.generation;
            let c = s.current.as_mut().filter(|c| !c.cancelled)?;
            if document != shell_url(generation) || c.files >= FILES_LIMIT {
                return None;
            }
            c.files += 1;
            let token = s.next_token;
            s.next_token += 1;
            let id = c.req.id;
            s.tasks.insert(token, task.clone());
            let need = FromHelper::NeedFile { token, id, area, path };
            let _ = s.out.send((need, Vec::new()));
            Some(())
        })
    });
    if asked.is_none() {
        respond(&task, 404, &url, None, &[]);
    }
}

pub fn stop_task(task: &ProtocolObject<dyn WKURLSchemeTask>) {
    // after this WebKit takes no more calls for it: one would raise an exception and end the process
    with(|s| {
        s.tasks
            .retain(|_, t| !std::ptr::addr_eq(Retained::as_ptr(t), std::ptr::from_ref(task)));
    });
}

fn file_answered(token: u64, found: bool, bytes: Vec<u8>) {
    let Some(task) = with(|s| s.tasks.remove(&token)) else {
        return;
    };
    let url = unsafe { task.request().URL() }.map_or(String::new(), |u| url_text(&u));
    if found && bytes.len() <= MAX_FILE {
        respond(&task, 200, &url, Some(mime_of(&url)), &bytes);
    } else {
        respond(&task, 404, &url, None, &[]);
    }
}

fn mime_of(url: &str) -> &'static str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn respond(task: &ProtocolObject<dyn WKURLSchemeTask>, status: isize, url: &str, mime: Option<&str>, bytes: &[u8]) {
    let fail_with = |code| {
        let error = unsafe { NSError::errorWithDomain_code_userInfo(ns_string!("markise"), code, None) };
        unsafe { task.didFailWithError(&error) };
    };
    let Some(nsurl) = NSURL::URLWithString(&NSString::from_str(url)) else {
        return fail_with(1);
    };
    let mut keys = vec![ns_string!("Content-Security-Policy")];
    let csp = NSString::from_str(page::CSP);
    let mut values = vec![&*csp];
    let mime = mime.map(NSString::from_str);
    if let Some(m) = &mime {
        keys.push(ns_string!("Content-Type"));
        values.push(m);
    }
    let headers = NSDictionary::<NSString, NSString>::from_slices(&keys, &values);
    let Some(response) = NSHTTPURLResponse::initWithURL_statusCode_HTTPVersion_headerFields(
        NSHTTPURLResponse::alloc(),
        &nsurl,
        status,
        Some(ns_string!("HTTP/1.1")),
        Some(&headers),
    ) else {
        return fail_with(2);
    };
    unsafe {
        task.didReceiveResponse(&response);
        if !bytes.is_empty() {
            task.didReceiveData(&NSData::with_bytes(bytes));
        }
        task.didFinish();
    }
}

//! PROTOTYPE (proto/block-forms): the block edit dialog — one visible, reusable editor window
//! (separate from the hidden snapshot view) with a JSON Forms page (`form/dist/form.js`,
//! bundled into the binary; no network).
//!
//! Requests (Markise → helper, same framing as snapshots, told apart by `op`):
//! - `{"op":"form_open","id","title","block","schema","uischema","data","lang","theme","vars",
//!   "note","site_js","site_css"}` — shows the window (activating the helper) with this form;
//! - `{"op":"form_result","id","ok","error"}` — answer to a `save` event: ok closes the window,
//!   otherwise the error is shown in the form;
//! - `{"op":"form_close"}` — hides the window; `{"op":"form_eval","js"}` — test hook.
//!
//! Events (helper → Markise, unsolicited frames with `event`): `shown` (`id`, `ms_open` = op
//! received → form painted, `ms_page`, `cold`), `save` (`id`, `data`), `cancel` (`id`).

use std::cell::RefCell;
use std::time::Instant;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSApplicationActivationPolicy, NSBackingStoreType,
    NSEventModifierFlags, NSMenu, NSMenuItem, NSRunningApplication, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{NSError, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, ns_string};
use objc2_web_kit::{
    WKScriptMessage, WKScriptMessageHandler, WKUserContentController, WKWebView, WKWebViewConfiguration,
};
use serde_json::{Value, json};

const FORM_JS: &str = include_str!("../form/dist/form.js");
const FORM_CSS: &str = include_str!("../form/dist/form.css");

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "MarkiseFormHandler"]
    #[ivars = ()]
    struct Handler;

    unsafe impl NSObjectProtocol for Handler {}

    unsafe impl WKScriptMessageHandler for Handler {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(&self, _c: &WKUserContentController, m: &WKScriptMessage) {
            let body = unsafe { m.body() };
            if let Some(s) = body.downcast_ref::<NSString>() {
                on_message(&s.to_string());
            }
        }
    }

    unsafe impl NSWindowDelegate for Handler {
        #[unsafe(method(windowShouldClose:))]
        fn should_close(&self, _sender: &NSWindow) -> bool {
            // the close button / ⌘W is Cancel; the window is only ever hidden
            let id = with_editor(|e| e.current);
            if let Some(id) = id {
                emit(json!({"event": "cancel", "id": id}));
            }
            hide();
            false
        }
    }
);

impl Handler {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

struct Editor {
    window: Retained<NSWindow>,
    web: Retained<WKWebView>,
    _handler: Retained<Handler>,
    ready: bool,
    pending: Option<String>,
    current: Option<u64>,
    opened_at: Instant,
    /// The open in flight is the first one of this window (page load included).
    cold: bool,
    created: Instant,
}

thread_local! {
    static ED: RefCell<Option<Editor>> = const { RefCell::new(None) };
}

fn with_editor<R>(f: impl FnOnce(&mut Editor) -> R) -> R {
    ED.with(|e| f(e.borrow_mut().as_mut().expect("editor")))
}

fn emit(v: Value) {
    crate::mac::send_event(v);
}

/// Builds the minimal main menu so ⌘C/⌘V/⌘X/⌘A/⌘Z/⇧⌘Z/⌘W reach the web view.
pub fn install_menu(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    let bar = NSMenu::new(mtm);
    let item = |title: &NSString, action, key: &NSString| unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), title, Some(action), key)
    };
    let app_item = NSMenuItem::new(mtm);
    let app_menu = NSMenu::new(mtm);
    app_menu.addItem(&item(ns_string!("Close"), sel!(performClose:), ns_string!("w")));
    app_item.setSubmenu(Some(&app_menu));
    bar.addItem(&app_item);
    let edit_item = NSMenuItem::new(mtm);
    let edit = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Edit"));
    edit.addItem(&item(ns_string!("Undo"), sel!(undo:), ns_string!("z")));
    let redo = item(ns_string!("Redo"), sel!(redo:), ns_string!("z"));
    redo.setKeyEquivalentModifierMask(NSEventModifierFlags::Command | NSEventModifierFlags::Shift);
    edit.addItem(&redo);
    edit.addItem(&item(ns_string!("Cut"), sel!(cut:), ns_string!("x")));
    edit.addItem(&item(ns_string!("Copy"), sel!(copy:), ns_string!("c")));
    edit.addItem(&item(ns_string!("Paste"), sel!(paste:), ns_string!("v")));
    edit.addItem(&item(ns_string!("Select All"), sel!(selectAll:), ns_string!("a")));
    edit_item.setSubmenu(Some(&edit));
    bar.addItem(&edit_item);
    app.setMainMenu(Some(&bar));
}

fn create(mtm: MainThreadMarker) {
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(560.0, 680.0));
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect,
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Resizable
                | NSWindowStyleMask::Miniaturizable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(ns_string!("Markise"));
    window.center();
    let handler = Handler::new(mtm);
    window.setDelegate(Some(ProtocolObject::from_ref(&*handler)));
    let config = unsafe { WKWebViewConfiguration::new(mtm) };
    unsafe {
        config
            .userContentController()
            .addScriptMessageHandler_name(ProtocolObject::from_ref(&*handler), ns_string!("markise"));
    }
    let web = unsafe { WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), rect, &config) };
    window.setContentView(Some(&web));
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>{}</style></head><body><div id=\"root\"></div><script>{}</script></body></html>",
        FORM_CSS,
        FORM_JS.replace("</script", "<\\/script")
    );
    unsafe {
        let _ = web.loadHTMLString_baseURL(&NSString::from_str(&html), None);
    }
    ED.with(|e| {
        *e.borrow_mut() = Some(Editor {
            window,
            web,
            _handler: handler,
            ready: false,
            pending: None,
            current: None,
            opened_at: Instant::now(),
            cold: true,
            created: Instant::now(),
        })
    });
}

fn eval(js: &str) {
    let web = with_editor(|e| e.web.clone());
    let block = block2::RcBlock::new(|_res: *mut AnyObject, err: *mut NSError| {
        if let Some(e) = unsafe { err.as_ref() } {
            eprintln!("[web-render] form js error: {}", e.localizedDescription());
        }
    });
    unsafe { web.evaluateJavaScript_completionHandler(&NSString::from_str(js), Some(&block)) };
}

fn activate_self(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    // Prohibited processes can never become active: switch to Accessory (still no Dock icon)
    if app.activationPolicy() == NSApplicationActivationPolicy::Prohibited {
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    }
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
}

/// Hides the window and gives the focus back to Markise (the parent process).
pub fn hide() {
    let Some(window) = ED.with(|e| e.borrow().as_ref().map(|e| e.window.clone())) else {
        return;
    };
    with_editor(|e| e.current = None);
    window.orderOut(None);
    eval("window.markiseForm && window.markiseForm.close()");
    let ppid = unsafe { libc::getppid() };
    if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(ppid) {
        #[allow(deprecated)]
        app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
    }
}

/// A form request from Markise (main thread).
pub fn handle(v: Value) {
    let mtm = MainThreadMarker::new().expect("main thread");
    match v["op"].as_str().unwrap_or("") {
        "form_open" => {
            let fresh = ED.with(|e| e.borrow().is_none());
            if fresh {
                create(mtm);
            }
            let id = v["id"].as_u64().unwrap_or(0);
            let site_js = v["site_js"].as_str().unwrap_or("").to_string();
            let mut req = v.clone();
            if let Some(o) = req.as_object_mut() {
                o.remove("op");
                o.remove("site_js");
            }
            let js = format!(
                "try {{ (0, eval)({}); }} catch (e) {{ console.log('site form js', e); }}\nwindow.markiseForm.open({});",
                serde_json::to_string(&site_js).unwrap_or_default(),
                req
            );
            let title = v["title"].as_str().unwrap_or("Markise").to_string();
            let ready = with_editor(|e| {
                e.current = Some(id);
                e.opened_at = Instant::now();
                if !fresh {
                    e.cold = false;
                }
                e.window.setTitle(&NSString::from_str(&title));
                if !e.ready {
                    e.pending = Some(js.clone());
                }
                e.ready
            });
            if ready {
                eval(&js);
            }
            activate_self(mtm);
            let window = with_editor(|e| e.window.clone());
            window.makeKeyAndOrderFront(None);
        }
        "form_result" => {
            if v["ok"] == true {
                hide();
            } else {
                eval(&format!(
                    "window.markiseForm.result({})",
                    json!({"ok": false, "error": v["error"]})
                ));
            }
        }
        "form_close" => hide(),
        "form_shot" => shot(v["path"].as_str().unwrap_or("form.png").to_string()),
        "form_eval" => {
            if ED.with(|e| e.borrow().is_some()) {
                eval(v["js"].as_str().unwrap_or(""));
            }
        }
        _ => {}
    }
}

fn on_message(s: &str) {
    let Ok(m) = serde_json::from_str::<Value>(s) else {
        return;
    };
    match m["event"].as_str().unwrap_or("") {
        "ready" => {
            let pending = with_editor(|e| {
                e.ready = true;
                eprintln!(
                    "[web-render] form page ready {:.1} ms after the window was created",
                    e.created.elapsed().as_secs_f64() * 1e3
                );
                e.pending.take()
            });
            if let Some(js) = pending {
                eval(&js);
            }
        }
        "shown" => {
            let (ms, cold) = with_editor(|e| (e.opened_at.elapsed().as_secs_f64() * 1e3, e.cold));
            emit(json!({"event": "shown", "id": m["id"], "ms_open": ms, "ms_page": m["ms"], "cold": cold}));
        }
        "save" => emit(json!({"event": "save", "id": m["id"], "data": m["data"]})),
        "cancel" => {
            emit(json!({"event": "cancel", "id": m["id"]}));
            hide();
        }
        _ => {}
    }
}

/// Writes a PNG of the editor's page (measurement script; `screencapture` may be unavailable).
fn shot(path: String) {
    let Some(web) = ED.with(|e| e.borrow().as_ref().map(|e| e.web.clone())) else {
        return;
    };
    let block = block2::RcBlock::new(move |img: *mut objc2_app_kit::NSImage, _err: *mut NSError| {
        let Some(img) = (unsafe { img.as_ref() }) else { return };
        let Some(cg) = (unsafe { img.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }) else {
            return;
        };
        let rep = objc2_app_kit::NSBitmapImageRep::initWithCGImage(objc2_app_kit::NSBitmapImageRep::alloc(), &cg);
        let png = unsafe {
            rep.representationUsingType_properties(
                objc2_app_kit::NSBitmapImageFileType::PNG,
                &objc2_foundation::NSDictionary::new(),
            )
        };
        if let Some(d) = png {
            let r = std::fs::write(&path, d.to_vec());
            eprintln!("[web-render] form screenshot {path}: {r:?}");
        }
    });
    unsafe { web.takeSnapshotWithConfiguration_completionHandler(None, &block) };
}

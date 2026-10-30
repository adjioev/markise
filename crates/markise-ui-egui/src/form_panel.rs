//! PROTOTYPE (branch `proto/block-forms`, not product code): the block edit form as a right
//! side panel docked inside the Markise window (macOS only).
//!
//! One `WKWebView` lives in the Markise process as a child `NSView` of the main window's
//! content view, positioned every frame over the rectangle egui reserves for the panel; it is
//! created lazily on the first open and kept (hidden) afterwards, so later opens are warm. It
//! shows the JSON Forms page (`form/dist/form.js`, bundled, offline). Block snapshots still
//! come from the helper process; this is the only web view in Markise itself.
//!
//! Requests ([`handle`]): `form_open` (title, block, schema, uischema, data, lang, theme, vars,
//! note, site_js, site_css), `form_result` (ok / error), `form_close`, `form_eval` and
//! `form_shot` (measurement hooks). Events (to the sink set with [`set_sink`]): `shown`
//! (`ms_open`, `ms_page`, `cold`), `save` (`data`), `cancel`.
//!
//! Known limitation: egui popups/menus overlapping the panel rectangle are drawn under the
//! native view (hidden).

use std::sync::mpsc::Sender;

use serde_json::Value;

#[cfg(target_os = "macos")]
pub use mac::{forward, frame_end, frame_start, handle, place, set_sink};

#[cfg(not(target_os = "macos"))]
mod other {
    use super::*;
    pub fn set_sink(_s: Sender<Value>, _ctx: egui::Context) {}
    pub fn handle(v: Value) {
        let _ = v;
        eprintln!("[blockform] the edit panel is macOS only in this prototype");
    }
    pub fn place(_r: egui::Rect, _scale: f32) {}
    pub fn frame_start() {}
    pub fn frame_end() {}
    pub fn forward(_id: &str) -> bool {
        false
    }
}
#[cfg(not(target_os = "macos"))]
pub use other::{forward, frame_end, frame_start, handle, place, set_sink};

/// Width of the panel in egui points.
pub const WIDTH: f32 = 440.0;

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::RefCell;
    use std::time::Instant;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
    use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send};
    use objc2_app_kit::{NSApplication, NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSWindow};
    use objc2_foundation::{NSDictionary, NSError, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, ns_string};
    use objc2_web_kit::{
        WKScriptMessage, WKScriptMessageHandler, WKUserContentController, WKWebView, WKWebViewConfiguration,
    };
    use serde_json::json;

    use super::*;

    const FORM_JS: &str = include_str!("../form/dist/form.js");
    const FORM_CSS: &str = include_str!("../form/dist/form.css");

    define_class!(
        // SAFETY: NSObject has no subclassing requirements; no Drop.
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[name = "MarkiseFormPanelHandler"]
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
    );

    impl Handler {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(());
            unsafe { msg_send![super(this), init] }
        }
    }

    struct Panel {
        window: Retained<NSWindow>,
        web: Retained<WKWebView>,
        _handler: Retained<Handler>,
        ready: bool,
        pending: Option<String>,
        open: bool,
        opened_at: Instant,
        cold: bool,
        created: Instant,
        /// Placed by egui this frame / last frame rect (window points).
        placed: bool,
        frame: Option<NSRect>,
    }

    thread_local! {
        static P: RefCell<Option<Panel>> = const { RefCell::new(None) };
        static SINK: RefCell<Option<(Sender<Value>, egui::Context)>> = const { RefCell::new(None) };
    }

    fn with<R>(f: impl FnOnce(&mut Panel) -> R) -> Option<R> {
        P.with(|p| p.borrow_mut().as_mut().map(f))
    }

    fn emit(v: Value) {
        SINK.with(|s| {
            if let Some((s, ctx)) = s.borrow().as_ref() {
                let _ = s.send(v);
                ctx.request_repaint();
            }
        });
    }

    pub fn set_sink(s: Sender<Value>, ctx: egui::Context) {
        SINK.with(|x| *x.borrow_mut() = Some((s, ctx)));
    }

    fn main_window(mtm: MainThreadMarker) -> Option<Retained<NSWindow>> {
        let app = NSApplication::sharedApplication(mtm);
        app.mainWindow()
            .or_else(|| app.keyWindow())
            .or_else(|| app.windows().firstObject())
    }

    fn create(mtm: MainThreadMarker) -> Result<(), String> {
        let window = main_window(mtm).ok_or("no Markise window")?;
        let content = window.contentView().ok_or("no content view")?;
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(super::WIDTH as f64, 400.0));
        let handler = Handler::new(mtm);
        let config = unsafe { WKWebViewConfiguration::new(mtm) };
        unsafe {
            config
                .userContentController()
                .addScriptMessageHandler_name(ProtocolObject::from_ref(&*handler), ns_string!("markise"));
        }
        let web = unsafe { WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), rect, &config) };
        web.setHidden(true);
        content.addSubview(&web);
        let html = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><style>{FORM_CSS}\nhtml{{border-left:1px solid var(--line)}}</style></head><body><div id=\"root\"></div><script>{}</script></body></html>",
            FORM_JS.replace("</script", "<\\/script")
        );
        unsafe {
            let _ = web.loadHTMLString_baseURL(&NSString::from_str(&html), None);
        }
        P.with(|p| {
            *p.borrow_mut() = Some(Panel {
                window,
                web,
                _handler: handler,
                ready: false,
                pending: None,
                open: false,
                opened_at: Instant::now(),
                cold: true,
                created: Instant::now(),
                placed: false,
                frame: None,
            })
        });
        Ok(())
    }

    fn eval(js: &str) {
        let Some(web) = with(|p| p.web.clone()) else { return };
        let block = block2::RcBlock::new(|_res: *mut AnyObject, err: *mut NSError| {
            if let Some(e) = unsafe { err.as_ref() } {
                eprintln!("[blockform] form js error: {}", e.localizedDescription());
            }
        });
        unsafe { web.evaluateJavaScript_completionHandler(&NSString::from_str(js), Some(&block)) };
    }

    fn hide() {
        let Some((web, window)) = with(|p| {
            p.open = false;
            (p.web.clone(), p.window.clone())
        }) else {
            return;
        };
        web.setHidden(true);
        eval("window.markiseForm && window.markiseForm.close()");
        // keyboard back to egui (winit's view is the content view)
        if let Some(c) = window.contentView() {
            window.makeFirstResponder(Some(&c));
        }
    }

    /// Called at the start of every Markise frame; [`frame_end`] hides the panel when no pane
    /// placed it this frame (Side by Side left, document closed).
    pub fn frame_start() {
        with(|p| p.placed = false);
    }

    pub fn frame_end() {
        let Some((open, placed, hidden, web, window)) =
            with(|p| (p.open, p.placed, p.web.isHidden(), p.web.clone(), p.window.clone()))
        else {
            return;
        };
        let show = open && placed;
        if show == hidden {
            web.setHidden(!show);
            if show {
                window.makeFirstResponder(Some(&web));
            }
        }
    }

    /// Puts the panel over `r` (egui points; `scale` = window points per egui point).
    pub fn place(r: egui::Rect, scale: f32) {
        with(|p| {
            p.placed = true;
            let Some(content) = p.window.contentView() else { return };
            let b = content.bounds();
            let (x, w, h) = (
                (r.left() * scale) as f64,
                (r.width() * scale) as f64,
                (r.height() * scale) as f64,
            );
            let top = (r.top() * scale) as f64;
            let y = if content.isFlipped() {
                top
            } else {
                b.size.height - top - h
            };
            let f = NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
            if p.frame != Some(f) {
                if std::env::var_os("MARKISE_BLOCK_LOG").is_some() {
                    eprintln!("[blockform] panel at egui rect {r:?} → window frame {f:?}");
                }
                p.web.setFrame(f);
                p.frame = Some(f);
            }
        });
    }

    /// Markise's menu key equivalents (⌘C/⌘V/⌘X/⌘A/⌘Z/⇧⌘Z, ⌘S, ⌘W) are custom menu items that
    /// would act on the document: while the panel has the keyboard focus they go to the web
    /// view instead. Returns true when `id` was handled here.
    pub fn forward(id: &str) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let Some((web, window, open)) = with(|p| (p.web.clone(), p.window.clone(), p.open)) else {
            return false;
        };
        if !open || web.isHidden() {
            return false;
        }
        let focused = window
            .firstResponder()
            .and_then(|r| r.downcast::<objc2_app_kit::NSView>().ok())
            .is_some_and(|v| v.isDescendantOf(&web));
        if !focused {
            return false;
        }
        let action = match id {
            "edit.cut" => objc2::sel!(cut:),
            "edit.copy" => objc2::sel!(copy:),
            "edit.paste" => objc2::sel!(paste:),
            "edit.undo" => objc2::sel!(undo:),
            "edit.redo" => objc2::sel!(redo:),
            "select.all" => objc2::sel!(selectAll:),
            "file.save" => {
                eval("window.markiseForm && window.markiseForm.save()");
                return true;
            }
            "file.close" => {
                eval("window.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape'}))");
                return true;
            }
            _ => return false,
        };
        unsafe { NSApplication::sharedApplication(mtm).sendAction_to_from(action, None, None) };
        true
    }

    /// A request from Markise (UI thread = main thread).
    pub fn handle(v: Value) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        match v["op"].as_str().unwrap_or("") {
            "form_open" => {
                let fresh = P.with(|p| p.borrow().is_none());
                if fresh && let Err(e) = create(mtm) {
                    eprintln!("[blockform] cannot create the panel: {e}");
                    return;
                }
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
                let ready = with(|p| {
                    p.open = true;
                    p.opened_at = Instant::now();
                    if !fresh {
                        p.cold = false;
                    }
                    if !p.ready {
                        p.pending = Some(js.clone());
                    }
                    p.ready
                })
                .unwrap_or(false);
                if ready {
                    eval(&js);
                }
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
            "form_eval" => eval(v["js"].as_str().unwrap_or("")),
            "form_shot" => shot(v["path"].as_str().unwrap_or("form.png").to_string()),
            _ => {}
        }
    }

    fn on_message(s: &str) {
        let Ok(m) = serde_json::from_str::<Value>(s) else {
            return;
        };
        match m["event"].as_str().unwrap_or("") {
            "ready" => {
                let pending = with(|p| {
                    p.ready = true;
                    if std::env::var_os("MARKISE_BLOCK_LOG").is_some() {
                        eprintln!(
                            "[blockform] panel page ready {:.1} ms after the web view was created",
                            p.created.elapsed().as_secs_f64() * 1e3
                        );
                    }
                    p.pending.take()
                })
                .flatten();
                if let Some(js) = pending {
                    eval(&js);
                }
            }
            "shown" => {
                if let Some((ms, cold)) = with(|p| (p.opened_at.elapsed().as_secs_f64() * 1e3, p.cold)) {
                    emit(json!({"event": "shown", "id": m["id"], "ms_open": ms, "ms_page": m["ms"], "cold": cold}));
                }
            }
            "save" => emit(json!({"event": "save", "id": m["id"], "data": m["data"]})),
            "cancel" => {
                emit(json!({"event": "cancel", "id": m["id"]}));
                hide();
            }
            _ => {}
        }
    }

    /// Writes a PNG of the panel's page (measurement script).
    fn shot(path: String) {
        let Some(web) = with(|p| p.web.clone()) else { return };
        let block = block2::RcBlock::new(move |img: *mut NSImage, _err: *mut NSError| {
            let Some(img) = (unsafe { img.as_ref() }) else { return };
            let Some(cg) = (unsafe { img.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) })
            else {
                return;
            };
            let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
            let png =
                unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) };
            if let Some(d) = png {
                let r = std::fs::write(&path, d.to_vec());
                eprintln!("[blockform] panel screenshot {path}: {r:?}");
            }
        });
        unsafe { web.takeSnapshotWithConfiguration_completionHandler(None, &block) };
    }
}

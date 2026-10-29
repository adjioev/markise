//! macOS: whether a menu is open (issue #81).
//!
//! AppKit posts `NSMenuDidBeginTrackingNotification` when the user opens a menu (from the menu
//! bar, or a context menu) and `NSMenuDidEndTrackingNotification` when it closes. An observer
//! records whether a tracking session is running (AppKit pairs every begin with an end); the
//! native menu bar makes no native change while one is ([`markise_ui_egui::menu_sync`]), and
//! the end of a session wakes the UI so a change held back meanwhile is applied with the next
//! frame.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject};
use objc2::{ClassType, define_class, msg_send, sel};

static OPEN: AtomicBool = AtomicBool::new(false);
static CONTEXT: OnceLock<egui::Context> = OnceLock::new();

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    static NSMenuDidBeginTrackingNotification: *const AnyObject;
    static NSMenuDidEndTrackingNotification: *const AnyObject;
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the class has no ivars and no Drop.
    #[unsafe(super(NSObject))]
    #[name = "MarkiseMenuTrackingObserver"]
    struct Observer;

    impl Observer {
        #[unsafe(method(menuDidBeginTracking:))]
        fn began(&self, _note: *mut AnyObject) {
            OPEN.store(true, Ordering::SeqCst);
        }

        #[unsafe(method(menuDidEndTracking:))]
        fn ended(&self, _note: *mut AnyObject) {
            OPEN.store(false, Ordering::SeqCst);
            if let Some(ctx) = CONTEXT.get() {
                ctx.request_repaint();
            }
        }
    }
);

/// Starts observing menu tracking (once; later calls do nothing). `ctx` is woken when a menu
/// closes.
pub fn install(ctx: &egui::Context) {
    if CONTEXT.set(ctx.clone()).is_err() {
        return;
    }
    let Some(center_cls) = AnyClass::get(c"NSNotificationCenter") else {
        return;
    };
    // SAFETY: `Observer` is a registered NSObject subclass (`new` returns a +1 instance);
    // `defaultCenter` and `addObserver:selector:name:object:` exist with these types, the
    // selectors are methods of `Observer` taking one object, and the notification names are
    // AppKit's NSString constants. The observer is leaked: it must outlive the app.
    unsafe {
        let observer: Retained<Observer> = msg_send![Observer::class(), new];
        let center: *mut AnyObject = msg_send![center_cls, defaultCenter];
        let Some(center) = center.as_ref() else { return };
        let nil: *const AnyObject = std::ptr::null();
        let _: () = msg_send![
            center,
            addObserver: &*observer,
            selector: sel!(menuDidBeginTracking:),
            name: NSMenuDidBeginTrackingNotification,
            object: nil
        ];
        let _: () = msg_send![
            center,
            addObserver: &*observer,
            selector: sel!(menuDidEndTracking:),
            name: NSMenuDidEndTrackingNotification,
            object: nil
        ];
        std::mem::forget(observer);
    }
}

/// Whether a menu is open now.
pub fn is_tracking() -> bool {
    OPEN.load(Ordering::SeqCst)
}

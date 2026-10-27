//! macOS "Open With…", double-clicking a document and dropping files on the Dock icon.
//!
//! Finder does not pass such files as arguments: it sends an "open documents" Apple Event,
//! which AppKit forwards to `-[NSApplicationDelegate application:openURLs:]`. winit 0.30 does
//! not implement that method, so [`install`] adds it to winit's delegate class at run time,
//! after the event loop (and with it the delegate) exists and before the app finishes launching
//! (the event that launched the app arrives between `applicationWillFinishLaunching:` and
//! `applicationDidFinishLaunching:`). Received paths are queued; the app drains the queue at the
//! start of each frame ([`take_pending`]) and the handler requests a repaint once the egui
//! context is known.

use std::ffi::{CStr, c_char};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use eframe::egui;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{msg_send, sel};

static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static CONTEXT: OnceLock<egui::Context> = OnceLock::new();

/// Adds `application:openURLs:` to winit's application delegate class. Returns false if the
/// class is missing (winit renamed it) or already has the method.
pub fn install() -> bool {
    let Some(class) = AnyClass::get(c"WinitApplicationDelegate") else {
        return false;
    };
    type OpenUrls = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject);
    let imp: OpenUrls = open_urls;
    // SAFETY: the implementation matches the method's type encoding `v@:@@` (void return;
    // self, _cmd, NSApplication *, NSArray<NSURL *> *); `Imp` is the untyped function pointer
    // the runtime expects. Adding a method to a registered class is allowed at any time.
    let added: Bool = unsafe {
        objc2::ffi::class_addMethod(
            std::ptr::from_ref(class).cast_mut(),
            sel!(application:openURLs:),
            std::mem::transmute::<OpenUrls, Imp>(imp),
            c"v@:@@".as_ptr(),
        )
    };
    added.as_bool()
}

/// Lets the handler wake the UI when files arrive while the app is running.
pub fn set_context(ctx: egui::Context) {
    let _ = CONTEXT.set(ctx);
}

/// Paths received since the last call, in arrival order.
pub fn take_pending() -> Vec<PathBuf> {
    match PENDING.lock() {
        Ok(mut q) if !q.is_empty() => std::mem::take(&mut *q),
        _ => Vec::new(),
    }
}

unsafe extern "C-unwind" fn open_urls(_this: *mut AnyObject, _cmd: Sel, _app: *mut AnyObject, urls: *mut AnyObject) {
    // SAFETY: AppKit passes a valid NSArray of NSURL; all messages below exist on those classes
    // and their return types match the declarations.
    let paths = unsafe { file_paths(urls) };
    if paths.is_empty() {
        return;
    }
    if let Ok(mut q) = PENDING.lock() {
        q.extend(paths);
    }
    if let Some(ctx) = CONTEXT.get() {
        ctx.request_repaint();
    }
}

/// File-system paths of the `file://` URLs in an `NSArray<NSURL *>`.
unsafe fn file_paths(urls: *mut AnyObject) -> Vec<PathBuf> {
    let mut out = Vec::new();
    // SAFETY: see `open_urls`.
    unsafe {
        let Some(urls) = urls.as_ref() else {
            return out;
        };
        let count: usize = msg_send![urls, count];
        for i in 0..count {
            let url: *mut AnyObject = msg_send![urls, objectAtIndex: i];
            let Some(url) = url.as_ref() else { continue };
            let is_file: bool = msg_send![url, isFileURL];
            if !is_file {
                continue;
            }
            let path: *mut AnyObject = msg_send![url, path];
            let Some(path) = path.as_ref() else { continue };
            let utf8: *const c_char = msg_send![path, UTF8String];
            if utf8.is_null() {
                continue;
            }
            out.push(PathBuf::from(CStr::from_ptr(utf8).to_string_lossy().into_owned()));
        }
    }
    out
}

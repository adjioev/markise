//! No way out. The content rules and the policy of a page stop what a page asks for by URL; these
//! close what they do not see (ADR 0023). Each one was found by the self-test with a listener:
//!
//! - A dead proxy on the data store: the engine sends everything HTTP-like there, resolving no name
//!   itself, also what no rule is asked about (a link's click preconnects before any policy).
//! - The engine's own features off, by name: peer connections, whose UDP goes around both rules and
//!   proxy, and a link's preconnect, which happens before any policy, in any frame, and no rule is
//!   asked. No public switch exists, so a page is also checked for having no peer connections before
//!   any of a package's code runs ([`page::PREPARE`]).
//! - This process itself: it has no business with the network or with the user's files.
//!
//! Without any of these, there is no helper: [`usable`] says so before anything loads.

use std::ffi::{CStr, CString, c_char, c_void};

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2_foundation::{NSArray, NSObject, NSString};
use objc2_web_kit::{WKPreferences, WKWebsiteDataStore};

#[link(name = "Network", kind = "framework")]
unsafe extern "C" {
    fn nw_endpoint_create_host(hostname: *const c_char, port: *const c_char) -> *mut c_void;
}

type CreateProxy = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type SetFailover = unsafe extern "C" fn(*mut c_void, bool);

/// Looked up when it runs: they came with macOS 14, and the app starts on older ones, where linking
/// them would stop this program before it could say it has no engine.
fn proxy_functions() -> Option<(CreateProxy, SetFailover)> {
    // for a test of what an older system does: it can only take something away
    if std::env::var_os("MARKISE_WEB_HELPER_NO_PROXY_API").is_some() {
        return None;
    }
    // SAFETY: valid names; the signatures are those of Network.framework's headers
    unsafe {
        let create = libc::dlsym(libc::RTLD_DEFAULT, c"nw_proxy_config_create_socksv5".as_ptr());
        let failover = libc::dlsym(libc::RTLD_DEFAULT, c"nw_proxy_config_set_failover_allowed".as_ptr());
        if create.is_null() || failover.is_null() {
            return None;
        }
        Some((
            std::mem::transmute::<*mut c_void, CreateProxy>(create),
            std::mem::transmute::<*mut c_void, SetFailover>(failover),
        ))
    }
}

unsafe extern "C" {
    fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> i32;
    fn sandbox_free_error(errorbuf: *mut c_char);
}

/// What must be off for a page to run at all.
const MUST_BE_OFF: &[&str] = &["PeerConnectionEnabled", "LinkPreconnect", "LinkDNSPrefetchEnabled"];
/// What is off where this engine knows it: others that reach out with nobody asked.
const ALSO_OFF: &[&str] = &[
    "LinkPreconnectEarlyHintsEnabled",
    "LinkPrefetchEnabled",
    "SpeculationRulesPrefetchEnabled",
    "WebTransportEnabled",
];

/// What can ask a person, or the system on their behalf: nobody is there, and a request can wait for ever
/// (capture did, on a CI runner). A page has none of these to ask with.
const NOTHING_TO_ASK: &[&str] = &[
    "MediaDevicesEnabled",
    "ScreenCaptureEnabled",
    "SpeechRecognitionEnabled",
    "GeolocationAPIEnabled",
    "NotificationsEnabled",
    "WebShareEnabled",
    "AsyncClipboardAPIEnabled",
];

/// A page nobody sees is still to run on time: left on, its timers wait for a second or more, and on a
/// busy machine its process can be held back, and a render runs out of its limit doing nothing.
const KEEP_ON_TIME: &[&str] = &[
    "HiddenPageDOMTimerThrottlingEnabled",
    "DOMTimersThrottlingEnabled",
    "PageVisibilityBasedProcessSuppressionEnabled",
    "BackgroundWebContentRunningBoardThrottlingEnabled",
];

/// The engine's features with their names.
fn features() -> Vec<(String, Retained<NSObject>)> {
    let class = objc2::class!(WKPreferences);
    // SAFETY: every class answers this
    let there: bool = unsafe { msg_send![class, respondsToSelector: objc2::sel!(_features)] };
    if !there {
        return Vec::new();
    }
    // SAFETY: it responds; it answers an array of feature objects, each with a key
    let list: Option<Retained<NSArray<NSObject>>> = unsafe { msg_send![class, _features] };
    let list = list.map_or(Vec::new(), |l| l.to_vec());
    let named = list.into_iter().filter_map(|f| {
        let key: Option<Retained<NSString>> = unsafe { msg_send![&*f, key] };
        Some((key?.to_string(), f))
    });
    named.collect()
}

/// Whether this system has what it takes (macOS 14 for the proxy).
pub fn usable(store: &WKWebsiteDataStore, prefs: &WKPreferences) -> bool {
    let known = features();
    proxy_functions().is_some()
        && store.respondsToSelector(objc2::sel!(setProxyConfigurations:))
        && prefs.respondsToSelector(objc2::sel!(_setEnabled:forFeature:))
        && MUST_BE_OFF.iter().all(|name| known.iter().any(|(k, _)| k == name))
}

/// Everything HTTP-like of this store goes to a port nothing listens on, with no failover.
pub fn dead_proxy(store: &WKWebsiteDataStore) {
    let (host, port): (&CStr, &CStr) = (c"127.0.0.1", c"9");
    let Some((create_proxy, set_failover)) = proxy_functions() else {
        return; // [`usable`] said so already
    };
    // SAFETY: valid C strings; the three objects are retained by what they are handed to, and the
    // few that stay for the life of a store are no leak to speak of in a process of one purpose
    unsafe {
        let endpoint = nw_endpoint_create_host(host.as_ptr(), port.as_ptr());
        let proxy = create_proxy(endpoint);
        set_failover(proxy, false);
        let object: &AnyObject = &*(proxy as *const AnyObject);
        let list = NSArray::from_slice(&[object]);
        let _: () = msg_send![store, setProxyConfigurations: &*list];
    }
}

pub fn engine_features_off(prefs: &WKPreferences) {
    for (name, feature) in features() {
        let name = name.as_str();
        let off = [MUST_BE_OFF, ALSO_OFF, NOTHING_TO_ASK, KEEP_ON_TIME];
        if off.iter().any(|list| list.contains(&name)) {
            // SAFETY: [`usable`] checked it responds
            let _: () = unsafe { msg_send![prefs, _setEnabled: false, forFeature: &*feature] };
        }
    }
}

/// From here on this process reaches no network and no file of the user. `network`: the self-test
/// listens for what gets through, in this process. WebKit's own processes have sandboxes of theirs.
pub fn confine(network: bool) -> bool {
    let home = std::env::var("HOME").unwrap_or_default().replace(['\\', '"'], "");
    let deny_network = if network { "" } else { "(deny network*)" };
    let profile = format!(
        "(version 1)(allow default){deny_network}(deny file-write*)\
         (allow file-write* (subpath \"/private/var/folders\") (subpath \"/private/tmp\") (subpath \"/dev\"))\
         {}",
        if home.is_empty() {
            String::new()
        } else {
            format!("(deny file-read* (subpath \"{home}\"))")
        }
    );
    let Ok(profile) = CString::new(profile) else {
        return false;
    };
    let mut error = std::ptr::null_mut();
    // SAFETY: a valid C string; the error, if any, is freed by its own function
    let result = unsafe { sandbox_init(profile.as_ptr(), 0, &mut error) };
    if !error.is_null() {
        unsafe { sandbox_free_error(error) };
    }
    result == 0
}

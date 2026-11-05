//! No way out. The content rules and the policy of a page stop what a page asks for by URL; these
//! close what they do not see (ADR 0023). Each one was found by the self-test with a listener:
//!
//! - A dead proxy on the data store: the engine sends everything HTTP-like there, resolving no name
//!   itself, also what no rule is asked about (a link's click preconnects before any policy).
//! - Peer connections off: their UDP goes around both rules and proxy. No public switch exists, so a
//!   page is also checked for not having them before any of a package's code runs ([`page::PREPARE`]).
//! - This process itself: it has no business with the network or with the user's files.
//!
//! Without any of these, there is no helper: [`usable`] says so before anything loads.

use std::ffi::{CStr, CString, c_char, c_void};

use objc2::msg_send;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2_foundation::NSArray;
use objc2_web_kit::{WKPreferences, WKWebsiteDataStore};

#[link(name = "Network", kind = "framework")]
unsafe extern "C" {
    fn nw_endpoint_create_host(hostname: *const c_char, port: *const c_char) -> *mut c_void;
    fn nw_proxy_config_create_socksv5(proxy: *mut c_void) -> *mut c_void;
    fn nw_proxy_config_set_failover_allowed(config: *mut c_void, allowed: bool);
}

unsafe extern "C" {
    fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> i32;
    fn sandbox_free_error(errorbuf: *mut c_char);
}

/// Whether this system has what it takes (macOS 14 for the proxy).
pub fn usable(store: &WKWebsiteDataStore, prefs: &WKPreferences) -> bool {
    store.respondsToSelector(objc2::sel!(setProxyConfigurations:))
        && prefs.respondsToSelector(objc2::sel!(_setPeerConnectionEnabled:))
}

/// Everything HTTP-like of this store goes to a port nothing listens on, with no failover.
pub fn dead_proxy(store: &WKWebsiteDataStore) {
    let (host, port): (&CStr, &CStr) = (c"127.0.0.1", c"9");
    // SAFETY: valid C strings; the three objects are retained by what they are handed to, and the
    // few that stay for the life of a store are no leak to speak of in a process of one purpose
    unsafe {
        let endpoint = nw_endpoint_create_host(host.as_ptr(), port.as_ptr());
        let proxy = nw_proxy_config_create_socksv5(endpoint);
        nw_proxy_config_set_failover_allowed(proxy, false);
        let object: &AnyObject = &*(proxy as *const AnyObject);
        let list = NSArray::from_slice(&[object]);
        let _: () = msg_send![store, setProxyConfigurations: &*list];
    }
}

pub fn no_peer_connections(prefs: &WKPreferences) {
    // SAFETY: [`usable`] checked it responds
    let _: () = unsafe { msg_send![prefs, _setPeerConnectionEnabled: false] };
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

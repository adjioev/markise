//! The classes WebKit calls: files through the client, navigation only to the page itself, and no
//! windows or dialogs. Whatever a page asks here, it gets nothing and nothing waits.

use block2::DynBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_foundation::{NSArray, NSError, NSString, NSURL};
use objc2_web_kit::{
    WKFrameInfo, WKMediaCaptureType, WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate,
    WKNavigationResponse, WKNavigationResponsePolicy, WKOpenPanelParameters, WKPermissionDecision, WKSecurityOrigin,
    WKUIDelegate, WKURLSchemeHandler, WKURLSchemeTask, WKWebView, WKWebViewConfiguration, WKWindowFeatures,
};

use super::render;

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "MarkiseWebHelperDelegate"]
    pub struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl WKURLSchemeHandler for Delegate {
        #[unsafe(method(webView:startURLSchemeTask:))]
        unsafe fn start_task(&self, _web: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            render::start_task(task.retain());
        }

        #[unsafe(method(webView:stopURLSchemeTask:))]
        unsafe fn stop_task(&self, _web: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            render::stop_task(task);
        }
    }

    unsafe impl WKNavigationDelegate for Delegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        unsafe fn decide_action(
            &self,
            _web: &WKWebView,
            action: &WKNavigationAction,
            decide: &DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let url = unsafe { action.request().URL() }.and_then(|u| u.absoluteString());
            let main = unsafe { action.targetFrame() }.is_some_and(|f| unsafe { f.isMainFrame() });
            let allow = main && url.is_some_and(|u| render::navigation_allowed(&u.to_string()));
            decide.call((if allow {
                WKNavigationActionPolicy::Allow
            } else {
                WKNavigationActionPolicy::Cancel
            },));
        }

        #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
        unsafe fn decide_response(
            &self,
            _web: &WKWebView,
            response: &WKNavigationResponse,
            decide: &DynBlock<dyn Fn(WKNavigationResponsePolicy)>,
        ) {
            let main = unsafe { response.isForMainFrame() };
            decide.call((if main {
                WKNavigationResponsePolicy::Allow
            } else {
                WKNavigationResponsePolicy::Cancel
            },));
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        unsafe fn did_finish(&self, _web: &WKWebView, navigation: Option<&WKNavigation>) {
            render::page_loaded(navigation);
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        unsafe fn did_fail_early(&self, _web: &WKWebView, navigation: Option<&WKNavigation>, error: &NSError) {
            render::page_failed(navigation, &error.localizedDescription().to_string());
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        unsafe fn did_fail(&self, _web: &WKWebView, navigation: Option<&WKNavigation>, error: &NSError) {
            render::page_failed(navigation, &error.localizedDescription().to_string());
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        unsafe fn did_terminate(&self, web: &WKWebView) {
            render::page_crashed(web);
        }
    }

    unsafe impl WKUIDelegate for Delegate {
        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        unsafe fn create(
            &self,
            _web: &WKWebView,
            _configuration: &WKWebViewConfiguration,
            _action: &WKNavigationAction,
            _features: &WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            None
        }

        #[unsafe(method(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:))]
        unsafe fn alert(&self, _web: &WKWebView, _message: &NSString, _frame: &WKFrameInfo, done: &DynBlock<dyn Fn()>) {
            done.call(());
        }

        #[unsafe(method(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:))]
        unsafe fn confirm(
            &self,
            _web: &WKWebView,
            _message: &NSString,
            _frame: &WKFrameInfo,
            done: &DynBlock<dyn Fn(Bool)>,
        ) {
            done.call((Bool::NO,));
        }

        #[unsafe(method(webView:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:))]
        unsafe fn prompt(
            &self,
            _web: &WKWebView,
            _prompt: &NSString,
            _text: Option<&NSString>,
            _frame: &WKFrameInfo,
            done: &DynBlock<dyn Fn(*mut NSString)>,
        ) {
            done.call((std::ptr::null_mut(),));
        }

        #[unsafe(method(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:))]
        unsafe fn capture(
            &self,
            _web: &WKWebView,
            _origin: &WKSecurityOrigin,
            _frame: &WKFrameInfo,
            _kind: WKMediaCaptureType,
            decide: &DynBlock<dyn Fn(WKPermissionDecision)>,
        ) {
            decide.call((WKPermissionDecision::Deny,));
        }

        #[unsafe(method(webView:requestDeviceOrientationAndMotionPermissionForOrigin:initiatedByFrame:decisionHandler:))]
        unsafe fn motion(
            &self,
            _web: &WKWebView,
            _origin: &WKSecurityOrigin,
            _frame: &WKFrameInfo,
            decide: &DynBlock<dyn Fn(WKPermissionDecision)>,
        ) {
            decide.call((WKPermissionDecision::Deny,));
        }

        #[unsafe(method(webView:runOpenPanelWithParameters:initiatedByFrame:completionHandler:))]
        unsafe fn open_panel(
            &self,
            _web: &WKWebView,
            _parameters: &WKOpenPanelParameters,
            _frame: &WKFrameInfo,
            done: &DynBlock<dyn Fn(*mut NSArray<NSURL>)>,
        ) {
            done.call((std::ptr::null_mut(),));
        }
    }
);

impl Delegate {
    pub fn new(mtm: MainThreadMarker) -> Retained<Delegate> {
        unsafe { msg_send![Delegate::alloc(mtm), init] }
    }
}

/// A URL as text.
pub fn url_text(url: &NSURL) -> String {
    url.absoluteString().map(|s| s.to_string()).unwrap_or_default()
}

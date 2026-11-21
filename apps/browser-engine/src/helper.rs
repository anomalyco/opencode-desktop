//! CEF subprocess handler (`HelperCefApp` + `HelperRenderProcessHandler`), shared by:
//! - Windows: `RunWinMain` in `opencode-browser-engine.dll` (launched via CEF's sandboxed `bootstrap.exe`).
//! - macOS / Linux: `opencode-browser-engine-helper` binary (macOS initializes `libcef_sandbox.dylib`
//!   before loading `Chromium Embedded Framework.framework`).
//!
//! Runs inside sandboxed renderer, GPU, and utility subprocesses: never touches the filesystem,
//! registry, sockets, or pipes directly.

use cef::rc::Rc as _;
use cef::{
    App, Browser, CefString, DictionaryValue, Frame, ImplApp, ImplBrowser, ImplFrame,
    ImplListValue, ImplProcessMessage, ImplRenderProcessHandler, ImplV8Context, ImplV8Value,
    ProcessId, ProcessMessage, RenderProcessHandler, SchemeRegistrar, V8Context, WrapApp,
    WrapRenderProcessHandler, sys, wrap_app, wrap_render_process_handler,
};

use crate::preview::scheme as preview_scheme;

/// DENY-BY-DEFAULT V8 lockdown for ANY non-main extension subframe (any depth, including
/// `chrome-extension://`, `about:blank`, `about:srcdoc`, `blob:`, `data:`).
/// Walks `chrome` / `browser`'s own property keys and locks every key NOT in
/// `{runtime, i18n, storage, dom}` (plus `alwaysLock` lazy API names and
/// `runtime.{connectNative,sendNativeMessage}`) to non-configurable `undefined` BEFORE line 1 of
/// any script runs.
const CHILD_FRAME_V8_LOCKDOWN_JS: &str = r#"(function(){
  var allowed = { runtime: 1, i18n: 1, storage: 1, dom: 1 };
  var alwaysLock = [
    "tabs","windows","action","browserAction","pageAction","contextMenus","webNavigation",
    "downloads","management","notifications","permissions","history","bookmarks","sessions",
    "privacy","fontSettings","identity","sidePanel","search","contentSettings",
    "desktopCapture","tabCapture","pageCapture","webAuthenticationProxy","browsingData",
    "system","userScripts","tts","ttsEngine","debugger","proxy","topSites","readingList",
    "tabGroups","omnibox","devtools","extension","commands","webRequest",
    "declarativeNetRequest","scripting","offscreen","cookies","alarms","idle","power",
    "app","csi","loadTimes","gcm","instanceID","declarativeContent"
  ];
  var lockProp = function(obj, key) {
    try {
      Object.defineProperty(obj, key, {
        value: undefined,
        configurable: false,
        enumerable: false,
        writable: false
      });
    } catch (e) {}
  };
  var roots = ["chrome","browser"];
  for (var r = 0; r < roots.length; r++) {
    var root;
    try { root = globalThis[roots[r]]; } catch (e) { continue; }
    if (!root || typeof root !== "object") continue;
    var keys = [];
    try { keys = Object.getOwnPropertyNames(root); } catch (e) {}
    for (var k = 0; k < keys.length; k++) {
      var name = keys[k];
      if (!Object.prototype.hasOwnProperty.call(allowed, name)) {
        lockProp(root, name);
      }
    }
    for (var i = 0; i < alwaysLock.length; i++) {
      lockProp(root, alwaysLock[i]);
    }
    try {
      var rt = root.runtime;
      if (rt && typeof rt === "object") {
        lockProp(rt, "connectNative");
        lockProp(rt, "sendNativeMessage");
      }
    } catch (e) {}
  }
})();"#;

fn is_extension_url(url: &str) -> bool {
    (url.len() >= 19 && url.as_bytes()[..19].eq_ignore_ascii_case(b"chrome-extension://"))
        || (url.len() >= 24 && url.as_bytes()[..24].eq_ignore_ascii_case(b"blob:chrome-extension://"))
}

fn is_extension_tree(browser: Option<&Browser>, frame: &Frame) -> bool {
    let mut cur = Some(frame.clone());
    for _ in 0..64 {
        let Some(f) = cur else { break };
        if is_extension_url(&CefString::from(&f.url()).to_string()) {
            return true;
        }
        cur = f.parent();
    }
    browser
        .and_then(|b| b.main_frame())
        .is_some_and(|mf| is_extension_url(&CefString::from(&mf.url()).to_string()))
}

/// Neutralizes every non-allow-listed `chrome.*` / `browser.*` API in ANY non-main extension
/// subframe directly inside V8's `on_context_created` BEFORE line 1 of any script (or synchronous
/// `document.write` / Custom Element `connectedCallback` / `Range.insertNode` / `setHTMLUnsafe`)
/// can observe `window[0]`.
fn lockdown_extension_child_frame(
    browser: Option<&Browser>,
    frame: Option<&Frame>,
    context: Option<&V8Context>,
) {
    let (Some(frame), Some(ctx)) = (frame, context) else { return };
    if frame.is_main() != 0 {
        return;
    }
    if ctx.enter() == 0 {
        return;
    }
    let has_ext_root = is_extension_tree(browser, frame)
        || ctx
            .global()
            .and_then(|g| g.value_bykey(Some(&CefString::from("chrome"))))
            .filter(|c| c.is_object() != 0)
            .and_then(|c| c.value_bykey(Some(&CefString::from("runtime"))))
            .filter(|r| r.is_object() != 0)
            .and_then(|r| r.value_bykey(Some(&CefString::from("id"))))
            .is_some_and(|id| id.is_string() != 0);
    if has_ext_root {
        let mut retval = None;
        let mut exception = None;
        let _ = ctx.eval(
            Some(&CefString::from(CHILD_FRAME_V8_LOCKDOWN_JS)),
            Some(&CefString::from("opencode-child-lockdown.js")),
            1,
            Some(&mut retval),
            Some(&mut exception),
        );
    }
    let _ = ctx.exit();
}

fn send_renderer_pid(frame: Option<Frame>) {
    let Some(frame) = frame else { return };
    if frame.is_main() == 0 {
        return;
    }
    let Some(mut msg) = cef::process_message_create(Some(&"opencode.renderer_pid".into())) else {
        return;
    };
    if let Some(args) = msg.argument_list() {
        args.set_int(0, std::process::id() as i32);
        frame.send_process_message(sys::cef_process_id_t::PID_BROWSER.into(), Some(&mut msg));
    }
}

wrap_render_process_handler! {
    struct HelperRenderProcessHandler;

    impl RenderProcessHandler {
        fn on_browser_created(&self, browser: Option<&mut Browser>, _extra_info: Option<&mut DictionaryValue>) {
            send_renderer_pid(browser.and_then(|b| b.main_frame()));
        }

        fn on_context_created(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            context: Option<&mut V8Context>,
        ) {
            send_renderer_pid(frame.as_deref().cloned());
            lockdown_extension_child_frame(browser.as_deref(), frame.as_deref(), context.as_deref());
        }

        fn on_process_message_received(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            source_process: ProcessId,
            message: Option<&mut ProcessMessage>,
        ) -> ::std::os::raw::c_int {
            if source_process != ProcessId::from(sys::cef_process_id_t::PID_BROWSER) {
                return 0;
            }
            let Some(msg) = message else { return 0 };
            if CefString::from(&msg.name()).to_string() != "opencode.renderer_pid_ping" {
                return 0;
            }
            send_renderer_pid(frame.cloned().or_else(|| browser.and_then(|b| b.main_frame())));
            1
        }
    }
}

wrap_app! {
    pub struct HelperCefApp;

    impl App {
        fn on_register_custom_schemes(&self, registrar: Option<&mut SchemeRegistrar>) {
            preview_scheme::register(registrar);
        }

        fn render_process_handler(&self) -> Option<RenderProcessHandler> {
            Some(HelperRenderProcessHandler::new())
        }
    }
}

pub fn new_helper_app() -> App {
    HelperCefApp::new()
}

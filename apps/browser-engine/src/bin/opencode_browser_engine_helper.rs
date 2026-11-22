//! CEF's helper process executable (on macOS `Helper`, `Helper (GPU)`, `Helper (Renderer)`, etc.;
//! elsewhere the one program for every subprocess).
#![cfg_attr(windows, windows_subsystem = "windows")]

#[path = "../preview/scheme.rs"]
mod preview_scheme;

use cef::args::Args;
use cef::rc::Rc as _;
use cef::{
    App, Browser, CefString, DictionaryValue, Frame, ImplApp, ImplBrowser, ImplDictionaryValue, ImplFrame,
    ImplListValue, ImplProcessMessage, ImplRenderProcessHandler, ImplV8Context, ImplV8Value,
    ProcessId, ProcessMessage, RenderProcessHandler, SchemeRegistrar, V8Context, WrapApp,
    WrapRenderProcessHandler, sys, wrap_app, wrap_render_process_handler,
};

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

/// Browsers the engine created print-gated (agent-driven or preview at birth: `extra_info`'s
/// `opencode.print_gated`). Every frame's context in them gets `PRINT_LOCK_JS` before any page
/// script, so `window.print()` can never open the native dialog that stops the whole engine (#662).
static PRINT_GATED: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());

const PRINT_LOCK_JS: &str = r#"(function(){
  var p = globalThis.print;
  if (typeof p !== "function") return;
  Object.defineProperty(globalThis, "print", {
    value: new Proxy(p, { apply: function() { return undefined; } }),
    writable: false, configurable: false, enumerable: true
  });
})();"#;

fn lock_print(browser: Option<&Browser>, context: Option<&V8Context>) {
    let (Some(browser), Some(ctx)) = (browser, context) else { return };
    if !PRINT_GATED.lock().unwrap().contains(&browser.identifier()) || ctx.enter() == 0 {
        return;
    }
    let (mut retval, mut exception) = (None, None);
    let _ = ctx.eval(Some(&CefString::from(PRINT_LOCK_JS)), Some(&CefString::from("opencode-print-lock.js")), 1, Some(&mut retval), Some(&mut exception));
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
        fn on_browser_created(&self, browser: Option<&mut Browser>, extra_info: Option<&mut DictionaryValue>) {
            let gated = extra_info.is_some_and(|d| d.bool(Some(&CefString::from("opencode.print_gated"))) != 0);
            if let Some(b) = browser.as_deref().filter(|_| gated) {
                PRINT_GATED.lock().unwrap().push(b.identifier());
            }
            send_renderer_pid(browser.and_then(|b| b.main_frame()));
        }

        fn on_browser_destroyed(&self, browser: Option<&mut Browser>) {
            if let Some(b) = browser {
                let id = b.identifier();
                PRINT_GATED.lock().unwrap().retain(|g| *g != id);
            }
        }

        fn on_context_created(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            context: Option<&mut V8Context>,
        ) {
            send_renderer_pid(frame.as_deref().cloned());
            lock_print(browser.as_deref(), context.as_deref());
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
    struct HelperCefApp;

    impl App {
        fn on_register_custom_schemes(&self, registrar: Option<&mut SchemeRegistrar>) {
            preview_scheme::register(registrar);
        }

        fn render_process_handler(&self) -> Option<RenderProcessHandler> {
            Some(HelperRenderProcessHandler::new())
        }
    }
}

fn main() {
    // Chromium applies its crash fallback switches after CEF's before-launch callback.
    #[cfg(target_os = "linux")]
    if std::env::args().any(|a| a == "--type=gpu-process") {
        let switch = |prefix: &str| std::env::args().filter_map(|a| a.strip_prefix(prefix).map(str::to_owned)).last().unwrap_or_default();
        eprintln!("browser engine: gpu process gl={} angle={} transport=bgra", switch("--use-gl="), switch("--use-angle="));
    }
    let args = Args::new();

    #[cfg(target_os = "macos")]
    let _loader = {
        let exe = std::env::current_exe().expect("helper current_exe");
        let loader = cef::library_loader::LibraryLoader::new(&exe, true);
        assert!(loader.load(), "failed to load Chromium Embedded Framework in helper");
        loader
    };

    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let mut app = HelperCefApp::new();
    let code = cef::execute_process(Some(args.as_main_args()), Some(&mut app), std::ptr::null_mut());
    std::process::exit(code);
}

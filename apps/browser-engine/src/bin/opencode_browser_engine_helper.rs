//! CEF's helper process executable (on macOS `Helper`, `Helper (GPU)`, `Helper (Renderer)`, etc.;
//! elsewhere the one program for every subprocess).
#![cfg_attr(windows, windows_subsystem = "windows")]

#[path = "../preview/scheme.rs"]
mod preview_scheme;

use cef::args::Args;
use cef::rc::Rc as _;
use cef::{
    App, Browser, CefString, DictionaryValue, Frame, ImplApp, ImplBrowser, ImplFrame,
    ImplListValue, ImplProcessMessage, ImplRenderProcessHandler, ImplV8Context, ImplV8Value,
    RenderProcessHandler, SchemeRegistrar, V8Context, V8Propertyattribute, WrapApp,
    WrapRenderProcessHandler, sys, wrap_app, wrap_render_process_handler,
};

const CHILD_FRAME_LOCKDOWN_APIS: &[&str] = &[
    "tabs",
    "windows",
    "action",
    "browserAction",
    "pageAction",
    "contextMenus",
    "webNavigation",
];

const CHILD_FRAME_V8_LOCKDOWN_JS: &str = r#"(function(){
  var apis = ["tabs","windows","action","browserAction","pageAction","contextMenus","webNavigation"];
  var roots = ["chrome","browser"];
  for (var r = 0; r < roots.length; r++) {
    var root;
    try { root = globalThis[roots[r]]; } catch (e) { continue; }
    if (!root || typeof root !== "object") continue;
    for (var i = 0; i < apis.length; i++) {
      try {
        Object.defineProperty(root, apis[i], {
          value: undefined,
          configurable: false,
          enumerable: false,
          writable: false
        });
      } catch (e) {}
    }
  }
})();"#;

fn is_extension_url(url: &str) -> bool {
    url.len() >= 19 && url.as_bytes()[..19].eq_ignore_ascii_case(b"chrome-extension://")
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

/// Neutralizes native `chrome.{tabs,windows,action,browserAction,pageAction,contextMenus,webNavigation}`
/// (and `browser.*`) in ANY non-main extension subframe directly inside V8's `on_context_created`
/// BEFORE line 1 of any script (or synchronous `document.write` / Custom Element `connectedCallback`
/// / `Range.insertNode` / `setHTMLUnsafe`) can observe `window[0]`, while preserving `chrome.runtime`
/// (READER-12 #441 re-attack Critical #1).
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
        let locked = V8Propertyattribute::from(
            sys::cef_v8_propertyattribute_t::V8_PROPERTY_ATTRIBUTE_READONLY
                | sys::cef_v8_propertyattribute_t::V8_PROPERTY_ATTRIBUTE_DONTENUM
                | sys::cef_v8_propertyattribute_t::V8_PROPERTY_ATTRIBUTE_DONTDELETE,
        );
        if let Some(global) = ctx.global() {
            for root_name in ["chrome", "browser"] {
                let Some(root) = global.value_bykey(Some(&CefString::from(root_name))) else {
                    continue;
                };
                if root.is_object() == 0 {
                    continue;
                }
                for api in CHILD_FRAME_LOCKDOWN_APIS {
                    let key = CefString::from(*api);
                    let _ = root.delete_value_bykey(Some(&key));
                    if let Some(mut undef) = cef::v8_value_create_undefined() {
                        let _ = root.set_value_bykey(Some(&key), Some(&mut undef), locked.clone());
                    }
                }
            }
        }
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

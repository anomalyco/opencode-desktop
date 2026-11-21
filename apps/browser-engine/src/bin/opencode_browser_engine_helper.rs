//! CEF's helper process executable (on macOS `Helper`, `Helper (GPU)`, `Helper (Renderer)`, etc.;
//! elsewhere the one program for every subprocess).
#![cfg_attr(windows, windows_subsystem = "windows")]

#[path = "../preview/scheme.rs"]
mod preview_scheme;

use cef::args::Args;
use cef::rc::Rc as _;
use cef::{
    App, Browser, DictionaryValue, Frame, ImplApp, ImplBrowser, ImplFrame, ImplListValue,
    ImplProcessMessage, ImplRenderProcessHandler, RenderProcessHandler, SchemeRegistrar, V8Context,
    WrapApp, WrapRenderProcessHandler, sys, wrap_app, wrap_render_process_handler,
};

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
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _context: Option<&mut V8Context>,
        ) {
            send_renderer_pid(frame.cloned());
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

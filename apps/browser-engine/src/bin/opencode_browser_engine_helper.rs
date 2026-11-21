//! CEF's helper process executable (on macOS `Helper`, `Helper (GPU)`, `Helper (Renderer)`, etc.;
//! elsewhere the one program for every subprocess).
#![cfg_attr(windows, windows_subsystem = "windows")]

#[path = "../preview/scheme.rs"]
mod preview_scheme;

use cef::args::Args;
use cef::rc::Rc as _;
use cef::{App, ImplApp, SchemeRegistrar, WrapApp, wrap_app};

wrap_app! {
    struct HelperCefApp;

    impl App {
        fn on_register_custom_schemes(&self, registrar: Option<&mut SchemeRegistrar>) {
            preview_scheme::register(registrar);
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

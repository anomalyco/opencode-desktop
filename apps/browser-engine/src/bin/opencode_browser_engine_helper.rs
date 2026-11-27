//! CEF's helper process executable on macOS (`Helper`, `Helper (GPU)`, `Helper (Renderer)`, etc.)
//! and Linux (single executable for all subprocesses). On Windows, CEF's sandboxed `bootstrap.exe`
//! runs both the browser and subprocesses through `RunWinMain` in `opencode-browser-engine.dll`.
#![cfg_attr(windows, windows_subsystem = "windows")]

use cef::args::Args;

fn main() {
    // Chromium applies its crash fallback switches after CEF's before-launch callback.
    #[cfg(target_os = "linux")]
    if std::env::args().any(|a| a == "--type=gpu-process") {
        let switch = |prefix: &str| {
            std::env::args()
                .filter_map(|a| a.strip_prefix(prefix).map(str::to_owned))
                .last()
                .unwrap_or_default()
        };
        eprintln!(
            "browser engine: gpu process gl={} angle={} transport=bgra",
            switch("--use-gl="),
            switch("--use-angle=")
        );
    }

    let args = Args::new();

    // macOS Seatbelt V2 sandbox: MUST initialize `libcef_sandbox.dylib` BEFORE loading
    // `Chromium Embedded Framework.framework` (`tests/cefsimple/process_helper_mac.cc`).
    #[cfg(target_os = "macos")]
    let _sandbox = {
        let mut sandbox = cef::sandbox::Sandbox::new();
        sandbox.initialize(args.as_main_args());
        sandbox
    };

    #[cfg(target_os = "macos")]
    let _loader = {
        let exe = std::env::current_exe().expect("helper current_exe");
        let loader = cef::library_loader::LibraryLoader::new(&exe, true);
        assert!(loader.load(), "failed to load Chromium Embedded Framework in helper");
        loader
    };

    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let mut app = opencode_browser_engine::helper::new_helper_app();
    let code = cef::execute_process(Some(args.as_main_args()), Some(&mut app), std::ptr::null_mut());
    #[cfg(target_os = "macos")]
    drop(_loader);
    #[cfg(target_os = "macos")]
    drop(_sandbox);
    std::process::exit(code);
}

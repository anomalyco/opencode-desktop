//! Browser engine binary entry point (macOS and Linux; on Windows the shipping launcher is CEF's
//! `bootstrap.exe` renamed to `opencode-browser-engine.exe`, which loads `opencode-browser-engine.dll`
//! and invokes `RunWinMain` with Chromium's initialized Windows sandbox info).
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    let code = opencode_browser_engine::run_browser(std::ptr::null_mut(), None);
    if code != 0 {
        std::process::exit(code);
    }
}

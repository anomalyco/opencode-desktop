//! CEF's helper process executable (on macOS `Helper`, `Helper (GPU)`, `Helper (Renderer)`, etc.;
//! elsewhere the one program for every subprocess).
#![cfg_attr(windows, windows_subsystem = "windows")]
use cef::args::Args;

fn main() {
    let args = Args::new();

    #[cfg(target_os = "macos")]
    let _loader = {
        let exe = std::env::current_exe().expect("helper current_exe");
        let loader = cef::library_loader::LibraryLoader::new(&exe, true);
        assert!(loader.load(), "failed to load Chromium Embedded Framework in helper");
        loader
    };

    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let code = cef::execute_process(Some(args.as_main_args()), None::<&mut cef::App>, std::ptr::null_mut());
    std::process::exit(code);
}

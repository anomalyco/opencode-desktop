//! `hello-floor` on the desktop app's allocator (`apps/desktop/src/main.rs`).
#![cfg_attr(windows, windows_subsystem = "windows")]

#[path = "main.rs"]
mod hello;

#[global_allocator]
static ALLOC: procwatch::alloc::Counting<mimalloc::MiMalloc> = procwatch::alloc::Counting::new(mimalloc::MiMalloc);

/// As `opencode_desktop::commit_pages_on_demand`.
#[allow(unsafe_code)]
fn commit_pages_on_demand() {
    unsafe extern "C" {
        fn mi_version() -> std::ffi::c_int;
        fn mi_option_set(option: std::ffi::c_int, value: std::ffi::c_long);
    }
    // SAFETY: plain calls into the linked mimalloc, before any other thread exists.
    unsafe {
        if mi_version() / 100 == 303 {
            mi_option_set(40, 1);
        }
    }
}

fn main() {
    commit_pages_on_demand();
    hello::run();
}

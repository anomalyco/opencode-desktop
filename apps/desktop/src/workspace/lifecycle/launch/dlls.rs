//! Windows: the system DLLs the way to the first frame calls into, loaded on a thread beside it.
//!
//! They are loaded on first use, not before `main` (`build.rs`'s `/DELAYLOAD` list, and the
//! input method's library, which Windows loads with a thread's first window), so the first call
//! into each loaded it on the main thread, between `main` and the first frame. In four sampled
//! launches (200 µs samples, main thread):
//!
//! - `shell32.dll`, and `shcore.dll` with it: 800 to 1,200 µs under
//!   `SetCurrentProcessExplicitAppUserModelID` (`notifications::identify`);
//! - `msctf.dll`: 400 to 600 µs inside the event loop's first window;
//! - `comctl32.dll`: 800 to 1,000 µs under the window's first subclass (Rune's
//!   `follow_suggested_rect`).
//!
//! [`warm`], first thing in `main`, starts a thread that loads them in the order the launch
//! reaches them. The launch finds each already loaded, or waits only for what is left of its
//! load. `OPENCODE_RUNE_LAUNCH_DLLS=off` leaves them to their first calls (a bench's arm).
// `LoadLibraryExW` is Win32; the block says what it relies on.
#![allow(unsafe_code)]

use windows_sys::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};

/// In the order the launch calls into them, each with whether it is System32's. Common Controls
/// is the manifest's version 6 (a side-by-side assembly), found by name as the delay-load helper
/// finds it.
const FIRST_FRAME: [(&str, bool); 4] = [("shell32.dll", true), ("shcore.dll", true), ("msctf.dll", true), ("comctl32.dll", false)];

/// Starts loading the DLLs the first frame's path uses.
pub fn warm() {
    if std::env::var_os("OPENCODE_RUNE_LAUNCH_DLLS").is_some_and(|v| v == "off") {
        return;
    }
    let loading = std::thread::Builder::new().name("launch-dlls".into()).spawn(|| {
        for (name, system) in FIRST_FRAME {
            let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
            let flags = if system { LOAD_LIBRARY_SEARCH_SYSTEM32 } else { 0 };
            // SAFETY: a NUL-terminated name, no file handle, flags the loader defines. The
            // module handle is kept: these stay loaded for the life of the process, as after
            // a first call.
            unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), flags) };
        }
        rune::marks::mark("launch.dlls");
    });
    if loading.is_ok() {
        rune::marks::mark("launch.dlls.start");
    }
}

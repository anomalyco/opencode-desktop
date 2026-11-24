//! The exe of a packaged macOS build whose app is a library (`apps/desktop-app`): it loads
//! `Contents/Frameworks/libopencode_desktop_app.dylib` and runs it. `build-app.sh` ships it as
//! `Contents/MacOS/opencode-desktop`. It stays small on purpose: the system reads a new exe whole
//! before its first run, and this is the only file it reads that way.
#![allow(unsafe_code)]

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    use std::os::unix::ffi::OsStrExt;

    unsafe extern "C" {
        fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn dlerror() -> *const c_char;
    }
    const RTLD_NOW: c_int = 2;
    const NAME: &str = "libopencode_desktop_app.dylib";

    let fail = |why: String| -> ! {
        eprintln!("OpenCode cannot start: {why}. Install OpenCode again.");
        std::process::exit(1)
    };
    let exe = std::env::current_exe().unwrap_or_else(|e| fail(format!("its own path is unknown ({e})")));
    let folder = exe.parent().unwrap_or_else(|| fail("its own folder is unknown".to_owned()));
    // In the bundle; beside the exe in a build folder.
    let library = [folder.join("../Frameworks").join(NAME), folder.join(NAME)].into_iter().find(|path| path.is_file());
    let library = library.unwrap_or_else(|| fail(format!("{NAME} is missing from the app")));
    let path = CString::new(library.as_os_str().as_bytes()).unwrap_or_else(|e| fail(e.to_string()));
    // SAFETY: a NUL-terminated path; the library stays loaded for the life of the process.
    let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW) };
    if handle.is_null() {
        // SAFETY: after a failed dlopen on this thread dlerror gives its message, or null.
        let message = unsafe { dlerror() };
        // SAFETY: a non-null dlerror answer is a C string.
        let message = if message.is_null() { String::new() } else { unsafe { CStr::from_ptr(message) }.to_string_lossy().into_owned() };
        fail(format!("{} did not load ({message})", library.display()));
    }
    // SAFETY: a live handle and a NUL-terminated name.
    let entry = unsafe { dlsym(handle, c"opencode_desktop_app_main".as_ptr()) };
    if entry.is_null() {
        fail(format!("{} is not this app's library", library.display()));
    }
    // SAFETY: the library exports this name as `extern "C" fn() -> i32` (`apps/desktop-app`).
    let run = unsafe { std::mem::transmute::<*mut c_void, extern "C" fn() -> i32>(entry) };
    std::process::exit(run());
}
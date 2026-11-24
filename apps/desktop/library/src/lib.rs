//! OpenCode (Rune) as a library: the whole app, with one exported function that runs it. The
//! packaged macOS build can ship this in `Contents/Frameworks` behind a small exe
//! (`apps/desktop/launcher`) in place of the one big exe (`apps/desktop/src/main.rs`).
//!
//! Why: macOS reads every byte of an exe it has not seen before its first run (5.4 ms per MiB on an
//! M4: 1.2 s for the dev channel's 136 MiB exe), on every install and update. A library in the
//! bundle is not read that way: a 30-line exe with 143 MiB of this app's own code in libraries
//! beside it first ran in 551 ms, with none 254 ms (`lmg/fb2.py`, 24 Nov).
//!
//! Everything else is `main.rs`'s: the same allocator, the same entry.
#![cfg(not(target_arch = "wasm32"))]
#![allow(unsafe_code)]

/// Counts each thread's allocations for the frame diagnostics and the benchmarks, over mimalloc
/// (`apps/desktop/src/main.rs`).
#[global_allocator]
static ALLOC: procwatch::alloc::Counting<opencode_desktop::OomGuard<mimalloc::MiMalloc>> =
    procwatch::alloc::Counting::new(opencode_desktop::OomGuard(mimalloc::MiMalloc));

/// The app, from `main`'s first line to its exit. Called once, on the process's main thread, by
/// the launcher. A panic that reaches here ends the process as one that reaches `main` does.
#[unsafe(no_mangle)]
pub extern "C" fn opencode_desktop_app_main() -> i32 {
    match std::panic::catch_unwind(opencode_desktop::run_native) {
        Ok(()) => 0,
        Err(_) => 101,
    }
}
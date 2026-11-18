//! Thin native binary entrypoint for OpenCode (Rune) Desktop.
//!
//! All shared application logic lives in [`opencode_desktop`] (`src/lib.rs`) so both the native
//! desktop binary and `apps/web` (`wasm32`) compile the same library target without manifest
//! duplication.

/// Counts each thread's allocations for the frame diagnostics and the benchmarks, over mimalloc:
/// the system heap costs a Luau plugin's rebuild (its tables, strings and elements) about a fifth
/// more on Windows.
#[cfg(not(target_arch = "wasm32"))]
#[global_allocator]
static ALLOC: procwatch::alloc::Counting<opencode_desktop::OomGuard<mimalloc::MiMalloc>> =
    procwatch::alloc::Counting::new(opencode_desktop::OomGuard(mimalloc::MiMalloc));

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    opencode_desktop::run_native();
}

#[cfg(target_arch = "wasm32")]
fn main() {}

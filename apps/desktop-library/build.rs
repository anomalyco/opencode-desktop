//! The library's system DLLs load on first use, as the one-file exe's do (`apps/desktop/build.rs`):
//! the list is the app's own.

include!("../desktop/delay_loaded.rs");

fn main() {
    if std::env::var_os("CARGO_CFG_TARGET_OS").is_none_or(|os| os != "windows") {
        return;
    }
    for dll in DELAY_LOADED {
        println!("cargo:rustc-link-arg-cdylib=/DELAYLOAD:{dll}");
    }
    println!("cargo:rustc-link-arg-cdylib=delayimp.lib");
}

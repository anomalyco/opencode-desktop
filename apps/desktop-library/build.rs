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
    // The build's version is the library's to say (Properties > Details): the thin exe's own
    // version information is the same in every build (apps/desktop-thin/build.rs).
    for name in ["OPENCODE_RUNE_BUILD_CHANNEL", "OPENCODE_RUNE_WINDOW_TITLE", "OPENCODE_RUNE_VERSION"] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    #[allow(clippy::disallowed_methods)]
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let title = env("OPENCODE_RUNE_WINDOW_TITLE").unwrap_or_else(|| win_install::default_name(env("OPENCODE_RUNE_BUILD_CHANNEL").as_deref()).into());
    let version = env("OPENCODE_RUNE_VERSION").or_else(|| env("CARGO_PKG_VERSION")).unwrap_or_default();
    // The windows' icon is the library's too: `launch::apply_app_icon` names resource 1, and each
    // window loads it from the module the window code is in, which is this library, not the exe.
    let icon = std::path::PathBuf::from(env("CARGO_MANIFEST_DIR").unwrap_or_default()).join("../desktop/assets/icon.ico");
    if let Err(e) = win_install::resources::embed_library(&icon, "opencode-desktop.dll", &version, &title, &title) {
        panic!("{e}");
    }
}

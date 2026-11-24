//! The thin exe is what Windows starts, so it carries what `apps/desktop/build.rs` gives the one-file
//! exe: the windowed subsystem, the main thread's stack, and the icon, version and manifest.

fn main() {
    for name in ["OPENCODE_RUNE_BUILD_CHANNEL", "OPENCODE_RUNE_WINDOW_TITLE", "OPENCODE_RUNE_VERSION"] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    #[allow(clippy::disallowed_methods)]
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    if env("CARGO_CFG_TARGET_OS").as_deref() != Some("windows") {
        return;
    }
    // No console window and no console host, as for the one-file exe (apps/desktop/build.rs).
    if env("OPENCODE_RUNE_BUILD_CHANNEL").is_some_and(|c| c != "local") || env("PROFILE").as_deref() == Some("release") {
        println!("cargo:rustc-link-arg-bin=opencode-desktop-thin=/SUBSYSTEM:WINDOWS");
        println!("cargo:rustc-link-arg-bin=opencode-desktop-thin=/ENTRY:mainCRTStartup");
    }
    // The main thread's stack is the exe's to reserve: 8 MiB, for the layout's recursion (apps/desktop/build.rs).
    if env("CARGO_CFG_TARGET_ENV").as_deref() == Some("msvc") {
        println!("cargo:rustc-link-arg-bin=opencode-desktop-thin=/STACK:8388608");
    }
    let title = env("OPENCODE_RUNE_WINDOW_TITLE").unwrap_or_else(|| win_install::default_name(env("OPENCODE_RUNE_BUILD_CHANNEL").as_deref()).into());
    let version = env("OPENCODE_RUNE_VERSION").or_else(|| env("CARGO_PKG_VERSION")).unwrap_or_default();
    let dir = std::path::PathBuf::from(env("CARGO_MANIFEST_DIR").unwrap_or_default());
    let (icon, manifest) = (dir.join("../desktop/assets/icon.ico"), dir.join("../desktop/assets/windows/app.manifest"));
    let resources = win_install::resources::Resources::new(&icon, &manifest, win_install::EXE, &version).names(&title, &title);
    if let Err(e) = win_install::resources::embed(&resources) {
        panic!("{e}");
    }
}

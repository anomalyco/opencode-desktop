fn main() {
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_WINDOW_TITLE");
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_VERSION");
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_BUNDLE_ID");
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_URL_SCHEME");
    #[allow(clippy::disallowed_methods)]
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    if env("CARGO_CFG_TARGET_OS").as_deref() != Some("windows") {
        return;
    }
    // A release build is a windowed program: no console window opens beside it from the Start
    // menu, a link or a toast (`launch::attach_console` still prints to a terminal that starts
    // it). Development builds keep their console for logs.
    if env("OPENCODE_RUNE_BUILD_CHANNEL").is_some_and(|c| c != "local") {
        println!("cargo:rustc-link-arg-bin=opencode-desktop=/SUBSYSTEM:WINDOWS");
        println!("cargo:rustc-link-arg-bin=opencode-desktop=/ENTRY:mainCRTStartup");
    }
    // The exe's icon (Explorer, the taskbar, Alt+Tab, a pinned shortcut), version and manifest.
    let title = env("OPENCODE_RUNE_WINDOW_TITLE").unwrap_or_else(|| "OpenCode (Rune)".into());
    let version = env("OPENCODE_RUNE_VERSION").or_else(|| env("CARGO_PKG_VERSION")).unwrap_or_default();
    let dir = std::path::PathBuf::from(env("CARGO_MANIFEST_DIR").unwrap_or_default());
    let (icon, manifest) = (dir.join("assets/icon.ico"), dir.join("assets/windows/app.manifest"));
    let resources = win_install::resources::Resources::new(&icon, &manifest, win_install::EXE, &version).names(&title, &title);
    if let Err(e) = win_install::resources::embed(&resources) {
        panic!("{e}");
    }
}

//! The thin exe is what Windows starts, so it carries what `apps/desktop/build.rs` gives the one-file
//! exe: the windowed subsystem, the main thread's stack, the icon and the manifest.
//!
//! It is the same bytes in every build of a channel, so a new build's exe is a file Windows
//! Defender has already checked on a machine that ran any earlier build: the link is reproducible
//! (`/Brepro`: the header's link time is a hash of the file, not the clock) and its version
//! information is the thin exe's own, [`THIN_VERSION`], not the build's. The build's version is
//! in the library's version information (`apps/desktop-library/build.rs`) and in `--version`.
//! Defender decides "seen" by the header: a copy with only another link time paid its full
//! check (1,085-2,671 ms for a 64 KiB exe), and one with other bytes where a signature goes,
//! another checksum and another security directory paid the price of a seen file (25-48 ms).

/// The thin exe's own version: raise it when `src/main.rs` changes what it does.
const THIN_VERSION: &str = "1.0.0";

fn main() {
    for name in ["OPENCODE_RUNE_BUILD_CHANNEL", "OPENCODE_RUNE_WINDOW_TITLE"] {
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
        println!("cargo:rustc-link-arg-bin=opencode-desktop-thin=/Brepro");
    }
    let title = env("OPENCODE_RUNE_WINDOW_TITLE").unwrap_or_else(|| win_install::default_name(env("OPENCODE_RUNE_BUILD_CHANNEL").as_deref()).into());
    let dir = std::path::PathBuf::from(env("CARGO_MANIFEST_DIR").unwrap_or_default());
    let (icon, manifest) = (dir.join("../desktop/assets/icon.ico"), dir.join("../desktop/assets/windows/app.manifest"));
    let resources = win_install::resources::Resources::new(&icon, &manifest, win_install::EXE, THIN_VERSION).names(&title, &title);
    if let Err(e) = win_install::resources::embed(&resources) {
        panic!("{e}");
    }
}

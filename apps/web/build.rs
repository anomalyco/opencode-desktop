//! The browser build runs the desktop app's build script (its embedded Luau plugins and beta
//! notes). For any other target the library is empty: `opencode_web_host` turns main.rs off.

mod desktop {
    include!("../desktop/build.rs");
    pub fn run() {
        main();
    }
}

fn main() {
    #[allow(clippy::disallowed_methods)]
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if arch != "wasm32" {
        println!("cargo:rustc-cfg=opencode_web_host");
        return;
    }
    desktop::run();
}

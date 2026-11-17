fn main() {
    println!("cargo::rerun-if-env-changed=DEP_CEF_DLL_WRAPPER_CEF_DIR");
    if let Ok(cef_dir) = std::env::var("DEP_CEF_DLL_WRAPPER_CEF_DIR") {
        println!("cargo::rustc-env=OPENCODE_CEF_DIR={cef_dir}");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        // Both the engine and its renderer helper load the libcef shipped beside them.
        println!("cargo::rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // `mt.exe` refuses a `\\?\` prefix when Cargo canonicalizes the workspace path.
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
        let manifest = std::path::Path::new(dir.trim_start_matches(r"\\?\")).join("win").join("engine.exe.manifest");
        println!("cargo::rerun-if-changed={}", manifest.display());
        println!("cargo::rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo::rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
    }
}

fn main() {
    println!("cargo::rerun-if-env-changed=DEP_CEF_DLL_WRAPPER_CEF_DIR");
    if let Ok(cef_dir) = std::env::var("DEP_CEF_DLL_WRAPPER_CEF_DIR") {
        println!("cargo::rustc-env=OPENCODE_CEF_DIR={cef_dir}");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("win").join("engine.exe.manifest");
        println!("cargo::rerun-if-changed={}", manifest.display());
        println!("cargo::rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo::rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
    }
}

fn main() {
    println!("cargo::rerun-if-env-changed=DEP_CEF_DLL_WRAPPER_CEF_DIR");
    if let Ok(cef_dir) = std::env::var("DEP_CEF_DLL_WRAPPER_CEF_DIR") {
        println!("cargo::rustc-env=OPENCODE_CEF_DIR={cef_dir}");
    }
}

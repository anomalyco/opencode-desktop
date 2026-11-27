fn main() {
    println!("cargo::rerun-if-env-changed=DEP_CEF_DLL_WRAPPER_CEF_DIR");
    if let Ok(cef_dir) = std::env::var("DEP_CEF_DLL_WRAPPER_CEF_DIR") {
        println!("cargo::rustc-env=OPENCODE_CEF_DIR={cef_dir}");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        // Both the engine and its renderer helper load the libcef shipped beside them.
        println!("cargo::rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        // `mt.exe` refuses a `\\?\` prefix when Cargo canonicalizes the workspace path.
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
        let manifest = std::path::Path::new(dir.trim_start_matches(r"\\?\")).join("win").join("engine.exe.manifest");
        println!("cargo::rerun-if-changed={}", manifest.display());
        println!("cargo::rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo::rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
        // Match Chromium's 8 MiB Windows stack reserve (`chrome.exe`) so V8 compiling large MV3
        // service workers (Bitwarden 3.5 MB, 1Password 3.0 MB) never overflows the default 1 MiB stack.
        println!("cargo::rustc-link-arg-bins=/STACK:8388608");

        // Windows sandboxed client DLL (`opencode-browser-engine.dll`, loaded by CEF's `bootstrap.exe`):
        // delay-load CEF and browser-only Win32 DLLs (`cmake/cef_variables.cmake` `CEF_DELAYLOAD_FLAGS`)
        // so sandboxed renderer/GPU/utility subprocesses never eagerly load forbidden libraries before
        // `TargetServices::LowerToken()`. `delayimp.lib` is already linked by `cef-dll-sys`.
        println!("cargo::rustc-link-arg-cdylib=/IGNORE:4199");
        for dll in [
            "libcef.dll",
            "advapi32.dll",
            "comctl32.dll",
            "crypt32.dll",
            "d3d11.dll",
            "dwmapi.dll",
            "dxgi.dll",
            "gdi32.dll",
            "hid.dll",
            "iphlpapi.dll",
            "ole32.dll",
            "oleaut32.dll",
            "powrprof.dll",
            "propsys.dll",
            "rpcrt4.dll",
            "secur32.dll",
            "setupapi.dll",
            "shell32.dll",
            "shlwapi.dll",
            "user32.dll",
            "userenv.dll",
            "winhttp.dll",
            "winmm.dll",
            "wintrust.dll",
            "ws2_32.dll",
        ] {
            println!("cargo::rustc-link-arg-cdylib=/DELAYLOAD:{dll}");
        }
    }
}

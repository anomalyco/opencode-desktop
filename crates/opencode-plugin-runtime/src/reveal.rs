//! `Desktop:Reveal` and a toast's Reveal: a path in the system's file manager.
//!
//! Refuses UNC/SMB shares, Windows device namespaces, quotes, control/bidi characters, and
//! traversal segments BEFORE touching the filesystem, and never executes a file via the OS shell.

/// Shows `path` in the system's file manager, selected when it's a file (`desktop.reveal`); a
/// headless run or browser host opens nothing.
pub fn reveal(path: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if rune::is_headless() || path.is_empty() {
            return;
        }
        let Some(clean) = browser_policy::safe_local_reveal_path(path) else {
            return;
        };
        let meta = match std::fs::symlink_metadata(&clean) {
            Ok(m) => m,
            Err(_) => return,
        };
        let is_file = meta.is_file();
        let is_dir = meta.is_dir();
        if !is_file && !is_dir {
            return;
        }
        #[cfg(target_os = "windows")]
        let spawned = {
            let native = clean.replace('/', "\\");
            // Never pass a comma/quote/shell-metacharacter path to `explorer.exe /select,` and
            // never invoke bare `explorer.exe <path>` on anything other than a verified directory.
            if native.contains([',', '"', ';', '&']) {
                return;
            }
            if is_file {
                std::process::Command::new("explorer.exe").arg(format!("/select,{native}")).spawn()
            } else {
                std::process::Command::new("explorer.exe").arg(&native).spawn()
            }
        };
        #[cfg(target_os = "macos")]
        let spawned = if is_file || browser_policy::is_macos_bundle_path(&clean) {
            std::process::Command::new("/usr/bin/open").args(["-R", "--", &clean]).spawn()
        } else {
            std::process::Command::new("/usr/bin/open").args(["--", &clean]).spawn()
        };
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let spawned = std::process::Command::new("xdg-open")
            .arg(if is_file { std::path::Path::new(&clean).parent().map_or(clean.as_str(), |p| p.to_str().unwrap_or(clean.as_str())) } else { clean.as_str() })
            .spawn();
        if let Err(e) = spawned {
            crate::tasks::log(format!("reveal {clean}: {e}"));
        }
    }
}

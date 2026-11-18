//! `Desktop:Reveal` and a toast's Reveal: a path in the system's file manager.

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
        let file = std::path::Path::new(path).is_file();
        #[cfg(target_os = "windows")]
        let spawned = if file {
            std::process::Command::new("explorer.exe").arg(format!("/select,{path}")).spawn()
        } else {
            std::process::Command::new("explorer.exe").arg(path).spawn()
        };
        #[cfg(target_os = "macos")]
        let spawned = if file {
            std::process::Command::new("/usr/bin/open").args(["-R", path]).spawn()
        } else {
            std::process::Command::new("/usr/bin/open").arg(path).spawn()
        };
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let spawned = std::process::Command::new("xdg-open")
            .arg(if file { std::path::Path::new(path).parent().map_or(path, |p| p.to_str().unwrap_or(path)) } else { path })
            .spawn();
        if let Err(e) = spawned {
            crate::tasks::log(format!("reveal {path}: {e}"));
        }
    }
}

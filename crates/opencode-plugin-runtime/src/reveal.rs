//! `Desktop:Reveal` and a toast's Reveal: a path in the system's file manager.
//!
//! Delegates to `browser_policy::reveal_in_file_manager` so every reveal runs pure string
//! checks, `QueryDosDeviceW` (`subst`-to-UNC) / `DRIVE_FIXED` / `MNT_LOCAL` verification, and a
//! component-wise no-reparse-point walk BEFORE touching disk, and never spawns bare `explorer.exe`
//! or `/usr/bin/open <path>`.

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
        if let Err(e) = browser_policy::reveal_in_file_manager(path) {
            crate::tasks::log(format!("reveal {path}: {e}"));
        }
    }
}

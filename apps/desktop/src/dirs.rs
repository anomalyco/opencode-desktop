//! Where this app keeps its files, resolved once from the environment through `rune::dirs`.
//!
//! | What | macOS | Windows |
//! | --- | --- | --- |
//! | Settings, tabs, drafts, prompt history, model prefs, perf | `~/Library/Application Support/rune-opencode` | `%APPDATA%\rune-opencode` |
//! | Session page cache | `~/Library/Caches/rune-opencode` | `%LOCALAPPDATA%\rune-opencode\Cache` |
//! | Motion log | `~/Library/Logs/opencode-rune` | `%LOCALAPPDATA%\rune-opencode\Logs` |
//! | Update downloads (macOS only) | `~/Library/Caches/<team bundle id>/Updates` | |
//!
//! Linux follows XDG. The folder names differ from OpenCode Desktop's (`ai.opencode.desktop*`),
//! so the two apps never share files. `RUNE_DIRS_ROOT=<root>` moves every folder this app writes
//! under `<root>` for an isolated run.

use std::path::PathBuf;
use std::sync::OnceLock;

use rune::dirs::AppDirs;

use crate::App;

const APP_ID: &str = "rune-opencode";

fn app() -> Option<&'static AppDirs> {
    static DIRS: OnceLock<Option<AppDirs>> = OnceLock::new();
    DIRS.get_or_init(|| AppDirs::new(APP_ID)).as_ref()
}

/// Settings, tabs, the open session and drafts, prompt history, model prefs and perf records.
pub fn data() -> Option<PathBuf> {
    Some(app()?.data().to_owned())
}

/// Session pages kept between launches; safe to delete.
pub fn cache() -> Option<PathBuf> {
    Some(app()?.cache().to_owned())
}

/// The motion log (`motion.jsonl`). macOS keeps the folder it always had.
pub fn motion_log() -> Option<PathBuf> {
    let app = app()?;
    let logs = if cfg!(target_os = "macos") { app.sibling("opencode-rune").logs().to_owned() } else { app.logs().to_owned() };
    Some(logs.join("motion.jsonl"))
}

/// Where the updater downloads a release for the bundle `bundle_id`.
#[cfg(target_os = "macos")]
pub fn updates(bundle_id: &str) -> Option<PathBuf> {
    Some(app()?.sibling(bundle_id).cache().join("Updates"))
}

/// OpenCode Desktop Beta's drafts database (in its Electron `userData`), read to import its tabs.
pub fn beta_drafts() -> Option<PathBuf> {
    Some(app()?.sibling("ai.opencode.desktop.beta").config().join("drafts.sqlite"))
}

/// The folder the Hex dictation app writes its endpoint file to.
pub fn hex_support() -> Option<PathBuf> {
    Some(app()?.sibling("hex").data().to_owned())
}

/// Where Export saves a session.
pub fn downloads() -> Option<PathBuf> {
    Some(app()?.downloads()?.to_owned())
}

/// The user's home folder as text (`USERPROFILE` on Windows), or empty when there is none.
pub fn home() -> String {
    app().and_then(AppDirs::home).map(|h| h.to_string_lossy().into_owned()).unwrap_or_default()
}

impl App {
    /// `dir` with the home folder (the fixture's, in a fixture run) shown as `~`.
    pub(crate) fn tilde(&self, dir: &str) -> String {
        opencode_kit::path::tilde(dir, &self.home)
    }
}

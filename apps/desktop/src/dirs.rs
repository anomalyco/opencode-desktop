//! Where this app keeps its files, resolved once from the environment through `rune::dirs`.
//!
//! | What | macOS | Windows |
//! | --- | --- | --- |
//! | The app's database (`opencode.sqlite`: settings, windows, tabs, drafts, prompt history) | `~/Library/Application Support/rune-opencode` | `%APPDATA%\rune-opencode` |
//! | Caches (Office fonts, updates) | `~/Library/Caches/rune-opencode` | `%LOCALAPPDATA%\rune-opencode\Cache` |
//! | Motion log, frame diagnostics (`perf/`) | `~/Library/Logs/opencode-rune` | `%LOCALAPPDATA%\rune-opencode\Logs` |
//! | Update downloads and records | `~/Library/Caches/<team bundle id>/Updates` | `%LOCALAPPDATA%\rune-opencode\Cache\Updates` |
//!
//! Linux follows XDG. The folder names differ from OpenCode Desktop's (`ai.opencode.desktop*`),
//! so the two apps never share files. `RUNE_DIRS_ROOT=<root>` moves every folder this app writes
//! under `<root>` for an isolated run.
//!
//! A debug build keeps its own folders, `rune-opencode.debug` in place of `rune-opencode`, unless
//! `RUNE_DIRS_ROOT` places them: a development or harness run never reads or writes the profile a
//! release build keeps, as OpenCode Desktop's unpackaged runs keep `ai.opencode.desktop.dev`
//! apart from the release's (`packages/desktop/src/main/constants.ts`, `APP_ID`).

use std::path::PathBuf;
use std::sync::OnceLock;

use rune::dirs::AppDirs;

use crate::App;

const APP_ID: &str = "rune-opencode";
const DEBUG_APP_ID: &str = "rune-opencode.debug";

/// The app's folders, for code that removes them (`install`).
pub(crate) fn app() -> Option<&'static AppDirs> {
    static DIRS: OnceLock<Option<AppDirs>> = OnceLock::new();
    DIRS.get_or_init(|| {
        let dirs = AppDirs::new(APP_ID)?;
        if cfg!(debug_assertions) && !dirs.is_rooted() { AppDirs::new(DEBUG_APP_ID) } else { Some(dirs) }
    })
    .as_ref()
}

/// Whether `RUNE_DIRS_ROOT` put every folder this app writes under one scratch root.
pub fn rooted() -> bool {
    app().is_some_and(AppDirs::is_rooted)
}

/// The app's database (`opencode.sqlite`) and the files it imported from, retired beside it.
pub fn data() -> Option<PathBuf> {
    Some(app()?.data().to_owned())
}

/// User configuration: `plugins-override/` (the same folder as `data` on macOS and Windows).
pub fn config() -> Option<PathBuf> {
    Some(app()?.config().to_owned())
}

/// Rebuildable caches; safe to delete.
pub fn cache() -> Option<PathBuf> {
    Some(app()?.cache().to_owned())
}

/// The log folder: the motion log and frame diagnostics (`perf/`), what Export logs packs.
/// macOS keeps the folder it always had.
pub fn logs() -> Option<PathBuf> {
    let app = app()?;
    Some(if cfg!(target_os = "macos") { app.sibling("opencode-rune").logs().to_owned() } else { app.logs().to_owned() })
}

/// The motion log (`motion.jsonl`).
pub fn motion_log() -> Option<PathBuf> {
    Some(logs()?.join("motion.jsonl"))
}

/// Where the updater downloads a release and keeps its records: the team bundle's caches on
/// macOS (the folder it always had), the app's cache folder elsewhere.
pub fn updates() -> Option<PathBuf> {
    let app = app()?;
    let cache = if cfg!(target_os = "macos") { app.sibling("anomaly.opencode-rune").cache().to_owned() } else { app.cache().to_owned() };
    Some(cache.join("Updates"))
}

/// OpenCode Desktop's Electron `userData` folders (production, Beta, Dev), whose `drafts.sqlite`
/// seeds a fresh profile, read-only, after the first frame.
pub fn official() -> Vec<PathBuf> {
    let Some(app) = app() else { return Vec::new() };
    ["ai.opencode.desktop", "ai.opencode.desktop.beta", "ai.opencode.desktop.dev"].iter().map(|n| app.sibling(n).config().to_owned()).collect()
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
    /// `dir` on `server` with that server's home folder shown as `~`: this machine's (the
    /// fixture's, in a fixture run) for the app's own server, another's as it reported it
    /// (`servers::health::home`), else the folder as it is (`previewPath`, `tab-nav.tsx:146-155`).
    pub(crate) fn tilde(&self, server: &opencode_kit::servers::ServerKey, dir: &str) -> String {
        if server.is_sidecar() {
            return opencode_kit::path::tilde(dir, &self.home);
        }
        crate::servers::health::home(server).map_or_else(|| dir.to_owned(), |home| opencode_kit::path::tilde(dir, &home))
    }
}

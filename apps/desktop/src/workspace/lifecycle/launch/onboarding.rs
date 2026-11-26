//! First-launch default project creation (`packages/desktop/src/main/lifecycle/onboarding.ts`).

use std::sync::atomic::{AtomicBool, Ordering};

use rune::Tx;

use crate::{App, Route};

/// Set once a window took the first launch's draft, so a reconnect doesn't again.
static FIRST_LAUNCH: AtomicBool = AtomicBool::new(false);

/// A new profile's first window, on Home with no tabs and no server added: Desktop's first launch
/// (`lifecycle/onboarding.ts`, `renderer/onboarding`) makes `Documents/Default Project`, adds it as
/// a project and opens a new session there with its composer ready, rather than an empty Home.
pub(super) fn first_launch(s: &mut App, tx: &mut Tx) {
    if !s.window.fresh || !s.persists() || *s.route != Route::Home || !s.tabs.is_empty() || !crate::servers::list().is_empty() {
        return;
    }
    if FIRST_LAUNCH.swap(true, Ordering::Relaxed) {
        return;
    }
    let Some(dir) = documents_dir().map(|d| d.join(opencode_i18n::t!("desktop.onboarding.defaultProject"))) else { return };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let dir = dir.to_string_lossy().into_owned();
    let server = crate::servers::runtime::active(s).clone();
    crate::projects::home::add(s, &server, vec![dir.clone()], tx);
    s.new_session_in(&dir, tx);
    s.composer.focused = true;
}

/// The user's Documents folder, as Electron's `app.getPath("documents")` finds it; under the run's
/// own root when `RUNE_DIRS_ROOT` moves every folder the app writes.
fn documents_dir() -> Option<std::path::PathBuf> {
    if let Some(root) = std::env::var_os("RUNE_DIRS_ROOT") {
        return Some(std::path::PathBuf::from(root).join("Documents"));
    }
    #[cfg(target_os = "windows")]
    return win_install::documents();
    #[cfg(not(target_os = "windows"))]
    {
        let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
        #[cfg(target_os = "linux")]
        if let Some(dir) = xdg_documents(&home) {
            return Some(dir);
        }
        Some(home.join("Documents"))
    }
}

/// `XDG_DOCUMENTS_DIR` from `user-dirs.dirs`, as GLib reads it (`"$HOME/…"` or an absolute path).
#[cfg(target_os = "linux")]
fn xdg_documents(home: &std::path::Path) -> Option<std::path::PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| home.join(".config"));
    let text = std::fs::read_to_string(config.join("user-dirs.dirs")).ok()?;
    let value = text.lines().find_map(|l| l.trim().strip_prefix("XDG_DOCUMENTS_DIR="))?.trim().trim_matches('"');
    let path = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => std::path::PathBuf::from(value),
    };
    (path != home).then_some(path)
}

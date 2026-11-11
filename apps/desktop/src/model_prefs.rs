//! The model list's Favorites and Recent (`session_view::pickers::prefs`), kept in the app's
//! `model-prefs.json`. A run with no person at it (headless, tests) reads them but never writes
//! them back, as it never writes tabs, drafts or settings.

use std::path::PathBuf;

pub use session_view::pickers::ModelPrefs;

pub fn load() -> ModelPrefs {
    let write: fn(PathBuf, String) = if crate::headless() { keep } else { write };
    ModelPrefs::load(crate::dirs::data().map(|d| d.join("model-prefs.json")), write)
}

fn write(path: PathBuf, body: String) {
    crate::disk::later("model-prefs", move || {
        crate::disk::replace(&path, body.as_bytes());
    });
}

/// Picks and stars in a headless run stay in its memory.
fn keep(_: PathBuf, _: String) {}

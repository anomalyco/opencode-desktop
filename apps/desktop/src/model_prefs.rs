//! The model list's Favorites and Recent (`session_view::pickers::prefs`), kept in the app's
//! `model-prefs.json`.

use std::path::PathBuf;

pub use session_view::pickers::ModelPrefs;

pub fn load() -> ModelPrefs {
    ModelPrefs::load(crate::dirs::data().map(|d| d.join("model-prefs.json")), write)
}

fn write(path: PathBuf, body: String) {
    crate::disk::later("model-prefs", move || {
        crate::disk::replace(&path, body.as_bytes());
    });
}

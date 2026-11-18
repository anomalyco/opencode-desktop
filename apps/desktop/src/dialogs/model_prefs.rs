//! The model list's Favorites and Recent (`session_view::pickers::prefs`), kept in Desktop's
//! global `model` key (`providers/models.tsx` `ModelState`). A run with no person at it
//! (headless, tests) reads them but never writes them back, as it never writes tabs, drafts or
//! settings.

pub use session_view::pickers::ModelPrefs;

pub fn load() -> ModelPrefs {
    let v = crate::storage::json("opencode.global.dat", "model").unwrap_or_default();
    ModelPrefs::from_json(&v, (!crate::headless()).then_some(write as fn(String)))
}

fn write(body: String) {
    crate::storage::set("opencode.global.dat", "model", body);
}

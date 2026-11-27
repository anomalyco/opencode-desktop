//! The model list's Favorites and Recent, each model's speed and effort
//! (`session_view::pickers::prefs`), kept in Desktop's global `model` key
//! (`providers/models.tsx` `ModelState`). A run with no person at it (headless) reads them but
//! never writes them back, as it never writes tabs, drafts or settings; the harness standing in
//! for one (`--harness-writes`) saves them into its own root, as a window does.

pub use session_view::pickers::ModelPrefs;

pub fn load() -> ModelPrefs {
    let v = crate::storage::json("opencode.global.dat", "model").unwrap_or_default();
    ModelPrefs::from_json(&v, crate::attended().then_some(write as fn(String)))
}

fn write(body: String) {
    crate::storage::set("opencode.global.dat", "model", body);
}

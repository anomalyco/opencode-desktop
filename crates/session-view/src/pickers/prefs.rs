//! The model list's Favorites and Recent, as Desktop keeps them (`providers/models.tsx`
//! `ModelState`): lists of `{ providerID, modelID }`, newest first. Recent holds the last five
//! picks. Saved to the host's `model-prefs.json`; fixtures and tests keep them in memory.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

const RECENT_LIMIT: usize = 5;

/// How a host writes the prefs file: the path and the JSON body.
pub type Write = fn(PathBuf, String);

#[derive(Default)]
pub struct ModelPrefs {
    /// `provider:id` keys, newest first.
    pub favorites: Vec<String>,
    pub recent: Vec<String>,
    path: Option<PathBuf>,
    write: Option<Write>,
}

fn keys(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten()
        .filter_map(|m| Some(format!("{}:{}", m["providerID"].as_str()?, m["modelID"].as_str()?)))
        .collect()
}

fn refs(keys: &[String]) -> Vec<Value> {
    keys.iter().filter_map(|k| k.split_once(':')).map(|(p, m)| json!({ "providerID": p, "modelID": m })).collect()
}

/// Writes `body` to `path` through a temporary file, so a crash never leaves half a file.
pub fn replace(path: &Path, body: &str) -> bool {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).is_ok() && std::fs::rename(tmp, path).is_ok()
}

/// The default writer: off the main thread, through [`replace`].
pub fn write_later(path: PathBuf, body: String) {
    std::thread::spawn(move || replace(&path, &body));
}

impl ModelPrefs {
    /// Reads `path` (none keeps them in memory); picks and stars are saved with `write`.
    pub fn load(path: Option<PathBuf>, write: Write) -> ModelPrefs {
        let v: Value = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let mut recent = keys(&v["recent"]);
        recent.truncate(RECENT_LIMIT);
        ModelPrefs { favorites: keys(&v["favorite"]), recent, path, write: Some(write) }
    }

    pub fn is_favorite(&self, key: &str) -> bool {
        self.favorites.iter().any(|k| k == key)
    }

    /// Stars or unstars a model; a new favourite goes first.
    pub fn toggle_favorite(&mut self, key: &str) {
        if self.is_favorite(key) {
            self.favorites.retain(|k| k != key);
        } else {
            self.favorites.insert(0, key.to_owned());
        }
        self.save();
    }

    /// A pick: first in Recent, once.
    pub fn picked(&mut self, key: &str) {
        self.recent.retain(|k| k != key);
        self.recent.insert(0, key.to_owned());
        self.recent.truncate(RECENT_LIMIT);
        self.save();
    }

    fn save(&self) {
        let (Some(path), Some(write)) = (self.path.clone(), self.write) else { return };
        let body = json!({ "favorite": refs(&self.favorites), "recent": refs(&self.recent) }).to_string();
        write(path, body);
    }
}

//! The model list's Favorites, Recent and what Manage models shows, as Desktop keeps them
//! (`providers/models.tsx` `ModelState`): lists of `{ providerID, modelID }`, newest first.
//! Recent holds the last five picks; `user` holds each model Manage models showed or hid. The
//! host keeps them (Desktop's `model` key); fixtures and tests keep them in memory.

use std::collections::BTreeMap;

use serde_json::{Value, json};

const RECENT_LIMIT: usize = 5;

/// How a host keeps the prefs: their JSON body, after each change.
pub type Write = fn(String);

#[derive(Default)]
pub struct ModelPrefs {
    /// `provider:id` keys, newest first.
    pub favorites: Vec<String>,
    pub recent: Vec<String>,
    /// `provider:id` keys Manage models showed (true) or hid (false), in the order first set.
    pub user: Vec<(String, bool)>,
    /// The speed last chosen for each model, by `provider:id`; a model without one runs at its
    /// standard speed (`ModelState.speed`, keyed `provider/model` there).
    speeds: BTreeMap<String, String>,
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

impl ModelPrefs {
    /// The prefs saved as `v` (Desktop's `ModelState`); picks and stars are kept with `write`.
    pub fn from_json(v: &Value, write: Option<Write>) -> ModelPrefs {
        let mut recent = keys(&v["recent"]);
        recent.truncate(RECENT_LIMIT);
        let user = v["user"].as_array().into_iter().flatten()
            .filter_map(|m| Some((format!("{}:{}", m["providerID"].as_str()?, m["modelID"].as_str()?), m["visibility"].as_str()? == "show")))
            .collect();
        let speeds = v["speed"].as_object().into_iter().flatten()
            .filter_map(|(k, speed)| Some((k.split_once('/').map(|(p, m)| format!("{p}:{m}"))?, speed.as_str()?.to_owned())))
            .collect();
        ModelPrefs { favorites: keys(&v["favorite"]), recent, user, speeds, write }
    }

    pub fn is_favorite(&self, key: &str) -> bool {
        self.favorites.iter().any(|k| k == key)
    }

    /// Whether Manage models showed (`Some(true)`) or hid the model; none when it never did.
    pub fn visibility(&self, key: &str) -> Option<bool> {
        self.user.iter().find(|(k, _)| k == key).map(|(_, shown)| *shown)
    }

    /// Shows or hides models in the list, as Manage models' switches do.
    pub fn set_visibility<'a>(&mut self, keys: impl IntoIterator<Item = &'a str>, shown: bool) {
        for key in keys {
            match self.user.iter_mut().find(|(k, _)| k == key) {
                Some(entry) => entry.1 = shown,
                None => self.user.push((key.to_owned(), shown)),
            }
        }
        self.save();
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

    /// The speed last chosen for the model `key`, if not its standard one.
    pub fn speed(&self, key: &str) -> Option<&str> {
        self.speeds.get(key).map(String::as_str)
    }

    /// Remembers the model `key`'s speed; none is its standard one (`models.tsx` `speed.remember`).
    pub fn set_speed(&mut self, key: &str, speed: Option<&str>) {
        match speed {
            Some(speed) => self.speeds.insert(key.to_owned(), speed.to_owned()),
            None => self.speeds.remove(key),
        };
        self.save();
    }

    fn save(&self) {
        let Some(write) = self.write else { return };
        let user: Vec<Value> = self.user.iter().filter_map(|(k, shown)| {
            let (p, m) = k.split_once(':')?;
            Some(json!({ "providerID": p, "modelID": m, "visibility": if *shown { "show" } else { "hide" } }))
        }).collect();
        let speed: serde_json::Map<String, Value> = self.speeds.iter()
            .filter_map(|(k, speed)| Some((k.split_once(':').map(|(p, m)| format!("{p}/{m}"))?, json!(speed))))
            .collect();
        let body = json!({ "favorite": refs(&self.favorites), "recent": refs(&self.recent), "user": user, "speed": speed }).to_string();
        write(body);
    }
}

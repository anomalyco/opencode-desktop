//! What the host keeps for plugins across their reloads: each plugin's stores, what the app
//! knows that plugins ask about, and the images they draw.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use serde_json::Value;

#[derive(Default)]
pub struct Shared {
    pub stores: Stores,
    pub facts: Facts,
    /// Images plugins handed over or loaded (`images`), drawn from renders too.
    pub images: RefCell<super::images::Images>,
}

/// What the app knows that plugins ask about.
#[derive(Default)]
pub struct Facts {
    /// The app's own server's version, once it answered.
    pub server_version: Option<String>,
    /// Contributed servers' versions once they answered healthy, by `<plugin>:<id>`.
    pub health: std::collections::HashMap<String, String>,
}
type Values = BTreeMap<String, BTreeMap<String, String>>;

thread_local! {
    /// The values every window's stores read and write: one copy per process, so two windows
    /// never write the same file from different copies.
    static VALUES: Rc<RefCell<Values>> = Rc::default();
}

/// Each plugin's stores: text values by name (`name` global, `name@<session>` per session),
/// in `<data>/plugins-state/<id>.json`, loaded before the plugin sets up, and shared by every
/// window.
pub struct Stores {
    plugins: Rc<RefCell<Values>>,
    pub persist: bool,
}

impl Default for Stores {
    fn default() -> Self {
        Stores { plugins: VALUES.with(Rc::clone), persist: false }
    }
}

fn file(plugin: &str) -> Option<PathBuf> {
    crate::dirs::data().map(|d| d.join("plugins-state").join(format!("{plugin}.json")))
}

pub fn key(name: &str, session: &str) -> String {
    if session.is_empty() { name.to_owned() } else { format!("{name}@{session}") }
}

impl Stores {
    /// Reads plugin `plugin`'s stores the first time something asks for one of its values, so a
    /// launch opens only the store files plugins read (16 probes for files that didn't exist
    /// cost 2,074-2,324 µs on the UI thread before the first frame).
    fn load(&self, plugin: &str) {
        if self.plugins.borrow().contains_key(plugin) {
            return;
        }
        let read = || -> Option<BTreeMap<String, String>> {
            let v: Value = serde_json::from_str(&std::fs::read_to_string(file(plugin)?).ok()?).ok()?;
            Some(v.as_object()?.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect())
        };
        let values = if self.persist { read().unwrap_or_default() } else { BTreeMap::new() };
        self.plugins.borrow_mut().insert(plugin.to_owned(), values);
    }

    /// The value as text.
    pub fn text(&self, plugin: &str, name: &str, session: &str) -> Option<String> {
        self.load(plugin);
        self.plugins.borrow().get(plugin)?.get(&key(name, session)).cloned()
    }

    pub fn set(&self, plugin: &str, name: &str, session: &str, value: &str) {
        // Before it's changed, so the file written keeps the values it held.
        self.load(plugin);
        let mut plugins = self.plugins.borrow_mut();
        let values = plugins.entry(plugin.to_owned()).or_default();
        if values.get(&key(name, session)).map(String::as_str) == Some(value) {
            return;
        }
        values.insert(key(name, session), value.to_owned());
        if !self.persist {
            return;
        }
        let Some(path) = file(plugin) else { return };
        let json = serde_json::to_string(values).unwrap_or_default();
        crate::disk::later(super::icons::intern(&format!("plugin-store-{plugin}")), move || {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).ok();
            }
            crate::disk::replace(&path, json.as_bytes());
        });
    }
}

//! What the app keeps for plugins: each plugin's enabled override (`plugin`, Desktop's
//! `enable-state.ts`) and the files a third-party plugin's user trusted (`plugin_trust`), written
//! together as one Tier 0 record; each plugin's own values are `state` rows under
//! [`crate::plugin_namespace`] (Desktop's `plugin/storage.ts:31`).
//!
//! The first frame's plugins read their values from the boot record ([`boot_slice`]): every
//! plugin's global values and the values of the sessions the windows show, small ones only, with
//! a `Covered` entry saying the record knows them. Everything else loads when a plugin starts.

#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod native;

use crate::boot_packet::{BootPacket, Kind, Writer};
use crate::cache::Lookup;
use crate::{Cmd, Done, Storage};

/// A value larger than this stays out of the boot record (it reads as omitted).
pub(crate) const VALUE_CAP: usize = 8 * 1024;
/// The plugin values in the boot record in all: past it, values read as omitted.
pub(crate) const SLICE_CAP: usize = 16 * 1024;
/// Host bookkeeping keys in plugin namespaces (`rune.recent:<name>`): never in the record.
pub const HOST_KEY_PREFIX: &str = "rune.";

/// One plugin's record: its enabled override (none: as shipped, on) and the SHA-256 of the
/// files the user trusted (none: never trusted, or no longer).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginRecord {
    pub enabled: Option<bool>,
    pub trusted: Option<String>,
}

/// Plugin enable overrides and trust, Tier 0.
pub struct PluginStore<'a> {
    pub(crate) storage: &'a Storage,
}

impl PluginStore<'_> {
    /// The override for plugin `id`; none when it never changed (built-ins are on).
    pub fn enabled(&self, id: &str) -> Option<bool> {
        self.storage.cache().plugins.get(id).copied()
    }

    /// Every override.
    pub fn overrides(&self) -> Vec<(String, bool)> {
        self.storage.cache().plugins.iter().map(|(id, on)| (id.clone(), *on)).collect()
    }

    /// The SHA-256 the user trusted for plugin `id`'s files.
    pub fn trusted(&self, id: &str) -> Option<String> {
        self.storage.cache().trust.get(id).cloned()
    }

    pub fn trusted_all(&self) -> Vec<(String, String)> {
        self.storage.cache().trust.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// Writes plugin `id`'s whole record in one durable transaction; `done` runs once it is
    /// (`synchronous = FULL`), or with why it isn't.
    pub fn record(&self, id: &str, record: PluginRecord, done: Option<Done>) {
        let Some(_permit) = self.storage.write_permit() else {
            if let Some(done) = done { done(Err(crate::StorageError("the workspace is closing".into()))); }
            return;
        };
        {
            let mut cache = self.storage.cache();
            match record.enabled {
                Some(on) => cache.plugins.insert(id.to_owned(), on),
                None => cache.plugins.remove(id),
            };
            match &record.trusted {
                Some(sha) => cache.trust.insert(id.to_owned(), sha.clone()),
                None => cache.trust.remove(id),
            };
        }
        self.storage.send(Cmd::PluginRecord { id: id.to_owned(), record, done });
    }
}

/// The session a plugin value's key belongs to (`name@<scope>\0<dir>/<session>`, or an older
/// `name@<session>`); none for a global value.
fn session_of_key(key: &str) -> Option<&str> {
    let (_, scoped) = key.split_once('@')?;
    scoped.rsplit('/').next()
}

/// Whether the record carries plugin value `name`/`key` when it fits: a global value or a shown
/// session's, of a plugin namespace the record doesn't carry whole already.
pub(crate) fn wanted(name: &str, key: &str, shown: &[&str]) -> bool {
    name.starts_with("plugin.") && !crate::is_boot_namespace(name) && !key.starts_with(HOST_KEY_PREFIX) && session_of_key(key).is_none_or(|s| shown.contains(&s))
}

/// Writes the plugin slice: `Covered` when the record is authoritative for plugins, the shown
/// sessions, then each wanted value that fits (`PluginValue`) or doesn't (`Omitted`).
pub(crate) fn boot_slice<'a>(w: &mut Writer, covered: bool, shown: &[&str], rows: impl Iterator<Item = (&'a str, &'a str, &'a str)>) {
    if covered {
        w.push(Kind::Covered, "plugins", "", &[]);
    }
    for s in shown {
        w.push(Kind::Shown, s, "", &[]);
    }
    let mut used = 0;
    for (name, key, value) in rows {
        if value.len() <= VALUE_CAP && used + value.len() <= SLICE_CAP {
            used += value.len();
            w.push(Kind::PluginValue, name, key, value.as_bytes());
        } else {
            w.push(Kind::Omitted, name, key, &[]);
        }
    }
}

impl BootPacket {
    /// Whether the record knows the plugins' overrides, trust and values (the plugin files were
    /// imported, or there were none): otherwise only the database does.
    pub fn plugins_covered(&self) -> bool {
        self.of(Kind::Covered).next().is_some()
    }

    /// A plugin value: `Ready` when the record answers for it (its value, or none stored),
    /// `Pending` when only the database can.
    pub fn plugin_value(&self, name: &str, key: &str) -> Lookup {
        if !self.plugins_covered() {
            return Lookup::Pending;
        }
        if crate::is_boot_namespace(name) {
            return Lookup::Ready(self.state(name, key).map(Into::into));
        }
        if let Some(e) = self.of(Kind::PluginValue).find(|e| self.str_of(&e.a) == name && self.str_of(&e.b) == key) {
            return Lookup::Ready(Some(self.str_of(&e.value).into()));
        }
        let shown: Vec<&str> = self.of(Kind::Shown).map(|e| self.str_of(&e.a)).collect();
        let omitted = self.of(Kind::Omitted).any(|e| self.str_of(&e.a) == name && self.str_of(&e.b) == key);
        if omitted || !wanted(name, key, &shown) {
            return Lookup::Pending;
        }
        Lookup::Ready(None)
    }

    /// The bytes of plugin values the record holds.
    pub fn plugin_bytes(&self) -> usize {
        self.of(Kind::PluginValue).map(|e| e.a.len() + e.b.len() + e.value.len() + 12).sum()
    }

    /// The plugin values the record holds, by namespace and key.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn plugin_values(&self) -> impl Iterator<Item = (&str, &str, &str)> {
        self.of(Kind::PluginValue).map(|e| (self.str_of(&e.a), self.str_of(&e.b), self.str_of(&e.value)))
    }
}

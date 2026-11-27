//! `widget.storage` (`widgets-live.md`, Storage): what a live run starts with as
//! `__input().storage`, and the host's backstop checks of a `storage` post, for a program that
//! posts around the prelude's own. The client follows the space and carries the writes.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// A key's most UTF-16 units.
pub const MAX_KEY: usize = 256;
/// A value's most UTF-8 bytes of JSON text.
pub const MAX_VALUE: usize = 64 * 1024;
/// The most keys a widget template's storage holds.
pub const MAX_KEYS: usize = 1000;
/// The most UTF-8 bytes of keys and values together a widget template's storage holds.
pub const MAX_BYTES: usize = 1 << 20;

/// What the client gives a live run of `widget.storage`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Storage {
    /// The client has no spaces sync: `storage` is left out of the input (`Unsupported`).
    #[default]
    Unsupported,
    /// The widget template doesn't declare `storage`: `storage: null` (`CapabilityDenied`).
    Undeclared,
    /// The client's copy of the widget template's space, JSON text by key.
    Copy(BTreeMap<String, String>),
}

impl Storage {
    /// `__input().storage`, when it has one.
    pub(super) fn input(&self) -> Option<Value> {
        match self {
            Storage::Unsupported => None,
            Storage::Undeclared => Some(Value::Null),
            Storage::Copy(entries) => Some(json!({ "entries": entries.iter().map(|(key, value)| json!({ "key": key, "value": value })).collect::<Vec<_>>() })),
        }
    }

    /// A change of the space: the run's copy follows it, so a run started again from `main`
    /// starts with what it holds now.
    pub(super) fn apply(&mut self, key: &str, value: Option<&str>) {
        let Storage::Copy(entries) = self else { return };
        match value {
            Some(v) => {
                entries.insert(key.to_owned(), v.to_owned());
            }
            None => {
                entries.remove(key);
            }
        }
    }

    /// Checks a storage post of the run's (`value` none: a delete) against the limits, as the TS
    /// host does (`packages/widget/src/live.ts`, `accept`), and against what the copy holds, in
    /// the prelude's words (`prelude-live.js`, `widget.storage.set`). Why it's invalid, if it is.
    pub(super) fn check(&self, key: Option<&str>, value: Option<Option<&str>>) -> Result<(String, Option<String>), String> {
        let key = key.filter(|k| !k.is_empty() && k.encode_utf16().count() <= MAX_KEY).ok_or_else(|| format!("A storage post needs a key of 1 to {MAX_KEY} characters"))?;
        let value = match value {
            None => None,
            Some(v) => Some(v.filter(|v| v.len() <= MAX_VALUE).ok_or_else(|| format!("A storage post's value must be JSON text of at most {} KiB", MAX_VALUE >> 10))?),
        };
        let entries = match self {
            Storage::Copy(entries) => entries,
            Storage::Unsupported => return Err("Unsupported: widget.storage: storage needs a client with spaces sync (OpenCode desktop or Android); this client has none".into()),
            Storage::Undeclared => return Err("CapabilityDenied: widget.storage: storage is not declared: save the widget template with capabilities: { storage: {} }".into()),
        };
        if let Some(v) = value {
            let before = entries.get(key);
            if before.is_none() && entries.len() >= MAX_KEYS {
                return Err(format!("TooLarge: widget.storage: storage holds {MAX_KEYS} keys already"));
            }
            let total: usize = entries.iter().map(|(k, v)| k.len() + v.len()).sum();
            if total + key.len() + v.len() - before.map_or(0, |b| key.len() + b.len()) > MAX_BYTES {
                return Err(format!("TooLarge: widget.storage: storage would hold over {} MiB", MAX_BYTES >> 20));
            }
        }
        Ok((key.to_owned(), value.map(str::to_owned)))
    }
}

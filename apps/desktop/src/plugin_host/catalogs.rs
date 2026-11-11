//! A plugin's message catalogs (`locales/<lang>.json`), by language. A built-in's are the table
//! `build.rs` made of them (`catalog_table`), read where it lies in the binary: starting the plugin
//! parses and copies nothing, and a lookup touches only the language asked for. A folder plugin's
//! files are parsed one language at a time, the first time a string is read in it. Either way a
//! language's machine-translated fill (`locales/fill/<lang>.json`) is under its own file.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub enum Catalogs {
    Table(&'static [u8]),
    Folder(Arc<Folder>),
}

impl Default for Catalogs {
    fn default() -> Self {
        Catalogs::Table(&[])
    }
}

/// A folder plugin's catalog files by language, and the ones parsed so far (shared with the
/// hot-reload thread, which prepares them).
#[derive(Default)]
pub struct Folder {
    texts: BTreeMap<String, String>,
    parsed: Mutex<BTreeMap<String, Arc<BTreeMap<String, String>>>>,
}

impl Catalogs {
    /// The catalogs among a plugin folder's `files`: flat objects of strings.
    pub fn folder(files: &[(String, String)]) -> Catalogs {
        let texts = files.iter().filter_map(|(path, text)| Some((path.strip_prefix("locales/")?.strip_suffix(".json")?.to_owned(), text.clone()))).collect();
        Catalogs::Folder(Arc::new(Folder { texts, parsed: Mutex::default() }))
    }

    /// Message `key` in language `lang`.
    pub fn get(&self, lang: &str, key: &str) -> Option<Cow<'static, str>> {
        match self {
            Catalogs::Table(t) => Table(t).get(lang, key).map(Cow::Borrowed),
            Catalogs::Folder(f) => f.language(lang)?.get(key).cloned().map(Cow::Owned),
        }
    }
}

impl Folder {
    /// Language `lang`'s strings, parsed on first use; None when the plugin has no file for it.
    fn language(&self, lang: &str) -> Option<Arc<BTreeMap<String, String>>> {
        if let Some(found) = self.parsed.lock().unwrap_or_else(|e| e.into_inner()).get(lang) {
            return Some(found.clone());
        }
        let parse = |text: &str| -> Option<BTreeMap<String, String>> {
            let v: serde_json::Value = serde_json::from_str(text).ok()?;
            Some(v.as_object()?.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect())
        };
        // `fill/<lang>`: machine translations of what the plugin's own file lacks, under it.
        let own = self.texts.get(lang).and_then(|t| parse(t));
        let fill = self.texts.get(&format!("fill/{lang}")).and_then(|t| parse(t));
        if own.is_none() && fill.is_none() {
            return None;
        }
        let mut strings = fill.unwrap_or_default();
        strings.extend(own.unwrap_or_default());
        let map = Arc::new(strings);
        self.parsed.lock().unwrap_or_else(|e| e.into_inner()).insert(lang.to_owned(), map.clone());
        Some(map)
    }
}

/// `catalog_table`'s layout: `u32`s `[langs, keys]`, per language (name offset, name length,
/// entries offset), per key (offset, length) in sorted order, per language one (offset, length)
/// per key (length `u32::MAX` when missing), then the UTF-8 the offsets point into.
struct Table(&'static [u8]);

impl Table {
    fn get(&self, lang: &str, key: &str) -> Option<&'static str> {
        let (langs, keys) = (self.word(0)? as usize, self.word(1)? as usize);
        let lang = find(langs, |i| self.text(2 + i * 3).map(|l| l.cmp(lang)))?;
        let entries = self.word(2 + lang * 3 + 2)? as usize / 4;
        let key = find(keys, |i| self.text(2 + langs * 3 + i * 2).map(|k| k.cmp(key)))?;
        self.text(entries + key * 2)
    }

    fn word(&self, i: usize) -> Option<u32> {
        self.0.get(i * 4..i * 4 + 4).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_le_bytes)
    }

    /// The string whose (offset, length) is at word `i`.
    fn text(&self, i: usize) -> Option<&'static str> {
        let (at, len) = (self.word(i)? as usize, self.word(i + 1)?);
        if len == u32::MAX {
            return None;
        }
        std::str::from_utf8(self.0.get(at..at + len as usize)?).ok()
    }
}

/// The index in `0..n` whose item `cmp` finds equal, by binary search over sorted items.
fn find(n: usize, cmp: impl Fn(usize) -> Option<Ordering>) -> Option<usize> {
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        match cmp(mid)? {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Some(mid),
        }
    }
    None
}

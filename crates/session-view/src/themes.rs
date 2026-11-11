//! Desktop's colour themes (Settings › Appearance › Theme). `assets/themes.json` holds each
//! of Desktop's theme files resolved as Desktop resolves them (`tools/themes.ts`), so a theme
//! here paints what it paints there. The default, OpenCode, keeps `assets/theme.json` and
//! takes only its code colours from there.
//!
//! The chosen theme is per thread, like Rune's blend mode: the view reads it through
//! `palette::pal`, and tests on other threads keep the default.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

use serde_json::{Map, Value};

const THEMES: &str = include_str!("../../../apps/desktop/assets/themes.json");

/// Desktop's id for its default theme.
pub const DEFAULT: &str = "oc-2";

pub struct Info {
    pub id: String,
    pub name: String,
    /// The theme's tokens for light and dark, laid over `assets/theme.json`'s.
    tokens: [Map<String, Value>; 2],
}

fn desktop() -> &'static [Info] {
    static ALL: OnceLock<Vec<Info>> = OnceLock::new();
    ALL.get_or_init(|| {
        let list: Vec<Value> = serde_json::from_str(THEMES).unwrap_or_else(|e| panic!("themes.json: {e}"));
        let mode = |t: &Value, m: &str| t[m].as_object().cloned().unwrap_or_default();
        let default = list.iter().find(|t| t["id"] == DEFAULT);
        let tokens = default.map(|t| [mode(t, "light"), mode(t, "dark")]).unwrap_or_default();
        std::iter::once(Info { id: DEFAULT.into(), name: "OpenCode".into(), tokens })
            .chain(list.iter().filter(|t| t["id"] != DEFAULT).map(|t| Info {
                id: t["id"].as_str().unwrap_or_default().to_owned(),
                name: t["name"].as_str().unwrap_or_default().to_owned(),
                tokens: [mode(t, "light"), mode(t, "dark")],
            }))
            .collect()
    })
}

/// The list, Desktop's themes then the ones the host registered. Each list is built once per
/// registration and kept, so a `&'static` slice stays valid.
static LIST: RwLock<Option<&'static [&'static Info]>> = RwLock::new(None);

/// Every theme in Desktop's picker order (OpenCode, then the rest by id), then the host's own
/// (a plugin's, registered at run time).
pub fn all() -> &'static [&'static Info] {
    if let Some(list) = *LIST.read().unwrap_or_else(|e| e.into_inner()) {
        return list;
    }
    let mut list = LIST.write().unwrap_or_else(|e| e.into_inner());
    list.get_or_insert_with(|| Box::leak(desktop().iter().collect::<Vec<_>>().into_boxed_slice()))
}

/// Adds theme `id` (or replaces the one with that id, keeping its place, so its index stays
/// put) with `light` and `dark` tokens laid over OpenCode's own, so a theme may set only the
/// tokens it changes. A change moves [`generation`], so the colours built for the old tokens
/// go and a hot-reloaded theme repaints at once.
pub fn register(id: &str, name: &str, tokens: [Map<String, Value>; 2]) {
    let [light, dark] = tokens;
    // Only colours: a token that doesn't read as one keeps OpenCode's, so a theme can't break
    // the palette.
    let over = |m: usize, own: Map<String, Value>| {
        let mut full = desktop()[0].tokens[m].clone();
        full.extend(own.into_iter().filter(|(_, v)| v.as_str().and_then(rune::color::hex).is_some()));
        full
    };
    let tokens = [over(0, light), over(1, dark)];
    let mut list: Vec<&'static Info> = all().to_vec();
    let at = list.iter().position(|t| t.id == id);
    if at.is_some_and(|i| list[i].name == name && list[i].tokens == tokens) {
        return;
    }
    let info: &'static Info = Box::leak(Box::new(Info { id: id.to_owned(), name: name.to_owned(), tokens }));
    match at {
        Some(i) => list[i] = info,
        None => list.push(info),
    }
    *LIST.write().unwrap_or_else(|e| e.into_inner()) = Some(Box::leak(list.into_boxed_slice()));
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Which registration of the themes is current: it moves each time [`register`] adds or
/// changes one. Caches of a theme's colours key on it with the theme's index.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// The index of `id` in [`all`], or the default's (0) for an unknown id.
pub fn index(id: &str) -> usize {
    all().iter().position(|t| t.id == id).unwrap_or(0)
}

thread_local! {
    static ACTIVE: Cell<usize> = const { Cell::new(0) };
}

/// The theme `palette::pal` paints with on this thread, as an index into [`all`].
pub fn active() -> usize {
    ACTIVE.with(Cell::get)
}

pub fn set_active(id: &str) {
    ACTIVE.with(|a| a.set(index(id)));
}

/// `base` (the default theme's JSON) with theme `i`'s tokens laid over both modes (the default's
/// are its code colours alone), or None when `base` has no modes to lay them over.
pub fn json(i: usize, base: &str) -> Option<String> {
    let t = all().get(i)?;
    let mut v: Value = serde_json::from_str(base).ok()?;
    v["name"] = Value::String(t.name.clone());
    for (m, tokens) in ["light", "dark"].into_iter().zip(&t.tokens) {
        let map = v[m].as_object_mut()?;
        map.extend(tokens.iter().map(|(k, val)| (k.clone(), val.clone())));
    }
    Some(v.to_string())
}


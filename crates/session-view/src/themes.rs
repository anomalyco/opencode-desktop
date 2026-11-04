//! Desktop's colour themes (Settings › Appearance › Theme). `assets/themes.json` holds each
//! of Desktop's theme files resolved as Desktop resolves them (`tools/themes.ts`), so a theme
//! here paints what it paints there. The default, OpenCode, keeps `assets/theme.json`.
//!
//! The chosen theme is per thread, like Rune's blend mode: the view reads it through
//! `palette::pal`, and tests on other threads keep the default.

use std::cell::Cell;
use std::sync::OnceLock;

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

/// Every theme in Desktop's picker order: OpenCode, then the rest by id.
pub fn all() -> &'static [Info] {
    static ALL: OnceLock<Vec<Info>> = OnceLock::new();
    ALL.get_or_init(|| {
        let list: Vec<Value> = serde_json::from_str(THEMES).unwrap_or_else(|e| panic!("themes.json: {e}"));
        let mode = |t: &Value, m: &str| t[m].as_object().cloned().unwrap_or_default();
        std::iter::once(Info { id: DEFAULT.into(), name: "OpenCode".into(), tokens: Default::default() })
            .chain(list.iter().map(|t| Info {
                id: t["id"].as_str().unwrap_or_default().to_owned(),
                name: t["name"].as_str().unwrap_or_default().to_owned(),
                tokens: [mode(t, "light"), mode(t, "dark")],
            }))
            .collect()
    })
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

/// `base` (the default theme's JSON) with theme `i`'s tokens laid over both modes, or None for
/// the default itself.
pub fn json(i: usize, base: &str) -> Option<String> {
    let t = all().get(i).filter(|_| i > 0)?;
    let mut v: Value = serde_json::from_str(base).ok()?;
    v["name"] = Value::String(t.name.clone());
    for (m, tokens) in ["light", "dark"].into_iter().zip(&t.tokens) {
        let map = v[m].as_object_mut()?;
        map.extend(tokens.iter().map(|(k, val)| (k.clone(), val.clone())));
    }
    Some(v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_themes_load_in_its_order_with_its_names() {
        let names: Vec<&str> = all().iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names[0], "OpenCode");
        assert!(names.contains(&"Tokyonight") && names.contains(&"Catppuccin") && names.contains(&"Synthwave '84"));
        assert_eq!(all().len(), 36, "Desktop's 36 theme files");
        assert_eq!(index("nope"), 0);
        assert!(json(0, "{}").is_none(), "the default keeps theme.json");
    }

    #[test]
    fn the_active_theme_is_per_thread() {
        set_active("tokyonight");
        assert_eq!(all()[active()].id, "tokyonight");
        std::thread::spawn(|| assert_eq!(active(), 0)).join().unwrap();
        set_active(DEFAULT);
        assert_eq!(active(), 0);
    }
}

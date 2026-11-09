//! `IconPool`: every string and icon a plugin hands the host for longer than one call, copied
//! into host memory and interned. The palette, `rune::El::role` and `rune::Icon` take
//! `&'static` data; pointing them into a plugin library's read-only data would dangle the moment
//! a hot reload unmapped it. Interning copies once per distinct value, so a reload that hands
//! the same strings and icons again costs nothing and leaks nothing new.

use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, MutexGuard};

use rune::Icon;

#[derive(Default)]
struct Pool {
    strings: BTreeSet<&'static str>,
    /// Ids are 1-based indexes; 0 is no icon.
    icons: Vec<&'static Icon>,
    /// `(plugin, name)` → its newest icon's id.
    named: HashMap<(String, String), u32>,
    /// `(name, body)` hash → id, so an icon interned again reuses its slot.
    by_content: HashMap<u64, u32>,
}

static POOL: Mutex<Option<Pool>> = Mutex::new(None);

fn pool() -> MutexGuard<'static, Option<Pool>> {
    POOL.lock().unwrap_or_else(|e| e.into_inner())
}

/// `s` as a `&'static str`, copied once per distinct string.
pub fn intern(s: &str) -> &'static str {
    let mut guard = pool();
    let pool = guard.get_or_insert_with(Pool::default);
    if let Some(&s) = pool.strings.get(s) {
        return s;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    pool.strings.insert(leaked);
    leaked
}

/// Copies plugin `plugin`'s icon `name` into the pool and returns its id. An empty `body` only
/// looks the name up (0 when the plugin has none by that name).
pub fn intern_icon(plugin: &str, name: &str, body: &str, view: f32) -> u32 {
    let key = (plugin.to_owned(), name.to_owned());
    if body.is_empty() {
        return pool().as_ref().and_then(|p| p.named.get(&key).copied()).unwrap_or(0);
    }
    let mut h = std::hash::DefaultHasher::new();
    (plugin, name, body, view.to_bits()).hash(&mut h);
    let content = h.finish();
    // The icon's name is what Rune caches its raster under: unique per content.
    let icon_name = intern(&format!("plugin-{plugin}-{name}-{content:016x}"));
    let body = intern(body);
    let mut guard = pool();
    let pool = guard.get_or_insert_with(Pool::default);
    let id = match pool.by_content.get(&content) {
        Some(&id) => id,
        None => {
            pool.icons.push(Box::leak(Box::new(Icon { name: icon_name, view: if view > 0.0 { view } else { 16.0 }, body })));
            let id = pool.icons.len() as u32;
            pool.by_content.insert(content, id);
            id
        }
    };
    pool.named.insert(key, id);
    id
}

/// Interns plugin `plugin`'s SVG file as its icon `name`: the `<svg>` element's content, sized
/// by its `viewBox` width (else its `width`, else 16). False when it isn't an `<svg>`.
pub fn intern_svg(plugin: &str, name: &str, svg: &str) -> bool {
    let Some(open) = svg.find("<svg") else { return false };
    let Some(end) = svg[open..].find('>').map(|e| open + e + 1) else { return false };
    let Some(close) = svg.rfind("</svg>").filter(|c| *c >= end) else { return false };
    let tag = &svg[open..end];
    let attr = |key: &str| tag.split(&format!("{key}=\"")).nth(1).and_then(|rest| rest.split('"').next());
    let view = attr("viewBox").and_then(|v| v.split_whitespace().nth(2)).or_else(|| attr("width")).and_then(|w| w.trim_end_matches("px").parse::<f32>().ok()).unwrap_or(16.0);
    intern_icon(plugin, name, svg[end..close].trim(), view) != 0
}

/// The icon with id `id`.
pub fn icon(id: u32) -> Option<&'static Icon> {
    let guard = pool();
    guard.as_ref()?.icons.get(id.checked_sub(1)? as usize).copied()
}

/// Plugin `plugin`'s icon `name`, else Desktop's icon by that name (`window-analytics`,
/// `chevron-right` …), else the host's panel icon by that name.
pub fn resolve(plugin: &str, name: &str) -> &'static Icon {
    // `file:<name>`: the file type icon Desktop's `FileIcon` draws for that name.
    if let Some(file) = name.strip_prefix("file:") {
        return rune_theme_opencode::file_icons::file(file);
    }
    let own = intern_icon(plugin, name, "", 0.0);
    // Icons the host made itself (a QR code) go by the empty plugin.
    let host = || intern_icon("", name, "", 0.0);
    let panel_names = ["globe", "terminal", "console", "review", "code", "file-tree", "folder"];
    icon(own)
        .or_else(|| icon(host()))
        .or_else(|| super::luau::native::icons::named(name))
        .or_else(|| (!panel_names.contains(&name)).then(|| rune_theme_opencode::icons::ALL.iter().copied().find(|i| i.name == name)).flatten())
        .unwrap_or_else(|| crate::panels::icon_by_name(Some(name)))
}

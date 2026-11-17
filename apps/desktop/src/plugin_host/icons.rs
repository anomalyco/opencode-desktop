//! `IconPool`: every string and icon a plugin hands the host for longer than one render,
//! copied into host memory and interned. The palette, `rune::El::role` and `rune::Icon` take
//! `&'static` data, while a plugin's strings live in its VM and go with it. Interning copies once
//! per distinct value, so a reload that hands the same strings and icons again costs nothing and
//! leaks nothing new.

use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, MutexGuard};

use rune::Icon;

#[derive(Default)]
struct Pool {
    strings: BTreeSet<&'static str>,
    /// Ids are 1-based indexes; 0 is no icon.
    icons: Vec<&'static Icon>,
    /// Plugin → name → its newest icon's id.
    named: HashMap<String, HashMap<String, u32>>,
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
    if body.is_empty() {
        return pool().as_ref().and_then(|p| p.named.get(plugin)?.get(name).copied()).unwrap_or(0);
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
    pool.named.entry(plugin.to_owned()).or_default().insert(name.to_owned(), id);
    NAMED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    id
}

/// Moves whenever a plugin names an icon, so a name [`resolve`] remembered resolves again.
static NAMED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
/// `chevron-right` …), else the host's panel icon by that name. `file:<file name>` is the file
/// icon Desktop's `FileIcon` draws for that file.
pub fn resolve(plugin: &str, name: &str) -> &'static Icon {
    // `file:<name>`: the file type icon Desktop's `FileIcon` draws for that name.
    if let Some(file) = name.strip_prefix("file:") {
        return rune_theme_opencode::file_icons::file(file);
    }
    // A render resolves the same few names many times: each thread remembers them until a
    // plugin names an icon of its own.
    thread_local! {
        static RESOLVED: std::cell::RefCell<(u64, Vec<(Box<str>, Box<str>, &'static Icon)>)> = std::cell::RefCell::default();
    }
    let named = NAMED.load(std::sync::atomic::Ordering::Relaxed);
    let hit = RESOLVED.with_borrow_mut(|(at, seen)| {
        if *at != named {
            *at = named;
            seen.clear();
        }
        seen.iter().find(|(p, n, _)| **n == *name && **p == *plugin).map(|(_, _, icon)| *icon)
    });
    if let Some(icon) = hit {
        return icon;
    }
    let found = resolve_fresh(plugin, name);
    RESOLVED.with_borrow_mut(|(_, seen)| {
        if seen.len() < 128 {
            seen.push((plugin.into(), name.into(), found));
        }
    });
    found
}

fn resolve_fresh(plugin: &str, name: &str) -> &'static Icon {
    let own = intern_icon(plugin, name, "", 0.0);
    // Icons the host made itself (a QR code) go by the empty plugin.
    let host = || intern_icon("", name, "", 0.0);
    icon(own)
        .or_else(|| icon(host()))
        .or_else(|| super::native::icons::named(name))
        .or_else(|| theme_icon(name))
        .unwrap_or_else(|| crate::panels::icon_by_name(Some(name)))
}

/// Desktop's icon named `name`, from a map built on first use.
fn theme_icon(name: &str) -> Option<&'static Icon> {
    static BY_NAME: std::sync::OnceLock<HashMap<&'static str, &'static Icon>> = std::sync::OnceLock::new();
    BY_NAME.get_or_init(|| rune_theme_opencode::icons::ALL.iter().rev().map(|i| (i.name, *i)).collect()).get(name).copied()
}

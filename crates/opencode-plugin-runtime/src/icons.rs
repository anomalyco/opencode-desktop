//! `IconPool`: every string and icon a plugin hands the host for longer than one render,
//! copied into host memory and interned. The palette, `rune::El::role` and `rune::Icon` take
//! `&'static` data, while a plugin's strings live in its VM and go with it. Interning copies once
//! per distinct value, so a reload that hands the same strings and icons again costs nothing and
//! leaks nothing new.

use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, MutexGuard};

use rune::Icon;

/// Desktop's `terminal` icon (`additional-icons.ts`): a prompt in a box.
pub static TERMINAL_ICON: Icon = Icon {
    name: "terminal-tab",
    view: 20.0,
    body: r#"<path d="M6.5 8L8.64286 10L6.5 12M10.9286 12H13.5M2 18H18V2H2V18Z" stroke="currentColor" stroke-linecap="square" fill="none"/>"#,
};

type ResolvedCache = (u64, Vec<(Box<str>, Box<str>, &'static Icon)>);

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

/// Hard cap on distinct icon files one plugin may register.
pub const MAX_ICONS_PER_PLUGIN: usize = 64;

/// Moves whenever a plugin names or withdraws an icon, so a name [`resolve`] remembered resolves again.
static NAMED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Sanitizes and interns plugin `plugin`'s SVG file as its icon `name`, returning an error reason
/// without interning anything when the SVG fails hostile-input checks.
pub fn intern_svg(plugin: &str, name: &str, svg: &str) -> Result<(), String> {
    let clean = match crate::svg::sanitize(svg) {
        Ok(clean) => clean,
        Err(why) => {
            #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
            eprintln!("plugin {plugin}: icons/{name}.svg rejected: {why}");
            return Err(why);
        }
    };
    if intern_icon(plugin, name, &clean.body, clean.view) == 0 {
        return Err("failed to intern sanitized icon".into());
    }
    Ok(())
}

/// Replaces all icons registered for `plugin` with sanitized entries from `(name, svg)` pairs.
/// Withdraws icons no longer present, enforces [`MAX_ICONS_PER_PLUGIN`], logs every refusal in
/// debug builds, and returns rejected `(name, reason)` entries so the host can log them to the
/// plugin console.
pub fn replace_plugin(plugin: &str, entries: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut rejected = Vec::new();
    let mut next_ids: Vec<(String, u32)> = Vec::new();
    for &(name, svg) in entries {
        if next_ids.len() >= MAX_ICONS_PER_PLUGIN {
            let why = format!("plugin exceeds {MAX_ICONS_PER_PLUGIN} icon limit");
            #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
            eprintln!("plugin {plugin}: icons/{name}.svg rejected: {why}");
            rejected.push((name.to_owned(), why));
            continue;
        }
        match crate::svg::sanitize(svg) {
            Ok(clean) => {
                let id = intern_icon_raw(plugin, name, &clean.body, clean.view);
                if id != 0 {
                    next_ids.push((name.to_owned(), id));
                }
            }
            Err(why) => {
                #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
                eprintln!("plugin {plugin}: icons/{name}.svg rejected: {why}");
                rejected.push((name.to_owned(), why));
            }
        }
    }
    let mut guard = pool();
    let pool = guard.get_or_insert_with(Pool::default);
    if next_ids.is_empty() {
        pool.named.remove(plugin);
    } else {
        let map = pool.named.entry(plugin.to_owned()).or_default();
        map.clear();
        for (name, id) in next_ids {
            map.insert(name, id);
        }
    }
    NAMED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    rejected
}

/// Withdraws all icons registered for `plugin` (on unload, disable, or trust revocation).
pub fn clear_plugin(plugin: &str) {
    let mut guard = pool();
    if let Some(pool) = guard.as_mut()
        && pool.named.remove(plugin).is_some()
    {
        NAMED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Whether `plugin` currently has a registered icon named `name`.
pub fn has_icon(plugin: &str, name: &str) -> bool {
    !plugin.is_empty() && intern_icon(plugin, name, "", 0.0) != 0
}

fn intern_icon_raw(plugin: &str, name: &str, body: &str, view: f32) -> u32 {
    if body.is_empty() {
        return 0;
    }
    let mut h = std::hash::DefaultHasher::new();
    (plugin, name, body, view.to_bits()).hash(&mut h);
    let content = h.finish();
    let icon_name = intern(&format!("plugin-{plugin}-{name}-{content:016x}"));
    let body = intern(body);
    let mut guard = pool();
    let pool = guard.get_or_insert_with(Pool::default);
    match pool.by_content.get(&content) {
        Some(&id) => id,
        None => {
            pool.icons.push(Box::leak(Box::new(Icon { name: icon_name, view: if view > 0.0 { view } else { 16.0 }, body })));
            let id = pool.icons.len() as u32;
            pool.by_content.insert(content, id);
            id
        }
    }
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
        static RESOLVED: std::cell::RefCell<ResolvedCache> = std::cell::RefCell::default();
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

/// Fallback icon lookup for standard panel icon names (matching `opencode-feature-panels`'s
/// `icon_by_name`).
pub fn panel_icon(name: &str) -> &'static Icon {
    use rune_theme_opencode::icons as ic;
    match name {
        "globe" => &ic::GLOBE,
        // Desktop's `terminal` icon: the prompt in its box, as the terminal tabs draw it.
        "terminal" => &TERMINAL_ICON,
        "console" => &ic::CONSOLE,
        "review" => &ic::REVIEW,
        "code" => &ic::CODE_LINES,
        "file-tree" => &ic::FILE_TREE,
        _ => &ic::FOLDER,
    }
}

fn resolve_fresh(plugin: &str, name: &str) -> &'static Icon {
    // `<owner>:<icon>`: explicit plugin-qualified icon name for cross-surface persistence
    // (such as closed tabs or menus). Never falls through to another plugin's icon when unloaded.
    if let Some((owner, icon_name)) = name.split_once(':')
        && !owner.is_empty()
        && !icon_name.is_empty()
    {
        return icon(intern_icon(owner, icon_name, "", 0.0)).unwrap_or_else(|| panel_icon(icon_name));
    }
    let own = intern_icon(plugin, name, "", 0.0);
    // Icons the host made itself (a QR code) go by the empty plugin.
    let host = || intern_icon("", name, "", 0.0);
    icon(own)
        .or_else(|| icon(host()))
        .or_else(|| crate::native_icons::named(name))
        .or_else(|| theme_icon(name))
        .unwrap_or_else(|| panel_icon(name))
}

/// Desktop's icon named `name`, from a map built on first use.
fn theme_icon(name: &str) -> Option<&'static Icon> {
    static BY_NAME: std::sync::OnceLock<HashMap<&'static str, &'static Icon>> = std::sync::OnceLock::new();
    BY_NAME
        .get_or_init(|| rune_theme_opencode::icons::ALL.iter().rev().map(|i| (i.name, *i)).collect())
        .get(name)
        .copied()
}

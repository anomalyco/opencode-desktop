//! Desktop's colour themes (Settings › Appearance › Theme). `assets/themes.json` holds each
//! of Desktop's theme files resolved as Desktop resolves them (`tools/themes.ts`), so a theme
//! here paints what it paints there. The default, OpenCode, keeps `assets/theme.json` and
//! takes only its code colours from there.
//!
//! The chosen theme is per thread, like Rune's blend mode: the view reads it through
//! `palette::pal`, and tests on other threads keep the default.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::hash::{BuildHasher, RandomState};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use serde_json::{Map, Value};

pub mod seed;

const THEMES: &str = include_str!("../../../apps/desktop/assets/themes.json");

/// Desktop's id for its default theme.
pub const DEFAULT: &str = "oc-2";

pub struct Info {
    pub id: String,
    pub name: String,
    tokens: Tokens,
    palette_revision: u64,
}

/// A theme's tokens for light and dark, laid over `assets/theme.json`'s.
enum Tokens {
    /// One of Desktop's: its object in `themes.json`, read when the theme is first painted or
    /// compared. Reading every theme at start kept 1 MB that only the theme in use needs.
    Asset {
        json: &'static str,
        read: OnceLock<[Map<String, Value>; 2]>,
    },
    /// A registered theme's own.
    Given([Map<String, Value>; 2]),
}

impl Info {
    fn registered(id: String, name: String, tokens: [Map<String, Value>; 2]) -> Self {
        static PALETTES: OnceLock<RandomState> = OnceLock::new();
        let palette_revision = PALETTES
            .get_or_init(RandomState::new)
            .hash_one((&id, &tokens));
        Self {
            id,
            name,
            tokens: Tokens::Given(tokens),
            palette_revision,
        }
    }

    fn tokens(&self) -> &[Map<String, Value>; 2] {
        match &self.tokens {
            Tokens::Given(tokens) => tokens,
            Tokens::Asset { json, read } => read.get_or_init(|| {
                let t: Value = serde_json::from_str(json)
                    .unwrap_or_else(|e| panic!("themes.json: {}: {e}", self.id));
                ["light", "dark"].map(|m| t[m].as_object().cloned().unwrap_or_default())
            }),
        }
    }
}

/// A theme's id and name in `themes.json`; its modes are skipped, not built.
#[derive(serde::Deserialize)]
struct Head {
    id: String,
    name: String,
}

fn desktop() -> &'static [Arc<Info>] {
    static ALL: OnceLock<Vec<Arc<Info>>> = OnceLock::new();
    ALL.get_or_init(|| {
        // Each element's own text, so one theme can be read alone later.
        let mut rest = THEMES
            .trim_start()
            .strip_prefix('[')
            .unwrap_or_else(|| panic!("themes.json: not a list"));
        let mut list: Vec<(Head, &'static str)> = Vec::new();
        loop {
            rest = rest.trim_start().trim_start_matches(',').trim_start();
            if rest.is_empty() || rest.starts_with(']') {
                break;
            }
            let mut one = serde_json::Deserializer::from_str(rest).into_iter::<Head>();
            let head = match one.next() {
                Some(Ok(head)) => head,
                Some(Err(e)) => panic!("themes.json: {e}"),
                None => break,
            };
            let (json, after) = rest.split_at(one.byte_offset());
            list.push((head, json));
            rest = after;
        }
        let asset = |json| Tokens::Asset {
            json,
            read: OnceLock::new(),
        };
        let default = list.iter().find(|(h, _)| h.id == DEFAULT).map_or_else(
            || Tokens::Given(Default::default()),
            |(_, json)| asset(*json),
        );
        std::iter::once(Info {
            id: DEFAULT.into(),
            name: "OpenCode".into(),
            tokens: default,
            palette_revision: 0,
        })
        .chain(
            list.into_iter()
                .filter(|(h, _)| h.id != DEFAULT)
                .map(|(h, json)| Info {
                    id: h.id,
                    name: h.name,
                    tokens: asset(json),
                    palette_revision: 0,
                }),
        )
        .map(Arc::new)
        .collect()
    })
}
type List = Arc<[Arc<Info>]>;

#[derive(Clone)]
struct Snapshot {
    themes: List,
    generation: u64,
}

static LIST: RwLock<Option<Snapshot>> = RwLock::new(None);

fn builtins() -> Snapshot {
    if let Some(list) = LIST.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return list.clone();
    }
    LIST.write()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(|| Snapshot {
            themes: desktop().into(),
            generation: 0,
        })
        .clone()
}

pub struct PluginTheme {
    pub id: String,
    pub name: String,
    pub tokens: [Map<String, Value>; 2],
}

/// Only the window that adopted a plugin can see the themes that plugin contributed.
#[derive(Clone, Default)]
pub struct WindowThemes(Rc<RefCell<WindowList>>);

#[derive(Default)]
struct WindowList {
    plugins: BTreeMap<String, Vec<Arc<Info>>>,
    snapshot: Option<(u64, Snapshot)>,
    changed: bool,
    system: SystemTheme,
}

#[derive(Default)]
struct SystemTheme {
    contrast: bool,
    raised: bool,
}

impl WindowThemes {
    /// Atomically replaces one plugin's themes, including withdrawing themes missing after reload.
    pub fn replace(&self, plugin: &str, themes: Vec<PluginTheme>) -> bool {
        let mut state = self.0.borrow_mut();
        let mut entries = BTreeMap::new();
        for PluginTheme { id, name, tokens } in themes {
            let id = format!("{plugin}:{id}");
            entries.insert(
                id.clone(),
                Arc::new(Info::registered(id, name, with_defaults(tokens))),
            );
        }
        let entries: Vec<_> = entries.into_values().collect();
        let previous = state.plugins.get(plugin).map_or(&[][..], Vec::as_slice);
        if previous.len() == entries.len()
            && previous
                .iter()
                .zip(&entries)
                .all(|(a, b)| a.id == b.id && a.name == b.name && a.tokens() == b.tokens())
        {
            return false;
        }
        if entries.is_empty() {
            state.plugins.remove(plugin);
        } else {
            state.plugins.insert(plugin.to_owned(), entries);
        }
        state.snapshot = None;
        state.changed = true;
        true
    }

    pub fn take_changed(&self) -> bool {
        std::mem::take(&mut self.0.borrow_mut().changed)
    }

    /// The full Rune turn, including retained render regions and events, reads this window's themes.
    pub fn enter(&self, selected: &str) {
        let state = self.0.borrow();
        WINDOWS.with_borrow_mut(|windows| {
            windows.push(WindowTheme {
                list: Rc::clone(&self.0),
                selected: if state.system.contrast {
                    CONTRAST
                } else {
                    selected
                }
                .to_owned(),
                raised: RAISED.replace(state.system.raised),
            })
        });
    }

    pub fn leave(&self) {
        if let Some(window) = WINDOWS.with_borrow_mut(Vec::pop) {
            RAISED.with(|raised| raised.set(window.raised));
        }
    }

    pub fn note_system(&self, contrast: bool, raised: bool) {
        self.0.borrow_mut().system = SystemTheme { contrast, raised };
    }

    /// Renderers that bypass native window turns (such as command-line captures) still need
    /// this window's list. A native turn already has it installed.
    pub fn with<R>(&self, selected: &str, f: impl FnOnce() -> R) -> R {
        if self.is_current() {
            return f();
        }
        struct Leave<'a>(&'a WindowThemes);
        impl Drop for Leave<'_> {
            fn drop(&mut self) {
                self.0.leave();
            }
        }
        self.enter(selected);
        let _leave = Leave(self);
        f()
    }

    pub fn is_current(&self) -> bool {
        WINDOWS.with_borrow(|windows| windows.last().is_some_and(|w| Rc::ptr_eq(&w.list, &self.0)))
    }
}

struct WindowTheme {
    list: Rc<RefCell<WindowList>>,
    selected: String,
    raised: bool,
}

thread_local! {
    static WINDOWS: RefCell<Vec<WindowTheme>> = const { RefCell::new(Vec::new()) };
    static ACTIVE: RefCell<String> = const { RefCell::new(String::new()) };
}

fn snapshot() -> Snapshot {
    let base = builtins();
    WINDOWS.with_borrow(|windows| {
        let Some(window) = windows.last() else {
            return base;
        };
        let mut list = window.list.borrow_mut();
        if list.plugins.is_empty() {
            return base;
        }
        if let Some((generation, snapshot)) = &list.snapshot
            && *generation == base.generation
        {
            return snapshot.clone();
        }
        let snapshot = Snapshot {
            themes: base
                .themes
                .iter()
                .chain(list.plugins.values().flatten())
                .cloned()
                .collect(),
            generation: next_generation(),
        };
        list.snapshot = Some((base.generation, snapshot.clone()));
        snapshot
    })
}

/// Every theme in Desktop's picker order (OpenCode, then the rest by id), then the host's own
/// (a plugin's, registered at run time).
pub fn all() -> Arc<[Arc<Info>]> {
    snapshot().themes
}

/// Adds theme `id` (or replaces the one with that id, keeping its place, so its index stays
/// put) with `light` and `dark` tokens laid over OpenCode's own, so a theme may set only the
/// tokens it changes. A change moves [`generation`], so the colours built for the old tokens
/// go and a hot-reloaded theme repaints at once.
pub fn register(id: &str, name: &str, tokens: [Map<String, Value>; 2]) {
    let tokens = with_defaults(tokens);
    let _ = builtins();
    let mut list = LIST.write().unwrap_or_else(|e| e.into_inner());
    let Some(previous) = list.as_ref() else {
        return;
    };
    let mut entries = previous.themes.to_vec();
    let at = entries.iter().position(|t| t.id == id);
    if at.is_some_and(|i| entries[i].name == name && *entries[i].tokens() == tokens) {
        return;
    }
    let info = Arc::new(Info::registered(id.to_owned(), name.to_owned(), tokens));
    match at {
        Some(i) => entries[i] = info,
        None => entries.push(info),
    }
    *list = Some(Snapshot {
        themes: entries.into(),
        generation: next_generation(),
    });
}

fn with_defaults(tokens: [Map<String, Value>; 2]) -> [Map<String, Value>; 2] {
    let [light, dark] = tokens;
    // Only colours: a token that doesn't read as one keeps OpenCode's, so a theme can't break
    // the palette.
    let over = |m: usize, own: Map<String, Value>| {
        let mut full = desktop()[0].tokens()[m].clone();
        full.extend(
            own.into_iter()
                .filter(|(_, v)| v.as_str().and_then(rune::color::hex).is_some()),
        );
        full
    };
    [over(0, light), over(1, dark)]
}

fn next_generation() -> u64 {
    static GENERATION: AtomicU64 = AtomicU64::new(1);
    GENERATION.fetch_add(1, Ordering::Relaxed)
}

/// The revision of the picker list, including changes to unselected themes.
pub fn generation() -> u64 {
    snapshot().generation
}

/// The index of `id` in [`all`], or the default's (0) for an unknown id.
pub fn index(id: &str) -> usize {
    all().iter().position(|t| t.id == id).unwrap_or(0)
}

/// The theme `palette::pal` paints with on this thread, as an index into [`all`].
pub fn active() -> usize {
    active_in(&snapshot())
}

pub(crate) fn active_palette() -> (usize, Arc<Info>) {
    let snapshot = snapshot();
    let i = active_in(&snapshot);
    let info = &snapshot.themes[i];
    // Picker positions can shift when another plugin adds or removes an option.
    let slot = if matches!(info.tokens, Tokens::Given(_)) && i != 0 {
        usize::MAX
    } else {
        i
    };
    (slot, info.clone())
}

fn active_in(snapshot: &Snapshot) -> usize {
    let position = |id: &str| {
        snapshot
            .themes
            .iter()
            .position(|theme| theme.id == id)
            .unwrap_or(0)
    };
    WINDOWS.with_borrow(|windows| {
        windows.last().map_or_else(
            || ACTIVE.with_borrow(|id| position(id)),
            |window| position(&window.selected),
        )
    })
}

impl Info {
    pub(crate) fn palette_revision(&self) -> u64 {
        self.palette_revision
    }
}

pub fn set_active(id: &str) {
    WINDOWS.with_borrow_mut(|windows| {
        if let Some(window) = windows.last_mut() {
            if window.selected != id {
                id.clone_into(&mut window.selected);
            }
        } else {
            ACTIVE.with_borrow_mut(|selected| {
                if *selected != id {
                    id.clone_into(selected);
                }
            });
        }
    });
}

thread_local! {
    static RAISED: Cell<bool> = const { Cell::new(false) };
}

/// Whether this thread's palette draws the contrast highlights over its theme's colours:
/// the system asks for more contrast (macOS's Increase Contrast, `Cx::system_increased_contrast`).
pub fn raised_contrast() -> bool {
    RAISED.with(Cell::get)
}

pub fn set_raised_contrast(on: bool) {
    RAISED.with(|r| r.set(on));
}

/// `base` (the default theme's JSON) with theme `i`'s tokens laid over both modes (the default's
/// are its code colours alone), or None when `base` has no modes to lay them over.
pub fn json(i: usize, base: &str) -> Option<String> {
    let all = all();
    all.get(i)?.json(base)
}

impl Info {
    pub(crate) fn json(&self, base: &str) -> Option<String> {
        let mut v: Value = serde_json::from_str(base).ok()?;
        v["name"] = Value::String(self.name.clone());
        for (m, tokens) in ["light", "dark"].into_iter().zip(self.tokens()) {
            let map = v[m].as_object_mut()?;
            map.extend(tokens.iter().map(|(k, val)| (k.clone(), val.clone())));
        }
        Some(v.to_string())
    }
}

/// The id [`contrast`] registers.
pub const CONTRAST: &str = "rune-system-contrast";

/// A theme painted only in the system contrast theme's colours (Windows' Contrast themes, CSS
/// `forced-colors: active`), registered or brought up to date for `c`; its id. Every colour
/// OpenCode's theme names becomes one of them, by what it paints: grounds, layers and their
/// washes the window colour; text, icons, borders and code the text colour (borders solid);
/// accents, selections and state colours the highlight; what sits on an accent the highlight's
/// text. The same colours set on a theme again register nothing.
pub fn contrast(c: &rune::SystemContrast) -> &'static str {
    static LAST: std::sync::Mutex<Option<rune::SystemContrast>> = std::sync::Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if last.as_ref() == Some(c) {
        return CONTRAST;
    }
    // OpenCode's theme as it paints: `theme.json` with Desktop's own tokens over it (its code
    // colours among them).
    let src = json(0, crate::palette::THEME).unwrap_or_else(|| crate::palette::THEME.to_owned());
    let base: Value = serde_json::from_str(&src).unwrap_or_default();
    let mut names: Vec<String> = ["light", "dark", "palette"]
        .iter()
        .filter_map(|k| base[*k].as_object())
        .flat_map(|m| m.keys().cloned())
        .collect();
    names.sort();
    names.dedup();
    let mode = |dark: bool| -> Map<String, Value> {
        let Ok(t) = rune::Theme::from_json(&src, dark) else {
            return Map::new();
        };
        let (bg, fg) = (
            t.try_color("background-bg-base").unwrap_or(c.window),
            t.try_color("text-text-base").unwrap_or(c.text),
        );
        names
            .iter()
            .filter_map(|n| {
                t.try_color(n)
                    .map(|col| (n.clone(), Value::String(forced(n, col, bg, fg, c).hex())))
            })
            .collect()
    };
    register(CONTRAST, "Contrast", [mode(false), mode(true)]);
    *last = Some(*c);
    CONTRAST
}

/// What `col`, token `name` of OpenCode's theme over ground `bg` with text `fg`, paints as in
/// contrast theme `c`.
fn forced(
    name: &str,
    col: rune::Color,
    bg: rune::Color,
    fg: rune::Color,
    c: &rune::SystemContrast,
) -> rune::Color {
    let ink = name.starts_with("text")
        || name.starts_with("icon")
        || name.starts_with("syntax")
        || name.starts_with("border");
    let (r, g, b) = (
        rune::color::srgb_encode(col.r),
        rune::color::srgb_encode(col.g),
        rune::color::srgb_encode(col.b),
    );
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let saturated = max > 0.2 && (max - min) / max > 0.35;
    let pick = match () {
        _ if ink && name.contains("contrast") => c.highlight_text,
        _ if ink => c.text,
        _ if name.contains("contrast") || saturated => c.highlight,
        _ if (col.luminance() - bg.luminance()).abs()
            <= (col.luminance() - fg.luminance()).abs() =>
        {
            c.window
        }
        _ => c.text,
    };
    // Borders draw solid: a contrast theme's lines are what separates its surfaces.
    rune::Color {
        a: if name.starts_with("border") {
            1.0
        } else {
            col.a
        },
        ..pick
    }
}

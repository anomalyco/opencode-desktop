//! Settings › Shortcuts' changes over the table (Desktop's `settings.v3.keybinds`): a command id
//! and the keybind it takes instead ("mod+shift+l", "none" for unbound). [`apply`] installs them
//! for every key handler: the table's commands through [`get`] (so `Bind::keybind`, its labels
//! and its matching follow), the session view's through its own override, and the palette's
//! commands that have no row here through [`command`].

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, RwLock};

use rune::Keybind;

use super::{Bind, all};

/// The table's overrides, by `Bind`.
static TABLE: RwLock<Vec<Option<&'static Keybind>>> = RwLock::new(Vec::new());
/// Commands without a row in the table, by id.
static COMMANDS: RwLock<Vec<(String, &'static Keybind)>> = RwLock::new(Vec::new());
/// Each keybind spec parsed once and kept for the app's life, as the table's are.
static PARSED: Mutex<Option<HashMap<String, &'static Keybind>>> = Mutex::new(None);
/// Optional host callback invoked after [`apply`] installs new overrides (e.g. menu bar refresh).
static ON_CHANGED: RwLock<Option<fn()>> = RwLock::new(None);

fn parse(spec: &str) -> Option<&'static Keybind> {
    let mut cache = PARSED.lock().unwrap_or_else(|e| e.into_inner());
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some(k) = cache.get(spec) {
        return Some(k);
    }
    let k: &'static Keybind = Box::leak(Box::new(spec.parse::<Keybind>().ok()?));
    cache.insert(spec.to_owned(), k);
    Some(k)
}

/// The shared view shortcut a table row stands for, if any.
fn view_bind(b: Bind) -> Option<super::base::Bind> {
    use super::base::Bind as V;
    Some(match b {
        Bind::OpenProject => V::OpenProject,
        Bind::ZoomIn => V::ZoomIn,
        Bind::ZoomOut => V::ZoomOut,
        Bind::ZoomReset => V::ZoomReset,
        Bind::Model => V::Model,
        Bind::Agent => V::Agent,
        Bind::AgentBack => V::AgentBack,
        Bind::Variant => V::Variant,
        Bind::Attach => V::Attach,
        Bind::SubmitAnswer => V::QuestionAdvance,
        Bind::QuestionBack => V::QuestionBack,
        Bind::Stop => V::Stop,
        _ => return None,
    })
}

/// Registers a host hook invoked after [`apply`] updates overrides (for example, to refresh a
/// native menu bar's key equivalents). Replaces any previously registered hook.
pub fn set_on_changed(hook: fn()) {
    *ON_CHANGED.write().unwrap_or_else(|e| e.into_inner()) = Some(hook);
}

/// Clears any hook registered with [`set_on_changed`].
pub fn clear_on_changed() {
    *ON_CHANGED.write().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Installs `map` (command id to keybind) over the defaults and notifies any registered
/// [`set_on_changed`] host hook.
pub fn apply(map: &BTreeMap<String, String>) {
    let rows = all();
    let table: Vec<Option<&'static Keybind>> = rows.iter().map(|b| map.get(b.id).and_then(|spec| parse(spec))).collect();
    for b in rows {
        if let Some(v) = view_bind(b.bind) {
            super::base::set_override(v, table[b.bind as usize]);
        }
    }
    let commands = map.iter().filter(|(id, _)| !rows.iter().any(|b| b.id == id.as_str())).filter_map(|(id, spec)| Some((id.clone(), parse(spec)?))).collect();
    *TABLE.write().unwrap_or_else(|e| e.into_inner()) = table;
    *COMMANDS.write().unwrap_or_else(|e| e.into_inner()) = commands;
    let hook = *ON_CHANGED.read().unwrap_or_else(|e| e.into_inner());
    if let Some(on_changed) = hook {
        on_changed();
    }
}

/// The keybind Settings gave table row `b`, if it changed it.
pub fn get(b: Bind) -> Option<&'static Keybind> {
    TABLE.read().unwrap_or_else(|e| e.into_inner()).get(b as usize).copied().flatten()
}

/// The keybind Settings gave command `id`, which has no row in the table.
pub fn command(id: &str) -> Option<&'static Keybind> {
    COMMANDS.read().unwrap_or_else(|e| e.into_inner()).iter().find(|(c, _)| c == id).map(|(_, k)| *k)
}

/// Every command Settings bound that has no row in the table, with its keybind.
pub fn commands() -> Vec<(String, &'static Keybind)> {
    COMMANDS.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A spec as Settings saves it: `chord`'s canonical form ("mod+shift+l").
pub fn spec(chord: rune::Chord) -> String {
    chord.to_string()
}

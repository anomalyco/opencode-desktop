//! A popover's keys: the highlight that follows a query, arrow-key stepping, and what each key
//! asks for.

use rune::{Key, KeyEvent};

/// The lit row of a list under a search field, as Desktop's typeahead keeps it
/// (`ui/src/forms/typeahead/controller.ts:14-22`, `ui/src/hooks/list-machine.ts:20-21`): until
/// a row is chosen the first row is lit; a chosen row holds while it's listed, and a new query
/// starts from the first row again (`search-list.tsx:261-265`), whatever was chosen before.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Highlight {
    #[default]
    First,
    Chosen { key: String, by: Lit },
}

/// What chose the lit row (`ListHighlight.mode`): the pointer's row never scrolls the list
/// (`listScrollTarget`), and late results leave the list where it is for it (`search-list.tsx:189-207`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lit {
    Keys,
    Pointer,
}

pub enum HighlightEvent<'a> {
    /// The field's text changed (`onInput`: `choose(undefined)`).
    QueryChanged,
    /// The arrows, a section jump or the host chose `key` (`setActive`).
    Keys(String),
    /// The pointer moved on `key` (`onMouseMove` with movement).
    Pointer(String),
    /// The list now holds `keys`: a chosen row that left it lets the first row light
    /// (`controller.ts:17-20`).
    Rows(&'a [&'a str]),
    /// The list opened, or the host starts it over.
    Reset,
}

impl Highlight {
    pub fn step(&mut self, event: HighlightEvent<'_>) {
        *self = match (std::mem::take(self), event) {
            (_, HighlightEvent::QueryChanged | HighlightEvent::Reset) => Highlight::First,
            (_, HighlightEvent::Keys(key)) => Highlight::Chosen { key, by: Lit::Keys },
            (_, HighlightEvent::Pointer(key)) => Highlight::Chosen { key, by: Lit::Pointer },
            (Highlight::Chosen { key, by }, HighlightEvent::Rows(keys)) if keys.contains(&key.as_str()) => Highlight::Chosen { key, by },
            (_, HighlightEvent::Rows(_)) => Highlight::First,
        };
    }

    /// The lit row among `keys`: the chosen one while it's listed, else the first.
    pub fn resolve<'a>(&self, keys: impl Iterator<Item = &'a str> + Clone) -> Option<String> {
        let chosen = match self {
            Highlight::Chosen { key, .. } => keys.clone().find(|k| k == key),
            Highlight::First => None,
        };
        chosen.or_else(|| keys.clone().next()).map(str::to_owned)
    }

    /// Whether the pointer chose `key`.
    pub fn pointed(&self, key: &str) -> bool {
        matches!(self, Highlight::Chosen { key: k, by: Lit::Pointer } if k == key)
    }

    /// The keys chose `key`, as [`HighlightEvent::Keys`].
    pub fn set(&mut self, key: String) {
        self.step(HighlightEvent::Keys(key));
    }

    /// Back to the first row, as [`HighlightEvent::Reset`].
    pub fn clear(&mut self) {
        self.step(HighlightEvent::Reset);
    }
}

/// Up, Down, and on macOS Ctrl-N and Ctrl-P, as a step of ±1. `ctrl_alt` lets Ctrl-N/P through with Alt held.
pub fn nav_step(ev: &KeyEvent, ctrl_alt: bool) -> Option<i64> {
    use crate::keymap::Bind;
    let hit = |b: Bind, alt: Bind| b.matches(ev) || (ctrl_alt && alt.matches(ev));
    let plain = !ev.mods.alt && !ev.mods.cmd && !ev.mods.shift;
    match &ev.key {
        Key::Down if plain => Some(1),
        Key::Up if plain => Some(-1),
        _ if hit(Bind::PopoverNext, Bind::PopoverNextAlt) => Some(1),
        _ if hit(Bind::PopoverPrev, Bind::PopoverPrevAlt) => Some(-1),
        _ => None,
    }
}

/// The index `step` from `at` (the first when none), wrapping around `len` items.
pub fn cycle(len: usize, at: Option<usize>, step: i64) -> usize {
    (at.unwrap_or(0) as i64 + step).rem_euclid(len as i64) as usize
}

/// What a key asks an open popover to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Step(i64),
    Choose,
    Close,
    /// Tab: leave the popover for the next control.
    Leave,
}

/// The popover's keys, the same in every menu: ↑ ↓ (and Ctrl-N/P) move, Return picks, Escape
/// closes, Tab leaves.
pub fn nav(ev: &KeyEvent) -> Option<Nav> {
    if let Some(step) = nav_step(ev, false) {
        return Some(Nav::Step(step));
    }
    match ev.key {
        Key::Enter => Some(Nav::Choose),
        Key::Escape => Some(Nav::Close),
        Key::Tab => Some(Nav::Leave),
        _ => None,
    }
}

//! A popover's keys: the highlight that follows a query, arrow-key stepping, and what each key
//! asks for.

use rune::{Key, KeyEvent};

/// The highlighted row and the query it was picked under: it holds only while the query is
/// unchanged and the row is still listed, else the first row is highlighted.
#[derive(Default)]
pub struct Highlight(Option<(String, String)>);

impl Highlight {
    pub fn resolve<'a>(&self, query: &str, keys: impl Iterator<Item = &'a str> + Clone) -> Option<String> {
        let picked = self.0.as_ref().filter(|(_, q)| q == query).map(|(k, _)| k.as_str());
        picked.filter(|k| keys.clone().any(|x| x == *k)).map(str::to_owned).or_else(|| keys.clone().next().map(str::to_owned))
    }

    pub fn set(&mut self, key: String, query: String) {
        self.0 = Some((key, query));
    }

    pub fn clear(&mut self) {
        self.0 = None;
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

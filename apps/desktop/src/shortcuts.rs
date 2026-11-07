//! OpenCode Desktop's window-wide keys, as pure functions: which keymap command is which action
//! (`packages/app` `tab-strip.tsx`, `use-session-commands.tsx`, the View menu) and how tabs and
//! zoom step.

use rune::{KeyEvent, Tx};

use crate::keymap::Bind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    ZoomIn,
    ZoomOut,
    ZoomReset,
    /// Previous or next tab, wrapping.
    Tab(i32),
    /// Previous or next tab with an unread mark, wrapping.
    UnreadTab(i32),
    /// ⌘1 to ⌘9 pick the tab, or inbox row, with that number.
    TabAt(usize),
    NewSession,
    OpenProject,
    SelectProject,
    CloseTab,
    ReopenTab,
    Settings,
    AutoAccept,
}

/// The action `ev` is, by the keymap's chords (US punctuation; see `action_tx`).
pub fn action(ev: &KeyEvent) -> Option<Action> {
    resolve(|b| b.matches(ev))
}

/// `action`, following the keyboard layout as a key handler should.
pub fn action_tx(ev: &KeyEvent, tx: &Tx) -> Option<Action> {
    resolve(|b| b.matches_tx(ev, tx))
}

fn resolve(hit: impl Fn(Bind) -> bool) -> Option<Action> {
    const TABLE: [(Bind, Action); 25] = [
        (Bind::SessionPrev, Action::Tab(-1)),
        (Bind::SessionNext, Action::Tab(1)),
        (Bind::UnseenPrev, Action::UnreadTab(-1)),
        (Bind::UnseenNext, Action::UnreadTab(1)),
        (Bind::TabPrev, Action::Tab(-1)),
        (Bind::TabNext, Action::Tab(1)),
        (Bind::ZoomIn, Action::ZoomIn),
        (Bind::ZoomOut, Action::ZoomOut),
        (Bind::ZoomReset, Action::ZoomReset),
        (Bind::AutoAccept, Action::AutoAccept),
        (Bind::Settings, Action::Settings),
        (Bind::TabReopen, Action::ReopenTab),
        (Bind::TabNew, Action::NewSession),
        (Bind::OpenProject, Action::OpenProject),
        (Bind::SelectProject, Action::SelectProject),
        (Bind::TabClose, Action::CloseTab),
        (Bind::Tab1, Action::TabAt(0)),
        (Bind::Tab2, Action::TabAt(1)),
        (Bind::Tab3, Action::TabAt(2)),
        (Bind::Tab4, Action::TabAt(3)),
        (Bind::Tab5, Action::TabAt(4)),
        (Bind::Tab6, Action::TabAt(5)),
        (Bind::Tab7, Action::TabAt(6)),
        (Bind::Tab8, Action::TabAt(7)),
        (Bind::Tab9, Action::TabAt(8)),
    ];
    TABLE.iter().find(|(b, _)| hit(*b)).map(|(_, a)| *a)
}

/// The nearest other tab in `offset`'s direction, wrapping, that `include` accepts
/// (`adjacentTabKey`). Without a current tab, the first (forward) or last (backward) one.
pub fn adjacent(order: &[&str], current: Option<&str>, offset: i32, include: impl Fn(&str) -> bool) -> Option<String> {
    let n = order.len() as i32;
    let Some(index) = current.and_then(|c| order.iter().position(|k| *k == c)) else {
        let mut all: Vec<&str> = order.to_vec();
        if offset < 0 {
            all.reverse();
        }
        return all.into_iter().find(|k| include(k)).map(str::to_owned);
    };
    (1..n).map(|step| order[(index as i32 + offset * step).rem_euclid(n) as usize]).find(|k| include(k)).map(str::to_owned)
}

pub const ZOOM_MIN: f32 = 0.5;
pub const ZOOM_MAX: f32 = 3.0;

/// Desktop's zoom levels (`desktopZoomLevels`, Chromium's from 50% to 200%): the zoom keys and
/// Appearance › Zoom each pick one.
pub const ZOOM_LEVELS: [f32; 11] = [0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// The level after `zoom` in `dir`'s direction (`nextDesktopZoomLevel`), from wherever a pinch
/// left it; `dir` 0 resets.
pub fn step_zoom(zoom: f32, dir: i32) -> f32 {
    if dir == 0 {
        return 1.0;
    }
    if dir > 0 {
        return ZOOM_LEVELS.into_iter().find(|l| *l > zoom + 0.01).unwrap_or(ZOOM_LEVELS[ZOOM_LEVELS.len() - 1]);
    }
    ZOOM_LEVELS.into_iter().rev().find(|l| *l < zoom - 0.01).unwrap_or(ZOOM_LEVELS[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Key, Mods};

    fn ev(key: Key, f: impl FnOnce(&mut Mods)) -> KeyEvent {
        let mut mods = Mods::default();
        f(&mut mods);
        KeyEvent { key, mods }
    }

    #[test]
    fn maps_desktop_defaults() {
        let alt = |m: &mut Mods| m.alt = true;
        assert_eq!(action(&ev(Key::Up, alt)), Some(Action::Tab(-1)));
        assert_eq!(action(&ev(Key::Down, alt)), Some(Action::Tab(1)));
        assert_eq!(action(&ev(Key::Down, |m| (m.alt, m.shift) = (true, true))), Some(Action::UnreadTab(1)));
        assert_eq!(action(&ev(Key::Up, |_| {})), None);
        assert_eq!(action(&ev(Key::Left, alt)), None);
        assert_eq!(action(&ev(Key::Right, |m| (m.cmd, m.alt) = (true, true))), Some(Action::Tab(1)));
        assert_eq!(action(&ev(Key::Tab, |m| m.ctrl = true)), Some(Action::Tab(1)));
        assert_eq!(action(&ev(Key::Tab, |m| (m.ctrl, m.shift) = (true, true))), Some(Action::Tab(-1)));
        assert_eq!(action(&ev(Key::Tab, |_| {})), None);
        let cmd = |m: &mut Mods| m.cmd = true;
        assert_eq!(action(&ev(Key::Command('='), cmd)), Some(Action::ZoomIn));
        assert_eq!(action(&ev(Key::Command('+'), |m| (m.cmd, m.shift) = (true, true))), Some(Action::ZoomIn));
        assert_eq!(action(&ev(Key::Command('-'), cmd)), Some(Action::ZoomOut));
        assert_eq!(action(&ev(Key::Command('0'), cmd)), Some(Action::ZoomReset));
        assert_eq!(action(&ev(Key::Command('t'), cmd)), Some(Action::NewSession));
        assert_eq!(action(&ev(Key::Command('t'), |m| (m.cmd, m.shift) = (true, true))), Some(Action::ReopenTab));
        assert_eq!(action(&ev(Key::Command('o'), cmd)), Some(Action::OpenProject));
        assert_eq!(action(&ev(Key::Command('o'), |m| (m.cmd, m.shift) = (true, true))), Some(Action::SelectProject));
        assert_eq!(action(&ev(Key::Command('w'), cmd)), Some(Action::CloseTab));
        assert_eq!(action(&ev(Key::Command(','), cmd)), Some(Action::Settings));
        assert_eq!(action(&ev(Key::Command('3'), cmd)), Some(Action::TabAt(2)));
        assert_eq!(action(&ev(Key::Command('a'), cmd)), None);
    }

    #[test]
    fn adjacent_wraps_and_filters() {
        let order = ["a", "b", "c"];
        assert_eq!(adjacent(&order, Some("c"), 1, |_| true).as_deref(), Some("a"));
        assert_eq!(adjacent(&order, Some("a"), -1, |_| true).as_deref(), Some("c"));
        assert_eq!(adjacent(&order, Some("a"), 1, |k| k == "c").as_deref(), Some("c"));
        assert_eq!(adjacent(&order, Some("a"), 1, |k| k == "a"), None);
        assert_eq!(adjacent(&order, None, 1, |_| true).as_deref(), Some("a"));
        assert_eq!(adjacent(&order, None, -1, |_| true).as_deref(), Some("c"));
        assert_eq!(adjacent(&[], Some("a"), 1, |_| true), None);
    }

    #[test]
    fn zoom_steps_and_clamps() {
        assert_eq!(step_zoom(1.0, 1), 1.1);
        assert_eq!(step_zoom(1.1, -1), 1.0);
        assert_eq!(step_zoom(0.5, -1), 0.5);
        assert_eq!(step_zoom(2.0, 1), 2.0);
        assert_eq!(step_zoom(2.3, 0), 1.0);
        let mut z = 1.0;
        for _ in 0..30 {
            z = step_zoom(z, 1);
        }
        assert_eq!(z, 2.0);
    }
}

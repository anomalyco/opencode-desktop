//! OpenCode Desktop's window-wide keys: one declarative dispatch table (`packages/app`
//! `tab-strip.tsx`, `use-session-commands.tsx`, the View menu), tab stepping, and canonical
//! zoom helpers re-exported from `opencode-runtime`.

#[allow(unused_imports)]
pub use opencode_runtime::ZOOM_LEVELS;
pub use opencode_runtime::{ZOOM_MAX, ZOOM_MIN, step_zoom};

use rune::{KeyEvent, Tx};

use crate::App;
use crate::keymap::Bind;

#[derive(Clone, Copy)]
enum Target {
    /// Dispatches through the registered kernel command carrying the same `Bind`.
    Command,
    /// Key-only shortcut (or one whose global shortcut semantics differ from palette visibility,
    /// such as `AutoAccept` working outside a session and `TabClose` closing an empty window).
    Direct(fn(&mut App, &mut Tx)),
}

/// Window-wide shortcuts in match-priority order.
const TABLE: [(Bind, Target); 26] = [
    (Bind::SessionPrev, Target::Command),
    (Bind::SessionNext, Target::Command),
    (Bind::UnseenPrev, Target::Command),
    (Bind::UnseenNext, Target::Command),
    (Bind::TabPrev, Target::Direct(|s, tx| s.select_adjacent(-1, tx))),
    (Bind::TabNext, Target::Direct(|s, tx| s.select_adjacent(1, tx))),
    // Zoom lands at once; the scale and every re-laid-out row would otherwise spring.
    (Bind::ZoomIn, Target::Command),
    (Bind::ZoomOut, Target::Command),
    (Bind::ZoomReset, Target::Command),
    (Bind::AutoAccept, Target::Direct(|s, tx| s.toggle_auto_accept(tx))),
    (Bind::Settings, Target::Command),
    (Bind::TabReopen, Target::Command),
    (Bind::TabNew, Target::Direct(|s, tx| s.new_session(tx))),
    (Bind::OpenProject, Target::Command),
    (Bind::SelectProject, Target::Command),
    (Bind::TabClose, Target::Direct(|s, tx| s.close_tab_key(tx))),
    (Bind::TabCloseSession, Target::Direct(|s, tx| s.close_tab_key(tx))),
    // Desktop numbers the first nine it shows (`titlebar-tab-numbers`).
    (Bind::Tab1, Target::Direct(|s, tx| s.select_nth(0, tx))),
    (Bind::Tab2, Target::Direct(|s, tx| s.select_nth(1, tx))),
    (Bind::Tab3, Target::Direct(|s, tx| s.select_nth(2, tx))),
    (Bind::Tab4, Target::Direct(|s, tx| s.select_nth(3, tx))),
    (Bind::Tab5, Target::Direct(|s, tx| s.select_nth(4, tx))),
    (Bind::Tab6, Target::Direct(|s, tx| s.select_nth(5, tx))),
    (Bind::Tab7, Target::Direct(|s, tx| s.select_nth(6, tx))),
    (Bind::Tab8, Target::Direct(|s, tx| s.select_nth(7, tx))),
    (Bind::Tab9, Target::Direct(|s, tx| s.select_nth(8, tx))),
];

/// Runs the first matching window-wide shortcut, following the keyboard layout.
pub fn run(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(&(bind, target)) = TABLE.iter().find(|(b, _)| b.matches_tx(ev, tx)) else {
        return false;
    };
    match target {
        Target::Command => crate::commands::run_bind(s, bind, tx),
        Target::Direct(handler) => {
            handler(s, tx);
            true
        }
    }
}

/// Whether `ev` is a window-wide shortcut that a focused browser page yields to the shell
/// (every entry in [`TABLE`] except in-page zoom).
pub fn claims_from_browser(ev: &KeyEvent) -> bool {
    TABLE
        .iter()
        .find(|(b, _)| b.matches(ev))
        .is_some_and(|(b, _)| !matches!(b, Bind::ZoomIn | Bind::ZoomOut | Bind::ZoomReset))
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

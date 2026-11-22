//! OpenCode Desktop's window-wide shortcuts: one declarative dispatch table (`packages/app`
//! `tab-strip.tsx`, `use-session-commands.tsx`, `titlebar.tsx`, the View menu), tab stepping, and
//! canonical zoom helpers re-exported from `opencode-runtime`.
//!
//! Every static built-in shortcut lives once in [`TABLE`] tagged with its [`Phase`], browser-claim
//! policy, and dispatch [`Target`]. Duplicate `Bind` entries fail to compile; `Target::Command`
//! entries whose `Bind::id()` has no registered kernel command fail loudly at start-up in
//! [`validate`].

#[allow(unused_imports)]
pub use opencode_runtime::ZOOM_LEVELS;
pub use opencode_runtime::{ZOOM_MAX, ZOOM_MIN, step_zoom};

use rune::{KeyEvent, Tx};

use crate::App;
use crate::keymap::Bind;

/// Dispatch phase determining where in the key chain a shortcut entry is checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Native menu accelerators (`Ctrl+Shift+N` / `Ctrl+Shift+W` off macOS): dispatched in
    /// `keys::on_key` right before `App::shortcut`.
    WindowAccelerator,
    /// Built-in commands dispatched inside `App::shortcut` ahead of plugin shortcuts and default
    /// `.keybind(...)` commands (formerly `commands::KEY_BINDS`). First matching bind decides
    /// whether this phase consumes the key.
    BeforePlugins,
    /// Built-in window-wide shortcuts dispatched inside `App::shortcut` after plugin shortcuts,
    /// plain-text paste, and bracket history navigation (formerly `shortcuts::TABLE`). First
    /// matching bind decides whether this phase consumes the key.
    AfterPlugins,
    /// Window commands that focused inputs (`composer`, Home `search-input`) check ahead of field
    /// editing, and that `keys::on_key` checks after `picker_key` (formerly `keys::global`).
    Global,
}

#[derive(Clone, Copy)]
enum Target {
    /// Dispatches through the registered kernel command carrying the same `Bind`.
    Command,
    /// Direct handler returning `true` when it handled the key (used for key-only shortcuts or
    /// shortcuts whose global behaviour differs from palette command visibility/semantics).
    Direct(fn(&mut App, &mut Tx) -> bool),
}

#[derive(Clone, Copy)]
struct Entry {
    bind: Bind,
    phase: Phase,
    /// Whether a focused embedded browser page yields this shortcut to the app shell.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    claims_from_browser: bool,
    target: Target,
}

const fn cmd(bind: Bind, phase: Phase, claims_from_browser: bool) -> Entry {
    Entry { bind, phase, claims_from_browser, target: Target::Command }
}

const fn direct(
    bind: Bind,
    phase: Phase,
    claims_from_browser: bool,
    f: fn(&mut App, &mut Tx) -> bool,
) -> Entry {
    Entry { bind, phase, claims_from_browser, target: Target::Direct(f) }
}

/// Every static built-in shortcut in one table, ordered by match priority within each [`Phase`].
const TABLE: &[Entry] = &[
    // ── Window accelerators (non-macOS menu accelerators without a palette command) ──────────────
    direct(Bind::WindowNew, Phase::WindowAccelerator, false, |s, _| {
        crate::windows::new_window(s);
        true
    }),
    direct(Bind::WindowClose, Phase::WindowAccelerator, false, |s, tx| {
        s.close_window(tx);
        true
    }),
    // ── BeforePlugins (dispatched inside `App::shortcut` before plugin shortcuts) ────────────────
    cmd(Bind::FocusInput, Phase::BeforePlugins, false),
    cmd(Bind::AddSelection, Phase::BeforePlugins, false),
    cmd(Bind::PromptModeToggle, Phase::BeforePlugins, false),
    cmd(Bind::JumpToMessage, Phase::BeforePlugins, false),
    cmd(Bind::MessagePrev, Phase::BeforePlugins, false),
    cmd(Bind::MessageNext, Phase::BeforePlugins, false),
    // ── AfterPlugins (window-wide shortcuts at the tail of `App::shortcut`) ──────────────────────
    cmd(Bind::SessionPrev, Phase::AfterPlugins, true),
    cmd(Bind::SessionNext, Phase::AfterPlugins, true),
    cmd(Bind::UnseenPrev, Phase::AfterPlugins, true),
    cmd(Bind::UnseenNext, Phase::AfterPlugins, true),
    direct(Bind::TabPrev, Phase::AfterPlugins, true, |s, tx| {
        s.select_adjacent(-1, tx);
        true
    }),
    direct(Bind::TabNext, Phase::AfterPlugins, true, |s, tx| {
        s.select_adjacent(1, tx);
        true
    }),
    // Zoom stays in-page inside a focused browser pane (`claims_from_browser: false`) and lands at
    // once; the scale and every re-laid-out row would otherwise spring.
    cmd(Bind::ZoomIn, Phase::AfterPlugins, false),
    cmd(Bind::ZoomOut, Phase::AfterPlugins, false),
    cmd(Bind::ZoomReset, Phase::AfterPlugins, false),
    direct(Bind::AutoAccept, Phase::AfterPlugins, true, |s, tx| {
        s.toggle_auto_accept(tx);
        true
    }),
    cmd(Bind::Settings, Phase::AfterPlugins, true),
    cmd(Bind::TabReopen, Phase::AfterPlugins, true),
    direct(Bind::TabNew, Phase::AfterPlugins, true, |s, tx| {
        s.new_session(tx);
        true
    }),
    cmd(Bind::QuickOpen, Phase::AfterPlugins, true),
    cmd(Bind::SelectProject, Phase::AfterPlugins, true),
    direct(Bind::TabClose, Phase::AfterPlugins, true, |s, tx| {
        s.close_tab_key(tx);
        true
    }),
    direct(Bind::TabCloseSession, Phase::AfterPlugins, true, |s, tx| {
        s.close_tab_key(tx);
        true
    }),
    // Desktop numbers the first nine it shows (`titlebar-tab-numbers`).
    direct(Bind::Tab1, Phase::AfterPlugins, true, |s, tx| {
        s.select_nth(0, tx);
        true
    }),
    direct(Bind::Tab2, Phase::AfterPlugins, true, |s, tx| {
        s.select_nth(1, tx);
        true
    }),
    direct(Bind::Tab3, Phase::AfterPlugins, true, |s, tx| {
        s.select_nth(2, tx);
        true
    }),
    direct(Bind::Tab4, Phase::AfterPlugins, true, |s, tx| {
        s.select_nth(3, tx);
        true
    }),
    direct(Bind::Tab5, Phase::AfterPlugins, true, |s, tx| {
        s.select_nth(4, tx);
        true
    }),
    direct(Bind::Tab6, Phase::AfterPlugins, true, |s, tx| {
        s.select_nth(5, tx);
        true
    }),
    direct(Bind::Tab7, Phase::AfterPlugins, true, |s, tx| {
        s.select_nth(6, tx);
        true
    }),
    direct(Bind::Tab8, Phase::AfterPlugins, true, |s, tx| {
        s.select_nth(7, tx);
        true
    }),
    direct(Bind::Tab9, Phase::AfterPlugins, true, |s, tx| {
        s.select_nth(8, tx);
        true
    }),
    // ── Global (checked ahead of field editing in composer/Home search, and after picker_key) ───
    // Top tabs have no sidebar: the keys pass through, as Desktop registers no command there.
    direct(Bind::Sidebar, Phase::Global, false, |s, tx| s.shows_sidebar() && s.toggle_sidebar(tx)),
    // Matches Desktop's `home.toggle` (`packages/app/src/shell/titlebar/titlebar.tsx`): opens the
    // project browser when project navigation is active, toggles back to the previous tab when
    // already on Home, and focuses Home search when entering Home. Kept direct rather than routed
    // through the hidden `"home"` command (which currently calls `go_home`) to avoid changing behaviour.
    direct(Bind::Home, Phase::Global, false, |s, tx| {
        if s.toggle_home(tx) {
            s.focus_home_search(tx);
        }
        true
    }),
];

// Compile-time check: a `Bind` can appear at most once in `TABLE`.
const _: () = {
    let mut i = 0;
    while i < TABLE.len() {
        let mut j = i + 1;
        while j < TABLE.len() {
            assert!(TABLE[i].bind as usize != TABLE[j].bind as usize, "shortcuts::TABLE contains a duplicate Bind");
            j += 1;
        }
        i += 1;
    }
};

fn dispatch(s: &mut App, entry: Entry, tx: &mut Tx) -> bool {
    match entry.target {
        Target::Command => crate::commands::run_bind(s, entry.bind, tx),
        Target::Direct(handler) => handler(s, tx),
    }
}

/// Validates at start-up that every `Target::Command` row in [`TABLE`] matches a registered kernel
/// command carrying the same `Bind::id()`.
pub(crate) fn validate(commands: &opencode_kernel::CommandRegistry<App>) {
    for &entry in TABLE {
        if matches!(entry.target, Target::Command) {
            let bind_id = entry.bind.id();
            assert!(
                commands.iter().any(|c| c.bind_id == Some(bind_id)),
                "shortcuts::TABLE: Target::Command for {:?} ('{bind_id}') has no registered command with matching bind_id",
                entry.bind
            );
        }
    }
}

/// Dispatches non-macOS window menu accelerators (`Phase::WindowAccelerator`).
pub(crate) fn run_window_accelerators(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(&entry) = TABLE.iter().find(|e| e.phase == Phase::WindowAccelerator && e.bind.matches_tx(ev, tx)) else {
        return false;
    };
    dispatch(s, entry, tx)
}

/// Checks `Phase::BeforePlugins` entries. Returns `Some(handled)` when a `BeforePlugins` bind
/// matched (so `commands::key` skips fallback default `.keybind(...)` commands even if the matched
/// command is disabled), or `None` when no `BeforePlugins` bind matched.
pub(crate) fn match_before_plugins(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> Option<bool> {
    let &entry = TABLE.iter().find(|e| e.phase == Phase::BeforePlugins && e.bind.matches_tx(ev, tx))?;
    Some(dispatch(s, entry, tx))
}

/// Runs the first matching window-wide shortcut in `Phase::AfterPlugins`, following the keyboard layout.
pub fn run(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(&entry) = TABLE.iter().find(|e| e.phase == Phase::AfterPlugins && e.bind.matches_tx(ev, tx)) else {
        return false;
    };
    dispatch(s, entry, tx)
}

/// Runs `Phase::Global` window commands (`Bind::Sidebar`, `Bind::Home`), trying each matching entry
/// in order so a disabled `Bind::Sidebar` on top tabs still lets `Bind::Home` match if overridden.
pub(crate) fn run_global(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    for &entry in TABLE {
        if entry.phase == Phase::Global && entry.bind.matches_tx(ev, tx) && dispatch(s, entry, tx) {
            return true;
        }
    }
    false
}

/// Whether `ev` is a window-wide shortcut that a focused browser page yields to the shell
/// (every `Phase::AfterPlugins` entry in [`TABLE`] except in-page zoom).
#[cfg(not(target_arch = "wasm32"))]
pub fn claims_from_browser(ev: &KeyEvent) -> bool {
    TABLE
        .iter()
        .find(|e| e.phase == Phase::AfterPlugins && e.bind.matches(ev))
        .is_some_and(|e| e.claims_from_browser)
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

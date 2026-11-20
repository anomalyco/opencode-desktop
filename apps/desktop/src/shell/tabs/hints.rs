//! Tab shortcut numbers (`1`..`9`) shown on tab avatars while holding the platform Mod key
//! (`packages/app/src/shell/titlebar/tab-hints-machine.ts`, #63089).

use std::cell::Cell;
use std::time::Duration;

use opencode_kernel::Slot;
use rune::{Key, KeyEvent, Mods, PhysicalKeyCode, Tx};

use crate::App;

/// `TAB_HINTS_DELAY_MS` (`tab-hints-machine.ts:1`): hold Mod alone for 400 ms before showing.
pub const DELAY_SECS: f64 = 0.4;

/// `TabHintsState` (`tab-hints-machine.ts:4-7`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Idle { attempt: u64 },
    Pending { attempt: u64, due: f64 },
    Shown { attempt: u64 },
}

impl Default for Phase {
    fn default() -> Self {
        Self::Idle { attempt: 0 }
    }
}

impl Phase {
    pub fn is_shown(self) -> bool {
        matches!(self, Self::Shown { .. })
    }

    pub fn is_idle(self) -> bool {
        matches!(self, Self::Idle { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    Mod,
    Digit,
    Other,
}

/// `TabHintsEvent` (`tab-hints-machine.ts:13-23`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    KeyDown { key: KeyKind, mod_held: bool, extra: bool },
    Modifiers { mod_held: bool, extra: bool },
    Tick { now: f64 },
    WindowLeft,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    None,
    WakeAfter(Duration),
    Redraw,
}

/// Explicit state machine matching `transitionTabHints` (`tab-hints-machine.ts:40-65`).
pub fn step(phase: &mut Phase, event: Event, now: f64) -> Effect {
    match event {
        Event::Tick { now } => match *phase {
            Phase::Pending { attempt, due } if now >= due => {
                *phase = Phase::Shown { attempt };
                Effect::Redraw
            }
            Phase::Pending { due, .. } => Effect::WakeAfter(Duration::from_secs_f64((due - now).max(0.001))),
            Phase::Idle { .. } | Phase::Shown { .. } => Effect::None,
        },
        Event::WindowLeft => reset(phase),
        Event::Modifiers { mod_held, extra } => {
            if !mod_held || extra {
                return reset(phase);
            }
            Effect::None
        }
        Event::KeyDown { key, mod_held, extra } => {
            if !mod_held || extra {
                return reset(phase);
            }
            match key {
                KeyKind::Mod => match *phase {
                    Phase::Idle { attempt } => {
                        *phase = Phase::Pending { attempt: attempt + 1, due: now + DELAY_SECS };
                        Effect::WakeAfter(Duration::from_secs_f64(DELAY_SECS))
                    }
                    Phase::Pending { .. } | Phase::Shown { .. } => Effect::None,
                },
                // Once shown, the numbers stay while digits pick tabs; any other shortcut, or a
                // digit pressed before the delay elapses, cancels them.
                KeyKind::Digit if matches!(*phase, Phase::Shown { .. }) => Effect::None,
                KeyKind::Digit | KeyKind::Other => reset(phase),
            }
        }
    }
}

fn reset(phase: &mut Phase) -> Effect {
    match *phase {
        Phase::Idle { .. } => Effect::None,
        Phase::Pending { attempt, .. } => {
            *phase = Phase::Idle { attempt };
            Effect::None
        }
        Phase::Shown { attempt } => {
            *phase = Phase::Idle { attempt };
            Effect::Redraw
        }
    }
}

/// Per-window slot holding the tab-hints state machine and last Mod state.
///
/// Uses interior `Cell`s so non-visual transitions (`Idle <-> Pending`) never dirty tracked
/// view readers; `slots.touch(&TAB_HINTS)` fires only when `shown()` actually flips.
#[derive(Default)]
pub struct TabHints {
    phase: Cell<Phase>,
    last_mod: Cell<bool>,
}

pub(crate) static TAB_HINTS: Slot<TabHints> = Slot::new();

fn held(mods: Mods) -> (bool, bool) {
    let extra = if cfg!(target_os = "macos") {
        mods.shift || mods.alt || mods.ctrl
    } else if cfg!(target_arch = "wasm32") {
        mods.shift || mods.alt || (mods.ctrl && mods.logo)
    } else {
        mods.shift || mods.alt || mods.logo
    };
    (mods.cmd, extra)
}

fn classify_key(ev: &KeyEvent, tx: &Tx) -> KeyKind {
    if matches!(ev.key, Key::Modifier { .. }) {
        return KeyKind::Other;
    }
    let digit_phys = matches!(
        tx.physical_key(),
        Some(
            PhysicalKeyCode::Digit1
                | PhysicalKeyCode::Digit2
                | PhysicalKeyCode::Digit3
                | PhysicalKeyCode::Digit4
                | PhysicalKeyCode::Digit5
                | PhysicalKeyCode::Digit6
                | PhysicalKeyCode::Digit7
                | PhysicalKeyCode::Digit8
                | PhysicalKeyCode::Digit9
        )
    );
    let digit_key = matches!(&ev.key, Key::Command('1'..='9'))
        || matches!(&ev.key, Key::Text(t) if t.len() == 1 && matches!(t.as_bytes()[0], b'1'..=b'9'));
    if digit_phys || digit_key { KeyKind::Digit } else { KeyKind::Other }
}

fn apply_effect(effect: Effect, tx: &mut Tx) -> bool {
    match effect {
        Effect::None => false,
        Effect::WakeAfter(after) => {
            tx.wake_after(after);
            false
        }
        Effect::Redraw => {
            tx.instant();
            true
        }
    }
}

/// Whether the tab strip should currently display `1`..`9` shortcut overlays (`tab-strip.tsx:697`).
/// Suppressed when focus is inside the side panel because `Mod+1..9` picks side tabs there.
pub(crate) fn shown(s: &App) -> bool {
    s.kernel.slots.get(&TAB_HINTS).phase.get().is_shown() && !s.kernel.slots.get(&crate::panels::SIDE).focus.is_focused()
}

/// Called from `App::on_modifiers`. Returns `true` only when visual hint visibility changed,
/// preserving `tx.live_only()` on quick Mod taps and unrelated modifier changes.
pub(crate) fn modifiers(s: &mut App, mods: Mods, tx: &mut Tx) -> bool {
    let (mod_held, extra) = held(mods);
    let slot = s.kernel.slots.untracked(&TAB_HINTS);
    let was_mod = slot.last_mod.replace(mod_held);
    let event = if !was_mod && mod_held && !extra {
        Event::KeyDown { key: KeyKind::Mod, mod_held, extra }
    } else {
        Event::Modifiers { mod_held, extra }
    };
    let mut phase = slot.phase.get();
    let was_shown = phase.is_shown();
    let effect = step(&mut phase, event, *s.clock);
    slot.phase.set(phase);
    if was_shown != phase.is_shown() {
        s.kernel.slots.touch(&TAB_HINTS);
    }
    apply_effect(effect, tx)
}

/// Called on key-down in shell, composer, and Home search. Zero cost while `Idle`.
pub(crate) fn key_down(s: &mut App, ev: &KeyEvent, tx: &mut Tx) {
    let slot = s.kernel.slots.untracked(&TAB_HINTS);
    let mut phase = slot.phase.get();
    if phase.is_idle() {
        return;
    }
    let (mod_held, extra) = held(ev.mods);
    let event = Event::KeyDown { key: classify_key(ev, tx), mod_held, extra };
    let was_shown = phase.is_shown();
    let effect = step(&mut phase, event, *s.clock);
    slot.phase.set(phase);
    if was_shown != phase.is_shown() {
        s.kernel.slots.touch(&TAB_HINTS);
    }
    let _ = apply_effect(effect, tx);
}

/// Cancels pending or shown hints (e.g. on paste or non-key shortcut activity). Zero cost while `Idle`.
pub(crate) fn cancel(s: &mut App, tx: &mut Tx) {
    let slot = s.kernel.slots.untracked(&TAB_HINTS);
    let mut phase = slot.phase.get();
    if phase.is_idle() {
        return;
    }
    let was_shown = phase.is_shown();
    let effect = step(&mut phase, Event::WindowLeft, *s.clock);
    slot.phase.set(phase);
    if was_shown != phase.is_shown() {
        s.kernel.slots.touch(&TAB_HINTS);
    }
    let _ = apply_effect(effect, tx);
}

/// Window blur or hide (`window.left` in `tab-hints-machine.ts`).
pub(crate) fn window_left(s: &mut App, tx: &mut Tx) {
    let slot = s.kernel.slots.untracked(&TAB_HINTS);
    slot.last_mod.set(false);
    let mut phase = slot.phase.get();
    if phase.is_idle() {
        return;
    }
    let was_shown = phase.is_shown();
    let effect = step(&mut phase, Event::WindowLeft, *s.clock);
    slot.phase.set(phase);
    if was_shown != phase.is_shown() {
        s.kernel.slots.touch(&TAB_HINTS);
    }
    let _ = apply_effect(effect, tx);
}

/// Per-frame poll (`stage::REACT_SHELL`). Zero cost unless `Pending`.
pub(crate) fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let slot = s.kernel.slots.untracked(&TAB_HINTS);
    let mut phase = slot.phase.get();
    let Phase::Pending { due, .. } = phase else {
        return false;
    };
    if now < due {
        tx.wake_after(Duration::from_secs_f64((due - now).max(0.001)));
        return false;
    }
    let effect = step(&mut phase, Event::Tick { now }, now);
    slot.phase.set(phase);
    if phase.is_shown() {
        s.kernel.slots.touch(&TAB_HINTS);
    }
    apply_effect(effect, tx)
}

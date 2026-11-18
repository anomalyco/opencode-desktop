//! A menu button's press, as Kobalte's `Menu.Trigger` takes it (`@kobalte/core` menu trigger):
//! [`El::press_on_down`] runs its press handler as a primary mouse button goes down and swallows
//! that press's release, while keys, screen readers and taps still run it on the click. Kobalte's
//! items select on the pointer's release from any press origin (`allowsDifferentPressOrigin`), so
//! this machine keeps only whether a mouse press opened the menu, to pick the row under a release
//! away from the trigger.

use rune::{Drag, El, Phase, PointerButton, Tx};

use super::PopupHost;

/// Whether a held mouse press opened `anchor`'s drawn menu, so a release off the trigger picks the
/// row under it (`allowsDifferentPressOrigin`).
#[derive(Debug, Default, PartialEq, Eq)]
pub enum Press {
    #[default]
    Idle,
    /// A primary mouse press on the closed trigger opened `anchor`'s menu.
    Down { anchor: String },
}

pub enum Event<'a> {
    /// A primary mouse button went down on a closed trigger.
    Down { anchor: &'a str },
    /// The button came up; `on_trigger`: over the trigger.
    Up { on_trigger: bool },
    /// The press was cancelled (the window lost the pointer, a system dialog took capture): it
    /// chooses nothing and the menu stays as it is.
    Cancel,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    /// The press recorded `anchor`; its `on_press` (`El::press_on_down`) commits the open menu in
    /// the same frame.
    Armed,
    /// The release landed off the trigger: choose the menu row under it if `anchor` is still open.
    PickUnder { anchor: String },
    Nothing,
}

pub fn step(state: &mut Press, event: Event<'_>) -> Effect {
    match event {
        Event::Down { anchor } => {
            *state = Press::Down { anchor: anchor.to_owned() };
            Effect::Armed
        }
        Event::Up { on_trigger: true } | Event::Cancel => {
            *state = Press::Idle;
            Effect::Nothing
        }
        Event::Up { on_trigger: false } => match std::mem::take(state) {
            Press::Down { anchor } => Effect::PickUnder { anchor },
            Press::Idle => Effect::Nothing,
        },
    }
}

/// `el` as the menu button anchoring menus at `anchor` (its key): `open` builds and opens the
/// menu ([`super::open`]), as its `on_press` did.
pub fn menu_button<S: PopupHost + 'static>(el: El, anchor: impl Into<String>, open: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    let anchor: String = anchor.into();
    let (pressed, clicked) = (anchor.clone(), anchor);
    let el = if super::PRESS_ON_DOWN { el.press_on_down() } else { el };
    el.on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
        let event = match d.phase {
            Phase::Down
                if super::PRESS_ON_DOWN
                    && tx.button() == PointerButton::Primary
                    && !session_view::style::touch_controls()
                    && !s.popup().open_at(&pressed) =>
            {
                Event::Down { anchor: &pressed }
            }
            Phase::Up if d.cancelled() => Event::Cancel,
            Phase::Up => Event::Up { on_trigger: (0.0..=d.w).contains(&d.x) && (0.0..=d.h).contains(&d.y) },
            _ => return tx.live_only(),
        };
        if matches!(s.popup().press, Press::Idle) && !matches!(event, Event::Down { .. }) {
            return tx.live_only();
        }
        let at = d.window_point();
        match step(&mut s.popup_mut().press, event) {
            Effect::Armed => {}
            Effect::PickUnder { anchor } => {
                if !s.popup().open_at(&anchor) || !super::state::pick_at(s, at, tx) {
                    tx.live_only();
                }
            }
            Effect::Nothing => tx.live_only(),
        }
    })
    .on_press_tx(move |s: &mut S, tx: &mut Tx| {
        if !tx.by_keys() && s.popup().open_at(&clicked) {
            super::dismiss(s, tx);
        } else {
            open(s, tx);
        }
    })
}

/// [`menu_button`] as a builder step: `.menu_button(key, open)` where a trigger had `.on_press_tx(open)`.
pub trait MenuButton {
    fn menu_button<S: PopupHost + 'static>(self, anchor: impl Into<String>, open: impl Fn(&mut S, &mut Tx) + 'static) -> El;
}

impl MenuButton for El {
    fn menu_button<S: PopupHost + 'static>(self, anchor: impl Into<String>, open: impl Fn(&mut S, &mut Tx) + 'static) -> El {
        menu_button(self, anchor, open)
    }
}

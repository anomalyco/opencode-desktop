//! A menu button's press, as Kobalte's `Menu.Trigger` takes it (`@kobalte/core` menu trigger):
//! a primary mouse button opens the menu as it goes down, and a press on the open trigger closes
//! it as it goes down; a key or a finger's tap opens it on the click, as before. Kobalte's items
//! select on the pointer's release from any press origin (`allowsDifferentPressOrigin`), so a
//! press that drags onto an item and lets go there chooses it. A press released elsewhere
//! leaves the menu open, and the click its release would make on the trigger draws nothing.

use rune::{Drag, El, Phase, PointerButton, Tx};

use crate::App;

/// The mouse press on a menu button, from its button going down to the click its release makes.
#[derive(Debug, Default)]
pub enum Press {
    #[default]
    Idle,
    /// The button went down on `anchor`'s trigger; the press opened its menu or closed it.
    Down { anchor: String },
}

pub enum Event<'a> {
    /// A primary mouse button went down on the trigger; `open`: its menu shows now.
    Down { anchor: &'a str, open: bool },
    /// The button came up; `on_trigger`: over the trigger, so its click follows.
    Up { on_trigger: bool },
    /// The trigger's click: a mouse release over it, or a tap.
    Click { anchor: &'a str },
    /// Return or Space on the trigger: always the keyboard's own open.
    Key,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    Open,
    Close,
    /// The release landed off the trigger: choose the menu row under it, if any.
    PickUnder,
    Nothing,
}

pub fn step(state: &mut Press, event: Event<'_>) -> Effect {
    match event {
        Event::Down { anchor, open } => {
            *state = Press::Down { anchor: anchor.to_owned() };
            if open { Effect::Close } else { Effect::Open }
        }
        Event::Up { on_trigger: true } => Effect::Nothing,
        Event::Up { on_trigger: false } => match std::mem::take(state) {
            Press::Down { .. } => Effect::PickUnder,
            Press::Idle => Effect::Nothing,
        },
        Event::Click { anchor } => match std::mem::take(state) {
            // The press already opened or closed it: its click is the same gesture.
            Press::Down { anchor: pressed } if pressed == anchor => Effect::Nothing,
            _ => Effect::Open,
        },
        Event::Key => {
            *state = Press::Idle;
            Effect::Open
        }
    }
}

/// `el` as the menu button anchoring menus at `anchor` (its key): `open` builds and opens the
/// menu (`popup::open`), as its `on_press` did.
pub fn menu_button(el: El, anchor: impl Into<String>, open: impl Fn(&mut App, &mut Tx) + 'static) -> El {
    let anchor: String = anchor.into();
    let open = std::rc::Rc::new(open);
    let (pressed, clicked, open_on_press) = (anchor.clone(), anchor, open.clone());
    el.on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| {
        let event = match d.phase {
            Phase::Down if tx.button() == PointerButton::Primary && !session_view::style::touch_controls() => Event::Down { anchor: &pressed, open: s.popup.open_at(&pressed) },
            Phase::Up => Event::Up { on_trigger: !d.cancelled() && (0.0..=d.w).contains(&d.x) && (0.0..=d.h).contains(&d.y) },
            _ => return tx.live_only(),
        };
        let at = d.window_point();
        match step(&mut s.popup.press, event) {
            Effect::Open => open_on_press(s, tx),
            Effect::Close => super::dismiss(s, tx),
            Effect::PickUnder => {
                if !super::pick_at(s, at, tx) {
                    tx.live_only();
                }
            }
            Effect::Nothing => tx.live_only(),
        }
    })
    .on_press_tx(move |s: &mut App, tx: &mut Tx| match step(&mut s.popup.press, if tx.by_keys() { Event::Key } else { Event::Click { anchor: &clicked } }) {
        Effect::Open => open(s, tx),
        _ => tx.live_only(),
    })
}

/// [`menu_button`] as a builder step: `.menu_button(key, open)` where a trigger had `.on_press_tx(open)`.
pub trait MenuButton {
    fn menu_button(self, anchor: impl Into<String>, open: impl Fn(&mut App, &mut Tx) + 'static) -> El;
}

impl MenuButton for El {
    fn menu_button(self, anchor: impl Into<String>, open: impl Fn(&mut App, &mut Tx) + 'static) -> El {
        menu_button(self, anchor, open)
    }
}

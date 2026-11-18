//! Where focus goes while a drawn menu is open and as it closes, as Desktop's menu machine has
//! it (`ui/src/navigation/menu/machine.ts`, `menu.tsx:333-356`).
//!
//! An open menu holds focus, as Kobalte's does: one the pointer opened focuses its own surface
//! (`menu-v2-content`), one the keyboard opened its first row with the ring, and the row the
//! pointer or the keys light takes it from there. As it closes:
//! - **Escape or Tab** put it back on the trigger, without a ring when the pointer opened the menu
//!   (`selectMenuRestoreTriggerWithoutRing`).
//! - **A press outside** a dropdown, which is modal, puts it back on the trigger the same way: the
//!   press reached nothing under the menu. A context menu's outside press (`outside.interacted`)
//!   leaves focus where that press put it: on the scroll region it landed on, which the scrim
//!   focuses (`El::press_focuses_below`), else nowhere.
//! - **A pick** puts it back on the trigger too, unless the pick moved it on (a dialog, a field,
//!   another menu), as Kobalte's restore finds it outside the menu and leaves it.
//!
//! A menu at a point (a context menu) has no trigger: Kobalte puts focus back where the
//! right-click left it, and as the keys closed it, Chromium rings it.

/// How the menu opened (`MenuOpenModality`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Modality {
    #[default]
    Pointer,
    Keyboard,
}

/// An open menu's focus facts (`MenuState`'s `open`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    pub modality: Modality,
    pub origin: Origin,
}

/// What the menu opened from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A control's key: a dropdown.
    Trigger(String),
    /// A point (a context menu), and the element focused as it opened.
    Point { focused: Option<String> },
}

/// Why it closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// Escape or Tab.
    Keys,
    /// A press outside, or another menu taking its place.
    Outside,
    Picked,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    /// Focus `key`, its ring showing when `ring`.
    Focus { key: String, ring: bool },
    /// Drop focus: the keys closed a context menu opened where nothing had focus.
    Blur,
}

/// What opening focuses, and whether its ring shows: the keyboard focuses the lit row, the
/// pointer the surface.
pub fn opening(opened: &Opened, surface: &str, lit: Option<String>) -> (String, bool) {
    match (opened.modality, lit) {
        (Modality::Keyboard, Some(key)) => (key, true),
        _ => (surface.to_owned(), false),
    }
}

/// What closing for `reason` does once the pick has run.
/// `keys` is whether a key closed it, which a context menu's ring follows (Chromium's
/// `:focus-visible` after Kobalte puts focus back).
pub fn closing(opened: Opened, reason: Reason, keys: bool) -> Option<Effect> {
    let keyboard = opened.modality == Modality::Keyboard;
    match (opened.origin, reason) {
        // Kobalte restores the trigger after a pick too, with the ring when the keys opened it.
        (Origin::Trigger(key), Reason::Picked) => Some(Effect::Focus { key, ring: keyboard }),
        (Origin::Point { focused: Some(key) }, Reason::Picked) => Some(Effect::Focus { key, ring: keys }),
        (Origin::Point { focused: None }, Reason::Picked) => None,
        (Origin::Trigger(key), Reason::Keys) => Some(Effect::Focus { key, ring: keyboard }),
        (Origin::Trigger(key), Reason::Outside) => Some(Effect::Focus { key, ring: false }),
        (Origin::Point { focused: Some(key) }, Reason::Keys) => Some(Effect::Focus { key, ring: true }),
        // The press focused the scroll region it landed on, or nothing (`press_focuses_below`).
        (Origin::Point { .. }, Reason::Outside) => None,
        (Origin::Point { focused: None }, Reason::Keys) => Some(Effect::Blur),
    }
}

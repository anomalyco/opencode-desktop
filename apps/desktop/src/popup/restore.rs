//! Where focus goes as a drawn menu closes, as Desktop's menu machine has it
//! (`ui/src/navigation/menu/machine.ts`): Escape or Tab puts it back on the trigger, without a
//! ring when the pointer opened the menu (`selectMenuRestoreTriggerWithoutRing`); a press outside
//! leaves it where that press put it; a pick leaves it to what the pick does, as a dialog the pick
//! opens has taken it before Kobalte's restore runs. A menu at a point (a context menu) runs no
//! machine in Desktop: Kobalte puts focus back where the right-click left it, and as the keys
//! closed it (Escape, or Return on an item), Chromium rings it.

/// How the menu opened (`MenuOpenModality`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Modality {
    #[default]
    Pointer,
    Keyboard,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum MenuFocus {
    #[default]
    Closed,
    /// `trigger` is the key of the control it opened from; a menu at a point has none.
    Open { modality: Modality, trigger: Option<String> },
}

/// Why it closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reason {
    /// Escape or Tab.
    Keys,
    /// A press outside, or another menu taking its place.
    Outside,
    Picked,
}

pub(crate) enum Event {
    Opened { modality: Modality, trigger: Option<String> },
    Closed(Reason),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    FocusTrigger { key: String, ring: bool },
    /// The focus the menu leaves shows its ring, when keys closed it.
    RingFocused,
}

pub(crate) fn step(state: MenuFocus, event: Event) -> (MenuFocus, Option<Effect>) {
    match (state, event) {
        (_, Event::Opened { modality, trigger }) => (MenuFocus::Open { modality, trigger }, None),
        (MenuFocus::Open { modality, trigger: Some(key) }, Event::Closed(Reason::Keys)) => {
            (MenuFocus::Closed, Some(Effect::FocusTrigger { key, ring: modality == Modality::Keyboard }))
        }
        (MenuFocus::Open { trigger: None, .. }, Event::Closed(Reason::Keys | Reason::Picked)) => (MenuFocus::Closed, Some(Effect::RingFocused)),
        (_, Event::Closed(_)) => (MenuFocus::Closed, None),
    }
}

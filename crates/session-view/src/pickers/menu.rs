//! Explicit state machine for the picker overlays: the model dialog (`DialogSelectModel`) and the
//! dropdown menus (`ComposerEditorSelect` in `packages/app/src/composer/editor/editor.tsx:1038-1184`,
//! `packages/ui/src/navigation/menu/machine.ts`, `packages/ui/src/navigation/menu/menu.tsx:455-531`,
//! and `@kobalte/core` `DropdownMenu`).
//!
//! In Official Desktop:
//! - Opening `#agent` or `#effort` by keyboard (`Enter`, `Space`, `ArrowDown`) transitions the menu
//!   to `open(modality: "keyboard")` and focuses the first `menuitemradio` (`ArrowUp` focuses the
//!   last), with `:focus-visible` on (`@kobalte/core` `MenuTrigger.onKeyDown`, `createSelectableList`).
//! - Opening by mouse (`pointerdown`) transitions to `open(modality: "pointer")` and focuses the
//!   `role="menu"` surface itself with no highlighted option until hover or an arrow key.
//! - Hovering an option moves focus onto that `menuitemradio` without a focus ring.
//! - Closing by picking an option, `Escape`, or `Tab` returns focus to the trigger (`#agent` or
//!   `#effort`), with a focus ring only when `modality == Modality::Keyboard`
//!   (`selectMenuRestoreTriggerWithoutRing` in `machine.ts:105-108`, `pointerFocusReturn` in
//!   `editor.tsx:1054-1062`).
//! - Dismissing a non-modal menu (`modal={false}`, `editor.tsx:1152`) by an outside press leaves
//!   focus where that press landed (`outside.interacted` in `machine.ts:88-94`).

use super::Kind;

/// How a dropdown menu opened (`MenuOpenModality` in `packages/ui/src/navigation/menu/machine.ts:8`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modality {
    Pointer,
    Keyboard,
}

/// Which row a keyboard open highlights (`MenuTrigger.onKeyDown` in `@kobalte/core`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryFocus {
    First,
    Last,
}

/// The input gesture that asked to open a picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Pointer,
    Keyboard(EntryFocus),
}

/// Which dropdown menu is open (`Model` is a modal search dialog, not a `Menu`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKind {
    Agent,
    Effort,
    Speed,
    Project,
}

impl MenuKind {
    pub fn kind(self) -> Kind {
        match self {
            Self::Agent => Kind::Agent,
            Self::Effort => Kind::Effort,
            Self::Speed => Kind::Speed,
            Self::Project => Kind::Project,
        }
    }

    pub fn from_kind(kind: Kind) -> Option<Self> {
        match kind {
            Kind::Agent => Some(Self::Agent),
            Kind::Effort => Some(Self::Effort),
            Kind::Speed => Some(Self::Speed),
            Kind::Project => Some(Self::Project),
            Kind::Model => None,
        }
    }
}

/// Single state enum for the picker overlay (`State::Closed` carries nothing; `State::Model` has
/// no menu modality; `State::Menu` carries its `MenuKind`, `Modality`, and optional lit row).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Closed,
    Model,
    Menu {
        kind: MenuKind,
        modality: Modality,
        active: Option<String>,
    },
}

impl State {
    pub fn kind(&self) -> Option<Kind> {
        match self {
            Self::Closed => None,
            Self::Model => Some(Kind::Model),
            Self::Menu { kind, .. } => Some(kind.kind()),
        }
    }

    pub fn menu_active(&self) -> Option<&str> {
        match self {
            Self::Menu { active, .. } => active.as_deref(),
            _ => None,
        }
    }
}

/// Why an open picker is closing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseReason {
    Picked,
    Escape,
    Tab,
    /// Press on the scrim outside the overlay (`outside.interacted` for menus; modal backdrop for Model).
    Outside,
    /// Another surface (palette, settings, session switch) replaced it.
    Replaced,
}

pub enum Event<'a> {
    Open {
        kind: Kind,
        gesture: Gesture,
        items: &'a [String],
    },
    Close(CloseReason),
    Hover(String),
    Step {
        step: i64,
        items: &'a [String],
    },
    Jump {
        last: bool,
        items: &'a [String],
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    /// Focus the model dialog's search input.
    FocusModelSearch,
    /// Focus an element by key (`picker-<kind>` or `option-<id>`), showing `:focus-visible` when `ring`.
    Focus {
        key: String,
        ring: bool,
        scroll_option: Option<String>,
    },
    /// Return focus to a menu trigger via `Tx::return_focus` (`onCloseAutoFocus`).
    ReturnTrigger {
        key: &'static str,
        ring: bool,
    },
    /// Hand focus back to the composer (`Model` dialog close, `Project` pick/escape).
    FocusComposer,
    /// Clear composer focus and leave DOM/runtime focus where the caller or outside press placed it.
    LeaveFocus,
}

pub fn step(state: &mut State, event: Event<'_>) -> Effect {
    match event {
        Event::Open { kind, gesture, items } => {
            if state.kind() == Some(kind) {
                return step(state, Event::Close(CloseReason::Escape));
            }
            match MenuKind::from_kind(kind) {
                None => {
                    *state = State::Model;
                    Effect::FocusModelSearch
                }
                Some(mk) => {
                    let modality = match gesture {
                        Gesture::Pointer => Modality::Pointer,
                        Gesture::Keyboard(_) => Modality::Keyboard,
                    };
                    let active = match gesture {
                        Gesture::Pointer => None,
                        Gesture::Keyboard(EntryFocus::First) => items.first().cloned(),
                        Gesture::Keyboard(EntryFocus::Last) => items.last().cloned(),
                    };
                    let (key, ring) = match (&active, modality) {
                        (Some(k), Modality::Keyboard) => (format!("option-{k}"), true),
                        _ => (format!("picker-{}", kind.key()), false),
                    };
                    let scroll_option = active.clone();
                    *state = State::Menu { kind: mk, modality, active };
                    Effect::Focus { key, ring, scroll_option }
                }
            }
        }
        Event::Close(reason) => {
            let prev = std::mem::take(state);
            match prev {
                State::Closed => Effect::None,
                State::Model => match reason {
                    CloseReason::Replaced => Effect::LeaveFocus,
                    CloseReason::Picked | CloseReason::Escape | CloseReason::Tab | CloseReason::Outside => Effect::FocusComposer,
                },
                State::Menu { kind: MenuKind::Project, .. } => match reason {
                    CloseReason::Replaced | CloseReason::Outside => Effect::LeaveFocus,
                    CloseReason::Picked | CloseReason::Escape | CloseReason::Tab => Effect::FocusComposer,
                },
                State::Menu { kind, modality, .. } => match reason {
                    CloseReason::Outside | CloseReason::Replaced => Effect::LeaveFocus,
                    CloseReason::Picked | CloseReason::Escape | CloseReason::Tab => Effect::ReturnTrigger {
                        key: kind.kind().key(),
                        ring: modality == Modality::Keyboard,
                    },
                },
            }
        }
        Event::Hover(key) => match state {
            State::Menu { active, .. } => {
                *active = Some(key.clone());
                Effect::Focus { key: format!("option-{key}"), ring: false, scroll_option: None }
            }
            _ => Effect::None,
        },
        Event::Step { step: delta, items } => match state {
            State::Menu { active, .. } if !items.is_empty() => {
                let len = items.len();
                let next_idx = match active.as_ref().and_then(|cur| items.iter().position(|k| k == cur)) {
                    Some(at) => (at as i64 + delta).rem_euclid(len as i64) as usize,
                    None if delta >= 0 => 0,
                    None => len - 1,
                };
                let key = items[next_idx].clone();
                *active = Some(key.clone());
                Effect::Focus { key: format!("option-{key}"), ring: true, scroll_option: Some(key) }
            }
            _ => Effect::None,
        },
        Event::Jump { last, items } => match state {
            State::Menu { active, .. } => {
                let Some(key) = (if last { items.last() } else { items.first() }).cloned() else {
                    return Effect::None;
                };
                *active = Some(key.clone());
                Effect::Focus { key: format!("option-{key}"), ring: true, scroll_option: Some(key) }
            }
            _ => Effect::None,
        },
    }
}

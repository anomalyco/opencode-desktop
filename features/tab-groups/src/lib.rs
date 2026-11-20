//! Experimental side tab groups (Settings › Experimental › Tab groups;
//! `packages/app/src/shell/titlebar/tab-groups.ts`, `tab-groups-ui.tsx`,
//! `tab-group-drag-machine.ts`, #64407).

#![forbid(unsafe_code)]

pub mod drag;
pub mod model;

use rune::TextEdit;

/// Per-window tab groups state, stored in `opencode_kernel::StateSlots`.
#[derive(Default)]
pub struct TabGroupsState {
    /// Window's saved `tabs.groups` document.
    pub doc: model::Doc,
    /// Inline filter above the side tabs (`TabFilterInput`), transient.
    pub filter: TextEdit,
    pub drag: drag::State,
    pub rename: Rename,
    /// Dedicated fallback scratch field for late callbacks after rename finishes (never mutates `filter`).
    pub scratch: TextEdit,
    /// Last tab keys and current tab seen by poll (draft promotion + auto-expand collapsed group).
    pub seen_tabs: Vec<String>,
    pub seen_current: Option<String>,
}

/// Inline group header rename state machine (`Idle -> Arming -> Editing -> Idle`).
#[derive(Default)]
pub enum Rename {
    #[default]
    Idle,
    Arming { group: String, field: TextEdit },
    Editing { group: String, field: TextEdit },
}

pub enum RenameEvent<'a> {
    Start { group: &'a str, name: &'a str },
    Focused(bool),
    Typed,
    Done,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RenameEffect {
    None,
    Focus,
    Rename { group: String, name: String },
}

impl Rename {
    pub fn editing(&self, group: &str) -> Option<&TextEdit> {
        match self {
            Rename::Arming { group: g, field } | Rename::Editing { group: g, field } if g == group => Some(field),
            _ => None,
        }
    }

    pub fn step(&mut self, event: RenameEvent<'_>) -> RenameEffect {
        match (std::mem::take(self), event) {
            (_, RenameEvent::Start { group, name }) => {
                let mut field = TextEdit::new(name);
                field.set_selection(0, field.text.len());
                *self = Rename::Arming { group: group.to_owned(), field };
                RenameEffect::Focus
            }
            (Rename::Arming { group, field }, RenameEvent::Focused(true))
            | (Rename::Editing { group, field }, RenameEvent::Focused(true)) => {
                *self = Rename::Editing { group, field };
                RenameEffect::None
            }
            (Rename::Arming { group, field }, RenameEvent::Focused(false)) => {
                *self = Rename::Arming { group, field };
                RenameEffect::Focus
            }
            (Rename::Arming { group, field }, RenameEvent::Typed) => {
                let effect = RenameEffect::Rename { group: group.clone(), name: field.text.clone() };
                *self = Rename::Arming { group, field };
                effect
            }
            (Rename::Editing { group, field }, RenameEvent::Typed) => {
                let effect = RenameEffect::Rename { group: group.clone(), name: field.text.clone() };
                *self = Rename::Editing { group, field };
                effect
            }
            (Rename::Editing { .. }, RenameEvent::Focused(false))
            | (_, RenameEvent::Done)
            | (Rename::Idle, RenameEvent::Typed | RenameEvent::Focused(_)) => RenameEffect::None,
        }
    }
}

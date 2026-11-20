//! Experimental side tab groups (Settings › Experimental › Tab groups;
//! `packages/app/src/shell/titlebar/tab-groups.ts`, `tab-groups-ui.tsx`,
//! `tab-group-drag-machine.ts`): the saved state and the groups it makes ([`model`]), the
//! header drag ([`drag`]) and the header's rename field ([`Rename`]). The desktop adapter
//! (`apps/desktop/src/shell/tabs/groups.rs`) keeps [`TabGroupsState`] in a kernel slot and
//! draws it in the sidebar.

#![forbid(unsafe_code)]

pub mod drag;
pub mod model;

use rune::TextEdit;

/// Per-window tab groups, in `opencode_kernel::StateSlots`.
#[derive(Default)]
pub struct TabGroupsState {
    /// The window's `tabs.groups`.
    pub doc: model::Doc,
    /// The inline filter above the side tabs (`TabFilterInput`), never saved.
    pub filter: TextEdit,
    pub drag: drag::State,
    pub rename: Rename,
    /// The tab keys and current tab the last poll saw: a draft promoted in place carries its
    /// group, and opening a tab in a collapsed group opens the group.
    pub seen_tabs: Vec<String>,
    pub seen_current: Option<String>,
}

/// A group header's name field (`GroupEditorMenuItems`' TextInput): editing renames the group
/// as each letter is typed, as Desktop's does; Enter, Escape or leaving the field ends it. It
/// arms first: the menu that started it hands focus back as it closes, so the field asks for
/// focus until it has it, and only then does losing it end the rename.
#[derive(Default)]
pub enum Rename {
    #[default]
    Idle,
    Arming { group: String, field: TextEdit },
    Editing { group: String, field: TextEdit },
}

pub enum RenameEvent<'a> {
    Start { group: &'a str, name: &'a str },
    /// The frame's look at the field: whether it has focus.
    Focused(bool),
    Typed,
    Done,
}

#[derive(Debug, PartialEq)]
pub enum RenameEffect {
    None,
    /// Focus the field.
    Focus,
    /// Save `name` as group `group`'s.
    Rename { group: String, name: String },
}

impl Rename {
    pub fn editing(&self, group: &str) -> Option<&TextEdit> {
        match self {
            Rename::Arming { group: g, field } | Rename::Editing { group: g, field } if g == group => Some(field),
            _ => None,
        }
    }

    pub fn field_mut(&mut self) -> Option<&mut TextEdit> {
        match self {
            Rename::Arming { field, .. } | Rename::Editing { field, .. } => Some(field),
            Rename::Idle => None,
        }
    }

    pub fn active(&self) -> bool {
        !matches!(self, Rename::Idle)
    }

    pub fn step(&mut self, event: RenameEvent) -> RenameEffect {
        match (std::mem::take(self), event) {
            (_, RenameEvent::Start { group, name }) => {
                let mut field = TextEdit::new(name);
                field.set_selection(0, field.text.len());
                *self = Rename::Arming { group: group.to_owned(), field };
                RenameEffect::Focus
            }
            (Rename::Arming { group, field }, RenameEvent::Focused(true)) | (Rename::Editing { group, field }, RenameEvent::Focused(true)) => {
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
            (Rename::Editing { .. }, RenameEvent::Focused(false)) | (_, RenameEvent::Done) | (Rename::Idle, RenameEvent::Typed | RenameEvent::Focused(_)) => RenameEffect::None,
        }
    }
}

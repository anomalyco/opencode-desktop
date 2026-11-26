//! Dragging side tabs between sections: a press that moves 4 points drags its tab, or every
//! selected tab when it's one of them; over a header it lands in that section, over a tab just
//! before or after it, past the last tab at the end, ungrouped. Release lands it, Esc or a lost
//! pointer puts it back. The click that ends a drag opens nothing.

use super::model::Drop;

/// How far a press moves before it drags.
const SLOP: f32 = 4.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum DragState {
    #[default]
    Idle,
    /// Pressed on `lead`, carrying `uids` once it moves.
    Pending { lead: String, uids: Vec<String> },
    Dragging { lead: String, uids: Vec<String>, dy: f32, drop: Option<Drop> },
    /// Released after a drag: the press that follows is swallowed.
    Dropped,
}

pub enum Event {
    Down { lead: String, uids: Vec<String> },
    /// The pointer moved `dy` from the press, over `drop`.
    Move { dy: f32, drop: Option<Drop> },
    Up,
    Cancel,
    /// The row's press (a click), after its `Up`.
    Press,
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    None,
    Redraw,
    Commit { uids: Vec<String>, drop: Drop },
    Click,
}

pub fn step(state: &mut DragState, event: Event) -> Effect {
    match (std::mem::take(state), event) {
        (_, Event::Down { lead, uids }) => {
            *state = DragState::Pending { lead, uids };
            Effect::None
        }
        (DragState::Pending { lead, uids }, Event::Move { dy, .. }) if dy.abs() < SLOP => {
            *state = DragState::Pending { lead, uids };
            Effect::None
        }
        (DragState::Pending { lead, uids } | DragState::Dragging { lead, uids, .. }, Event::Move { dy, drop }) => {
            *state = DragState::Dragging { lead, uids, dy, drop };
            Effect::Redraw
        }
        (DragState::Dragging { uids, drop: Some(drop), .. }, Event::Up) => {
            *state = DragState::Dropped;
            Effect::Commit { uids, drop }
        }
        (DragState::Dragging { .. }, Event::Up) => {
            *state = DragState::Dropped;
            Effect::Redraw
        }
        (DragState::Dragging { .. }, Event::Cancel) => Effect::Redraw,
        (DragState::Idle | DragState::Pending { .. }, Event::Press) => Effect::Click,
        (DragState::Dropped, Event::Press) | (DragState::Pending { .. }, Event::Up | Event::Cancel) => Effect::None,
        (s, _) => {
            *state = s;
            Effect::None
        }
    }
}

impl DragState {
    /// How far tab `uid` is carried: the pressed one follows the pointer, the others it carries
    /// stay put, dimmed (`Some(0.0)`).
    pub fn carried(&self, uid: &str) -> Option<f32> {
        match self {
            DragState::Dragging { lead, dy, .. } if lead == uid => Some(*dy),
            DragState::Dragging { uids, .. } if uids.iter().any(|u| u == uid) => Some(0.0),
            _ => None,
        }
    }

    pub fn drop(&self) -> Option<&Drop> {
        match self {
            DragState::Dragging { drop, .. } => drop.as_ref(),
            _ => None,
        }
    }
}

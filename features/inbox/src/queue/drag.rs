//! Dragging a queued prompt to a new place, as Desktop's queue does with dnd-kit's sortable list
//! (`queue-panel.tsx:102-123`, `@dnd-kit/dom` 0.5). The pointer lifts a row 4 pt after a press on
//! its handle (`PointerActivationConstraints.Distance({ value: 4 })`); the row follows it up and
//! down inside the list (`RestrictToVerticalAxis`, `RestrictToElement`) at 60% (`opacity-60`),
//! the rows it passes step aside over 250 ms (`defaultSortableTransition`), and a drop puts it
//! there at once (`dropAnimation: null`). The keys do the same from the focused handle
//! (`KeyboardSensor`, `SortableKeyboardPlugin`): Space or Enter lifts it, Up and Down move it a
//! slot, Space, Enter or Tab drops it, and Escape puts it back.

/// The row's pitch: 32 tall and 1 below (`h-8 mb-px`).
pub const PITCH: f32 = 33.0;
/// Points the pointer travels before a press on the handle becomes a drag.
const SLOP: f32 = 4.0;

/// What moves the lifted row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum By {
    Pointer,
    Keys,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Drag {
    #[default]
    Idle,
    /// The handle of row `id` is down and the pointer hasn't gone 4 pt.
    Pressed { session: String, id: String },
    /// Row `id` follows the pointer or the keys: it started at `from` of `ids` (the strip's
    /// order then) and is over slot `to`, `dy` from where it started.
    Lifted { session: String, id: String, ids: Vec<String>, from: usize, to: usize, dy: f32, by: By },
}

pub enum Event<'a> {
    Down { session: &'a str, id: &'a str },
    /// The pointer, `dy` from the press, with the strip's rows as they are now.
    Move { dx: f32, dy: f32, ids: &'a [String] },
    /// The pointer let go, or the gesture was taken away (`cancelled`).
    Up { cancelled: bool },
    /// Space or Enter on the focused handle of row `id`.
    Pick { session: &'a str, id: &'a str, ids: &'a [String] },
    /// Up (-1) or Down (1) while the keys hold the row.
    Step(i32),
    /// Space, Enter or Tab while the keys hold the row.
    Drop,
    /// Escape.
    Cancel,
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    /// The strip's rows in their new order, for the queue to rewrite.
    Reorder { session: String, ids: Vec<String> },
}

impl Drag {
    /// Row `id` of `session` is lifted.
    pub fn lifted(&self, session: &str, id: &str) -> bool {
        matches!(self, Drag::Lifted { session: s, id: i, .. } if s == session && i == id)
    }

    /// Whether a row of `session` is lifted.
    pub fn active(&self, session: &str) -> bool {
        matches!(self, Drag::Lifted { session: s, .. } if s == session)
    }

    /// How far row `index` of `session` sits from its place: the lifted row where the pointer
    /// has it, and a row between its place and the slot it's over a pitch toward its place.
    pub fn offset(&self, session: &str, index: usize) -> f32 {
        let Drag::Lifted { session: s, from, to, dy, .. } = self else { return 0.0 };
        if s != session {
            return 0.0;
        }
        if index == *from {
            *dy
        } else if *from < index && index <= *to {
            -PITCH
        } else if *to <= index && index < *from {
            PITCH
        } else {
            0.0
        }
    }

    pub fn step(&mut self, event: Event<'_>) -> Option<Effect> {
        match (std::mem::take(self), event) {
            (Drag::Idle | Drag::Pressed { .. }, Event::Down { session, id }) => {
                *self = Drag::Pressed { session: session.to_owned(), id: id.to_owned() };
                None
            }
            (Drag::Pressed { session, id }, Event::Move { dx, dy, ids }) => {
                let from = ids.iter().position(|i| *i == id)?;
                if dx.hypot(dy) < SLOP {
                    *self = Drag::Pressed { session, id };
                    return None;
                }
                *self = Drag::Lifted { session, id, ids: ids.to_vec(), from, to: from, dy: 0.0, by: By::Pointer };
                self.step(Event::Move { dx, dy, ids })
            }
            (Drag::Lifted { session, id, ids, from, by: By::Pointer, .. }, Event::Move { dy, .. }) => {
                // It stays inside the list, and lands on the slot under it.
                let last = ids.len().saturating_sub(1);
                let dy = dy.clamp(-(from as f32) * PITCH, (last - from) as f32 * PITCH);
                let to = (from as f32 + dy / PITCH).round().clamp(0.0, last as f32) as usize;
                *self = Drag::Lifted { session, id, ids, from, to, dy, by: By::Pointer };
                None
            }
            (Drag::Lifted { session, id, ids, from, to, by: By::Pointer, .. }, Event::Up { cancelled }) => drop_row(session, id, ids, from, to, cancelled),
            (Drag::Pressed { .. }, Event::Up { .. }) => None,
            (Drag::Idle, Event::Pick { session, id, ids }) => {
                let from = ids.iter().position(|i| i == id)?;
                *self = Drag::Lifted { session: session.to_owned(), id: id.to_owned(), ids: ids.to_vec(), from, to: from, dy: 0.0, by: By::Keys };
                None
            }
            (Drag::Lifted { session, id, ids, from, to, by: By::Keys, .. }, Event::Step(d)) => {
                let to = (to as i64 + d as i64).clamp(0, ids.len() as i64 - 1) as usize;
                let dy = (to as f32 - from as f32) * PITCH;
                *self = Drag::Lifted { session, id, ids, from, to, dy, by: By::Keys };
                None
            }
            (Drag::Lifted { session, id, ids, from, to, by: By::Keys, .. }, Event::Drop) => drop_row(session, id, ids, from, to, false),
            (Drag::Lifted { .. }, Event::Cancel) => None,
            (state, _) => {
                *self = state;
                None
            }
        }
    }
}

/// The row lands in slot `to`: the rows in their new order, unless it never moved.
fn drop_row(session: String, id: String, mut ids: Vec<String>, from: usize, to: usize, cancelled: bool) -> Option<Effect> {
    if cancelled || from == to || ids.get(from) != Some(&id) {
        return None;
    }
    let moved = ids.remove(from);
    ids.insert(to, moved);
    Some(Effect::Reorder { session, ids })
}

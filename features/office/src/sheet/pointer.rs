//! A grid's pointer gesture (`sheet-pointer-machine.ts`): a press on a cell, a whole row or column
//! header, or the corner that selects all; a drag moves the selection's far corner, along the
//! headers only along them. Rune's drag already belongs to the pointer that started it, so the
//! machine has no pointer ids, and its release always arrives (`Phase::Up`, cancelled or not).

use super::select::Cell;

/// What a drag moves the selection's far corner along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// `pressed`: the cell a plain press went down on, cleared once the drag leaves it.
    Cells { pressed: Option<Cell> },
    Rows,
    Columns,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pointer {
    #[default]
    Idle,
    Dragging(Axis),
}

/// Where a pointer went down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    All,
    Column(Cell),
    Row(Cell),
    Cell(Cell),
}

pub enum PointerEvent {
    Down { shift: bool, hit: Option<Hit> },
    /// A move during a drag: the cell under the pointer (held to the nearest one) and the
    /// selection's far corner now.
    Moved { cell: Option<Cell>, focus: Cell },
    Up { cell: Option<Cell> },
    Cancelled,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PointerEffect {
    SelectAll,
    SelectColumn { col: u32, extend: bool },
    SelectRow { row: u32, extend: bool },
    SelectCell { cell: Cell, extend: bool },
    Extend(Cell),
    /// A release on the cell a plain press went down on: a link there opens.
    FollowLink(Cell),
}

impl Pointer {
    pub fn step(self, event: PointerEvent) -> (Pointer, Vec<PointerEffect>) {
        match (self, event) {
            (_, PointerEvent::Down { shift, hit: Some(hit) }) => down(shift, hit),
            (state, PointerEvent::Down { hit: None, .. }) => (state, Vec::new()),
            (Pointer::Idle, _) => (Pointer::Idle, Vec::new()),
            (Pointer::Dragging(_), PointerEvent::Cancelled) => (Pointer::Idle, Vec::new()),
            (Pointer::Dragging(Axis::Cells { pressed: Some(pressed) }), PointerEvent::Up { cell: Some(cell) }) if pressed == cell => (Pointer::Idle, vec![PointerEffect::FollowLink(cell)]),
            (Pointer::Dragging(_), PointerEvent::Up { .. }) => (Pointer::Idle, Vec::new()),
            (Pointer::Dragging(axis), PointerEvent::Moved { cell: None, .. }) => (Pointer::Dragging(axis), Vec::new()),
            (Pointer::Dragging(axis), PointerEvent::Moved { cell: Some(cell), focus }) => {
                let axis = match axis {
                    Axis::Cells { pressed: Some(p) } if p != cell => Axis::Cells { pressed: None },
                    other => other,
                };
                let target = match axis {
                    Axis::Columns => [0, cell[1]],
                    Axis::Rows => [cell[0], 0],
                    Axis::Cells { .. } => cell,
                };
                let effects = if target == focus { Vec::new() } else { vec![PointerEffect::Extend(target)] };
                (Pointer::Dragging(axis), effects)
            }
        }
    }
}

fn down(shift: bool, hit: Hit) -> (Pointer, Vec<PointerEffect>) {
    match hit {
        // Selecting everything is not a drag.
        Hit::All => (Pointer::Idle, vec![PointerEffect::SelectAll]),
        Hit::Column(cell) => (Pointer::Dragging(Axis::Columns), vec![PointerEffect::SelectColumn { col: cell[1], extend: shift }]),
        Hit::Row(cell) => (Pointer::Dragging(Axis::Rows), vec![PointerEffect::SelectRow { row: cell[0], extend: shift }]),
        Hit::Cell(cell) => (Pointer::Dragging(Axis::Cells { pressed: (!shift).then_some(cell) }), vec![PointerEffect::SelectCell { cell, extend: shift }]),
    }
}

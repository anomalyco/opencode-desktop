//! A side region resize handle's gesture (`resize-handle-machine.ts`). The chat divider, the
//! split divider and the bottom dock's top edge each hold one (`SideState`).

/// One handle's gesture.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Resize {
    #[default]
    Idle,
    /// Dragging, from the size the pane had as the press came and what the session stored then.
    Dragging { from: f32, was: Option<f32> },
}

#[derive(Clone, Copy, Debug)]
pub enum ResizeEvent {
    /// A press on the handle: the pane's laid-out `size`, the session's `stored` setting, and
    /// which click of a run it is.
    Down { size: f32, stored: Option<f32>, clicks: u32 },
    /// The pointer moved `by` points, in the direction the pane grows, since the press.
    Move { by: f32 },
    Up { cancelled: bool },
}

/// What the handle's owner does after a step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResizeEffect {
    None,
    /// Size the pane to this, before its owner clamps it.
    Size(f32),
    /// Put back what the session stored as the drag began.
    Restore(Option<f32>),
}

impl Resize {
    pub fn dragging(self) -> bool {
        self != Self::Idle
    }

    pub fn step(&mut self, event: ResizeEvent) -> ResizeEffect {
        let (next, effect) = match (*self, event) {
            // Desktop's handle ignores the second press of a double click (`event.detail > 1`).
            (_, ResizeEvent::Down { size, stored, clicks: ..=1 }) => (Self::Dragging { from: size, was: stored }, ResizeEffect::None),
            (Self::Dragging { from, was }, ResizeEvent::Move { by }) => (Self::Dragging { from, was }, ResizeEffect::Size(from + by)),
            (Self::Dragging { was, .. }, ResizeEvent::Up { cancelled: true }) => (Self::Idle, ResizeEffect::Restore(was)),
            (_, ResizeEvent::Up { .. }) => (Self::Idle, ResizeEffect::None),
            (state, ResizeEvent::Down { .. } | ResizeEvent::Move { .. }) => (state, ResizeEffect::None),
        };
        *self = next;
        effect
    }
}

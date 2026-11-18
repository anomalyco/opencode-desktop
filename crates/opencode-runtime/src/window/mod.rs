//! Pure headless window lifecycle and placement state machines.

pub mod life;
pub mod placement;

pub use life::{Life, LifeEffect, LifeEvent};
pub use placement::{Frame, Mode, Normal, Placement, fit};

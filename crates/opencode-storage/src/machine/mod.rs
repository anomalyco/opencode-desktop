//! The storage machines, as explicit `rune::machine` machines: pure transitions that return
//! effects for the storage thread (or the web backend) to carry out.

pub mod admission;
pub mod checkpoint;
pub mod draft_gc;
pub mod reservation;
pub mod write_behind;

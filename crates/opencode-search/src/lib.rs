//! Local search primitives. No platform, clock, worker, database or rendering dependencies.

mod arena;
mod fold;
pub mod fulltext;
mod slab;
pub mod tokenizer;

pub use arena::SearchArena;
pub use fold::{Folded, SearchFold};
pub use slab::{SlabFind, SlabMatch};

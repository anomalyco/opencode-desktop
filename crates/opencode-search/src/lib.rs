//! Local search, fuzzy-matching, path-indexing, file-ranking, and URL query-encoding primitives.
//! Strictly headless: `#![forbid(unsafe_code)]`, zero UI, Rune, database, or network dependencies.

#![forbid(unsafe_code)]

mod arena;
pub mod file_rank;
mod fold;
pub mod fulltext;
pub mod fuzzysort;
pub mod model_search;
pub mod path_index;
pub mod query;
mod slab;
pub mod tokenizer;

pub use arena::{SUBSTRING, SearchArena};
pub use fold::{Folded, SearchFold};
pub use slab::{SlabFind, SlabMatch};

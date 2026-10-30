//! A Rune-side OpenCode client: the server connection and event stream (`client`), and the
//! sessions and transcripts they describe (`model`).

pub mod client;
pub mod mention;
pub mod model;
pub mod pairing;
pub mod project;
pub mod prompt;
pub mod pty;
pub mod quote;
mod session_ops;

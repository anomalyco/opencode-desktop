//! Authenticated WebSocket upgrade client, negotiated stream controller, per-origin HTTP/1.1
//! keep-alive pool, and loopback tunnel proxy for remote browser attachments (`opencode#66217`).

pub mod client;
mod pool;
pub mod proxy;
mod pump;
mod stream;

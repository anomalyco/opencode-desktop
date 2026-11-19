//! General multi-server client primitives: server identity keys (`key`), `/api/info` health
//! probes (`probe`), `/api/fs` directory listing (`fs`), single-server link tracking (`link`),
//! and SSE/endpoint stream sync (`sync`).
//!
//! Desktop shell multi-server machines (Home, tabs, Settings, plugin servers, multi-server
//! registry/health/controllers, and legacy file migration) live in `opencode-servers`.

pub mod fs;
pub mod key;
pub mod link;
pub mod probe;
pub mod sync;

pub use key::{Route, ServerKey, ServerScope, TabKey};
pub use probe::{HttpBase, ProbeResult, loopback_origin};

//! Many servers live at once, as Desktop has them (`packages/app/src/runtime/server`): the keys a
//! server and its tabs go by, the registry's plain values, the per-server-to-mixed migration of
//! what a window saved, the `/api/info` and `/api/fs` clients, and the explicit machines that run
//! each server's lifetime, each ported transition for transition from Desktop's `*-machine.ts`:
//!
//! | Machine | Desktop |
//! | --- | --- |
//! | [`controllers`] | `runtime/server/machine.ts` |
//! | [`health`] | `runtime/server/health-machine.ts` |
//! | [`plugin_server`] | `runtime/server/plugin-machine.ts` |
//! | [`sync::step_connection`] | `runtime/server/server-sync/machine.ts` |
//! | [`sync::step_endpoint`] | `runtime/server/server-sync/endpoint-machine.ts` |
//! | [`sidecar`] | `desktop/src/main/service/background-service-machine.ts` |
//! | [`resolution`] | `session/resolution-machine.ts` |
//! | [`tab_load`] | `shell/titlebar/tab-load-machine.ts` |
//! | [`home`] | `home/machine.ts` |
//! | [`home_load`] | `home/sessions/load-machine.ts` |
//! | [`settings::step`] | `settings/servers/machine.ts` |
//!
//! Each is one `State` enum and a `step(&mut State, Event) -> Effects<Effect>` that returns what
//! should happen; the caller runs the effects (a probe on a worker, a timer through
//! `Tx::wake_at`) and steps their answers back, fenced by the `Attempt` they carry.

pub mod controllers;
pub mod fs;
pub mod health;
pub mod home;
pub mod home_load;
pub mod key;
pub mod migration;
pub mod plugin_server;
pub mod probe;
pub mod registry;
pub mod resolution;
pub mod settings;
pub mod sidecar;
pub mod sync;
pub mod tab_load;
pub mod tabs;

pub use key::{Route, ServerKey, ServerScope, TabKey};
pub use registry::{Credentials, HttpBase, ServerConn};

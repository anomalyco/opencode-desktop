//! OpenCode agent widgets, without UI: the contract's types (`specs/v2/widgets.md` in
//! anomalyco/opencode), the per-widget lifecycle machine, and the step runner.
//!
//! - [`node`]: the view tree a step returns ([`Node`]).
//! - [`step`]: the step ABI's values ([`StepInput`], [`StepOutput`], [`WidgetEvent`], [`Response`]).
//! - [`draw`]: a canvas's [`Frame`] and draw [`Command`]s; [`shader`]: a GPU canvas's WGSL
//!   module and uniform layout.
//! - [`machine`]: loading → ready → stepping → submitted / dismissed / error, as an explicit
//!   machine whose effects (fetch, run a step, respond) the host runs.
//! - [`live`] (feature `runner`): live widget templates, one long-running program per widget on a
//!   thread of its own, with timers, display frames, `fetch`, pauses and saves.
//! - [`runner`] (feature `runner`, on by default): compiles a widget template program once on the Rust
//!   PsychoScript and runs steps under the 250 ms budget; [`step_json`] for JSON-only hosts.
//!
//! Pure Rust with no UI or platform dependencies, so a desktop renderer, a mobile native module
//! and a server can all embed it.

#![forbid(unsafe_code)]

pub mod draw;
pub mod fixtures;
#[cfg(feature = "runner")]
mod host_value;
pub mod machine;
pub mod node;
#[cfg(feature = "runner")]
pub mod live;
#[cfg(feature = "runner")]
pub mod runner;
pub mod shader;
pub mod step;
#[cfg(feature = "runner")]
pub mod tree;

pub use machine::{Closed, Effect, Event, Face, Mode, Phase, RespondError, Status, Widget};
pub use node::{Align, ButtonVariant, Inset, Layer, Layers, Node, Place, SelectOption, SelectValue, TextStyle, Tone};
#[cfg(feature = "runner")]
pub use runner::{Error, Program, STEP_TIMEOUT_MS, step_json};
pub use draw::{Command, DrawOutput, Frame, Paint, Rect};
pub use step::{EventKind, Key, PointerKind, Response, SendMessage, Step, StepInput, StepOutput, WidgetEvent};

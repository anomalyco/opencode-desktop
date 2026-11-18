//! Thin adapter binding `opencode_runtime::machines::fork` to the desktop [`SavedDraft`].

use crate::rewind::SavedDraft;

pub use opencode_runtime::machines::fork::{Surface, TIMEOUT_SECS};

pub type State = opencode_runtime::machines::fork::State<SavedDraft>;
pub type Fork = opencode_runtime::machines::fork::Fork<SavedDraft>;
pub type Event = opencode_runtime::machines::fork::Event<SavedDraft>;
pub type Effect = opencode_runtime::machines::fork::Effect<SavedDraft>;

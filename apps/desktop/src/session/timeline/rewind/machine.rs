//! Thin adapter binding `opencode_runtime::machines::rewind` to the desktop [`SavedDraft`].

use super::SavedDraft;

pub type Rollback = opencode_runtime::machines::rewind::Rollback<SavedDraft>;
pub type Operation = opencode_runtime::machines::rewind::Operation<SavedDraft>;
pub type Doubt = opencode_runtime::machines::rewind::Doubt<SavedDraft>;
pub type State = opencode_runtime::machines::rewind::State<SavedDraft>;
pub type SessionRevert = opencode_runtime::machines::rewind::SessionRevert<SavedDraft>;
pub type Event = opencode_runtime::machines::rewind::Event<SavedDraft>;
pub type Effect = opencode_runtime::machines::rewind::Effect<SavedDraft>;

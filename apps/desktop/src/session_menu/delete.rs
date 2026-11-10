//! Delete's confirmation, as Desktop's machine runs it (`session/delete-machine.ts`,
//! `home/sessions/controller.tsx:423-524`): asking, then waiting for the server with both
//! buttons off. The session and its subagents leave the list at once and come back if the delete
//! fails; once the server has deleted them their tabs go too and the dialog closes. A delete
//! that hasn't answered in 15 s counts as failed, so the buttons can't stay off.

use rune::machine::Effects;

use opencode_kit::model::SessionInfo;

/// `SESSION_DELETE_TIMEOUT_MS`.
pub(crate) const TIMEOUT: f64 = 15.0;

#[derive(Clone, Debug, Default)]
pub(crate) enum Delete {
    #[default]
    Closed,
    Confirming { session: String },
    /// `hidden`: the sessions taken off the list until the server answers.
    Deleting { session: String, hidden: Vec<SessionInfo>, due: f64 },
}

pub(crate) enum Event {
    Ask(String),
    /// Cancel, Escape or a press outside (`cancel.requested`).
    Cancel,
    /// Delete (`confirm.requested`): `tree` is the session and its subagents, empty once it's gone.
    Confirm { tree: Vec<SessionInfo>, now: f64 },
    Replied { session: String, ok: bool },
    Tick { now: f64 },
}

#[derive(Debug)]
pub(crate) enum Effect {
    /// Take these sessions off the list (the data layer's hide).
    Hide(Vec<String>),
    /// Send the delete.
    Run(String),
    WakeAt(f64),
    /// Put hidden sessions back.
    Restore(Vec<SessionInfo>),
    /// The server deleted them: their tabs and page go (`session.removed`).
    Removed(Vec<String>),
    Failed,
}

pub(crate) fn step(delete: &mut Delete, event: Event) -> Effects<Effect> {
    match (std::mem::take(delete), event) {
        (Delete::Closed | Delete::Confirming { .. }, Event::Ask(session)) => {
            *delete = Delete::Confirming { session };
            Effects::none()
        }
        (Delete::Confirming { .. }, Event::Cancel) => Effects::none(),
        // A delete that timed out can still land: the session is gone, so the dialog goes too.
        (Delete::Confirming { session }, Event::Replied { session: s, ok: true }) if s == session => Effects::one(Effect::Removed(vec![session])),
        (Delete::Confirming { session }, Event::Confirm { tree, now }) => {
            if tree.is_empty() {
                return Effects::none();
            }
            let due = now + TIMEOUT;
            let ids = tree.iter().map(|s| s.id.clone()).collect();
            *delete = Delete::Deleting { session: session.clone(), hidden: tree, due };
            Effects::Many(vec![Effect::Hide(ids), Effect::Run(session), Effect::WakeAt(due)])
        }
        (Delete::Deleting { session, hidden, .. }, Event::Replied { session: s, ok: true }) if s == session => {
            Effects::one(Effect::Removed(hidden.into_iter().map(|h| h.id).collect()))
        }
        (Delete::Deleting { session, hidden, .. }, Event::Replied { session: s, ok: false }) if s == session => {
            *delete = Delete::Confirming { session };
            Effects::two(Effect::Restore(hidden), Effect::Failed)
        }
        (Delete::Deleting { session, hidden, due }, Event::Tick { now }) if now + 1e-9 >= due => {
            *delete = Delete::Confirming { session };
            Effects::two(Effect::Restore(hidden), Effect::Failed)
        }
        // A second click, Enter or Cancel waits for the delete in flight.
        (state, _) => {
            *delete = state;
            Effects::none()
        }
    }
}

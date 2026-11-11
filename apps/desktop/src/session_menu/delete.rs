//! Delete's confirmation, as Desktop's machine runs it (`session/delete-machine.ts`,
//! `home/sessions/controller.tsx:423-524`): asking, then waiting for the server with both
//! buttons off. The session and its subagents leave the list at once and come back if the delete
//! fails; once the server has deleted them their tabs go too and the dialog closes. A delete
//! that hasn't answered in 15 s counts as failed, so the buttons can't stay off. The session is
//! on its own server, whichever one the window works against (`ProjectSessionRow`'s `ctx()`).

use rune::machine::Effects;

use opencode_kit::model::SessionInfo;
use opencode_kit::servers::ServerKey;

/// `SESSION_DELETE_TIMEOUT_MS`.
pub(crate) const TIMEOUT: f64 = 15.0;

/// The session Delete asks about: on its server, with the title the dialog names it by
/// (`sessionLabel(props.session)`, taken as it opens).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Asked {
    pub session: String,
    pub server: ServerKey,
    pub title: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) enum Delete {
    #[default]
    Closed,
    Confirming { asked: Asked },
    /// `hidden`: the sessions taken off the list until the server answers.
    Deleting { asked: Asked, hidden: Vec<SessionInfo>, due: f64 },
}

pub(crate) enum Event {
    Ask(Asked),
    /// Cancel, Escape or a press outside (`cancel.requested`).
    Cancel,
    /// Delete (`confirm.requested`): `tree` is the session and its subagents, empty once it's gone.
    Confirm { tree: Vec<SessionInfo>, now: f64 },
    /// `server` answered the delete of `session`.
    Replied { session: String, server: ServerKey, ok: bool },
    Tick { now: f64 },
}

#[derive(Debug)]
pub(crate) enum Effect {
    /// Take these sessions off `server`'s list (the data layer's hide).
    Hide(ServerKey, Vec<String>),
    /// Send the delete to `server`.
    Run(ServerKey, String),
    WakeAt(f64),
    /// Put hidden sessions back on `server`'s list.
    Restore(ServerKey, Vec<SessionInfo>),
    /// `server` deleted them: their tabs and page go (`session.removed`).
    Removed(ServerKey, Vec<String>),
    Failed,
}

pub(crate) fn step(delete: &mut Delete, event: Event) -> Effects<Effect> {
    let ours = |a: &Asked, session: &str, server: &ServerKey| a.session == session && a.server == *server;
    match (std::mem::take(delete), event) {
        (Delete::Closed | Delete::Confirming { .. }, Event::Ask(asked)) => {
            *delete = Delete::Confirming { asked };
            Effects::none()
        }
        (Delete::Confirming { .. }, Event::Cancel) => Effects::none(),
        // A delete that timed out can still land: the session is gone, so the dialog goes too.
        (Delete::Confirming { asked }, Event::Replied { session, server, ok: true }) if ours(&asked, &session, &server) => Effects::one(Effect::Removed(server, vec![session])),
        (Delete::Confirming { asked }, Event::Confirm { tree, now }) => {
            if tree.is_empty() {
                return Effects::none();
            }
            let due = now + TIMEOUT;
            let hide = Effect::Hide(asked.server.clone(), tree.iter().map(|s| s.id.clone()).collect());
            let run = Effect::Run(asked.server.clone(), asked.session.clone());
            *delete = Delete::Deleting { asked, hidden: tree, due };
            Effects::Many(vec![hide, run, Effect::WakeAt(due)])
        }
        (Delete::Deleting { asked, hidden, .. }, Event::Replied { session, server, ok: true }) if ours(&asked, &session, &server) => {
            Effects::one(Effect::Removed(server, hidden.into_iter().map(|h| h.id).collect()))
        }
        (Delete::Deleting { asked, hidden, .. }, Event::Replied { session, server, ok: false }) if ours(&asked, &session, &server) => {
            *delete = Delete::Confirming { asked };
            Effects::two(Effect::Restore(server, hidden), Effect::Failed)
        }
        (Delete::Deleting { asked, hidden, due }, Event::Tick { now }) if now + 1e-9 >= due => {
            let server = asked.server.clone();
            *delete = Delete::Confirming { asked };
            Effects::two(Effect::Restore(server, hidden), Effect::Failed)
        }
        // A second click, Enter or Cancel waits for the delete in flight.
        (state, _) => {
            *delete = state;
            Effects::none()
        }
    }
}

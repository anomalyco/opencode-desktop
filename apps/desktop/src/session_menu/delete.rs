//! Delete: Desktop's confirmation (`session/delete-machine.ts`, `home/sessions/controller.tsx:
//! 423-524`) stays, since once the server has deleted a session its copy is gone for good. Beyond
//! Desktop, which sends the delete at once and waits with both buttons off, Delete in the dialog
//! closes it and takes the session and its subagents off the list at once, and a toast offers it
//! back for its 8 s (`crate::undo`): only when the toast goes is the delete sent (at once when
//! the app quits). Undo puts the sessions back, nothing sent. Once sent, the server decides: they
//! go with their tabs when it has deleted them, and come back with Desktop's failure toast if it
//! refuses or hasn't answered in 15 s. Killed before the toast goes, nothing was sent: the
//! session is still there next launch. The session is on its own server, whichever one the
//! window works against (`ProjectSessionRow`'s `ctx()`).

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

/// The confirmation.
#[derive(Clone, Debug, Default)]
pub(crate) enum Delete {
    #[default]
    Closed,
    Confirming { asked: Asked },
}

pub(crate) enum Event {
    Ask(Asked),
    /// Cancel, Escape or a press outside (`cancel.requested`).
    Cancel,
    /// Delete (`confirm.requested`): `tree` is the session and its subagents, empty once it's gone.
    Confirm { tree: Vec<SessionInfo> },
}

#[derive(Debug)]
pub(crate) enum Effect {
    /// Take these sessions off `server`'s list (the data layer's hide).
    Hide(ServerKey, Vec<String>),
    /// Offer `session` back; the delete is sent when the offer's toast goes.
    Defer { server: ServerKey, session: String, hidden: Vec<SessionInfo> },
}

pub(crate) fn step(delete: &mut Delete, event: Event) -> Effects<Effect> {
    match (std::mem::take(delete), event) {
        (_, Event::Ask(asked)) => {
            *delete = Delete::Confirming { asked };
            Effects::none()
        }
        (Delete::Confirming { .. }, Event::Cancel) => Effects::none(),
        (Delete::Confirming { asked }, Event::Confirm { tree }) => {
            if tree.is_empty() {
                return Effects::none();
            }
            let hide = Effect::Hide(asked.server.clone(), tree.iter().map(|s| s.id.clone()).collect());
            Effects::two(hide, Effect::Defer { server: asked.server, session: asked.session, hidden: tree })
        }
        (state, _) => {
            *delete = state;
            Effects::none()
        }
    }
}

/// A delete sent, its sessions hidden until the server answers.
#[derive(Clone, Debug)]
struct Sending {
    server: ServerKey,
    session: String,
    hidden: Vec<SessionInfo>,
    due: f64,
}

/// The deletes sent and not answered yet.
#[derive(Clone, Debug, Default)]
pub(crate) struct Sent(Vec<Sending>);

pub(crate) enum SentEvent {
    /// The toast went: the delete of `session` on `server` goes now.
    Sent { server: ServerKey, session: String, hidden: Vec<SessionInfo>, now: f64 },
    /// `server` answered the delete of `session`.
    Replied { session: String, server: ServerKey, ok: bool },
    Tick { now: f64 },
}

#[derive(Debug)]
pub(crate) enum SentEffect {
    /// `server` deleted them: their tabs and page go (`session.removed`).
    Removed(ServerKey, Vec<String>),
    /// Put hidden sessions back on `server`'s list, and say it failed.
    Failed(ServerKey, Vec<SessionInfo>),
    WakeAt(f64),
}

impl Sent {
    pub(crate) fn step(&mut self, event: SentEvent) -> Effects<SentEffect> {
        match event {
            SentEvent::Sent { server, session, hidden, now } => {
                let due = now + TIMEOUT;
                self.0.retain(|s| !(s.session == session && s.server == server));
                self.0.push(Sending { server, session, hidden, due });
                Effects::one(SentEffect::WakeAt(due))
            }
            SentEvent::Replied { session, server, ok } => {
                let Some(at) = self.0.iter().position(|s| s.session == session && s.server == server) else {
                    // A delete that timed out can still land: the session is gone, so its tabs go too.
                    return if ok { Effects::one(SentEffect::Removed(server, vec![session])) } else { Effects::none() };
                };
                let s = self.0.remove(at);
                if ok {
                    return Effects::one(SentEffect::Removed(s.server, s.hidden.into_iter().map(|h| h.id).collect()));
                }
                Effects::one(SentEffect::Failed(s.server, s.hidden))
            }
            SentEvent::Tick { now } => {
                let (late, waiting): (Vec<Sending>, Vec<Sending>) = std::mem::take(&mut self.0).into_iter().partition(|s| now + 1e-9 >= s.due);
                self.0 = waiting;
                let mut effects: Vec<SentEffect> = late.into_iter().map(|s| SentEffect::Failed(s.server, s.hidden)).collect();
                effects.extend(self.next_due().map(SentEffect::WakeAt));
                Effects::Many(effects)
            }
        }
    }

    /// When the oldest delete in flight times out.
    pub(crate) fn next_due(&self) -> Option<f64> {
        self.0.iter().map(|s| s.due).reduce(f64::min)
    }
}

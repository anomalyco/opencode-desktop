//! One way back for what the app lets a person throw away, beyond Desktop (which has none of
//! these): a draft tab closed with words or files in it, a window closed with its tabs, a session
//! archived or deleted, a server removed. Each is offered back for as long as its toast shows (8 s,
//! held while the pointer is on it): its `Undo` runs the inverse. Each is done at once, as
//! Desktop does it, except a delete, which the server can't undo: it's only hidden until the toast
//! goes, sent then (at once when the app quits), and Undo shows it again. A closed draft tab and a
//! closed window stay reopenable after the toast (⇧⌘T, Reopen closed window), across a restart
//! too; the others' way back ends with their toast.
//!
//! One machine for every window's offers: an offer lives exactly as long as its toast.

use opencode_kit::servers::ServerKey;
use rune::Tx;

use crate::App;

/// How long an offer's toast shows.
pub const SECS: f64 = 8.0;

/// What an offer puts back.
#[derive(Clone, Debug)]
pub enum Undoable {
    /// Draft tab `draft`, closed: it reopens in its place with everything in it.
    DraftTab { draft: String },
    /// Window `id`, closed while others stayed: it opens again with its tabs and drafts.
    Window { id: String },
    /// Sessions archived, each on its server: they come back, with their tabs. A run of archives
    /// shares one offer (`archived`), so clearing a list still has one Undo for all of it.
    Archived { sessions: Vec<(ServerKey, String)> },
    /// A server added by address, removed: it's listed again where it was, password and all.
    Server { server: crate::servers::Server, index: usize },
    /// Session `session` on `server` and its subagents (`hidden`), deleted: off the list at once,
    /// the delete sent only when the toast goes (`session_menu::delete`). Undo lists them again.
    Deleted { server: ServerKey, session: String, hidden: Vec<opencode_kit::model::SessionInfo> },
    /// A queued prompt of `session`, removed: its words and files come back into the composer.
    Unqueued { session: String, text: String, files: Vec<opencode_kit::model::Attachment> },
    /// A comment box closed with words in it (`what` says which): it opens again with them.
    Comment(crate::line_comments::Discarded),
}

impl Undoable {
    /// Done only when its toast goes, not when it's offered.
    fn deferred(&self) -> bool {
        matches!(self, Undoable::Deleted { .. })
    }
}

/// One offer: `what` goes back if toast `toast`'s Undo is pressed.
#[derive(Debug)]
struct Offer {
    toast: u64,
    what: Undoable,
}

#[derive(Default)]
pub struct Undos {
    offers: Vec<Offer>,
}

pub enum Event {
    /// `what` was done and toast `toast` offers it back.
    Offered { toast: u64, what: Undoable },
    /// The toast went (its time ran out, it was dismissed or replaced): what was done stays done.
    ToastClosed(u64),
    /// Its Undo was pressed.
    UndoPressed(u64),
    /// One more session is being archived: the archive still on offer joins it under one toast.
    ArchivingAnother,
}

pub enum Effect {
    /// Put `what` back.
    Run(Undoable),
    /// Do `what` now: its toast went (or the app quits) and it was waiting for that.
    Commit(Undoable),
    /// Toast `toast`'s archived sessions, withdrawn: the next offer takes them along.
    Regroup { toast: u64, sessions: Vec<(ServerKey, String)> },
}

/// Offers in flight at most; older ones' toasts are gone by then anyway.
const MAX: usize = 8;

impl Undos {
    pub fn step(&mut self, event: Event) -> Option<Effect> {
        match event {
            Event::Offered { toast, what } => {
                self.offers.retain(|o| o.toast != toast);
                self.offers.push(Offer { toast, what });
                // Kept by count only while nothing waits on them: a deferred one goes when its toast does.
                let over = self.offers.len().saturating_sub(MAX);
                let mut dropped = 0;
                self.offers.retain(|o| {
                    let drop = dropped < over && !o.what.deferred();
                    dropped += usize::from(drop);
                    !drop
                });
                None
            }
            Event::ToastClosed(toast) => {
                let at = self.offers.iter().position(|o| o.toast == toast)?;
                let offer = self.offers.remove(at);
                offer.what.deferred().then_some(Effect::Commit(offer.what))
            }
            Event::UndoPressed(toast) => {
                let at = self.offers.iter().position(|o| o.toast == toast)?;
                Some(Effect::Run(self.offers.remove(at).what))
            }
            Event::ArchivingAnother => {
                let at = self.offers.iter().rposition(|o| matches!(o.what, Undoable::Archived { .. }))?;
                match self.offers.remove(at) {
                    Offer { toast, what: Undoable::Archived { sessions } } => Some(Effect::Regroup { toast, sessions }),
                    _ => None,
                }
            }
        }
    }

    /// Whether toast `toast` offers something back.
    pub fn offers(&self, toast: u64) -> bool {
        self.offers.iter().any(|o| o.toast == toast)
    }
}

/// `what` was just done: a toast titled `title` offers it back. A run with no server to undo
/// against (a story, a fixture) offers nothing and does a deferred `what` at once.
pub fn offer(s: &mut App, title: &str, what: Undoable) {
    if !s.live() {
        return commit(s, what, &mut Tx::default());
    }
    let toast = crate::status_toast::undoable(s, title);
    let _ = s.composer_ext.undos.step(Event::Offered { toast, what });
}

/// Session `session` on `server` was just archived. Archives made while the last one's toast
/// still shows share one toast, which counts them and whose Undo brings them all back: before,
/// each archive replaced the one before it, which lost its way back.
pub fn archived(s: &mut App, server: ServerKey, session: String) {
    let mut sessions = match s.composer_ext.undos.step(Event::ArchivingAnother) {
        // A toast the person closed took its offer with it: those stay archived.
        Some(Effect::Regroup { toast, sessions }) if s.toast.stack.entries().iter().any(|e| e.id == toast) => {
            crate::status_toast::dismiss_id(s, toast);
            sessions
        }
        _ => Vec::new(),
    };
    sessions.retain(|(on, id)| *on != server || *id != session);
    sessions.push((server, session));
    let title = match sessions.len() {
        1 => opencode_i18n::t!("rune.undo.sessionArchived").to_owned(),
        n => opencode_i18n::plural!("rune.undo.sessionsArchived", n),
    };
    offer(s, &title, Undoable::Archived { sessions });
}

/// Toast `toast`'s Undo.
pub fn pressed(s: &mut App, toast: u64, tx: &mut Tx) {
    let Some(Effect::Run(what)) = s.composer_ext.undos.step(Event::UndoPressed(toast)) else { return };
    run(s, what, tx);
}

/// Toast `toast` went: what it held back is done now (the next poll wakes for its answer).
pub fn toast_closed(s: &mut App, toast: u64) {
    if let Some(Effect::Commit(what)) = s.composer_ext.undos.step(Event::ToastClosed(toast)) {
        commit(s, what, &mut Tx::default());
    }
}

/// The app quits: everything still waiting on its toast is done now.
pub fn commit_all(s: &mut App) {
    let toasts: Vec<u64> = s.composer_ext.undos.offers.iter().filter(|o| o.what.deferred()).map(|o| o.toast).collect();
    for toast in toasts {
        toast_closed(s, toast);
    }
}

fn commit(s: &mut App, what: Undoable, tx: &mut Tx) {
    if let Undoable::Deleted { server, session, hidden } = what {
        s.send_delete(server, session, hidden, tx);
    }
}

fn run(s: &mut App, what: Undoable, tx: &mut Tx) {
    match what {
        Undoable::DraftTab { draft } => s.reopen_closed(Some(&draft), false, tx),
        Undoable::Window { id } => crate::windows::reopen(s, &id),
        Undoable::Archived { sessions } => {
            for (server, session) in sessions {
                crate::servers::runtime::with_server(s, &server, |s| s.unarchive_session(&session));
                s.reopen_closed(Some(&session), false, tx);
            }
        }
        Undoable::Server { server, index } => crate::servers::put_back(server, index),
        Undoable::Deleted { server, hidden, .. } => s.undelete(&server, hidden),
        Undoable::Unqueued { session, text, files } => s.unwithdraw(&session, text, files),
        Undoable::Comment(discarded) => s.restore_discarded_comment(discarded, tx),
    }
}

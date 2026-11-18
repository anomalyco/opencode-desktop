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
    /// Session `session` on `server`, archived: it comes back, with its tab.
    Archived { server: ServerKey, session: String },
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
}

pub enum Effect {
    /// Put `what` back.
    Run(Undoable),
    /// Do `what` now: its toast went (or the app quits) and it was waiting for that.
    Commit(Undoable),
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
    let _ = s.undos.step(Event::Offered { toast, what });
}

/// Toast `toast`'s Undo.
pub fn pressed(s: &mut App, toast: u64, tx: &mut Tx) {
    let Some(Effect::Run(what)) = s.undos.step(Event::UndoPressed(toast)) else { return };
    run(s, what, tx);
}

/// Toast `toast` went: what it held back is done now (the next poll wakes for its answer).
pub fn toast_closed(s: &mut App, toast: u64) {
    if let Some(Effect::Commit(what)) = s.undos.step(Event::ToastClosed(toast)) {
        commit(s, what, &mut Tx::default());
    }
}

/// The app quits: everything still waiting on its toast is done now.
pub fn commit_all(s: &mut App) {
    let toasts: Vec<u64> = s.undos.offers.iter().filter(|o| o.what.deferred()).map(|o| o.toast).collect();
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
        Undoable::Archived { server, session } => {
            crate::servers::runtime::with_server(s, &server, |s| s.unarchive_session(&session));
            s.reopen_closed(Some(&session), false, tx);
        }
        Undoable::Server { server, index } => crate::servers::put_back(server, index),
        Undoable::Deleted { server, hidden, .. } => s.undelete(&server, hidden),
        Undoable::Unqueued { session, text, files } => s.unwithdraw(&session, text, files),
        Undoable::Comment(discarded) => s.restore_discarded_comment(discarded, tx),
    }
}

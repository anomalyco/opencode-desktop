//! One way back for what the app lets a person throw away, beyond Desktop (which has none of
//! these): a draft tab closed with words or files in it, a window closed with its tabs, a session
//! archived, a server removed. Each is done at once, as Desktop does it, and offered back for as
//! long as its toast shows (8 s, held while the pointer is on it): its `Undo` runs the inverse. A
//! closed draft tab and a closed window stay reopenable after the toast (⇧⌘T, Reopen closed
//! window), across a restart too; the others' way back ends with their toast.
//!
//! One machine for every window's offers: an offer lives exactly as long as its toast.

use opencode_kit::servers::ServerKey;
use rune::Tx;

use crate::App;

/// How long an offer's toast shows.
pub const SECS: f64 = 8.0;

/// What an offer puts back.
#[derive(Clone, Debug, PartialEq)]
pub enum Undoable {
    /// Draft tab `draft`, closed: it reopens in its place with everything in it.
    DraftTab { draft: String },
    /// Window `id`, closed while others stayed: it opens again with its tabs and drafts.
    Window { id: String },
    /// Session `session` on `server`, archived: it comes back, with its tab.
    Archived { server: ServerKey, session: String },
    /// A server added by address, removed: it's listed again where it was, password and all.
    Server { server: crate::servers::Server, index: usize },
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
}

/// Offers in flight at most; older ones' toasts are gone by then anyway.
const MAX: usize = 8;

impl Undos {
    pub fn step(&mut self, event: Event) -> Option<Effect> {
        match event {
            Event::Offered { toast, what } => {
                self.offers.retain(|o| o.toast != toast);
                self.offers.push(Offer { toast, what });
                let over = self.offers.len().saturating_sub(MAX);
                self.offers.drain(..over);
                None
            }
            Event::ToastClosed(toast) => {
                self.offers.retain(|o| o.toast != toast);
                None
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

/// `what` was just done: a toast titled `title` offers it back.
pub fn offer(s: &mut App, title: &str, what: Undoable) {
    if !s.live() {
        return;
    }
    let toast = crate::status_toast::undoable(s, title);
    let _ = s.undos.step(Event::Offered { toast, what });
}

/// Toast `toast`'s Undo.
pub fn pressed(s: &mut App, toast: u64, tx: &mut Tx) {
    let Some(Effect::Run(what)) = s.undos.step(Event::UndoPressed(toast)) else { return };
    run(s, what, tx);
}

/// Toast `toast` went.
pub fn toast_closed(s: &mut App, toast: u64) {
    let _ = s.undos.step(Event::ToastClosed(toast));
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
    }
}

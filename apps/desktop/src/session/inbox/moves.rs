//! Archive and Unarchive, sent. Desktop asks the server first and moves the session when it has
//! answered (`shell/titlebar/inbox-model.ts:476-492`, `session/commands/use-session-commands.tsx:
//! 359-391`, `shell/titlebar/recent-sessions.tsx:914`), so a session the server refuses to move
//! stays where it is, with a toast. Beyond Desktop the window shows the move at once, so a move
//! the server refuses is put back: the session where it was, its tab in its place again, the
//! page it showed back on it, the Undo offer gone, and Desktop's toast. A session that was not the
//! page shown comes back without being shown: Desktop, which moved nothing, leaves the page
//! where it is (`inbox-model.ts:476-487`: the move and the tabs' removal run only once the server
//! has answered).
//!
//! One machine for the moves in flight. A session's writes reach its server in the order they
//! were made (`opencode-kit`'s write lanes), so its answers come back in the order its moves went.

use opencode_kit::servers::ServerKey;
use rune::Tx;

use crate::App;

/// Where a session stood before a move, for putting it back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Was {
    /// Listed, and archived by the move, which closed its tab.
    Listed { tab: TabWas },
    /// Archived at this time (wall-clock ms), and listed again by the move.
    Archived(i64),
}

/// The window's tab for a session when its archive closed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabWas {
    /// It had none.
    Absent,
    /// One that was not the page shown.
    Behind,
    /// The page shown.
    Shown,
}

/// A move the window made and asked `server` for.
#[derive(Clone, Debug)]
struct Move {
    server: ServerKey,
    session: String,
    was: Was,
}

/// The moves sent and not answered yet, oldest first.
#[derive(Clone, Debug, Default)]
pub struct Moves(Vec<Move>);

pub enum Event<'a> {
    /// The window moved `session` and asked `server` for it.
    Sent { server: ServerKey, session: String, was: Was },
    /// `server` answered an archive (`archive`) or an unarchive of `session`. A write that
    /// couldn't go answers at once, refused; a story's, which goes nowhere, at once, taken.
    Replied { server: &'a ServerKey, session: &'a str, archive: bool, ok: bool },
}

#[derive(Debug)]
pub enum Effect {
    /// The server refused the move: show `session` on `server` as it `was`.
    PutBack { server: ServerKey, session: String, was: Was },
}

impl Moves {
    pub fn step(&mut self, event: Event<'_>) -> Option<Effect> {
        match event {
            Event::Sent { server, session, was } => {
                self.0.push(Move { server, session, was });
                None
            }
            Event::Replied { server, session, archive, ok } => {
                let at = self.0.iter().position(|m| m.server == *server && m.session == session && matches!(m.was, Was::Listed { .. }) == archive)?;
                let Move { server, session, was } = self.0.remove(at);
                if ok {
                    return None;
                }
                // What the window did to the session after this move stood on it: those moves'
                // answers put nothing back.
                self.0.retain(|m| !(m.server == server && m.session == session));
                Some(Effect::PutBack { server, session, was })
            }
        }
    }
}

impl App {
    /// Runs `event` through the moves in flight and carries out what it asks.
    pub(crate) fn archive_move(&mut self, event: Event<'_>, tx: &mut Tx) {
        let Some(Effect::PutBack { server, session, was }) = self.sidebar_inbox.moves.step(event) else { return };
        crate::servers::runtime::with_server(self, &server, |s| match was {
            Was::Listed { tab } => {
                s.unarchive_session_locally(&session);
                crate::undo::withdraw_archived(s, &server, &session);
                match tab {
                    TabWas::Absent => {}
                    TabWas::Behind => s.put_back_closed(&session),
                    TabWas::Shown => s.reopen_closed(Some(&session), false, tx),
                }
            }
            Was::Archived(at) => {
                let Some(info) = s.sessions.iter_mut().find(|i| i.id == session) else { return };
                info.archived = Some(at);
                let info = info.clone();
                s.inbox_file(info);
            }
        });
    }

    /// The move of `session` on the server the window works against was just made and its write
    /// made (`sent`: it went).
    pub(crate) fn archive_move_sent(&mut self, session: &str, was: Was, sent: bool, tx: &mut Tx) {
        let server = crate::servers::runtime::active(self).clone();
        self.archive_move(Event::Sent { server: server.clone(), session: session.to_owned(), was }, tx);
        if self.fixture_run.stage.is_some() || !sent {
            self.archive_move(Event::Replied { server: &server, session, archive: matches!(was, Was::Listed { .. }), ok: sent }, tx);
        }
    }

    /// The server the window works against answered an archive (`archive`) or an unarchive of
    /// `session` this window sent.
    pub(crate) fn archive_move_replied(&mut self, session: &str, archive: bool, ok: bool, tx: &mut Tx) {
        let server = crate::servers::runtime::active(self).clone();
        self.archive_move(Event::Replied { server: &server, session, archive, ok }, tx);
    }
}

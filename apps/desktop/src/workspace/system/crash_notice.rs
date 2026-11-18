//! Next-launch notice after an unexpected close, matching Desktop's recovery lifecycle
//! (`packages/desktop/src/main/windows/recovery-machine.ts`, `recovery.ts`).
//!
//! Shown once after Frame 1 and `storage::is_ready()`, non-modally in `status_toast`:
//! - One true sentence per recorded `last_exit::Reason` (`OutOfMemory`, `StackOverflow`, `Fatal`);
//!   an external kill (`Previous::Interrupted`) says nothing and never guesses.
//! - When two consecutive closes while restoring the same session opened Home instead, names the
//!   skipped session and offers `Open anyway` beside `Export logs`.
//! - States `"Your drafts and open tabs were restored."` only when this launch attached and
//!   replayed an existing draft journal and restored open tabs.
//! - Once dismissed (`×`, `Escape`, or `Open anyway`), stays `State::Dismissed`.

use opencode_i18n::t;
use opencode_kit::servers::ServerKey;
use rune::Tx;

use crate::App;
use crate::launch::last_exit::{self, PollPrevious, Previous, Reason};
use crate::status_toast::NativeButton;

const MAX_TITLE_CHARS: usize = 72;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedSession {
    pub server: ServerKey,
    pub id: String,
    pub title: String,
}

/// Recovery notice lifecycle (`packages/desktop/src/main/windows/recovery-machine.ts`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum State {
    /// Waiting for `storage::is_ready()` and `last_exit::poll_previous()` after Frame 1.
    #[default]
    Waiting,
    /// `restore_ui` skipped restoring a session after two consecutive closes; waiting to present.
    WaitingSkipped(SkippedSession),
    /// Toast `toast_id` is about to bind or is currently shown in this window.
    Shown {
        toast_id: u64,
        reason: Reason,
        restored: bool,
        skipped: Option<SkippedSession>,
    },
    /// No notice owed (`Clean` / `Interrupted` / already taken), or the user dismissed it.
    Dismissed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    SkippedOnRestore(SkippedSession),
    Outcome { previous: Option<Previous>, restored: bool },
    ToastBound { id: u64 },
    OpenAnyway { id: u64 },
    ToastClosed { id: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Show {
        reason: Reason,
        restored: bool,
        skipped: Option<SkippedSession>,
    },
    OpenSession {
        server: ServerKey,
        id: String,
        toast_id: u64,
    },
}

pub fn step(state: &mut State, event: Event) -> Option<Effect> {
    match (state.clone(), event) {
        (State::Waiting, Event::SkippedOnRestore(sk)) => {
            *state = State::WaitingSkipped(sk);
            None
        }
        (State::Waiting | State::WaitingSkipped(_), Event::Outcome { previous, restored }) => {
            let kept = match state {
                State::WaitingSkipped(sk) => Some(sk.clone()),
                _ => None,
            };
            match previous {
                Some(Previous::Ended { reason, opened_home, .. }) => {
                    let skipped = kept.filter(|_| opened_home);
                    *state = State::Shown {
                        toast_id: 0,
                        reason,
                        restored,
                        skipped: skipped.clone(),
                    };
                    Some(Effect::Show { reason, restored, skipped })
                }
                _ => {
                    *state = State::Dismissed;
                    None
                }
            }
        }
        (State::Shown { reason, restored, skipped, .. }, Event::ToastBound { id }) => {
            *state = State::Shown { toast_id: id, reason, restored, skipped };
            None
        }
        (
            State::Shown { toast_id, skipped: Some(sk), .. },
            Event::OpenAnyway { id },
        ) if toast_id == id => {
            *state = State::Dismissed;
            Some(Effect::OpenSession { server: sk.server, id: sk.id, toast_id })
        }
        (State::Shown { toast_id, .. }, Event::ToastClosed { id }) if toast_id == id => {
            *state = State::Dismissed;
            None
        }
        _ => None,
    }
}

pub fn note_skipped(state: &mut State, server: ServerKey, id: String, title: String) {
    let _ = step(state, Event::SkippedOnRestore(SkippedSession { server, id, title }));
}

pub fn toast_closed(s: &mut App, id: u64) {
    let _ = step(s.kernel.slots.slot::<State>(), Event::ToastClosed { id });
}

pub fn open_anyway(s: &mut App, id: u64, tx: &mut Tx) {
    if let Some(Effect::OpenSession { server, id: sid, toast_id }) =
        step(s.kernel.slots.slot::<State>(), Event::OpenAnyway { id })
    {
        crate::status_toast::dismiss_id(s, toast_id);
        crate::servers::runtime::switch(s, &server);
        s.open(&sid, tx);
    }
}

pub fn poll(s: &mut App) -> bool {
    if !matches!(s.kernel.slots.get::<State>(), None | Some(State::Waiting | State::WaitingSkipped(_))) {
        return false;
    }
    if !crate::storage::is_ready() {
        return false;
    }
    let missing_title = match s.kernel.slots.get::<State>() {
        Some(State::WaitingSkipped(sk)) if sk.title.trim().is_empty() => Some((sk.server.clone(), sk.id.clone())),
        _ => None,
    };
    if let Some((server, id)) = missing_title {
        match lookup_session_title(s, &server, &id) {
            Some(found) => {
                if let State::WaitingSkipped(sk) = s.kernel.slots.slot::<State>() {
                    sk.title = found;
                }
            }
            None if crate::launch::waiting(s) => return false,
            None => {}
        }
    }
    let previous = match last_exit::poll_previous() {
        PollPrevious::Opening => return false,
        PollPrevious::Ready(p) => Some(p),
        PollPrevious::Taken => None,
    };
    let restored = crate::storage::drafts_replayed() && !s.tabs.is_empty();
    let Some(Effect::Show { reason, restored, skipped }) =
        step(s.kernel.slots.slot::<State>(), Event::Outcome { previous, restored })
    else {
        return false;
    };

    let title = t!("rune.crash.title").to_owned();
    let description = match (reason, &skipped) {
        (Reason::OutOfMemory, None) => t!("rune.crash.outOfMemory").to_owned(),
        (Reason::StackOverflow, None) => t!("rune.crash.stackOverflow").to_owned(),
        (Reason::Fatal, None) => t!("rune.crash.fatal").to_owned(),
        (Reason::OutOfMemory, Some(sk)) => {
            t!("rune.crash.outOfMemory.skipped", session = display_session_name(s, sk))
        }
        (Reason::StackOverflow, Some(sk)) => {
            t!("rune.crash.stackOverflow.skipped", session = display_session_name(s, sk))
        }
        (Reason::Fatal, Some(sk)) => {
            t!("rune.crash.fatal.skipped", session = display_session_name(s, sk))
        }
    };
    let restored_line = restored.then(|| t!("rune.crash.restored").to_owned());
    let buttons = if skipped.is_some() {
        vec![NativeButton::OpenSkippedSession, NativeButton::ExportLogs]
    } else {
        vec![NativeButton::ExportLogs]
    };
    let id = crate::status_toast::crash_notice(s, title, description, restored_line, buttons);
    let _ = step(s.kernel.slots.slot::<State>(), Event::ToastBound { id });
    true
}

fn lookup_session_title(s: &App, server: &ServerKey, id: &str) -> Option<String> {
    s.tabs
        .iter()
        .find(|t| t.is(server, id) && !t.title.trim().is_empty())
        .map(|t| t.title.trim().to_owned())
        .or_else(|| {
            s.sessions
                .iter()
                .find(|x| x.id == id && !x.title.trim().is_empty())
                .map(|x| x.title.trim().to_owned())
        })
}

fn display_session_name(s: &App, sk: &SkippedSession) -> String {
    let raw = if !sk.title.trim().is_empty() {
        sk.title.trim().to_owned()
    } else {
        lookup_session_title(s, &sk.server, &sk.id)
            .unwrap_or_else(|| t!("rune.session.untitled").to_owned())
    };
    if raw.chars().count() <= MAX_TITLE_CHARS {
        raw
    } else {
        let mut out: String = raw.chars().take(MAX_TITLE_CHARS.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

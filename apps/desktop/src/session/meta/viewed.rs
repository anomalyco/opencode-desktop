//! What this window's user has seen, reported to the server so every client clears its unread
//! dot (`POST /api/session/:id/view`), as Desktop's `MarkSessionViewed`
//! (`packages/app/src/session/route.tsx`) and its per-root report
//! (`packages/app/src/shell/notifications/{notification.tsx,machine.ts}`) do.
//!
//! Viewed state is shared by every client, so only a focused window reports what its user has
//! seen. The routed session's root is reported whenever it has an idle mark it isn't viewed up
//! to, so a run finishing while it stays open is reported too.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use opencode_kernel::Slot;
use opencode_kit::client::{Ticket, TicketKind, Write};
use opencode_kit::model::SessionInfo;
use opencode_kit::servers::ServerKey;
use opencode_kit::wire_error::{WireError, WireKind};
use rune::Tx;

use crate::{App, Route, Source};

/// `VIEW_RETRY_DELAY`, `VIEW_RETRY_MAX_DELAY` and `VIEW_RETRY_LIMIT` (`shell/notifications/machine.ts`).
const RETRY_DELAY_MS: i64 = 250;
const RETRY_MAX_DELAY_MS: i64 = 5_000;
const RETRY_LIMIT: u32 = 6;

pub static VIEWING: Slot<Viewing> = Slot::new();

/// The window's routed session and whether its user has seen it (`session/viewed-machine.ts`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Viewing {
    #[default]
    Nothing,
    /// Routed to `session`, but the window isn't focused or manual unread marks aren't loaded.
    Waiting { session: String },
    Viewed { session: String },
}

impl Viewing {
    /// `transitionSessionViewed`: `true` when the session has just been viewed.
    fn step(&mut self, session: Option<&str>, focused: bool, ready: bool) -> bool {
        let Some(session) = session else {
            *self = Viewing::Nothing;
            return false;
        };
        match self {
            Viewing::Viewed { session: s } if s == session => false,
            _ if focused && ready => {
                *self = Viewing::Viewed { session: session.to_owned() };
                true
            }
            Viewing::Waiting { session: s } if s == session => false,
            _ => {
                *self = Viewing::Waiting { session: session.to_owned() };
                false
            }
        }
    }
}

/// One root session's report (`ViewReportState`). `attempt` names one request, so a result for
/// a watermark a newer one replaced never settles it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Report {
    Unreported { attempt: u32 },
    /// The request in flight, or the wait before a retry.
    Reporting { idle: i64, attempt: u32, failures: u32, leg: Leg },
    Reported { idle: i64, attempt: u32 },
    /// Gave up; a later view or the connection coming back starts it again.
    Failed { idle: i64, attempt: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Leg {
    Sending,
    Backoff { due: i64 },
}

#[derive(Clone, Copy, Debug)]
enum Event {
    View { idle: i64 },
    Succeeded { attempt: u32 },
    Failed { attempt: u32, not_found: bool, now: i64 },
    Due { now: i64 },
    Connected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Effect {
    Send { idle: i64, attempt: u32 },
    /// The session is gone from the server.
    Forget,
}

impl Report {
    /// `transitionViewReport`.
    fn step(&mut self, event: Event) -> Option<Effect> {
        match (*self, event) {
            // Only a newer watermark starts a new report; one that gave up starts over on any view.
            (Report::Reporting { idle, .. } | Report::Reported { idle, .. }, Event::View { idle: next }) if next <= idle => None,
            (Report::Failed { idle, .. }, Event::View { idle: next }) => self.report(idle.max(next)),
            (_, Event::View { idle }) => self.report(idle),
            (Report::Failed { idle, .. }, Event::Connected) => self.report(idle),
            (Report::Reporting { idle, attempt, .. }, Event::Succeeded { attempt: a }) if a == attempt => {
                *self = Report::Reported { idle, attempt };
                None
            }
            (Report::Reporting { attempt, .. }, Event::Failed { attempt: a, not_found: true, .. }) if a == attempt => {
                *self = Report::Unreported { attempt };
                Some(Effect::Forget)
            }
            (Report::Reporting { idle, attempt, failures, .. }, Event::Failed { attempt: a, now, .. }) if a == attempt => {
                if failures >= RETRY_LIMIT {
                    *self = Report::Failed { idle, attempt };
                    return None;
                }
                let failures = failures + 1;
                let delay = (RETRY_DELAY_MS << (failures - 1)).min(RETRY_MAX_DELAY_MS);
                *self = Report::Reporting { idle, attempt, failures, leg: Leg::Backoff { due: now + delay } };
                None
            }
            (Report::Reporting { idle, attempt, failures, leg: Leg::Backoff { due } }, Event::Due { now }) if due <= now => {
                *self = Report::Reporting { idle, attempt, failures, leg: Leg::Sending };
                Some(Effect::Send { idle, attempt })
            }
            _ => None,
        }
    }

    fn report(&mut self, idle: i64) -> Option<Effect> {
        let attempt = self.attempt() + 1;
        *self = Report::Reporting { idle, attempt, failures: 0, leg: Leg::Sending };
        Some(Effect::Send { idle, attempt })
    }

    fn attempt(self) -> u32 {
        match self {
            Report::Unreported { attempt } | Report::Reporting { attempt, .. } | Report::Reported { attempt, .. } | Report::Failed { attempt, .. } => attempt,
        }
    }

    fn due(self) -> Option<i64> {
        match self {
            Report::Reporting { leg: Leg::Backoff { due }, .. } => Some(due),
            _ => None,
        }
    }
}

thread_local! {
    /// Per server and root session, shared by every window (they all run on the UI thread), so a
    /// failed report retries whichever window is looking.
    static REPORTS: RefCell<HashMap<(ServerKey, String), Report>> = RefCell::new(HashMap::new());
}

fn dispatch(s: &mut App, root: &str, event: Event) {
    let key = (crate::servers::runtime::active(s).clone(), root.to_owned());
    let effect = REPORTS.with_borrow_mut(|reports| reports.entry(key.clone()).or_insert(Report::Unreported { attempt: 0 }).step(event));
    match effect {
        Some(Effect::Send { idle, attempt }) => {
            if !send(s, root, idle, attempt) {
                let now = *s.now_ms;
                dispatch(s, root, Event::Failed { attempt, not_found: false, now });
            }
        }
        // Desktop drops its copy (`data.session.forget`); the `session.deleted` that comes with it
        // takes the session out here, so only the report goes.
        Some(Effect::Forget) => {
            REPORTS.with_borrow_mut(|reports| reports.remove(&key));
        }
        None => {}
    }
}

/// Sends the report quietly: a window that can't write reports a failure rather than a toast.
fn send(s: &App, root: &str, idle: i64, attempt: u32) -> bool {
    let Source::Live { writes: Some(writes), allow_send: true, .. } = &*s.source else { return false };
    let ticket = Ticket { kind: TicketKind::View, session: root.to_owned(), attempt };
    writes.send(Write::Tracked { ticket, write: Box::new(Write::View { session: root.to_owned(), idle }) }).is_ok()
}

/// Reports `id`'s root as viewed up to its latest idle mark when it isn't viewed up to it yet
/// (`markViewed`). The window shows it read at once.
pub fn mark(s: &mut App, id: &str) {
    let root = s.root_of(id).to_owned();
    let Some(idle) = s.session(&root).filter(|i| i.unread()).and_then(|i| i.idle) else { return };
    for info in s.sessions.iter_mut().filter(|i| i.id == root) {
        info.viewed = Some(idle);
    }
    crate::windows::peer::touch(s);
    if matches!(*s.source, Source::Live { .. }) && s.fixture_run.stage.is_none() {
        dispatch(s, &root, Event::View { idle });
    }
}

/// A report's `Incoming::Settled`.
pub fn settled(s: &mut App, root: &str, attempt: u32, result: Result<Option<SessionInfo>, WireError>) {
    let event = match result {
        Ok(_) => Event::Succeeded { attempt },
        Err(e) => {
            let not_found = matches!(&e.kind, WireKind::Status { tag: Some(tag), .. } if tag == "SessionNotFoundError");
            Event::Failed { attempt, not_found, now: *s.now_ms }
        }
    };
    dispatch(s, root, event);
}

/// The server answers again: reports that gave up while it was away go again.
pub fn connected(s: &mut App) {
    let server = crate::servers::runtime::active(s).clone();
    let roots: Vec<String> = REPORTS.with_borrow(|reports| reports.keys().filter(|(k, _)| *k == server).map(|(_, r)| r.clone()).collect());
    for root in roots {
        dispatch(s, &root, Event::Connected);
    }
}

/// Each frame on a checked-out server: sends the reports whose retry is due.
pub fn poll_retries(s: &mut App, tx: &mut Tx) {
    let server = crate::servers::runtime::active(s).clone();
    let now = *s.now_ms;
    let waiting: Vec<(String, i64)> = REPORTS.with_borrow(|reports| {
        reports.iter().filter(|((k, _), _)| *k == server).filter_map(|((_, root), r)| r.due().map(|due| (root.clone(), due))).collect()
    });
    for (root, due) in waiting {
        if due <= now {
            dispatch(s, &root, Event::Due { now });
        } else {
            tx.wake_after(Duration::from_millis(u64::try_from(due - now).unwrap_or(0)));
        }
    }
}

/// Each frame in the window: marks the routed session viewed while the window is focused, and
/// clears its manual unread mark once it's first seen (`MarkSessionViewed`).
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    poll_retries(s, tx);
    let session = match &*s.route {
        Route::Session { server, id } if *server == crate::servers::runtime::current() => Some(id.clone()),
        _ => None,
    };
    // The browser build gets no window focus events; its page is the window.
    let focused = cfg!(target_arch = "wasm32") || s.window.focused;
    let ready = crate::session_menu::unread::ready(s);
    let opened = s.kernel.slots.get_mut(&VIEWING).step(session.as_deref(), focused, ready);
    let Some(id) = session.filter(|_| focused) else { return opened };
    if opened {
        let root = s.root_of(&id).to_owned();
        crate::session_menu::unread::set(s, &crate::servers::runtime::current(), &root, false);
    }
    let unread = s.session(s.root_of(&id)).is_some_and(SessionInfo::unread);
    if unread {
        mark(s, &id);
    }
    opened || unread
}

//! What a terminal's worker owes the UI, as an explicit machine ([`Publish`]), and the hand-off
//! of snapshots between them ([`Handoff`]).
//!
//! The worker extracts a snapshot only when the UI has read the last one, so it extracts at most
//! once per frame the UI draws, and a terminal nobody draws (a hidden tab, a window that isn't
//! painting) costs one extraction after its last frame and nothing more: its output is still fed
//! to the emulator, but no rows are built and the UI is never woken for it. Reading a snapshot
//! the worker fell behind on wakes the worker; when the reader hadn't looked for a while (a tab
//! coming back), it waits for the fresh one so the first frame shows the screen as it is.

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use rune::time::Instant;

/// How long a program may hold its frame (synchronized output, `?2026`) before it shows anyway.
pub(crate) const RENDER_HOLD_TIMEOUT: Duration = Duration::from_secs(1);

/// The snapshot slot between worker and UI.
#[derive(Default)]
pub(crate) struct Handoff(AtomicU8);

/// The UI read the last snapshot: the next change publishes at once.
const READ: u8 = 0;
/// A snapshot waits for the UI.
const UNREAD: u8 = 1;
/// A snapshot waits for the UI, and the screen changed after it: reading it must wake the worker.
const WANTED: u8 = 2;

pub(crate) enum Claim {
    /// The slot is free: extract and publish.
    Publish,
    /// The UI hasn't read the last snapshot: the worker is behind until it does.
    Wait,
}

impl Handoff {
    /// The worker has a change to show.
    pub(crate) fn claim(&self) -> Claim {
        loop {
            match self.0.compare_exchange(READ, UNREAD, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Claim::Publish,
                Err(WANTED) => return Claim::Wait,
                Err(_) => {
                    if self.0.compare_exchange(UNREAD, WANTED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                        return Claim::Wait;
                    }
                }
            }
        }
    }

    /// The worker published its last screen whatever the UI had read (it is ending).
    pub(crate) fn published(&self) {
        self.0.store(UNREAD, Ordering::Release);
    }

    /// The UI read the snapshot; true when the worker is waiting for that.
    pub(crate) fn read(&self) -> bool {
        self.0.swap(READ, Ordering::AcqRel) == WANTED
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Publish {
    /// The UI has, or can read, the screen as it is.
    Current,
    /// The screen changed after the snapshot the UI hasn't read yet.
    Behind,
    /// The program holds its frame (`?2026`) since `since`.
    Held { since: Instant },
}

/// What the worker does after a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Nothing,
    /// Extract a snapshot and hand it over.
    Publish,
}

/// What happened to the worker's screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// Output, scrolling, selection, a resize or a theme changed the screen; `held` is whether the
    /// program is holding its frame.
    Changed { held: bool },
    /// The worker woke: the UI may have read the snapshot, or a hold may have timed out.
    Woke { held: bool },
}

impl Publish {
    /// When the worker must wake without input: the end of a hold.
    pub(crate) fn deadline(self) -> Option<Instant> {
        match self {
            Publish::Held { since } => Some(since + RENDER_HOLD_TIMEOUT),
            Publish::Current | Publish::Behind => None,
        }
    }

    pub(crate) fn step(self, event: Event, now: Instant, handoff: &Handoff) -> (Publish, Action) {
        let show = || match handoff.claim() {
            Claim::Publish => (Publish::Current, Action::Publish),
            Claim::Wait => (Publish::Behind, Action::Nothing),
        };
        match (self, event) {
            (Publish::Held { since }, Event::Changed { held: true } | Event::Woke { held: true })
                if now.duration_since(since) < RENDER_HOLD_TIMEOUT =>
            {
                (self, Action::Nothing)
            }
            (Publish::Held { .. }, _) => show(),
            (_, Event::Changed { held: true }) => (Publish::Held { since: now }, Action::Nothing),
            (_, Event::Changed { held: false }) => show(),
            (Publish::Behind, Event::Woke { .. }) => show(),
            (Publish::Current, Event::Woke { .. }) => (self, Action::Nothing),
        }
    }
}

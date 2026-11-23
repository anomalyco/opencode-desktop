//! The launch's follow-up reads settling in. The first reads land at once (`first_reads`), but
//! connecting asks for more, one after another on the connection's fetch thread: the info of tabs
//! past the listed page, running sessions' families, the restored session's own reads and every
//! Git project's worktrees. Each answer was its own animated commit, so against a remote server
//! sessions flew into the sidebar a round trip apart, and tabs flew from "Other" into their
//! projects. While they settle in, the session list's answers land as the first reads do.

#![forbid(unsafe_code)]

/// How long after connecting the list's answers count as the launch's.
pub const SETTLE: f64 = 3.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Settle {
    /// Not connected yet: the first reads land on their own.
    #[default]
    Before,
    /// Connected at `until - SETTLE`: the launch's reads are still answering.
    Settling { until: f64 },
    /// Past the launch: changes animate.
    Live,
}

pub enum SettleEvent {
    /// A server's first reads landed and its connection asked for the rest (`boot::connect`).
    Connected { now: f64 },
    /// An answer to the session list (sessions, a session's info, a project's worktrees).
    Answer { now: f64 },
}

#[derive(Debug, PartialEq, Eq)]
pub enum SettleEffect {
    /// The answer lands without animating.
    Instant,
    Animate,
}

impl Settle {
    pub fn step(&mut self, event: SettleEvent) -> SettleEffect {
        match (*self, event) {
            (_, SettleEvent::Connected { now }) => {
                *self = Settle::Settling { until: now + SETTLE };
                SettleEffect::Animate
            }
            (Settle::Settling { until }, SettleEvent::Answer { now }) if now < until => SettleEffect::Instant,
            (Settle::Settling { .. }, SettleEvent::Answer { .. }) => {
                *self = Settle::Live;
                SettleEffect::Animate
            }
            (Settle::Before | Settle::Live, SettleEvent::Answer { .. }) => SettleEffect::Animate,
        }
    }
}

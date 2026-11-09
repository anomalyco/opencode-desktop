//! Closing the side region's last tab: the tab stays drawn while the region slides shut on
//! `SIDE_SPRING`, so the flight shows what it closes rather than an empty pane, and lets go once
//! the flight has come to rest. The official app has no machine for this.

use std::time::Duration;

use super::state::ResolvedEntry;
use super::view::SIDE_REST;
use super::SessionPane;

#[derive(Default)]
pub enum State {
    #[default]
    Idle,
    /// `entry`, the session's last side tab, rides the slide out until `until` (`App::clock`).
    Sliding { session_id: String, entry: ResolvedEntry, until: f64 },
}

pub enum Event {
    /// The session's last side tab closed at `at`, and its region started sliding shut.
    LastTabClosed { session_id: String, entry: ResolvedEntry, at: f64 },
    /// A frame's poll at `clock`; `shut` is whether the sliding session's region is still shut
    /// and empty, which a tab shown in it or the window leaving the session ends.
    Frame { clock: f64, shut: bool },
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    /// Poll again this long from now, when the flight has come to rest.
    WakeAfter(Duration),
    /// The held tab let go: draw the region without it.
    Redraw,
}

impl State {
    pub fn step(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::LastTabClosed { session_id, entry, at } => {
                *self = State::Sliding { session_id, entry, until: at + SIDE_REST };
                vec![Effect::WakeAfter(Duration::from_secs_f64(SIDE_REST))]
            }
            Event::Frame { clock, shut } => match self {
                State::Sliding { until, .. } if clock >= *until || !shut => {
                    *self = State::Idle;
                    vec![Effect::Redraw]
                }
                _ => Vec::new(),
            },
        }
    }

    /// The session's sliding session id, while one is.
    pub fn session(&self) -> Option<&str> {
        match self {
            State::Sliding { session_id, .. } => Some(session_id),
            State::Idle => None,
        }
    }

    /// The tab `session`'s `pane` holds while its region slides shut.
    pub fn held(&self, session: &str, pane: SessionPane) -> Option<&ResolvedEntry> {
        match self {
            State::Sliding { session_id, entry, .. } if session_id == session && entry.pane == pane => Some(entry),
            _ => None,
        }
    }
}

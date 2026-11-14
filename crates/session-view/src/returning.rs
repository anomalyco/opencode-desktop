//! A reading place coming back as its session opens again (`jump::Ends::reopen`): after a
//! relaunch, from the list, from another tab. The timeline holds its loading look until its
//! transcript has the place's row, then lands the place in that commit, so the place is the first
//! thing it draws and nothing moves after it. A session read at its end has no place: it opens on
//! its end and follows it.
//!
//! - `Idle`: no place is coming.
//! - `Due`: the place waits for its timeline; no poll has found its row missing yet.
//! - `Waiting`: a poll found the row missing at `since`. Older pages are read on up to it, a page
//!   at a time; each page that comes without it waits again from the next poll ([`Event::Page`]).
//!   One that hasn't come [`WAIT`] after the last page is given up: the timeline shows its end.
//!
//! Desktop keeps a detached timeline only while the app runs (`session/timeline/cache.ts`) and
//! has no machine for a place that outlives it.

/// How long a place waits for its row after the last page that came without it, in seconds.
pub const WAIT: f64 = 3.0;

/// Whether a place is coming back, and which.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Returning {
    #[default]
    Idle,
    /// Session `session`'s row `row`, `at` below the viewport's top.
    Due { session: String, row: String, at: f32 },
    Waiting { session: String, row: String, at: f32, since: f64 },
}

#[derive(Clone, Copy, Debug)]
pub enum Event<'a> {
    /// Session `session` opens again, read up from its end at `place` (`None`: at its end).
    Reopen { session: &'a str, place: Option<(&'a str, f32)> },
    /// Session `session`'s transcript has the place's row.
    Here { session: &'a str },
    /// A poll at `now` found session `session`'s row missing.
    Missing { session: &'a str, now: f64 },
    /// A page came for session `session` without the row.
    Page { session: &'a str },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Put the row `at` below the viewport's top in this commit.
    Land { row: String, at: f32 },
    /// Read the page above what's loaded, and poll again soon.
    ReadOn,
    /// The row never came: the session's timeline shows its end and follows it.
    GiveUp,
}

impl Returning {
    /// The session whose place is coming, and its row.
    pub fn due(&self) -> Option<(&str, &str)> {
        match self {
            Returning::Idle => None,
            Returning::Due { session, row, .. } | Returning::Waiting { session, row, .. } => Some((session, row)),
        }
    }
}

/// One transition. An event for a session whose place isn't the one coming changes nothing.
pub fn step(state: &mut Returning, event: Event) -> Option<Effect> {
    let ours = |s: &str| state.due().is_some_and(|(due, _)| due == s);
    match event {
        Event::Reopen { session, place } => {
            *state = match place {
                Some((row, at)) => Returning::Due { session: session.to_owned(), row: row.to_owned(), at },
                None => Returning::Idle,
            };
            None
        }
        Event::Here { session } if ours(session) => match std::mem::take(state) {
            Returning::Due { row, at, .. } | Returning::Waiting { row, at, .. } => Some(Effect::Land { row, at }),
            Returning::Idle => None,
        },
        Event::Missing { session, now } if ours(session) => match std::mem::take(state) {
            Returning::Due { session, row, at } => {
                *state = Returning::Waiting { session, row, at, since: now };
                Some(Effect::ReadOn)
            }
            Returning::Waiting { since, .. } if now - since >= WAIT => Some(Effect::GiveUp),
            waiting => {
                *state = waiting;
                Some(Effect::ReadOn)
            }
        },
        Event::Page { session } if ours(session) => {
            if let Returning::Waiting { session, row, at, .. } = state {
                *state = Returning::Due { session: std::mem::take(session), row: std::mem::take(row), at: *at };
            }
            None
        }
        _ => None,
    }
}

//! State of a native AppKit pop-up menu (`popup::NATIVE`).
//!
//! When an open `NSMenu` is dismissed by pressing its own trigger button again, AppKit ends its
//! tracking loop (`picked = None`) and reposts that `leftMouseDown` to the window underneath. On a
//! trigger with [`rune::El::press_on_down`], that reposted press would immediately pop a second
//! menu unless the host recognises it as the closing press of the menu that just left.

use std::time::Duration;

/// How long after an unpicked native dismissal a reposted pointer press on the same anchor counts
/// as the press that closed it.
const REPOST_WINDOW_S: f64 = 0.08;

#[derive(Debug, Default, PartialEq)]
pub enum State {
    #[default]
    Closed,
    /// `tx.popup_menu` was asked to open on `anchor` (if keyed) and has not yet resolved.
    Open { anchor: Option<String> },
    /// `anchor`'s menu closed without a pick; until `due`, a pointer press on `anchor` is the
    /// reposted press that dismissed it.
    Dismissing { anchor: String, due: f64 },
}

pub enum Event<'a> {
    OpenRequested { anchor: Option<&'a str>, by_keys: bool },
    MenuClosed { picked: bool, now: f64 },
    Tick { now: f64 },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    Show,
    SwallowReposted,
    WakeAfter(Duration),
    None,
}

pub fn step(state: &mut State, event: Event<'_>) -> Effect {
    match event {
        Event::OpenRequested { anchor: Some(a), by_keys: false }
            if matches!(state, State::Open { anchor: Some(open) } | State::Dismissing { anchor: open, .. } if open == a) =>
        {
            *state = State::Closed;
            Effect::SwallowReposted
        }
        Event::OpenRequested { anchor, .. } => {
            *state = State::Open { anchor: anchor.map(str::to_owned) };
            Effect::Show
        }
        Event::MenuClosed { picked: false, now } => match std::mem::take(state) {
            State::Open { anchor: Some(anchor) } => {
                *state = State::Dismissing { anchor, due: now + REPOST_WINDOW_S };
                Effect::WakeAfter(Duration::from_secs_f64(REPOST_WINDOW_S))
            }
            _ => Effect::None,
        },
        Event::MenuClosed { picked: true, .. } => {
            *state = State::Closed;
            Effect::None
        }
        Event::Tick { now } => {
            if matches!(state, State::Dismissing { due, .. } if now >= *due) {
                *state = State::Closed;
            }
            Effect::None
        }
    }
}

//! One attachment to a persistent PTY, and the size its pane asks for, as explicit machines:
//! Desktop's `gui-plugins/src/terminal/connection-machine.ts` (`ConnectionState`) and the
//! sizing half of `pane-machine.ts` (`PaneSizing`). The worker (`persistent_pty.rs`) feeds them
//! the socket's messages, the pane's measurements and the user's input, and carries out what
//! they return; an attachment never reconnects (the panel mounts a new one, `surface-machine.ts`).

use std::time::Duration;
use rune::time::Instant;

use rune::machine::Effects;

/// Columns and rows.
pub(crate) type Size = (u16, u16);

/// Desktop waits this long after the pane's last size change before it asks for the size
/// (`TERMINAL_SIZE_DEBOUNCE_MS`).
pub(crate) const SIZE_DEBOUNCE: Duration = Duration::from_millis(100);

/// Why an attachment ended (`TerminalFailure`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    Disconnected,
    Failed,
    Outdated,
}

/// Input and a wish for control held until the pane has a size.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Held {
    wants_control: bool,
    input: Vec<Vec<u8>>,
}

/// A live attachment's pane: unmeasured (holding what can't go out yet), or measured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pane {
    Unsized(Held),
    Sized(Size),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Connection {
    /// Waiting for `attached`; the snapshot already painted the screen.
    Restoring { desired: Option<Size>, held: Held },
    /// Attached: the replay streams until `replay_complete`.
    Replaying { controller: bool, desired: Option<Size>, held: Held },
    /// Input and size frames go out at the pane's size.
    Live { controller: bool, pane: Pane },
    /// The shell exited: nothing more goes out, and the socket ending is no failure.
    Exited { code: Option<u32> },
    Failed(Failure),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Attached { controller: bool, protocol: Option<u64>, size: Option<Size> },
    Resized { size: Size, checkpoint: Option<Vec<u8>> },
    ReplayComplete,
    ControllerChanged { ours: bool },
    Exited { code: Option<u32> },
    /// Encoded input from the user.
    Input(Vec<u8>),
    /// The user pressed on the pane, which takes control at its size.
    Interacted,
    /// The pane's size, once its debounce settled (`size.changed`).
    SizeChanged(Size),
    SocketClosed,
    SocketErrored,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// The server's canonical size: the emulator takes it, after a reset when `reset`, then
    /// the checkpoint.
    Resize { size: Size, checkpoint: Option<Vec<u8>>, reset: bool },
    /// A frame for the server: the pane's size alone takes control; with input, it types.
    Send { size: Size, input: Option<Vec<u8>> },
    Fail(Failure),
}

impl Connection {
    pub(crate) fn new() -> Connection {
        Connection::Restoring { desired: None, held: Held::default() }
    }

    /// Socket output still reaches the screen until the attachment fails (`canAcceptOutput`).
    pub(crate) fn accepts_output(&self) -> bool {
        !matches!(self, Connection::Failed(_))
    }

    /// `transitionConnection`.
    pub(crate) fn step(&mut self, event: Event) -> Effects<Effect> {
        let state = std::mem::replace(self, Connection::Failed(Failure::Failed));
        let (next, effects) = transition(state, event);
        *self = next;
        effects
    }
}

fn transition(state: Connection, event: Event) -> (Connection, Effects<Effect>) {
    use Connection::*;
    match (state, event) {
        (state @ (Exited { .. } | Failed(_)), Event::SocketClosed | Event::SocketErrored) => (state, Effects::none()),
        (_, Event::SocketClosed) => fail(Failure::Disconnected),
        (_, Event::SocketErrored) => fail(Failure::Failed),
        (Restoring { desired, held }, Event::Attached { controller, protocol, size }) => {
            if protocol != Some(1) {
                return fail(Failure::Outdated);
            }
            let effects = size.map_or(Effects::none(), |size| Effects::one(Effect::Resize { size, checkpoint: None, reset: false }));
            (Replaying { controller, desired, held }, effects)
        }
        (state @ (Replaying { .. } | Live { .. }), Event::Resized { size, checkpoint }) => {
            (state, Effects::one(Effect::Resize { size, checkpoint, reset: true }))
        }
        // The replay is written as it arrives, so its end is drained at once (`replay.drained`).
        (Replaying { controller, desired: None, held }, Event::ReplayComplete) => (Live { controller, pane: Pane::Unsized(held) }, Effects::none()),
        (Replaying { controller, desired: Some(size), held }, Event::ReplayComplete) => {
            (Live { controller, pane: Pane::Sized(size) }, flush(size, controller, held))
        }
        (Replaying { .. } | Live { .. }, Event::Exited { code }) => (Exited { code }, Effects::none()),
        (Replaying { desired, held, .. }, Event::ControllerChanged { ours }) => (Replaying { controller: ours, desired, held }, Effects::none()),
        (Live { controller, pane }, Event::ControllerChanged { ours }) if controller != ours => {
            let effects = match &pane {
                Pane::Sized(size) if ours => Effects::one(Effect::Send { size: *size, input: None }),
                _ => Effects::none(),
            };
            (Live { controller: ours, pane }, effects)
        }
        (Restoring { desired, mut held }, Event::Input(data)) if !data.is_empty() => {
            held.input.push(data);
            (Restoring { desired, held }, Effects::none())
        }
        (Replaying { controller, desired, mut held }, Event::Input(data)) if !data.is_empty() => {
            held.input.push(data);
            (Replaying { controller, desired, held }, Effects::none())
        }
        (Live { controller, pane: Pane::Sized(size) }, Event::Input(data)) if !data.is_empty() => {
            (Live { controller, pane: Pane::Sized(size) }, Effects::one(Effect::Send { size, input: Some(data) }))
        }
        (Live { controller, pane: Pane::Unsized(mut held) }, Event::Input(data)) if !data.is_empty() => {
            held.input.push(data);
            (Live { controller, pane: Pane::Unsized(held) }, Effects::none())
        }
        (Restoring { desired, mut held }, Event::Interacted) => {
            held.wants_control = true;
            (Restoring { desired, held }, Effects::none())
        }
        (Replaying { controller, desired, mut held }, Event::Interacted) => {
            held.wants_control = true;
            (Replaying { controller, desired, held }, Effects::none())
        }
        (Live { controller, pane: Pane::Sized(size) }, Event::Interacted) => {
            (Live { controller, pane: Pane::Sized(size) }, Effects::one(Effect::Send { size, input: None }))
        }
        (Live { controller, pane: Pane::Unsized(mut held) }, Event::Interacted) => {
            held.wants_control = true;
            (Live { controller, pane: Pane::Unsized(held) }, Effects::none())
        }
        (Restoring { held, .. }, Event::SizeChanged(size)) => (Restoring { desired: Some(size), held }, Effects::none()),
        (Replaying { controller, held, .. }, Event::SizeChanged(size)) => (Replaying { controller, desired: Some(size), held }, Effects::none()),
        (Live { controller, pane: Pane::Unsized(held) }, Event::SizeChanged(size)) => {
            (Live { controller, pane: Pane::Sized(size) }, flush(size, controller, held))
        }
        (Live { controller, pane: Pane::Sized(was) }, Event::SizeChanged(size)) if was != size => {
            let effects = if controller { Effects::one(Effect::Send { size, input: None }) } else { Effects::none() };
            (Live { controller, pane: Pane::Sized(size) }, effects)
        }
        (state, _) => (state, Effects::none()),
    }
}

/// Sends the input held until the pane had a size, or else takes control when this attachment
/// controls or asked to.
fn flush(size: Size, controller: bool, held: Held) -> Effects<Effect> {
    if !held.input.is_empty() {
        let mut effects = Effects::none();
        effects.extend(held.input.into_iter().map(|data| Effect::Send { size, input: Some(data) }));
        return effects;
    }
    if controller || held.wants_control { Effects::one(Effect::Send { size, input: None }) } else { Effects::none() }
}

fn fail(failure: Failure) -> (Connection, Effects<Effect>) {
    (Connection::Failed(failure), Effects::one(Effect::Fail(failure)))
}

/// The size the pane asks its PTY for (`pane-machine.ts` `PaneSizing`): the first measurement
/// applies at once; a later one waits out [`SIZE_DEBOUNCE`], and coming back to the size
/// already asked for drops it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sizing {
    Unmeasured,
    Sized { size: Size, pending: Option<(Size, Instant)> },
}

impl Sizing {
    /// The pane measured `proposed` at `now`; returns the size to apply at once, if any.
    pub(crate) fn sampled(&mut self, proposed: Size, now: Instant) -> Option<Size> {
        match *self {
            Sizing::Unmeasured => {
                *self = Sizing::Sized { size: proposed, pending: None };
                Some(proposed)
            }
            Sizing::Sized { size, .. } if size == proposed => {
                *self = Sizing::Sized { size, pending: None };
                None
            }
            Sizing::Sized { pending: Some((p, _)), .. } if p == proposed => None,
            Sizing::Sized { size, .. } => {
                *self = Sizing::Sized { size, pending: Some((proposed, now)) };
                None
            }
        }
    }

    /// When a pending size settles.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        match *self {
            Sizing::Sized { pending: Some((_, since)), .. } => Some(since + SIZE_DEBOUNCE),
            _ => None,
        }
    }

    /// The debounce settled by `now`: returns the size to apply (`size.debounced`).
    pub(crate) fn due(&mut self, now: Instant) -> Option<Size> {
        match *self {
            Sizing::Sized { pending: Some((size, since)), .. } if now.duration_since(since) >= SIZE_DEBOUNCE => {
                *self = Sizing::Sized { size, pending: None };
                Some(size)
            }
            _ => None,
        }
    }
}

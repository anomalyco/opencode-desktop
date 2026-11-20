//! Media control visibility from packages/gui-plugins/src/file/artifact-view-machine.ts.

/// `VIDEO_IDLE_MS`: how long a playing clip keeps its controls after the pointer last moved.
const IDLE: f64 = 2.5;

/// Desktop's `VideoOverlayState` without the preview's `expanded`: loading until the first frame,
/// then paused, or playing with the controls up until `due`, or playing with them down.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Overlay {
    #[default]
    Loading,
    Paused,
    /// Playing, the pointer moved lately: the controls go at `due` on the window clock.
    Active { due: f64 },
    /// Playing, the pointer rested or left.
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// The clip has a frame to show (`media.loaded`).
    Loaded { paused: bool, now: f64 },
    /// It started or stopped playing (`playback.changed`).
    Playback { paused: bool, now: f64 },
    /// The pointer moved over the stage (`pointer.moved`).
    Moved { now: f64 },
    /// The pointer left the stage (`pointer.left`).
    Left,
    /// The window clock reached `now` (`idle.elapsed`).
    Tick { now: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    WakeAt(f64),
}

impl Overlay {
    pub fn shown(self) -> bool {
        matches!(self, Self::Paused | Self::Active { .. })
    }
}

/// `transitionVideoOverlay`, as one transition. A later move's timer replaces an earlier one's:
/// the state keeps only the newest `due`, so an older timer's tick changes nothing.
pub fn step(state: &mut Overlay, event: Event) -> Option<Effect> {
    let active = |now: f64| (Overlay::Active { due: now + IDLE }, Some(Effect::WakeAt(now + IDLE)));
    let (next, effect) = match (*state, event) {
        (Overlay::Loading, Event::Loaded { paused: true, .. }) => (Overlay::Paused, None),
        (Overlay::Loading, Event::Loaded { paused: false, now }) => active(now),
        (Overlay::Paused, Event::Playback { paused: false, now }) => active(now),
        (Overlay::Active { .. } | Overlay::Idle, Event::Playback { paused: true, .. }) => (Overlay::Paused, None),
        (Overlay::Active { .. } | Overlay::Idle, Event::Moved { now }) => active(now),
        (Overlay::Active { .. }, Event::Left) => (Overlay::Idle, None),
        (Overlay::Active { due }, Event::Tick { now }) if now >= due => (Overlay::Idle, None),
        _ => return None,
    };
    *state = next;
    effect
}

//! One canvas's drawing as an explicit machine. A canvas draws once it has a size and shows,
//! again on every change (a step, a resize, the pointer), and every `1 / fps` while it animates
//! (until a frame with `t >= duration`, when it has one). A draw that fails or runs over budget
//! keeps the last frame; three in a row freeze the canvas on it.
//!
//! ```text
//!            draw ok, animating            draw ok, done
//! Due ──────────────────────────▶ Animating ─────────────▶ Settled
//!  ▲  ◀─── change / resize / pointer ──┘ ▲ tick (1/fps)      │
//!  └──────────────── change / resize / pointer ──────────────┘
//! any ── 3rd failed draw in a row (or a failed still draw) ──▶ Frozen
//! ```

use std::rc::Rc;

use opencode_widget::Command;
use opencode_widget::draw::{Frame, MAX_FPS, OVERRUNS_TO_FREEZE, Pointer};

pub type Commands = Rc<Vec<Command>>;

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// Wants a draw; shows `kept` (its last frame, or nothing) until it has one.
    Due { kept: Commands, overruns: u32 },
    /// Draws every `1 / fps`; `overruns`: failed draws in a row.
    Animating { commands: Commands, overruns: u32 },
    /// Drawn; draws again only on a change.
    Settled { commands: Commands },
    /// Stopped on its last frame after failing; `error` says why.
    Frozen { commands: Commands, error: String },
}

/// Whether the canvas is on screen: only one that shows draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sight {
    #[default]
    Hidden,
    Shown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Canvas {
    pub phase: Phase,
    pub sight: Sight,
    /// Its logical size, once laid out.
    pub size: Option<[f32; 2]>,
    /// The clock when it first drew: `frame.t` counts from it.
    pub appeared: Option<f64>,
    /// The clock of its last draw (`frame.dt`, and when the next frame is due).
    pub last: f64,
    /// Where the pointer is over it, if it is.
    pub pointer: Option<Pointer>,
    /// The clock of the last pointer `move` sent, so moves go at most once a frame.
    pub moved: f64,
}

impl Default for Canvas {
    fn default() -> Self {
        Canvas { phase: Phase::Due { kept: Rc::default(), overruns: 0 }, sight: Sight::Hidden, size: None, appeared: None, last: 0.0, pointer: None, moved: f64::NEG_INFINITY }
    }
}

/// How a canvas node animates: its `fps` (0 to 60) and `duration`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Motion {
    pub fps: f64,
    pub duration: Option<f64>,
}

impl Motion {
    pub fn new(fps: Option<f64>, duration: Option<f64>) -> Motion {
        Motion { fps: fps.unwrap_or(0.0).clamp(0.0, MAX_FPS), duration: duration.filter(|d| d.is_finite() && *d >= 0.0) }
    }
}

pub enum Event {
    /// The widget stepped (its state may have changed), the canvas resized, or the pointer moved.
    Changed,
    Resized([f32; 2]),
    Seen(Sight),
    Pointer(Option<Pointer>),
    /// A draw at clock `at` came back.
    Drawn { at: f64, result: Result<Vec<Command>, String>, motion: Motion },
}

impl Canvas {
    pub fn commands(&self) -> &[Command] {
        match &self.phase {
            Phase::Due { kept: c, .. } | Phase::Animating { commands: c, .. } | Phase::Settled { commands: c } | Phase::Frozen { commands: c, .. } => c,
        }
    }

    pub fn error(&self) -> Option<&str> {
        match &self.phase {
            Phase::Frozen { error, .. } => Some(error),
            _ => None,
        }
    }

    fn redraw(&mut self) {
        self.phase = match std::mem::replace(&mut self.phase, Phase::Due { kept: Rc::default(), overruns: 0 }) {
            Phase::Animating { commands, overruns } => Phase::Due { kept: commands, overruns },
            Phase::Settled { commands } => Phase::Due { kept: commands, overruns: 0 },
            due @ Phase::Due { .. } => due,
            frozen @ Phase::Frozen { .. } => frozen,
        };
    }

    pub fn step(&mut self, event: Event) {
        match event {
            Event::Changed => self.redraw(),
            Event::Resized(size) => {
                if self.size != Some(size) {
                    self.size = Some(size);
                    self.redraw();
                }
            }
            Event::Seen(sight) => self.sight = sight,
            Event::Pointer(p) => {
                if self.pointer != p {
                    self.pointer = p;
                    self.redraw();
                }
            }
            Event::Drawn { at, result, motion } => {
                let t = at - *self.appeared.get_or_insert(at);
                self.last = at;
                let (kept, overruns) = match std::mem::replace(&mut self.phase, Phase::Due { kept: Rc::default(), overruns: 0 }) {
                    Phase::Due { kept, overruns } | Phase::Animating { commands: kept, overruns } => (kept, overruns),
                    Phase::Settled { commands } => (commands, 0),
                    frozen @ Phase::Frozen { .. } => {
                        self.phase = frozen;
                        return;
                    }
                };
                let animating = motion.fps > 0.0 && motion.duration.is_none_or(|d| t < d);
                self.phase = match result {
                    Ok(commands) if animating => Phase::Animating { commands: Rc::new(commands), overruns: 0 },
                    Ok(commands) => Phase::Settled { commands: Rc::new(commands) },
                    Err(error) if !animating || overruns + 1 >= OVERRUNS_TO_FREEZE => Phase::Frozen { commands: kept, error },
                    Err(_) => Phase::Animating { commands: kept, overruns: overruns + 1 },
                };
            }
        }
    }

    /// The frame to draw at clock `now`, when the canvas wants one: it shows, has a size, and is
    /// due or animating with its next frame due.
    pub fn due(&self, now: f64, motion: Motion, id: Option<&str>, scale: f64, dark: bool) -> Option<Frame> {
        let size = self.size.filter(|_| self.sight == Sight::Shown)?;
        let wants = match &self.phase {
            Phase::Due { .. } => true,
            Phase::Animating { .. } => motion.fps > 0.0 && now >= self.last + 1.0 / motion.fps - 0.002,
            Phase::Settled { .. } | Phase::Frozen { .. } => false,
        };
        wants.then(|| Frame {
            id: id.map(str::to_owned),
            t: self.appeared.map_or(0.0, |a| now - a),
            dt: if self.appeared.is_some() { now - self.last } else { 0.0 },
            width: f64::from(size[0]),
            height: f64::from(size[1]),
            scale,
            pointer: self.pointer,
            dark,
        })
    }

    /// When the next frame is due, while it animates and shows.
    pub fn next(&self, motion: Motion) -> Option<f64> {
        match &self.phase {
            Phase::Animating { .. } if self.sight == Sight::Shown && motion.fps > 0.0 => Some(self.last + 1.0 / motion.fps),
            Phase::Due { .. } if self.sight == Sight::Shown && self.size.is_some() => Some(self.last),
            _ => None,
        }
    }
}

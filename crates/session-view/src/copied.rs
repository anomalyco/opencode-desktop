//! A copy button's tick: pressing Copy turns its icon into a tick for a moment, as Desktop's
//! does, so the press shows it worked. The tick grows in on a short spring; both swaps back are
//! instant.
//!
//! Each button has its own feedback, as each Desktop copy button has its own
//! `CopyFeedbackState` (`session-ui/src/components/copy-machine.ts:7-10`): two buttons pressed
//! one after the other both show their ticks. Desktop's `copying` has no counterpart: Rune writes
//! the clipboard in the press, so a press goes straight to `copied`, and a press again restarts
//! its time (`copy.requested` from `copied`).

use std::collections::HashMap;

use rune::machine::Effects;
use rune::{El, Spring, SpringPair, Transition, Tx, icon};
use rune_knobs::{Group, Knob, Knobs};
use rune_theme_opencode::icons as ic;

use crate::TimelineHost;

/// `COPY_FEEDBACK_MS`: how long the tick stays.
pub const SHOW: Knob = Knob::new("Copied tick", 0.4, 3.0, 2.0).step(0.1).label("Tick shows").about("Seconds the tick stays after Copy");
pub const GROW: Knob = Knob::new("Copied grow", 0.0, 0.4, 0.15).step(0.01).label("Tick grows").about("Seconds the tick takes to grow in");
pub const FROM: Knob = Knob::new("Copied from", 0.5, 1.0, 0.7).step(0.05).label("Grows from").about("The tick's starting scale");

pub const GROUP: Group = Group::new("Copy", "copy", &[SHOW, GROW, FROM]).under("Timeline");

/// One button's feedback; a button not listed is `idle`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Feedback {
    /// `copied`: the tick shows until `until` on the app clock (`schedule-reset`).
    Copied { until: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// `copy.requested` and `copy.succeeded`: the press wrote the clipboard at `now`; the tick
    /// shows for `secs`.
    Copied { now: f64, secs: f64 },
    /// `reset.due`: the clock reached `now`.
    Tick { now: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    WakeAt(f64),
}

/// Every copy button showing its tick, by key.
#[derive(Default)]
pub struct Copied(HashMap<String, Feedback>);

impl Copied {
    /// `transitionCopyFeedback` for button `key` (any key for [`Event::Tick`], which steps all).
    pub fn step(&mut self, key: &str, event: Event) -> Effects<Effect> {
        match event {
            Event::Copied { now, secs } => {
                let until = now + secs;
                self.0.insert(key.to_owned(), Feedback::Copied { until });
                Effects::one(Effect::WakeAt(until))
            }
            Event::Tick { now } => {
                self.0.retain(|_, Feedback::Copied { until }| *until > now + 1e-9);
                let next = self.0.values().map(|Feedback::Copied { until }| *until).reduce(f64::min);
                next.map_or(Effects::none(), |due| Effects::one(Effect::WakeAt(due)))
            }
        }
    }

    fn shows(&self, key: &str, now: f64) -> bool {
        matches!(self.0.get(key), Some(Feedback::Copied { until }) if now < *until)
    }
}

/// Shows the tick on copy button `key`.
pub fn mark_copied<S: TimelineHost>(s: &mut S, key: impl Into<String>, tx: &mut Tx) {
    let event = Event::Copied { now: s.clock(), secs: f64::from(s.knobs().get(&SHOW)) };
    for Effect::WakeAt(due) in s.copied_state_mut().step(&key.into(), event) {
        tx.wake_at(due);
    }
}

/// Whether copy button `key` shows its tick.
pub fn is_copied<S: TimelineHost>(s: &S, key: &str) -> bool {
    // Only a button showing its tick reads the clock, which moves every frame.
    s.copied_state().0.contains_key(key) && s.copied_state().shows(key, s.clock())
}

/// The copy buttons showing their ticks now, in order: a row that draws any of them builds
/// again as they change.
pub fn current<S: TimelineHost>(s: &S) -> Vec<String> {
    // The clock moves every frame: a row that reads it can't be kept as it was while nothing
    // shows a tick.
    if s.copied_state().0.is_empty() {
        return Vec::new();
    }
    let now = s.clock();
    let mut keys: Vec<String> = s.copied_state().0.keys().filter(|k| s.copied_state().shows(k, now)).cloned().collect();
    keys.sort();
    keys
}

/// The button's glyph, `size` pt in `color`: the copy icon, or the tick growing in.
pub fn glyph<S: TimelineHost>(s: &S, key: &str, size: f32, color: rune::Color) -> El {
    glyph_of(s, key, size, color, &ic::OUTLINE_COPY)
}

/// [`glyph`] with `copy` as the resting icon.
pub fn glyph_of<S: TimelineHost>(s: &S, key: &str, size: f32, color: rune::Color, copy: &'static rune::Icon) -> El {
    if is_copied(s, key) {
        icon(&ic::CHECK, size).key("tick").color(color).transition(grow(s.knobs()))
    } else {
        icon(copy, size).key("copy-glyph").color(color).transition(Transition::none())
    }
}

fn grow(k: &Knobs) -> Transition {
    let enter = Spring::new(k.get(&GROW).max(0.01), 0.0);
    Transition::asymmetric(Transition::scale(k.get(&FROM)).combined(Transition::opacity()).allow_small_scale(), Transition::none())
        .springs(SpringPair::new(enter, Spring::NONE))
}

/// Drops the ticks whose time is up, and wakes for the next.
pub fn poll<S: TimelineHost>(s: &mut S, now: f64, tx: &mut Tx) -> bool {
    if s.copied_state().0.is_empty() {
        return false;
    }
    let before = s.copied_state().0.len();
    for Effect::WakeAt(due) in s.copied_state_mut().step("", Event::Tick { now }) {
        tx.wake_at(due);
    }
    s.copied_state().0.len() != before
}

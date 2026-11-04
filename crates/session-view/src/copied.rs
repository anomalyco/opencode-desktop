//! A copy button's tick: pressing Copy turns its icon into a tick for a moment, as Desktop's
//! does, so the press shows it worked. The tick grows in on a short spring; both swaps back are
//! instant.

use rune::{El, Spring, SpringPair, Transition, Tx, icon};
use rune_knobs::{Group, Knob, Knobs};
use rune_theme_opencode::icons as ic;

use crate::TimelineHost;

pub const SHOW: Knob = Knob::new("Copied tick", 0.4, 3.0, 1.2).step(0.1).label("Tick shows").about("Seconds the tick stays after Copy");
pub const GROW: Knob = Knob::new("Copied grow", 0.0, 0.4, 0.15).step(0.01).label("Tick grows").about("Seconds the tick takes to grow in");
pub const FROM: Knob = Knob::new("Copied from", 0.5, 1.0, 0.7).step(0.05).label("Grows from").about("The tick's starting scale");

pub const GROUP: Group = Group::new("Copy", "copy", &[SHOW, GROW, FROM]).under("Timeline");

/// The copy button last pressed (its key) and when, on the app clock.
#[derive(Default)]
pub struct Copied(Option<(String, f64)>);

/// Shows the tick on copy button `key`.
pub fn mark_copied<S: TimelineHost>(s: &mut S, key: impl Into<String>, tx: &mut Tx) {
    let clock = s.clock();
    let show = f64::from(s.knobs().get(&SHOW));
    s.copied_state_mut().0 = Some((key.into(), clock));
    tx.wake_after(std::time::Duration::from_secs_f64(show));
}

/// Whether copy button `key` shows its tick.
pub fn is_copied<S: TimelineHost>(s: &S, key: &str) -> bool {
    let clock = s.clock();
    let show = f64::from(s.knobs().get(&SHOW));
    s.copied_state().0.as_ref().is_some_and(|(k, at)| k == key && clock - at < show)
}

/// The button's glyph, `size` pt in `color`: the copy icon, or the tick growing in.
pub fn glyph<S: TimelineHost>(s: &S, key: &str, size: f32, color: rune::Color) -> El {
    if is_copied(s, key) {
        icon(&ic::CHECK, size).key("tick").color(color).transition(grow(s.knobs()))
    } else {
        icon(&ic::OUTLINE_COPY, size).key("copy-glyph").color(color).transition(Transition::none())
    }
}

fn grow(k: &Knobs) -> Transition {
    let enter = Spring::new(k.get(&GROW).max(0.01), 0.0);
    Transition::asymmetric(Transition::scale(k.get(&FROM)).combined(Transition::opacity()).allow_small_scale(), Transition::none())
        .springs(SpringPair::new(enter, Spring::NONE))
}

/// Drops the tick once its time is up.
pub fn poll<S: TimelineHost>(s: &mut S, now: f64, tx: &mut Tx) -> bool {
    let Some((_, at)) = &s.copied_state().0 else { return false };
    let left = *at + f64::from(s.knobs().get(&SHOW)) - now;
    if left > 0.0 {
        tx.wake_after(std::time::Duration::from_secs_f64(left));
        return false;
    }
    s.copied_state_mut().0 = None;
    true
}

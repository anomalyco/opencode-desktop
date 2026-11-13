//! CSS's timing functions and transitions, for what Desktop animates with CSS and the egg draws
//! itself (the blink, the shimmer, the paddle's colour and fade).

use rune::Easing;

pub const EASE_OUT: Easing = Easing::EASE_OUT;

/// `ease-in-out`.
pub fn css_ease_in_out(t: f64) -> f64 {
    f64::from(Easing::EASE_IN_OUT.eval(t.clamp(0.0, 1.0) as f32))
}

/// A CSS transition of one number: from where it was when the value last changed to where it
/// goes, over `secs` on `easing`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tween {
    At(f64),
    Moving { from: f64, to: f64, since: f64, secs: f64, easing: Easing },
}

impl Tween {
    pub fn value(&self, now: f64) -> f64 {
        match *self {
            Tween::At(v) => v,
            Tween::Moving { from, to, since, secs, easing } => {
                let t = ((now - since) / secs).clamp(0.0, 1.0);
                from + (to - from) * f64::from(easing.eval(t as f32))
            }
        }
    }

    pub fn target(&self) -> f64 {
        match *self {
            Tween::At(v) | Tween::Moving { to: v, .. } => v,
        }
    }

    /// Sets the value as CSS does when a transition applies: it moves from where it is now.
    pub fn set(&mut self, to: f64, now: f64, secs: f64, easing: Easing) {
        if self.target() == to {
            return;
        }
        let from = self.value(now);
        *self = Tween::Moving { from, to, since: now, secs, easing };
    }

    /// Still moving at `now`.
    pub fn moving(&self, now: f64) -> bool {
        matches!(*self, Tween::Moving { since, secs, .. } if now - since < secs)
    }
}

/// A phase's time has come. Desktop measures in milliseconds (`(now - t0) / 1000`) and Rune in
/// seconds, and Rune's clock sums frames, so at a time that falls exactly on a frame (1.2 s is 72
/// frames) the two land a last bit apart; a nanosecond's grace puts Rune on the same frame.
pub fn due(elapsed: f64, at: f64) -> bool {
    elapsed >= at - 1e-9
}

//! Opening and closing a disclosure: its key is in `App::expanded` while it is open.

use rune::{Spring, Tx};
use rune_knobs::{Group, Knob, Knobs};

use crate::App;

/// A tool group folding open or shut, as a critically damped spring's duration: Kit asked for
/// 150-200 ms, ease out, no bounce. However long the group, only what shows of it moves.
pub const FOLD_SPEED: Knob = Knob::new("Fold speed", 0.08, 0.5, 0.18).step(0.01).about("Seconds a tool group takes to fold");

rune_register::register!(crate::knobs::GROUP_LIST: Group = [Group::new("Tool groups", "tool-groups", &[FOLD_SPEED]).under("Timeline")]);

/// The tool group row's own spring (`El::fold_glide`).
pub fn fold(k: &Knobs) -> Spring {
    Spring::new(k.get(&FOLD_SPEED), 0.0)
}

/// How long a shut tool group keeps its steps mounted, for the row to fold shut over them:
/// twice the spring's duration, when under a twentieth of a point is left to go.
pub fn shut_secs(k: &Knobs) -> f64 {
    2.0 * crate::knobs::secs(k, &FOLD_SPEED)
}

impl App {
    /// Flips `key` in `expanded`; true when it is now open.
    pub fn toggle_expanded(&mut self, key: &str) -> bool {
        let opening = !self.expanded.remove(key);
        if opening {
            self.expanded.insert(key.to_owned());
        }
        opening
    }

    /// Whether tool group `key` is springing shut, its steps still laid out under the clip.
    pub fn shutting(&self, key: &str) -> bool {
        self.shutting.get(key).is_some_and(|at| self.clock - at < shut_secs(&self.knobs))
    }
}

/// A press handler that flips `key` at once: a disclosure opens and closes as a thought's does.
/// `hold`, the pressed trigger, stays where it shows, at the timeline's end too, so what opens
/// pushes the rows below it down rather than growing up past the pointer.
pub fn toggle(key: String, hold: String) -> impl Fn(&mut App, &mut Tx) + 'static {
    move |s: &mut App, tx: &mut Tx| {
        tx.instant();
        s.toggle_expanded(&key);
        tx.keep_on_screen(hold.clone());
    }
}

/// Opens or shuts tool group `key` (`turns::group`). Its header holds where it shows, stuck at
/// the timeline's top or not, while the row folds open or shut under it on its own spring: the timeline
/// scrolls to keep it there rather than following its end. The steps stay mounted while it
/// shuts, so the row's clip folds them away, and a press mid-motion springs back from there.
pub fn toggle_group(key: String) -> impl Fn(&mut App, &mut Tx) + 'static {
    move |s: &mut App, tx: &mut Tx| {
        if s.toggle_expanded(&key) {
            s.shutting.remove(&key);
        } else {
            s.shutting.insert(key.clone(), s.clock);
        }
        tx.keep_on_screen(format!("{key}/trigger"));
    }
}

/// Drops the steps of groups that have finished shutting.
pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    if s.shutting.is_empty() {
        return false;
    }
    let (before, secs) = (s.shutting.len(), shut_secs(&s.knobs));
    s.shutting.retain(|_, at| now - *at < secs);
    if let Some(next) = s.shutting.values().map(|at| at + secs - now).reduce(f64::min) {
        tx.wake_after(std::time::Duration::from_secs_f64(next.max(0.0)));
    }
    s.shutting.len() != before
}

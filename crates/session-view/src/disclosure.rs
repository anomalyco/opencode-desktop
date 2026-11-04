//! Opening and closing a disclosure: its key is in `App::expanded` while it is open.

use rune::{Spring, Tx};
use rune_knobs::{Group, Knob, Knobs};

use crate::TimelineHost;

/// A tool group folding open or shut, as a critically damped spring's duration: Kit asked for
/// 150-200 ms, ease out, no bounce. However long the group, only what shows of it moves.
pub const FOLD_SPEED: Knob = Knob::new("Fold speed", 0.08, 0.5, 0.18).step(0.01).about("Seconds a tool group takes to fold");

/// How deep a fold's bottom edge feathers while it moves, opening and shutting: soft enough
/// that lines dissolve into the rows below rather than being cut, crisp again once it lands.
pub const FOLD_FEATHER: Knob = Knob::new("Fold edge", 0.0, 64.0, 24.0).step(1.0).about("Points the moving edge fades over");

pub const GROUP: Group = Group::new("Tool groups", "tool-groups", &[FOLD_SPEED, FOLD_FEATHER]).under("Timeline");

/// The tool group row's own spring (`El::fold_glide`).
pub fn fold(k: &Knobs) -> Spring {
    Spring::new(k.get(&FOLD_SPEED), 0.0)
}

/// How long a shut tool group keeps its steps mounted, for the row to fold shut over them:
/// twice the spring's duration, when under a twentieth of a point is left to go.
pub fn shut_secs(k: &Knobs) -> f64 {
    2.0 * crate::knobs::secs(k, &FOLD_SPEED)
}

/// Flips `key` in `expanded`; true when it is now open.
pub fn toggle_expanded<S: TimelineHost>(s: &mut S, key: &str) -> bool {
    let expanded = s.expanded_mut();
    let opening = !expanded.remove(key);
    if opening {
        expanded.insert(key.to_owned());
    }
    opening
}

/// Whether fold `key` (a tool group, a Compacted summary) is springing shut, what it showed
/// still laid out under the clip.
pub fn is_shutting<S: TimelineHost>(s: &S, key: &str) -> bool {
    let clock = s.clock();
    let secs = shut_secs(s.knobs());
    s.shutting_map().get(key).is_some_and(|at| clock - at < secs)
}

/// A press handler that flips `key` at once: a disclosure opens and closes as a thought's does.
/// `hold`, the pressed trigger, stays where it shows, at the timeline's end too, so what opens
/// pushes the rows below it down rather than growing up past the pointer.
pub fn toggle<S: TimelineHost>(key: String, hold: String) -> impl Fn(&mut S, &mut Tx) + 'static {
    move |s: &mut S, tx: &mut Tx| {
        tx.instant();
        toggle_expanded(s, &key);
        tx.keep_on_screen(hold.clone());
    }
}

/// Flips fold `key`, a timeline row that folds on its own spring (`RowSpec::wrap`); true when it
/// is now open. Shutting, it keeps what it shows mounted until `poll` drops it, so the row's
/// clip folds it away, and a press mid-motion springs back from there.
pub fn flip_fold<S: TimelineHost>(s: &mut S, key: &str) -> bool {
    let open = toggle_expanded(s, key);
    if open {
        s.shutting_map_mut().remove(key);
    } else {
        let now = s.clock();
        s.shutting_map_mut().insert(key.to_owned(), now);
    }
    open
}

/// Opens or shuts tool group `key` (`turns::group`). Its header holds where it shows, stuck at
/// the timeline's top or not, while the row folds open or shut under it on its own spring: the timeline
/// scrolls to keep it there rather than following its end.
pub fn toggle_group<S: TimelineHost>(key: String) -> impl Fn(&mut S, &mut Tx) + 'static {
    move |s: &mut S, tx: &mut Tx| {
        flip_fold(s, &key);
        tx.keep_on_screen(format!("{key}/trigger"));
    }
}

/// Drops the steps of groups that have finished shutting.
pub fn poll<S: TimelineHost>(s: &mut S, now: f64, tx: &mut Tx) -> bool {
    if s.shutting_map().is_empty() {
        return false;
    }
    let secs = shut_secs(s.knobs());
    let shutting = s.shutting_map_mut();
    let before = shutting.len();
    shutting.retain(|_, at| now - *at < secs);
    if let Some(next) = shutting.values().map(|at| at + secs - now).reduce(f64::min) {
        tx.wake_after(std::time::Duration::from_secs_f64(next.max(0.0)));
    }
    shutting.len() != before
}

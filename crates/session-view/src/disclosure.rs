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

/// The mark that holds `key` shut where it would show open by default.
fn shut_key(key: &str) -> String {
    format!("{key}!shut")
}

/// Whether disclosure `key` is open: as last opened or shut, else `default` (Timeline detail's
/// Collapse setting for shells, edits and thinking, as Desktop's `disclosure.value(id) ?? default`).
pub fn is_open<S: TimelineHost>(s: &S, key: &str, default: bool) -> bool {
    let e = s.expanded();
    e.contains(key) || (default && !e.contains(&shut_key(key)))
}

/// Opens or shuts `key` whatever its default.
pub fn set_open<S: TimelineHost>(s: &mut S, key: &str, open: bool) {
    let shut = shut_key(key);
    let e = s.expanded_mut();
    if open {
        e.remove(&shut);
        e.insert(key.to_owned());
    } else {
        e.remove(key);
        e.insert(shut);
    }
}

/// A press handler that opens or shuts `key` at once, as [`toggle`] flips it, for a disclosure
/// with a default (`open`: whether it shows open now).
pub fn set<S: TimelineHost>(key: String, open: bool, hold: String) -> impl Fn(&mut S, &mut Tx) + 'static {
    move |s: &mut S, tx: &mut Tx| {
        tx.instant();
        set_open(s, &key, !open);
        anchor(s, tx, !open, &hold, &hold);
    }
}

/// Steps the open session's follow state with a disclosure opening or shutting, `row` the
/// opened row and `header` the pressed one, and makes the hold it asks for: none at the
/// timeline's end, where the end holds; scrolled up, the row or header (`follow::step`).
pub fn anchor<S: TimelineHost>(s: &mut S, tx: &mut Tx, opening: bool, row: &str, header: &str) {
    let id = s.active_session_id().map(str::to_owned);
    if let Some(crate::follow::Effect::Keep(key)) = s.ends_mut().step(id.as_deref(), crate::follow::Event::Disclosure { opening, row, header }) {
        tx.keep_on_screen(key);
    }
}

/// Whether fold `key` (a tool group, a Compacted summary) is springing shut, what it showed
/// still laid out under the clip.
pub fn is_shutting<S: TimelineHost>(s: &S, key: &str) -> bool {
    let clock = s.clock();
    let secs = shut_secs(s.knobs());
    s.shutting_map().get(key).is_some_and(|at| clock - at < secs)
}

/// A press handler that flips `key` at once: a disclosure opens and closes as a thought's does.
/// `hold`, the pressed trigger, holds as [`anchor`] says.
pub fn toggle<S: TimelineHost>(key: String, hold: String) -> impl Fn(&mut S, &mut Tx) + 'static {
    move |s: &mut S, tx: &mut Tx| {
        tx.instant();
        let opening = toggle_expanded(s, &key);
        anchor(s, tx, opening, &hold, &hold);
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

/// Opens or shuts tool group `key` (`turns::group`), the row folding open or shut on its own
/// spring. The timeline holds as [`anchor`] says: at its end, the end stays and the header rides
/// up as the steps open; scrolled up, the group's row keeps its top and the steps push the rows
/// below down. One pressed half under the top edge keeps its row's top there, and the header,
/// sticky once open, stops at the edge with its steps under it (p277).
pub fn toggle_group<S: TimelineHost>(key: String) -> impl Fn(&mut S, &mut Tx) + 'static {
    move |s: &mut S, tx: &mut Tx| {
        let opening = flip_fold(s, &key);
        anchor(s, tx, opening, &key, &format!("{key}/trigger"));
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

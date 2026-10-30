//! Opening and closing a disclosure: its key is in `App::expanded` while it is open.

use rune::Tx;

use crate::App;

/// How long a shut tool group keeps its steps mounted, for the row to spring shut over them.
pub const SHUT_SECS: f64 = 1.2;

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
        self.shutting.get(key).is_some_and(|at| self.clock - at < SHUT_SECS)
    }
}

/// A press handler that flips `key` at once: a disclosure opens and closes as a thought's does.
pub fn toggle(key: String) -> impl Fn(&mut App, &mut Tx) + 'static {
    move |s: &mut App, tx: &mut Tx| {
        tx.instant();
        s.toggle_expanded(&key);
    }
}

/// Opens or shuts tool group `key` (`turns::group`). Its header holds where it shows, stuck at
/// the timeline's top or not, while the row springs open or shut under it: the timeline
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
    let before = s.shutting.len();
    s.shutting.retain(|_, at| now - *at < SHUT_SECS);
    if let Some(next) = s.shutting.values().map(|at| at + SHUT_SECS - now).reduce(f64::min) {
        tx.wake_after(std::time::Duration::from_secs_f64(next.max(0.0)));
    }
    s.shutting.len() != before
}

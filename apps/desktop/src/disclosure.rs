//! Opening and closing a disclosure: its key is in `App::expanded` while it is open.

use rune::Tx;

use crate::App;

impl App {
    /// Flips `key` in `expanded`; true when it is now open.
    pub fn toggle_expanded(&mut self, key: &str) -> bool {
        let opening = !self.expanded.remove(key);
        if opening {
            self.expanded.insert(key.to_owned());
        }
        opening
    }
}

/// A press handler that flips `key` at once: a disclosure opens and closes as a thought's does.
pub fn toggle(key: String) -> impl Fn(&mut App, &mut Tx) + 'static {
    move |s: &mut App, tx: &mut Tx| {
        tx.instant();
        s.toggle_expanded(&key);
    }
}

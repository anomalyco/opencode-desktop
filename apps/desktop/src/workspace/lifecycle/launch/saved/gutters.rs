//! The scroll containers whose content overflowed when the app last ran, so a launch gives each
//! its scroll bar's gutter in the first frame. A container learns whether it overflows from its
//! first layout (`opencode_ui::scroller`), and takes the gutter the frame after: in the Inbox
//! layout every row of the sidebar's list narrowed by 8 pt in the launch's second frame, the one
//! the server's answer usually draws.
//!
//! Kept in the app's saved state (the boot record) with the saved session list (`saved_list`).

use serde_json::{Value, json};

use crate::App;

/// Desktop's global storage name: what every window shares, all of it in the boot record.
const GLOBAL: &str = "opencode.global.dat";
const KEY: &str = "rune.launch.gutters";
/// More containers than a window shows at once; one keyed by a session (a timeline's own) is
/// left out, so the list doesn't grow with the sessions opened.
const MOST: usize = 32;

/// Before the first frame: the containers that overflowed start with their gutter.
pub(crate) fn restore(s: &App) {
    if !s.persists() {
        return;
    }
    let Some(saved) = crate::storage::json(GLOBAL, KEY) else { return };
    let keys = saved.as_array().into_iter().flatten().filter_map(Value::as_str).take(MOST).map(str::to_owned);
    session_view::scroller::expect_overflow(keys);
}

/// Keeps which containers overflow now, for the next launch.
pub(crate) fn save(s: &App) {
    if !s.persists() {
        return;
    }
    let keys: Vec<String> = session_view::scroller::overflowing_keys().into_iter().filter(|key| !key.contains("ses_")).take(MOST).collect();
    // An unchanged list writes nothing (`storage::set`).
    crate::storage::set_json(GLOBAL, KEY, &json!(keys));
}

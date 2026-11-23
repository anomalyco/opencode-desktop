//! An agent's input at a higher off-screen frame rate. Blink handles a mouse move at the next
//! frame, and an off-screen tab's frames come at its `windowless_frame_rate` (60): each of a
//! drag's twelve moves waited up to 16.7 ms, 216 ms in all on blade where official's window, at
//! the display's rate, took 97. While an agent's `Input.*` calls arrive the tab runs at
//! [`AGENT_INPUT_FPS`], and back at the pane's rate [`IDLE_AFTER`] after the last one.
//! State lives on the engine's main thread, where every `TypedCdpCall` and `poll_frames` runs.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use cef::{BrowserHost, ImplBrowserHost};

pub const PANE_FPS: i32 = 60;
pub const AGENT_INPUT_FPS: i32 = 240;
const IDLE_AFTER: Duration = Duration::from_secs(1);

thread_local! {
    static BOOSTED: RefCell<HashMap<u64, (Instant, BrowserHost)>> = RefCell::new(HashMap::new());
}

/// An agent's input call for tab `tab_id` arrived.
pub fn boost(tab_id: u64, host: &BrowserHost) {
    BOOSTED.with(|b| {
        let mut b = b.borrow_mut();
        if !b.contains_key(&tab_id) {
            host.set_windowless_frame_rate(AGENT_INPUT_FPS);
        }
        b.insert(tab_id, (Instant::now() + IDLE_AFTER, host.clone()));
    });
}

/// Returns each tab whose agent input stopped to the pane's rate.
pub fn relax(now: Instant) {
    BOOSTED.with(|b| {
        b.borrow_mut().retain(|_, (until, host)| {
            if *until > now {
                return true;
            }
            host.set_windowless_frame_rate(PANE_FPS);
            false
        });
    });
}

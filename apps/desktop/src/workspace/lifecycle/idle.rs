//! Idle time: how long this thread's windows have gone without input. Work a launch can put off
//! (plugins no one has used yet, a shut region's layout) waits for [`AFTER`] of it.

use std::time::Duration;

/// How long the windows go without input before put-off work starts.
pub const AFTER: Duration = Duration::from_secs(1);

/// How long this thread's windows have gone without input (a key, the pointer, a wheel), counted
/// from the first ask before any.
pub fn quiet_for() -> Duration {
    static FIRST: std::sync::OnceLock<rune::time::Instant> = std::sync::OnceLock::new();
    let first = *FIRST.get_or_init(rune::time::Instant::now);
    rune::last_input().unwrap_or(first).elapsed()
}

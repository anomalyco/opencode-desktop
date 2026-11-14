//! The wall clock.

use crate::{arg, headless};

/// The wall clock in Unix ms. A headless run given `--clock <unix ms>` starts its clock there, so a
/// capture's "Today", "Yesterday" and "3h ago" read as they did when official was captured.
pub(crate) fn wall_ms() -> i64 {
    static FIXED: std::sync::OnceLock<Option<(rune::time::Instant, i64)>> = std::sync::OnceLock::new();
    let fixed = FIXED.get_or_init(|| arg("--clock").filter(|_| headless()).and_then(|v| v.parse().ok()).map(|ms| (rune::time::Instant::now(), ms)));
    if let Some((from, ms)) = fixed {
        return ms + from.elapsed().as_millis() as i64;
    }
    rune::time::SystemTime::now().duration_since(rune::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

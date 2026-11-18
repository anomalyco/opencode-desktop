//! The wall clock.

fn arg(name: &str) -> Option<String> {
    let args = rune::extra_args();
    args.iter().position(|x| x == name).and_then(|i| args.get(i + 1).cloned())
}

/// The wall clock in Unix ms. A headless run given `--clock <unix ms>` starts its clock there, so a
/// capture's "Today", "Yesterday" and "3h ago" read as they did when official was captured.
pub fn wall_ms() -> i64 {
    static FIXED: std::sync::OnceLock<Option<(rune::time::Instant, i64)>> = std::sync::OnceLock::new();
    let fixed = FIXED.get_or_init(|| {
        arg("--clock")
            .filter(|_| rune::is_headless())
            .and_then(|v| v.parse().ok())
            .map(|ms| (rune::time::Instant::now(), ms))
    });
    if let Some((from, ms)) = fixed {
        return ms + from.elapsed().as_millis() as i64;
    }
    #[allow(clippy::disallowed_methods)]
    rune::time::SystemTime::now()
        .duration_since(rune::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

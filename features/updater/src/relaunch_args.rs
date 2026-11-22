//! The arguments an update's restart passes on to the new app: this run's own, in order, less
//! those that must not outlive this process. Pure, so a table outside the repo checks it.

/// Dropped with their value, as `--flag value` or `--flag=value`. `--window-script FILE` (Rune
/// tooling) replays a script's input in the window and ignores the real pointer: an app started
/// again with it would replay the script from its start for its whole life.
const DROPPED: &[&str] = &["--window-script"];

pub fn forwarded(args: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if DROPPED.contains(&arg.as_str()) {
            it.next();
            continue;
        }
        if DROPPED.iter().any(|flag| arg.strip_prefix(flag).is_some_and(|rest| rest.starts_with('='))) {
            continue;
        }
        out.push(arg);
    }
    out
}

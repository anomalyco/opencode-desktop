//! Whether a proof run records its external opens, and where. Plain `std` only, so a table
//! program outside the repo can include this file by path.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

/// What [`recording`] decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recording {
    /// The run did not ask: external opens happen as in a normal launch.
    Off,
    /// The run asked: nothing external opens. Each open is a line in `log`, or is dropped when no
    /// safe place to write exists.
    On { log: Option<PathBuf> },
}

/// Recording for `HARNESS_OPEN_EXTERNAL_LOG` = `env`, decided once at launch:
/// - `env` unset: off.
/// - `anywhere` (a debug build or a headless driver run): the log is `env` as given; empty is off.
/// - Otherwise the run asked only when the app also passed `root`, the folder a `--harness-writes`
///   run roots every write under; without it: off. Asked means on, failing closed: the log is
///   [`safe_log`] inside the root's real path ([`real_root`]), or no log at all when there is
///   none (a root that is not absolute or cannot be resolved included).
pub fn recording(env: Option<&OsStr>, anywhere: bool, root: Option<&Path>) -> Recording {
    let Some(env) = env else {
        return Recording::Off;
    };
    if anywhere {
        if env.is_empty() {
            return Recording::Off;
        }
        return Recording::On { log: Some(PathBuf::from(env)) };
    }
    let Some(root) = root else {
        return Recording::Off;
    };
    Recording::On { log: real_root(root).and_then(|root| safe_log(Path::new(env), &root)) }
}

/// `root`'s real path (junctions and links followed), resolved once when recording is configured
/// and kept: a root replaced by a link later is not followed.
pub fn real_root(root: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(Some(root).filter(|r| r.is_absolute())?).ok()
}

/// `log` when its real location lies inside `root` (a real path from [`real_root`], not resolved
/// again), else `<root>/open-external.jsonl` checked the same way, else `None`.
pub fn safe_log(log: &Path, root: &Path) -> Option<PathBuf> {
    let lexical = log.is_absolute() && !log.components().any(|c| c == Component::ParentDir);
    lexical.then(|| real_inside(log, root)).flatten().or_else(|| real_inside(&root.join("open-external.jsonl"), root))
}

/// `path` at its real location when that lies inside `root` (already real): the nearest part of
/// `path` that exists is resolved (following junctions and links) and the rest, which does not
/// exist yet, is added back. `None` when it lies outside, or a part exists but cannot be resolved
/// (a dangling link).
fn real_inside(path: &Path, root: &Path) -> Option<PathBuf> {
    for part in path.ancestors() {
        if let Ok(real) = std::fs::canonicalize(part) {
            if !real.starts_with(root) {
                return None;
            }
            let rest = path.strip_prefix(part).ok()?;
            return Some(if rest.as_os_str().is_empty() { real } else { real.join(rest) });
        }
        if std::fs::symlink_metadata(part).is_ok() {
            return None;
        }
    }
    None
}

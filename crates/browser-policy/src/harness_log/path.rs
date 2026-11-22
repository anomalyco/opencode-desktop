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

/// Recording for `HARNESS_OPEN_EXTERNAL_LOG` = `env`:
/// - `env` unset: off.
/// - `anywhere` (a debug build or a headless driver run): the log is `env` as given; empty is off.
/// - Otherwise the run asked only when the app also passed `root`, the folder a `--harness-writes`
///   run roots every write under; without it: off. Asked means on, failing closed: the log is
///   `env` when its real location (junctions and links followed) lies inside the root's real
///   path, else `<root>/open-external.jsonl` checked the same way, else no log at all (a root that
///   is not absolute or cannot be resolved included).
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
    Recording::On { log: safe_log(Path::new(env), root) }
}

/// `log`, or else `<root>/open-external.jsonl`, at its real location when that lies inside the
/// real `root`; `None` when neither does or `root` cannot be resolved.
fn safe_log(log: &Path, root: &Path) -> Option<PathBuf> {
    let root = std::fs::canonicalize(Some(root).filter(|r| r.is_absolute())?).ok()?;
    let lexical = log.is_absolute() && !log.components().any(|c| c == Component::ParentDir);
    lexical.then(|| real_inside(log, &root)).flatten().or_else(|| real_inside(&root.join("open-external.jsonl"), &root))
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

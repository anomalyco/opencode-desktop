//! Where a proof run's external-open log goes. Plain `std` only, so a table program outside the
//! repo can include this file by path.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

/// The log file for `HARNESS_OPEN_EXTERNAL_LOG` = `env`, or `None` when recording is off:
/// - `env` unset: off.
/// - `anywhere` (a debug build or a headless driver run): `env` as given; empty is off.
/// - Otherwise only with `root`, the absolute folder a `--harness-writes` run roots every write
///   under, and only once `root` resolves to its real path (else off). The log is `env` when its
///   real location (junctions and links followed) lies inside the real root; otherwise (empty,
///   relative, `..`, elsewhere, or a link out of the root) `<root>/open-external.jsonl`, checked
///   the same way. If neither lies inside the root: off.
pub fn log_path(env: Option<&OsStr>, anywhere: bool, root: Option<&Path>) -> Option<PathBuf> {
    let env = env?;
    if anywhere {
        return (!env.is_empty()).then(|| PathBuf::from(env));
    }
    let root = std::fs::canonicalize(root.filter(|r| r.is_absolute())?).ok()?;
    let log = Path::new(env);
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

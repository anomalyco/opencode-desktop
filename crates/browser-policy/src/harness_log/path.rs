//! Where a proof run's external-open log goes. Plain `std` only, so a table program outside the
//! repo can include this file by path.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

/// The log file for `HARNESS_OPEN_EXTERNAL_LOG` = `env`, or `None` when recording is off:
/// - `env` unset: off.
/// - `anywhere` (a debug build or a headless driver run): `env` as given; empty is off.
/// - Otherwise only with `root`, the absolute folder a `--harness-writes` run roots every write
///   under: `env` when it is an absolute path inside `root` with no `..`, else (empty, relative,
///   `..`, or elsewhere) `<root>/open-external.jsonl`. Without such a root: off.
pub fn log_path(env: Option<&OsStr>, anywhere: bool, root: Option<&Path>) -> Option<PathBuf> {
    let env = env?;
    if anywhere {
        return (!env.is_empty()).then(|| PathBuf::from(env));
    }
    let root = root.filter(|r| r.is_absolute())?;
    let log = Path::new(env);
    let inside = log.is_absolute() && !log.components().any(|c| c == Component::ParentDir) && log.starts_with(root);
    Some(if inside { log.to_path_buf() } else { root.join("open-external.jsonl") })
}

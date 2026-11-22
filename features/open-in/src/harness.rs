//! `HARNESS_OPEN_EXTERNAL_LOG`: proofs record external opens, reveals and Open in launches here
//! instead of making them, so a scripted run never opens the person's browser, mail client, file
//! manager, editor or terminal.

use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use serde_json::json;

/// Set by [`admit_harness_log`]: the run's own root, the only place its log may be.
static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Lets `HARNESS_OPEN_EXTERNAL_LOG` take external opens and reveals in a windowed release run
/// too, so a proof that drives a real window never opens the person's browser or file manager.
/// The app calls it only for a harness run that roots every folder it writes under `root`
/// (`--harness-writes` with `RUNE_DIRS_ROOT`); the log must then lie inside `root`.
pub fn admit_harness_log(root: PathBuf) {
    drop(ROOT.set(root));
}

/// Appends `{pid, kind, <fields>}` to the log and returns true, or returns false when no log
/// applies and the caller must make the real open.
pub(crate) fn record(kind: &str, fields: &[(&str, &str)]) -> bool {
    let Some(log) = log_path() else {
        return false;
    };
    if let Some(parent) = log.parent() {
        drop(std::fs::create_dir_all(parent));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
        let mut entry = json!({ "pid": std::process::id(), "kind": kind });
        for (k, v) in fields {
            entry[*k] = json!(v);
        }
        // One write per line, so lines from several processes never interleave.
        drop(f.write_all(format!("{entry}\n").as_bytes()));
    }
    true
}

/// Where the log goes: in debug builds and headless driver runs (`--snap`, `--script`, `--tree`,
/// …) wherever it says; in a window only after [`admit_harness_log`], and only inside that run's
/// root (see [`confine`]). Never in a normal windowed release launch.
fn log_path() -> Option<PathBuf> {
    let log = PathBuf::from(std::env::var_os("HARNESS_OPEN_EXTERNAL_LOG").filter(|v| !v.is_empty())?);
    if cfg!(debug_assertions) || rune::is_headless() {
        return Some(log);
    }
    Some(confine(log, ROOT.get()?))
}

/// `log` when it is an absolute path inside `root` without `..`; otherwise
/// `<root>/open-external.jsonl`, so an admitted run still opens nothing and writes nowhere else.
fn confine(log: PathBuf, root: &Path) -> PathBuf {
    let inside = log.is_absolute() && !log.components().any(|c| matches!(c, Component::ParentDir)) && log.starts_with(root);
    if inside { log } else { root.join("open-external.jsonl") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confine_keeps_only_absolute_paths_inside_the_root() {
        let root = std::env::temp_dir().join("run-root");
        let fallback = root.join("open-external.jsonl");
        let cases = [
            (root.join("open.jsonl"), root.join("open.jsonl")),
            (root.join("logs").join("open.jsonl"), root.join("logs").join("open.jsonl")),
            (PathBuf::from("open.jsonl"), fallback.clone()),
            (PathBuf::from("run-root").join("open.jsonl"), fallback.clone()),
            (root.join("..").join("open.jsonl"), fallback.clone()),
            (root.join("logs").join("..").join("..").join("open.jsonl"), fallback.clone()),
            (std::env::temp_dir().join("open.jsonl"), fallback.clone()),
            (std::env::temp_dir().join("run-root-other").join("open.jsonl"), fallback.clone()),
        ];
        for (log, want) in cases {
            assert_eq!(confine(log.clone(), &root), want, "{}", log.display());
        }
    }
}

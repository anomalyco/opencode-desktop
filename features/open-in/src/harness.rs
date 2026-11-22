//! `HARNESS_OPEN_EXTERNAL_LOG`: proofs record external opens and reveals here instead of making
//! them, so a scripted run never opens the person's browser, mail client or file manager.

use std::io::Write as _;
use std::path::{Component, PathBuf};
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

/// Appends `{pid, kind, <field>: value}` to the log and returns true, or returns false when no
/// log applies and the caller must make the real open.
pub(crate) fn record(kind: &str, field: &str, value: &str) -> bool {
    let Some(log) = log_path() else {
        return false;
    };
    if let Some(parent) = log.parent() {
        drop(std::fs::create_dir_all(parent));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
        let mut entry = json!({ "pid": std::process::id(), "kind": kind });
        entry[field] = json!(value);
        drop(writeln!(f, "{entry}"));
    }
    true
}

/// Where the log goes: in debug builds and headless driver runs (`--snap`, `--script`, `--tree`,
/// …) wherever it says; in a window only after [`admit_harness_log`], and only inside that run's
/// root: a path outside it (relative, with `..`, or elsewhere) becomes `<root>/open-external.jsonl`,
/// so the run still opens nothing and writes nowhere else. Never in a normal windowed release launch.
fn log_path() -> Option<PathBuf> {
    let log = PathBuf::from(std::env::var_os("HARNESS_OPEN_EXTERNAL_LOG").filter(|v| !v.is_empty())?);
    if cfg!(debug_assertions) || rune::is_headless() {
        return Some(log);
    }
    let root = ROOT.get()?;
    let inside = log.is_absolute() && !log.components().any(|c| matches!(c, Component::ParentDir)) && log.starts_with(root);
    Some(if inside { log } else { root.join("open-external.jsonl") })
}

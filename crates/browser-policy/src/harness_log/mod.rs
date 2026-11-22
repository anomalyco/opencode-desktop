//! `HARNESS_OPEN_EXTERNAL_LOG`: a proof run records every external open here instead of making it
//! (a web or mail link, an app link, a reveal in the file manager, an Open in launch, Hex, the
//! system's microphone page), so a scripted run never opens anything on the person's desktop.
//! Off unless the app turned it on at launch ([`configure`]).

mod path;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::json;

pub use path::log_path;

static LOG: OnceLock<PathBuf> = OnceLock::new();

/// Turns recording on for this process, once at launch, when `HARNESS_OPEN_EXTERNAL_LOG` is set:
/// `anywhere` for a debug build or a headless driver run, else only inside `root`, the folder a
/// `--harness-writes` run roots every write under ([`log_path`]).
pub fn configure(anywhere: bool, root: Option<&Path>) {
    if let Some(log) = log_path(std::env::var_os("HARNESS_OPEN_EXTERNAL_LOG").as_deref(), anywhere, root) {
        drop(LOG.set(log));
    }
}

/// Appends `{pid, kind, <fields>}` to the log and returns true, or returns false when recording
/// is off and the caller makes the real open.
pub fn record(kind: &str, fields: &[(&str, &str)]) -> bool {
    let Some(log) = LOG.get() else {
        return false;
    };
    if let Some(parent) = log.parent() {
        drop(std::fs::create_dir_all(parent));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
        let mut entry = json!({ "pid": std::process::id(), "kind": kind });
        for (k, v) in fields {
            entry[*k] = json!(v);
        }
        // One write per line, so lines from several processes never interleave.
        drop(f.write_all(format!("{entry}\n").as_bytes()));
    }
    true
}

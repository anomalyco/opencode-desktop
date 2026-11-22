//! `HARNESS_OPEN_EXTERNAL_LOG`: a proof run records every external open here instead of making it
//! (a web or mail link, an app link, a reveal in the file manager, an Open in launch, Hex, the
//! system's microphone page), so a scripted run never opens anything on the person's desktop.
//! Off unless the app turned it on at launch ([`configure`]).

mod path;

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::json;

pub use path::log_path;

/// What [`configure`] was given, kept only when recording is on.
struct Config {
    env: OsString,
    anywhere: bool,
    root: Option<PathBuf>,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

/// Turns recording on for this process, once at launch, when `HARNESS_OPEN_EXTERNAL_LOG` is set:
/// `anywhere` for a debug build or a headless driver run, else only inside `root`, the folder a
/// `--harness-writes` run roots every write under ([`log_path`]).
pub fn configure(anywhere: bool, root: Option<&Path>) {
    let Some(env) = std::env::var_os("HARNESS_OPEN_EXTERNAL_LOG") else {
        return;
    };
    if log_path(Some(&env), anywhere, root).is_some() {
        drop(CONFIG.set(Config { env, anywhere, root: root.map(Path::to_path_buf) }));
    }
}

/// Appends `{pid, kind, <fields>}` to the log and returns true, or returns false when recording
/// is off and the caller makes the real open. The log's place is checked again at every write, so
/// a link made inside the root after launch cannot carry a line out of it; if no place inside the
/// root is left, the line is dropped and still nothing opens.
pub fn record(kind: &str, fields: &[(&str, &str)]) -> bool {
    let Some(config) = CONFIG.get() else {
        return false;
    };
    let Some(log) = log_path(Some(&config.env), config.anywhere, config.root.as_deref()) else {
        return true;
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

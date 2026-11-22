//! `HARNESS_OPEN_EXTERNAL_LOG`: a proof run records every external open here instead of making it
//! (a web or mail link, an app link, a reveal in the file manager, an Open in launch, Hex, the
//! system's microphone page), so a scripted run never opens anything on the person's desktop.
//! Off unless the app turned it on at launch ([`configure`]).

mod path;

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::json;

pub use path::{Recording, recording};

/// What [`configure`] was given, kept only when recording is on.
struct Config {
    env: OsString,
    anywhere: bool,
    root: Option<PathBuf>,
}

static CONFIG: OnceLock<Config> = OnceLock::new();
static SAID_NO_LOG: AtomicBool = AtomicBool::new(false);

/// Turns recording on for this process, once at launch, when the run asks for it
/// (`HARNESS_OPEN_EXTERNAL_LOG` set): `anywhere` for a debug build or a headless driver run, else
/// only with `root`, the folder a `--harness-writes` run roots every write under ([`recording`]).
pub fn configure(anywhere: bool, root: Option<&Path>) {
    let Some(env) = std::env::var_os("HARNESS_OPEN_EXTERNAL_LOG") else {
        return;
    };
    let Recording::On { log } = recording(Some(&env), anywhere, root) else {
        return;
    };
    if log.is_none() {
        say_no_log();
    }
    drop(CONFIG.set(Config { env, anywhere, root: root.map(Path::to_path_buf) }));
}

/// Appends `{pid, kind, <fields>}` to the log and returns true, or returns false when recording
/// is off and the caller makes the real open. Once on, it stays on: the log's place is checked
/// again at every write, so a link made inside the root after launch cannot carry a line out of
/// it, and with no safe place left the line is dropped and still nothing opens.
pub fn record(kind: &str, fields: &[(&str, &str)]) -> bool {
    let Some(config) = CONFIG.get() else {
        return false;
    };
    let Recording::On { log: Some(log) } = recording(Some(&config.env), config.anywhere, config.root.as_deref()) else {
        say_no_log();
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

fn say_no_log() {
    if !SAID_NO_LOG.swap(true, Ordering::Relaxed) {
        eprintln!("harness open log: recording is on but has no safe log path inside the run's root; external opens are dropped");
    }
}

#![cfg(not(target_arch = "wasm32"))]
//! Bounded diagnostic logs for the browser engine sidecar, kept beside Chromium's `debug.log`
//! inside the engine profile directory:
//! - `engine-host.log` (rotated to `engine-host.prev.log` at 256 KiB): host-side lifecycle,
//!   startup timing, timeouts, exit codes, and fault transitions.
//! - `engine-stderr.log` (rotated to `engine-stderr.prev.log` before each spawn): the sidecar's
//!   own stderr (Rust panics, bind/init errors, Chromium fatal messages), unless
//!   `OPENCODE_BROWSER_ENGINE_TRACE` keeps stderr inherited.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::Stdio;

use rune::time::{SystemTime, UNIX_EPOCH};

const HOST_LOG: &str = "engine-host.log";
const HOST_LOG_PREV: &str = "engine-host.prev.log";
const STDERR_LOG: &str = "engine-stderr.log";
const STDERR_LOG_PREV: &str = "engine-stderr.prev.log";
const MAX_HOST_LOG_BYTES: u64 = 256 * 1024;
const STDERR_TAIL_BYTES: usize = 1024;

/// Appends one timestamped line to `<profile>/engine-host.log`, rotating at `MAX_HOST_LOG_BYTES`.
/// Never panics and ignores I/O failures so logging can never disrupt engine startup.
pub(crate) fn log(profile: Option<&Path>, message: &str) {
    let Some(dir) = profile else { return };
    std::fs::create_dir_all(dir).ok();
    let path = dir.join(HOST_LOG);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() >= MAX_HOST_LOG_BYTES) {
        std::fs::rename(&path, dir.join(HOST_LOG_PREV)).ok();
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    // Wall-clock timestamp for correlating host events with CEF's `debug.log` (native-only module).
    #[allow(clippy::disallowed_methods)]
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    writeln!(
        file,
        "{}.{:03} [host_pid={}] {message}",
        since.as_secs(),
        since.subsec_millis(),
        std::process::id(),
    )
    .ok();
}

/// Opens `<profile>/engine-stderr.log` for a freshly spawned engine (rotating any prior run's
/// stderr to `engine-stderr.prev.log`), or inherits stderr when `OPENCODE_BROWSER_ENGINE_TRACE`
/// is set.
pub(crate) fn stderr_stdio(profile: &Path) -> Stdio {
    if std::env::var_os("OPENCODE_BROWSER_ENGINE_TRACE").is_some() {
        return Stdio::inherit();
    }
    std::fs::create_dir_all(profile).ok();
    let current = profile.join(STDERR_LOG);
    if current.is_file() {
        std::fs::rename(&current, profile.join(STDERR_LOG_PREV)).ok();
    }
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&current)
        .map(Stdio::from)
        .unwrap_or_else(|_| Stdio::null())
}

/// Reads the trailing portion of `<profile>/engine-stderr.log` as a single sanitized line for
/// inclusion in `engine-host.log` when the engine fails or exits unexpectedly.
pub(crate) fn read_stderr_tail(profile: Option<&Path>) -> Option<String> {
    let path = profile?.join(STDERR_LOG);
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        return None;
    }
    let start = len.saturating_sub(STDERR_TAIL_BYTES as u64);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let compact = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");
    (!compact.is_empty()).then_some(compact)
}

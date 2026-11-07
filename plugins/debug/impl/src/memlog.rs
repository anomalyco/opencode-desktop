//! Memory diagnostics (`main.ts`, `memory-log.ts`, `memory-record.ts`): while on, one JSONL line
//! of the app's memory every interval in a new `memory-<time>.jsonl` in the app's logs, which
//! exported logs include. Offered only in debug builds (local and Beta), off until the bar's
//! MEMLOG cell turns it on; `OPENCODE_DESKTOP_MEMORY_LOG=<seconds>` turns it on for one launch,
//! and the cell can't turn that off.

use std::io::Write;
use std::path::PathBuf;

use opencode_plugin::prelude::*;
use opencode_plugin::system;
use serde_json::{Value, json};

/// Whether the bar's toggle turned diagnostics on, remembered (`memory`).
const STORED: Store<bool> = Store::new("memory");
const DEFAULT_INTERVAL_MS: i64 = 30_000;

pub struct MemoryLog {
    /// The environment's interval, which keeps diagnostics on for the launch.
    env: Option<i64>,
    /// The run's file and when its next sample is due.
    run: Option<(PathBuf, i64)>,
}

impl MemoryLog {
    pub fn new() -> MemoryLog {
        #[allow(clippy::disallowed_methods)]
        let env = interval(std::env::var("OPENCODE_DESKTOP_MEMORY_LOG").ok().as_deref());
        MemoryLog { env, run: None }
    }

    pub fn on(&self) -> bool {
        self.run.is_some()
    }

    pub fn env(&self) -> bool {
        self.env.is_some()
    }

    /// Starts or stops the run to match the environment and the stored toggle.
    pub fn sync(&mut self, now: i64) {
        let enabled = self.env.is_some() || STORED.get() == Some(true);
        if enabled == self.run.is_some() {
            return;
        }
        if !enabled {
            let file = self.run.take().map(|(f, _)| f.display().to_string()).unwrap_or_default();
            system::log(false, &format!("memory diagnostics stopped: {file}"));
            return;
        }
        let Some(logs) = system::logs_dir() else { return };
        let file = PathBuf::from(logs).join(format!("memory-{}.jsonl", crate::stamp(now)));
        system::log(false, &format!("memory diagnostics started: {} every {} ms", file.display(), self.interval()));
        self.run = Some((file, now + self.interval()));
    }

    /// The cell: the environment's choice lasts for the launch.
    pub fn toggle(&mut self, now: i64) {
        if self.env.is_some() {
            return;
        }
        STORED.set(&!self.on());
        self.sync(now);
    }

    fn interval(&self) -> i64 {
        self.env.unwrap_or(DEFAULT_INTERVAL_MS)
    }

    /// When the next sample is due.
    pub fn due(&self) -> Option<i64> {
        self.run.as_ref().map(|(_, due)| *due)
    }

    /// Writes the sample that came due.
    pub fn poll(&mut self, now: i64) {
        let interval = self.interval();
        let Some((file, due)) = self.run.as_mut() else { return };
        if *due > now {
            return;
        }
        if let Err(e) = append(file, now) {
            system::log(true, &format!("memory diagnostics sample failed: {e}"));
        }
        *due = now + interval;
    }
}

/// One sample: what the OS counts for the app's process and what its allocator holds (Desktop
/// records Electron's per-process metrics and each renderer's JavaScript heap).
fn append(file: &PathBuf, now: i64) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let memory: Value = system::memory().and_then(|m| serde_json::from_str(&m).ok()).unwrap_or(Value::Null);
    let line = json!({
        "timestamp": crate::iso(now),
        "mainPid": std::process::id(),
        "nativeMemoryUnit": "bytes",
        "process": memory,
    });
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(file)?;
    writeln!(f, "{line}")
}

/// Invalid values disable diagnostics rather than turning a bad interval into a hot timer.
fn interval(value: Option<&str>) -> Option<i64> {
    let v = value?;
    let well_formed = !v.is_empty() && v.split_once('.').map_or(v.bytes().all(|b| b.is_ascii_digit()), |(a, b)| !a.is_empty() && !b.is_empty() && a.bytes().chain(b.bytes()).all(|c| c.is_ascii_digit()));
    let seconds: f64 = v.parse().ok().filter(|_| well_formed)?;
    (1.0..=2_147_483.0).contains(&seconds).then(|| (seconds * 1000.0) as i64)
}

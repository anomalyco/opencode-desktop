//! Capture heap snapshots (`main.ts`, `heap-machine.ts`, `server-heap.ts`): a new
//! `opencode-heap-<time>` folder in Downloads holding the app's own server's heap snapshot,
//! `memory.json` (where the memory is, per process) and `manifest.json`, written last so a script
//! can wait for it. The app is native, so its own process is measured rather than snapshotted.
//! One capture runs at a time; the command does nothing while it runs.

use std::path::{Path, PathBuf};

use opencode_plugin::prelude::*;
use opencode_plugin::system;
use serde_json::{Value, json};

use crate::Debug;

/// How long the server may take to write its snapshot before the manifest records it as failed.
pub const SNAPSHOT_TIMEOUT_MS: i64 = 5 * 60_000;

/// The capture running: its attempt, its folder, what was measured before, and when the
/// server's snapshot times out.
pub struct Capture {
    pub attempt: u64,
    directory: PathBuf,
    before: Value,
    started: i64,
    pub deadline: i64,
}

#[derive(Default)]
pub struct Heap {
    pub issued: u64,
    pub running: Option<Capture>,
}

impl Heap {
    /// The command: shows the loading toast and starts the capture, unless one runs.
    pub fn run(&mut self, cx: &mut ActionCx) {
        if self.running.is_some() {
            return;
        }
        self.issued += 1;
        let attempt = self.issued;
        let now = cx.now_ms();
        cx.toast_persistent(ToastVariant::Loading, "Capturing heap snapshots.", "", "", "");
        let Some(directory) = system::downloads_dir().map(|d| Path::new(&d).join(format!("opencode-heap-{}", crate::stamp(now)))) else {
            return failed(cx);
        };
        // Snapshots hold prompts, file contents and credentials.
        if create_private_dir(&directory).is_err() {
            return failed(cx);
        }
        system::log(false, &format!("capturing heap snapshots: {}", directory.display()));
        // Measurements first: writing a heap snapshot raises the memory of the process it serializes.
        let before = memory();
        let query: String = directory.display().to_string().bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
        cx.server_request("POST", &format!("{}/api/debug/heap-dump?directory={query}", opencode_plugin::abi::LOCAL_SERVER_PREFIX), "", move |s: &mut Debug, cx| {
            let server = match cx.input_extra() {
                "" => serde_json::from_str::<Value>(cx.input()).map(|v| if v.get("path").is_some() { v } else { v["data"].clone() }).unwrap_or_default(),
                e if e.contains("404") => json!({ "error": "The server does not support heap dumps. Update and restart it." }),
                e => json!({ "error": e }),
            };
            s.heap.finish(cx, attempt, server);
        });
        self.running = Some(Capture { attempt, directory, before, started: now, deadline: now + SNAPSHOT_TIMEOUT_MS });
    }

    /// The server's snapshot settled, or timed out: the files are written and the toast says how
    /// it went.
    pub fn finish(&mut self, cx: &mut ActionCx, attempt: u64, server: Value) {
        if self.running.as_ref().is_none_or(|c| c.attempt != attempt) {
            return;
        }
        let Some(capture) = self.running.take() else { return };
        match write(&capture, server, cx.now_ms()) {
            Ok(complete) => {
                let directory = capture.directory.display().to_string();
                system::log(!complete, &format!("heap snapshots captured: {directory} complete={complete}"));
                let (variant, title) = if complete { (ToastVariant::Success, "Heap snapshots saved") } else { (ToastVariant::Error, "Some heap snapshots failed. See manifest.json.") };
                cx.toast_persistent(variant, title, &directory, "Show in folder", &directory);
            }
            Err(e) => {
                system::log(true, &format!("heap snapshot capture failed: {e}"));
                failed(cx);
            }
        }
    }
}

fn failed(cx: &mut ActionCx) {
    cx.toast_persistent(ToastVariant::Error, "Could not capture heap snapshots", "", "", "");
}

fn create_private_dir(directory: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(directory)
}

fn memory() -> Value {
    system::memory().and_then(|m| serde_json::from_str(&m).ok()).unwrap_or(Value::Null)
}

/// Writes `memory.json` and `manifest.json`; whether every snapshot was written.
fn write(capture: &Capture, server: Value, now: i64) -> std::io::Result<bool> {
    let dir = &capture.directory;
    let snapshot = match server["path"].as_str() {
        Some(path) => {
            let path = Path::new(path);
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let bytes = std::fs::metadata(path).ok().map(|m| m.len());
            // The server joins the directory and file name, so the same join recognizes a snapshot it wrote there.
            let place = if dir.join(&name) == path { json!({ "file": name }) } else { json!({ "path": path.display().to_string() }) };
            let mut entry = json!({ "process": "server", "pid": server["pid"], "bytes": bytes, "memory": server["memory"] });
            entry.as_object_mut().into_iter().for_each(|o| o.extend(place.as_object().cloned().unwrap_or_default()));
            entry
        }
        None => json!({ "process": "server", "error": server["error"].as_str().unwrap_or("The server did not write its heap snapshot.") }),
    };
    let complete = snapshot.get("error").is_none();
    let after = memory();
    let mibs = |v: &Value| -> Value {
        let Some(o) = v.as_object() else { return Value::Null };
        o.iter().filter_map(|(k, v)| Some((k.clone(), json!((v.as_f64()? / 1_048_576.0 * 10.0).round() / 10.0)))).collect::<serde_json::Map<_, _>>().into()
    };
    let app = &capture.before;
    write_json(dir, "memory.json", &json!({
        "unit": "MiB",
        "app": { "footprint": mibs(&json!({ "footprint": app["footprint"], "footprintPeak": app["footprintPeak"], "resident": app["resident"] })), "heap": mibs(&app["heap"]), "heapTags": mibs(&app["heap"]["tags"]) },
        "server": if complete { mibs(&server["memory"]) } else { Value::Null },
        "note": "app: the app's process as the OS counts it, and what its allocator held, before the server's snapshot. server: the server's process.memoryUsage().",
    }))?;
    // Written last, so a script can wait for it.
    write_json(dir, "manifest.json", &json!({
        "created": crate::iso(now),
        "includeServer": true,
        "app": {
            "name": "OpenCode",
            "version": env!("CARGO_PKG_VERSION"),
            "channel": system::channel(),
            "durationSeconds": (now - capture.started) as f64 / 1000.0,
            "platform": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "files": {
            "memory.json": "Where the memory is, per process, in MiB. Start here.",
            "*.heapsnapshot": "Heap snapshots. Open them in the Memory panel of Chrome DevTools.",
        },
        "snapshots": [snapshot],
        "before": app,
        "after": after,
    }))?;
    Ok(complete)
}

fn write_json(dir: &Path, name: &str, value: &Value) -> std::io::Result<()> {
    std::fs::write(dir.join(name), serde_json::to_string_pretty(value).unwrap_or_default())
}

//! Bounded wake queue and version-keyed persisted listener set for waking Chromium-suspended
//! MV3 extension service workers on user-initiated clicks (`action.onClicked`,
//! `contextMenus.onClicked`, `notifications.onClicked`, `notifications.onClosed`).
//!
//! Matches official Electron's `worker-events.ts` (`startWorkerForScope`) semantics while
//! enforcing strict per-extension count + byte bounds, 10 s TTL, zero duplicate delivery,
//! listener-set gating (never wakes an extension that never registered a listener), and
//! immediate fail-closed cleanup on disable, uninstall, or failed integrity verification.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::integrity::read_regular_file_bounded;
use super::manifest::is_valid_ext_id;

pub const SW_WAKE_ID_BASE: i32 = 1_590_000_000;
pub const MAX_PENDING_EVENTS_PER_EXT: usize = 16;
pub const MAX_PENDING_BYTES_PER_EXT: usize = 64 * 1024;
pub const PENDING_EVENT_TTL: Duration = Duration::from_secs(10);
const MAX_PERSIST_BYTES: u64 = 4096;

pub fn is_wakeable_event(name: &str) -> bool {
    matches!(
        name,
        "action.onClicked"
            | "contextMenus.onClicked"
            | "notifications.onClicked"
            | "notifications.onClosed"
    )
}

struct PendingEvent {
    event: String,
    args: Vec<Value>,
    bytes: usize,
    queued_at: Instant,
}

#[derive(Default)]
struct PendingQueue {
    items: VecDeque<PendingEvent>,
    total_bytes: usize,
}

#[derive(Default)]
pub(super) struct SwWakeState {
    storage_dir: Option<PathBuf>,
    /// `ext_id -> (manifest_version, set of wakeable event names the SW subscribed to)`.
    known_listeners: HashMap<String, (String, HashSet<String>)>,
    pending: HashMap<String, PendingQueue>,
}

impl SwWakeState {
    pub fn with_storage_dir(storage_dir: Option<PathBuf>) -> Self {
        Self { storage_dir, known_listeners: HashMap::new(), pending: HashMap::new() }
    }

    /// Loads persisted wakeable listener names for `(ext_id, version)` on extension registration
    /// (dropping stale data if the extension version changed).
    pub fn configure_extension(&mut self, ext_id: &str, version: &str) {
        if !is_valid_ext_id(ext_id) {
            return;
        }
        let mut loaded = HashSet::new();
        if let Some(path) = self.file_path(ext_id)
            && let Some(bytes) = read_regular_file_bounded(&path, MAX_PERSIST_BYTES)
            && let Ok(val) = serde_json::from_slice::<Value>(&bytes)
            && val.get("version").and_then(Value::as_str) == Some(version)
            && let Some(arr) = val.get("events").and_then(Value::as_array)
        {
            for item in arr {
                if let Some(ev) = item.as_str()
                    && is_wakeable_event(ev)
                {
                    loaded.insert(ev.to_owned());
                }
            }
        }
        self.known_listeners.insert(ext_id.to_owned(), (version.to_owned(), loaded));
    }

    /// Updates `known_listeners` when a `SessionKind::ServiceWorker` adds or removes a wakeable
    /// listener, persisting the exact version-keyed set to disk.
    pub fn record_sw_listener(&mut self, ext_id: &str, version: &str, event: &str, listen: bool) {
        if !is_valid_ext_id(ext_id) || !is_wakeable_event(event) {
            return;
        }
        let entry = self
            .known_listeners
            .entry(ext_id.to_owned())
            .or_insert_with(|| (version.to_owned(), HashSet::new()));
        if entry.0 != version {
            entry.0 = version.to_owned();
            entry.1.clear();
        }
        let changed = if listen {
            entry.1.insert(event.to_owned())
        } else {
            entry.1.remove(event)
        };
        if changed {
            self.persist(ext_id);
        }
    }

    /// Queues `(event, args)` if `ext_id`'s service worker is known to listen for `event` and the
    /// payload fits within `MAX_PENDING_BYTES_PER_EXT`. Returns `true` when queued and the caller
    /// should trigger `ServiceWorker.startWorker`.
    pub fn enqueue_if_listening(&mut self, ext_id: &str, event: &str, args: Vec<Value>) -> bool {
        if !is_valid_ext_id(ext_id) || !is_wakeable_event(event) {
            return false;
        }
        let listens = self
            .known_listeners
            .get(ext_id)
            .is_some_and(|(_, set)| set.contains(event));
        if !listens {
            return false;
        }
        let Ok(encoded) = serde_json::to_vec(&args) else {
            return false;
        };
        let bytes = encoded.len().saturating_add(event.len());
        if bytes > MAX_PENDING_BYTES_PER_EXT {
            return false;
        }
        let now = Instant::now();
        let queue = self.pending.entry(ext_id.to_owned()).or_default();
        while queue
            .items
            .front()
            .is_some_and(|e| now.saturating_duration_since(e.queued_at) > PENDING_EVENT_TTL)
        {
            if let Some(old) = queue.items.pop_front() {
                queue.total_bytes = queue.total_bytes.saturating_sub(old.bytes);
            }
        }
        while queue.items.len() >= MAX_PENDING_EVENTS_PER_EXT
            || queue.total_bytes.saturating_add(bytes) > MAX_PENDING_BYTES_PER_EXT
        {
            let Some(old) = queue.items.pop_front() else {
                break;
            };
            queue.total_bytes = queue.total_bytes.saturating_sub(old.bytes);
        }
        queue.total_bytes = queue.total_bytes.saturating_add(bytes);
        queue.items.push_back(PendingEvent {
            event: event.to_owned(),
            args,
            bytes,
            queued_at: now,
        });
        true
    }

    /// Atomically pops and returns all non-expired queued events for `(ext_id, event)` so each
    /// event is delivered at most once (zero duplicate delivery).
    pub fn take_matching(&mut self, ext_id: &str, event: &str) -> Vec<(String, Vec<Value>)> {
        let Some(queue) = self.pending.get_mut(ext_id) else {
            return Vec::new();
        };
        let now = Instant::now();
        let mut matched = Vec::new();
        let mut kept = VecDeque::new();
        let mut kept_bytes = 0usize;
        while let Some(item) = queue.items.pop_front() {
            if now.saturating_duration_since(item.queued_at) > PENDING_EVENT_TTL {
                continue;
            }
            if item.event == event {
                matched.push((item.event, item.args));
            } else {
                kept_bytes = kept_bytes.saturating_add(item.bytes);
                kept.push_back(item);
            }
        }
        if kept.is_empty() {
            self.pending.remove(ext_id);
        } else {
            queue.items = kept;
            queue.total_bytes = kept_bytes;
        }
        matched
    }

    /// Immediately drops queued events and listener records for `ext_id` (on disable, uninstall,
    /// or failed integrity verification). Removes the persisted file when `remove_persisted`.
    pub fn remove_extension(&mut self, ext_id: &str, remove_persisted: bool) {
        self.pending.remove(ext_id);
        self.known_listeners.remove(ext_id);
        if remove_persisted && let Some(path) = self.file_path(ext_id) {
            if let Ok(meta) = std::fs::symlink_metadata(&path)
                && meta.is_file()
                && !meta.file_type().is_symlink()
            {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    fn file_path(&self, ext_id: &str) -> Option<PathBuf> {
        if !is_valid_ext_id(ext_id) {
            return None;
        }
        Some(self.storage_dir.as_ref()?.join(format!("{ext_id}.json")))
    }

    fn persist(&self, ext_id: &str) {
        let Some(dir) = &self.storage_dir else { return };
        let Some(path) = self.file_path(ext_id) else { return };
        let Some((version, set)) = self.known_listeners.get(ext_id) else {
            self.delete_persisted(&path);
            return;
        };
        if set.is_empty() || version.is_empty() {
            self.delete_persisted(&path);
            return;
        }
        if std::fs::create_dir_all(dir).is_err() || !is_safe_dir(dir) {
            return;
        }
        let mut events: Vec<&String> = set.iter().collect();
        events.sort();
        let payload = json!({ "version": version, "events": events }).to_string();
        if payload.len() as u64 > MAX_PERSIST_BYTES {
            return;
        }
        let tmp = dir.join(format!("{ext_id}.json.tmp.{}", std::process::id()));
        if std::fs::symlink_metadata(&tmp).is_ok_and(|m| m.file_type().is_symlink()) {
            return;
        }
        if std::fs::write(&tmp, payload.as_bytes()).is_ok() {
            if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
                let _ = std::fs::remove_file(&tmp);
                return;
            }
            if std::fs::rename(&tmp, &path).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }

    fn delete_persisted(&self, path: &Path) {
        if let Ok(meta) = std::fs::symlink_metadata(path)
            && meta.is_file()
            && !meta.file_type().is_symlink()
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn is_safe_dir(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}

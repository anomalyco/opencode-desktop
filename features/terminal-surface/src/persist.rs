//! Per-workspace terminal persistence, one row per workspace in the app's database (`plugin.terminal`),
//! matching OpenCode Desktop's `packages/gui-extensions/src/terminal/state.ts` & `model.ts`:
//! `active`, `all` tabs (`id`, `title`, `titleNumber`, `customTitle`, `cols`, `rows`, `scrollY`,
//! `cursor`, and 2,000-row serialized VT `buffer`), lazy workspace hydration, and saves captured and
//! written on a thread of their own, a second after the first change. The UI thread reads and writes
//! nothing.

use std::cell::Cell;
use std::collections::HashSet;
use std::time::Duration;

use opencode_kit::client::Server;
use serde_json::{Value, json};
use terminal::{RestoreState, SessionPersistHandle, TerminalConfig, TerminalSession};

use crate::{
    HoverLink, MAX_TERMINAL_WORKSPACES, TerminalSurfaceHost, TerminalTab, TerminalWorkspace, mount, ops, workspace_key,
};

/// The storage namespace the workspaces live in (Desktop's terminal plugin's), one row each.
pub const NAMESPACE: &str = "plugin.terminal";

/// Background-thread callback that replaces every row of `namespace` starting with `prefix`:
/// keys omitted from `rows` are deleted (evicted workspaces), `Some(v)` sets, and `None` deletes.
pub type StorageBatchWriter = fn(namespace: &'static str, prefix: String, rows: Vec<(String, Option<String>)>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredPty {
    pub id: String,
    pub title: String,
    pub title_number: usize,
    pub custom_title: bool,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub scroll_y: Option<usize>,
    pub cursor: Option<i64>,
    pub buffer: Option<String>,
}

impl StoredPty {
    pub fn to_json(&self) -> Value {
        let mut obj = serde_json::Map::new();
        obj.insert("id".into(), json!(self.id));
        obj.insert("title".into(), json!(self.title));
        obj.insert("titleNumber".into(), json!(self.title_number));
        if self.custom_title {
            obj.insert("customTitle".into(), json!(true));
        }
        if let Some(c) = self.cols {
            obj.insert("cols".into(), json!(c));
        }
        if let Some(r) = self.rows {
            obj.insert("rows".into(), json!(r));
        }
        if let Some(y) = self.scroll_y {
            obj.insert("scrollY".into(), json!(y));
        }
        if let Some(cur) = self.cursor {
            obj.insert("cursor".into(), json!(cur));
        }
        if let Some(buf) = &self.buffer
            && !buf.is_empty()
        {
            obj.insert("buffer".into(), json!(buf));
        }
        Value::Object(obj)
    }

    pub fn from_json(v: &Value) -> Option<Self> {
        let id = v["id"].as_str()?.trim();
        if id.is_empty() {
            return None;
        }
        let title = v["title"].as_str().unwrap_or("").to_owned();
        let raw_num = v["titleNumber"].as_u64().unwrap_or(0) as usize;
        let title_number = if raw_num > 0 {
            raw_num
        } else {
            number_from_title(&title).unwrap_or(0)
        };
        let custom_title = v["customTitle"].as_bool().unwrap_or(false);
        let cols = v["cols"].as_u64().and_then(|n| u16::try_from(n).ok()).filter(|&n| n > 0);
        let rows = v["rows"].as_u64().and_then(|n| u16::try_from(n).ok()).filter(|&n| n > 0);
        let scroll_y = v["scrollY"].as_u64().map(|n| n as usize);
        let cursor = v["cursor"].as_i64();
        let buffer = v["buffer"].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        Some(Self {
            id: id.to_owned(),
            title,
            title_number,
            custom_title,
            cols,
            rows,
            scroll_y,
            cursor,
            buffer,
        })
    }

    pub fn to_restore_state(&self) -> RestoreState {
        RestoreState {
            buffer: self.buffer.as_deref().unwrap_or("").as_bytes().to_vec(),
            cols: self.cols,
            rows: self.rows,
            scroll_y: self.scroll_y,
            cursor: self.cursor,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoredWorkspace {
    pub active: Option<String>,
    pub all: Vec<StoredPty>,
}

impl StoredWorkspace {
    pub fn to_json(&self) -> Value {
        let all: Vec<Value> = self.all.iter().map(StoredPty::to_json).collect();
        json!({
            "active": self.active,
            "all": all,
        })
    }

    pub fn from_json(v: &Value) -> Self {
        let mut seen = HashSet::new();
        let mut all = Vec::new();
        if let Some(arr) = v["all"].as_array() {
            for item in arr {
                if let Some(pty) = StoredPty::from_json(item)
                    && seen.insert(pty.id.clone())
                {
                    all.push(pty);
                }
            }
        }
        let active = v["active"]
            .as_str()
            .filter(|id| seen.contains(*id))
            .map(str::to_owned)
            .or_else(|| all.first().map(|p| p.id.clone()));
        Self { active, all }
    }
}

pub fn number_from_title(title: &str) -> Option<usize> {
    let rest = title.strip_prefix("Terminal ")?;
    let n: usize = rest.parse().ok()?;
    (1..=MAX_TERMINAL_WORKSPACES).contains(&n).then_some(n)
}

/// A workspace's row, `workspace:<server>:<base64url(directory)>`, as the importer keyed the
/// old `terminals.json`'s.
fn row_key(server_base: &str, dir: &str) -> String {
    format!("workspace:{server_base}:{}", base64url_encode(dir.as_bytes()))
}

fn row_dir(key: &str, server_base: &str) -> Option<String> {
    let encoded = key.strip_prefix("workspace:")?.strip_prefix(server_base)?.strip_prefix(':')?;
    String::from_utf8(base64url_decode(encoded)?).ok()
}

fn base64url_encode(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((bytes.len() * 4).div_ceil(3));
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(char::from(ABC[(n >> 18) as usize & 63]));
        out.push(char::from(ABC[(n >> 12) as usize & 63]));
        if chunk.len() > 1 {
            out.push(char::from(ABC[(n >> 6) as usize & 63]));
        }
        if chunk.len() > 2 {
            out.push(char::from(ABC[n as usize & 63]));
        }
    }
    out
}

fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() % 4 == 1 {
        return None;
    }
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(u32::from(c - b'A')),
        b'a'..=b'z' => Some(u32::from(c - b'a' + 26)),
        b'0'..=b'9' => Some(u32::from(c - b'0' + 52)),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(b.len() * 3 / 4);
    for chunk in b.chunks(4) {
        let v0 = val(chunk[0])?;
        let v1 = val(chunk[1])?;
        match chunk.len() {
            2 => {
                let n = (v0 << 18) | (v1 << 12);
                out.push((n >> 16) as u8);
            }
            3 => {
                let v2 = val(chunk[2])?;
                let n = (v0 << 18) | (v1 << 12) | (v2 << 6);
                out.push((n >> 16) as u8);
                out.push((n >> 8) as u8);
            }
            _ => {
                let v2 = val(chunk[2])?;
                let v3 = val(chunk[3])?;
                let n = (v0 << 18) | (v1 << 12) | (v2 << 6) | v3;
                out.push((n >> 16) as u8);
                out.push((n >> 8) as u8);
                out.push(n as u8);
            }
        }
    }
    Some(out)
}

#[derive(Clone)]
struct PendingTabCapture {
    id: String,
    title: String,
    title_number: usize,
    custom_title: bool,
    handle: SessionPersistHandle,
}

#[derive(Clone)]
struct PendingWorkspaceCapture {
    directory: String,
    active: Option<String>,
    tabs: Vec<PendingTabCapture>,
}

fn capture_workspace(ws: &PendingWorkspaceCapture, timeout: Duration) -> StoredWorkspace {
    let mut all = Vec::with_capacity(ws.tabs.len());
    for t in &ws.tabs {
        let snap = t.handle.capture(timeout);
        all.push(StoredPty {
            id: t.id.clone(),
            title: t.title.clone(),
            title_number: t.title_number,
            custom_title: t.custom_title,
            cols: snap.as_ref().map(|p| p.cols),
            rows: snap.as_ref().map(|p| p.rows),
            scroll_y: snap.as_ref().map(|p| p.scroll_y as usize),
            cursor: snap.as_ref().map(|p| p.cursor),
            buffer: snap
                .as_ref()
                .filter(|p| !p.buffer.is_empty())
                .map(|p| String::from_utf8_lossy(&p.buffer).into_owned()),
        });
    }
    let active = ws
        .active
        .as_ref()
        .filter(|id| all.iter().any(|p| &p.id == *id))
        .cloned()
        .or_else(|| all.first().map(|p| p.id.clone()));
    StoredWorkspace { active, all }
}

/// One server's workspaces as a save found them: the ones not opened this run as they were
/// read, the open ones to capture from their terminals, and the folders whose last tab closed.
struct Snapshot {
    server: String,
    unhydrated: Vec<(String, StoredWorkspace)>,
    live: Vec<PendingWorkspaceCapture>,
    empty: Vec<String>,
    writer: StorageBatchWriter,
}

impl Snapshot {
    /// Captures the open terminals and writes each workspace's row; a row of this server no
    /// workspace holds any more goes. An unchanged row writes nothing (the storage compares).
    fn write(self) {
        let mut rows: Vec<(String, Option<String>)> = self
            .unhydrated
            .into_iter()
            .map(|(dir, sw)| (row_key(&self.server, &dir), Some(sw.to_json().to_string())))
            .collect();
        for pending in &self.live {
            let sw = capture_workspace(pending, Duration::from_millis(100));
            let key = row_key(&self.server, &pending.directory);
            rows.retain(|(k, _)| *k != key);
            rows.push((key, (!sw.all.is_empty()).then(|| sw.to_json().to_string())));
        }
        for dir in self.empty {
            let key = row_key(&self.server, &dir);
            rows.retain(|(k, _)| *k != key);
            rows.push((key, None));
        }
        let prefix = format!("workspace:{}:", self.server);
        (self.writer)(NAMESPACE, prefix, rows);
    }
}

const SAVE_DELAY: Duration = Duration::from_millis(1000);

/// The newest snapshot of each server waiting to be written, and whether a write is due. A save
/// waits a second from the first change (the storage design's terminal tier) and captures once
/// for all that came meanwhile, on a task (`rune::task::spawn_after`): no thread waits for saves.
static PENDING: std::sync::Mutex<(Vec<Snapshot>, bool)> = std::sync::Mutex::new((Vec::new(), false));
/// Held across a take and its writes, so an older capture never lands after a newer one.
static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn pending() -> std::sync::MutexGuard<'static, (Vec<Snapshot>, bool)> {
    PENDING.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn save(snapshot: Snapshot) {
    let mut p = pending();
    p.0.retain(|s| s.server != snapshot.server);
    p.0.push(snapshot);
    if p.1 {
        return;
    }
    p.1 = true;
    drop(p);
    // No task thread: the next save tries again, and a flush writes it.
    if rune::task::spawn_after("terminal-saves", SAVE_DELAY, write_pending).is_err() {
        pending().1 = false;
    }
}

fn write_pending() {
    let _writing = WRITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let waiting = {
        let mut p = pending();
        p.1 = false;
        std::mem::take(&mut p.0)
    };
    waiting.into_iter().for_each(Snapshot::write);
}

/// A window closes or the app quits: the saves waiting land in the storage now (up to a second
/// for the terminals' captures), before the storage's own flush. Nothing waiting: returns at once.
pub fn flush() {
    if pending().0.is_empty() {
        return;
    }
    #[cfg(target_arch = "wasm32")]
    write_pending();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (done, wait) = std::sync::mpsc::sync_channel(1);
        let spawned = rune::task::spawn("terminal-saves", move || {
            write_pending();
            done.send(()).ok();
        });
        if spawned.is_ok() {
            wait.recv_timeout(Duration::from_secs(1)).ok();
        }
    }
}

/// Whether the stored workspaces have been taken in (`plugin.terminal` loads after the first frame).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Saved {
    /// A run that keeps nothing.
    #[default]
    Off,
    /// Server `server`'s rows are wanted; they come in when the dock first needs a workspace.
    Waiting { server: String },
    Taken,
}

/// Asks for the saved terminal workspaces of `server_base`: their rows load on the storage
/// thread after the first frame, and come in when a dock first needs one. Nothing is read here.
pub fn load_persisted_terminals(host: &mut dyn TerminalSurfaceHost, server_base: &str) {
    if !host.persists() {
        return;
    }
    host.terminal_mut().saved = Saved::Waiting { server: server_base.to_owned() };
    host.load_storage_namespace(NAMESPACE);
}

/// The stored workspaces, once their rows are in memory.
fn take_stored_terminals(host: &mut dyn TerminalSurfaceHost) {
    let Saved::Waiting { server } = &host.terminal().saved else { return };
    let Some(rows) = host.storage_items(NAMESPACE) else { return };
    let server = server.clone();
    host.terminal_mut().saved = Saved::Taken;
    for (key, value) in rows {
        let Some(dir) = row_dir(&key, &server) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&value) else { continue };
        let sw = StoredWorkspace::from_json(&v);
        if sw.all.is_empty() {
            continue;
        }
        let key = workspace_key(&server, &dir);
        if host.terminal().workspaces.contains_key(&key) {
            continue;
        }
        let pane = host.terminal_mut();
        pane.touch_lru(&key);
        pane.stored.entry(key).or_insert(sw);
    }
}

/// Hands what the window's terminals hold for its server to the saver thread, which captures
/// the open ones (their 2,000-row buffers) and writes their rows a second later.
pub fn queue_terminal_save(host: &mut dyn TerminalSurfaceHost) {
    if !host.persists() {
        return;
    }
    take_stored_terminals(host);
    let Some(server) = host.live_server() else {
        return;
    };
    let Some(writer) = host.terminal_storage_writer() else {
        return;
    };
    let server_base = server.base.clone();
    let prefix = format!("{server_base}\0");
    let pane = host.terminal();
    let unhydrated = pane
        .stored
        .iter()
        .filter_map(|(k, sw)| Some((k.strip_prefix(&prefix)?.to_owned(), sw.clone())).filter(|(_, sw)| !sw.all.is_empty()))
        .collect();
    let mut live = Vec::new();
    let mut empty = Vec::new();
    for (k, ws) in &pane.workspaces {
        let Some(dir) = k.strip_prefix(&prefix) else {
            continue;
        };
        if ws.tabs.is_empty() {
            empty.push(dir.to_owned());
            continue;
        }
        live.push(PendingWorkspaceCapture {
            directory: dir.to_owned(),
            active: ws.active.clone(),
            tabs: ws
                .tabs
                .iter()
                .map(|t| PendingTabCapture {
                    id: t.id.clone(),
                    title: t.title.clone(),
                    title_number: t.title_number,
                    custom_title: t.custom_title,
                    handle: t.session.persist_handle(),
                })
                .collect(),
        });
    }
    save(Snapshot { server: server_base, unhydrated, live, empty, writer });
}

/// Loads a workspace on first dock access: on a live server, the session's running
/// terminals in the order this window last kept them (others the server has join at the
/// end, as in Desktop's strip); elsewhere, the stored recordings.
pub fn ensure_terminal_workspace_hydrated(host: &mut dyn TerminalSurfaceHost, ws_key: &str) -> bool {
    if host.terminal().workspaces.contains_key(ws_key) {
        return false;
    }
    take_stored_terminals(host);
    if let Some((srv, _)) = ops::live_terminals(host) {
        return hydrate_from_server(host, ws_key, &srv);
    }
    let Some(stored) = host.terminal_mut().stored.remove(ws_key) else {
        return false;
    };
    if stored.all.is_empty() {
        return false;
    }

    let theme = host.terminal_theme(ops::terminal_dark(host));
    let (_, directory) = ops::terminal_session_dir(host, ws_key);
    let waker = ops::terminal_waker(host);
    host.terminal_mut().touch_lru(ws_key);

    let tabs: Vec<TerminalTab> = stored
        .all
        .into_iter()
        .filter_map(|item| {
            let cfg = TerminalConfig {
                cols: item.cols.unwrap_or(80),
                rows: item.rows.unwrap_or(24),
                theme: theme.clone(),
                ..TerminalConfig::default()
            };
            let (session, handle) =
                TerminalSession::spawn_stream(cfg, Some(item.to_restore_state()), waker.clone()).ok()?;
            Some(TerminalTab {
                id: item.id,
                title: item.title,
                title_number: item.title_number,
                custom_title: item.custom_title,
                session,
                stream: Some(handle),
                fixture_shell: false,
                scroll_px_accum: 0.0,
                drag: mount::TextDrag::Idle,
                mount: mount::Mount::Live { connection: 0 },
                fit: Cell::new(mount::Fit::Unmeasured),
                preedit: None,
                cwd: directory.clone(),
                find: None,
                focused: Cell::new(false),
                hover: HoverLink::None,
            })
        })
        .collect();

    if tabs.is_empty() {
        return false;
    }
    let active = stored
        .active
        .filter(|id| tabs.iter().any(|t| &t.id == id))
        .or_else(|| tabs.first().map(|t| t.id.clone()));
    host.terminal_mut()
        .workspaces
        .insert(ws_key.to_owned(), TerminalWorkspace { directory, active, tabs, failure: None });
    true
}

fn hydrate_from_server(host: &mut dyn TerminalSurfaceHost, ws_key: &str, srv: &Server) -> bool {
    let stored = host.terminal_mut().stored.remove(ws_key).unwrap_or_default();
    let (session_id, directory) = ops::terminal_session_dir(host, ws_key);
    host.terminal_mut().touch_lru(ws_key);
    let mut ws = TerminalWorkspace { directory, ..TerminalWorkspace::default() };
    match srv.terminal_list(&session_id) {
        Err(e) => ws.failure = Some(e.to_string()),
        Ok(mut list) => {
            list.retain(|info| info.status == opencode_kit::pty::PtyStatus::Running);
            let rank = |id: &str| stored.all.iter().position(|p| p.id == id).unwrap_or(usize::MAX);
            list.sort_by_key(|info| rank(&info.id));
            let h: &dyn TerminalSurfaceHost = &*host;
            ws.tabs = list.into_iter().filter_map(|info| ops::persistent_tab(h, srv.clone(), info).ok()).collect();
            ws.active = stored
                .active
                .filter(|id| ws.tabs.iter().any(|t| &t.id == id))
                .or_else(|| ws.tabs.first().map(|t| t.id.clone()));
        }
    }
    let loaded = !ws.tabs.is_empty();
    host.terminal_mut().workspaces.insert(ws_key.to_owned(), ws);
    loaded
}

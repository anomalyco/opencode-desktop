//! Per-workspace terminal persistence, one row per workspace in the app's database (`plugin.terminal`),
//! matching OpenCode Desktop's `packages/gui-extensions/src/terminal/state.ts` & `model.ts`:
//! `active`, `all` tabs (`id`, `title`, `titleNumber`, `customTitle`, `cols`, `rows`, `scrollY`,
//! `cursor`, and 2,000-row serialized VT `buffer`), lazy workspace hydration, and saves captured and
//! written on a thread of their own, a second after the first change. The UI thread reads and writes
//! nothing.

use std::collections::HashSet;
use std::time::Duration;

use serde_json::{Value, json};
use terminal::{RestoreState, SessionPersistHandle, TerminalConfig, TerminalSession};

use super::{
    MAX_TERMINAL_WORKSPACES, TerminalTab, TerminalWorkspace, theme_from_pal, workspace_key,
};
use crate::palette::pal;
use crate::{App, Source};

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

/// The storage namespace the workspaces live in (Desktop's terminal plugin's), one row each.
const NAMESPACE: &str = "plugin.terminal";

/// A workspace's row, `workspace:<server>:<base64url(directory)>`, as the importer keyed the
/// old `terminals.json`'s.
fn row_key(server_base: &str, dir: &str) -> String {
    use base64::Engine;
    format!("workspace:{server_base}:{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(dir))
}

fn row_dir(key: &str, server_base: &str) -> Option<String> {
    use base64::Engine;
    let encoded = key.strip_prefix("workspace:")?.strip_prefix(server_base)?.strip_prefix(':')?;
    String::from_utf8(base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded).ok()?).ok()
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
}

impl Snapshot {
    /// Captures the open terminals and writes each workspace's row; a row of this server no
    /// workspace holds any more goes. An unchanged row writes nothing (the storage compares).
    fn write(self) {
        let mut rows: Vec<(String, Option<String>)> = self.unhydrated.into_iter().map(|(dir, sw)| (row_key(&self.server, &dir), Some(sw.to_json().to_string()))).collect();
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
        crate::storage::with_storage(move |s| {
            // Evicted workspaces (past the 20 kept) leave with the save that dropped them.
            for (key, _) in s.state().items(NAMESPACE).unwrap_or_default() {
                if key.starts_with(&prefix) && !rows.iter().any(|(k, _)| *k == key) {
                    s.state().delete(NAMESPACE, &key);
                }
            }
            for (key, value) in rows {
                match value {
                    Some(v) => s.state().set(NAMESPACE, &key, v),
                    None => s.state().delete(NAMESPACE, &key),
                };
            }
        });
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
    let (done, wait) = std::sync::mpsc::sync_channel(1);
    let spawned = rune::task::spawn("terminal-saves", move || {
        write_pending();
        let _ = done.send(());
    });
    if spawned.is_ok() {
        let _ = wait.recv_timeout(Duration::from_secs(1));
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

impl App {
    /// Asks for the saved terminal workspaces of `server_base`: their rows load on the storage
    /// thread after the first frame, and come in when a dock first needs one. Nothing is read here.
    pub fn load_persisted_terminals(&mut self, server_base: &str) {
        if !self.persists() {
            return;
        }
        self.terminal.saved = Saved::Waiting { server: server_base.to_owned() };
        crate::storage::with_storage(|s| s.state().load(&[NAMESPACE], None));
    }

    /// The stored workspaces, once their rows are in memory.
    fn take_stored_terminals(&mut self) {
        let Saved::Waiting { server } = &self.terminal.saved else { return };
        let Some(rows) = crate::storage::items(NAMESPACE) else { return };
        let server = server.clone();
        self.terminal.saved = Saved::Taken;
        for (key, value) in rows {
            let Some(dir) = row_dir(&key, &server) else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&value) else { continue };
            let sw = StoredWorkspace::from_json(&v);
            if sw.all.is_empty() {
                continue;
            }
            let key = workspace_key(&server, &dir);
            if self.terminal.workspaces.contains_key(&key) {
                continue;
            }
            self.terminal.touch_lru(&key);
            self.terminal.stored.entry(key).or_insert(sw);
        }
    }

    /// Hands what the window's terminals hold for its server to the saver thread, which captures
    /// the open ones (their 2,000-row buffers) and writes their rows a second later.
    pub fn queue_terminal_save(&mut self) {
        if !self.persists() {
            return;
        }
        self.take_stored_terminals();
        let Source::Live { server, .. } = &*self.source else {
            return;
        };
        let server_base = server.base.clone();
        let prefix = format!("{server_base}\0");
        let unhydrated = self.terminal.stored.iter().filter_map(|(k, sw)| Some((k.strip_prefix(&prefix)?.to_owned(), sw.clone())).filter(|(_, sw)| !sw.all.is_empty())).collect();
        let mut live = Vec::new();
        let mut empty = Vec::new();
        for (k, ws) in &self.terminal.workspaces {
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
                    .map(|t| PendingTabCapture { id: t.id.clone(), title: t.title.clone(), title_number: t.title_number, custom_title: t.custom_title, handle: t.session.persist_handle() })
                    .collect(),
            });
        }
        save(Snapshot { server: server_base, unhydrated, live, empty });
    }

    /// Loads a workspace on first dock access: on a live server, the session's running
    /// terminals in the order this window last kept them (others the server has join at the
    /// end, as in Desktop's strip); elsewhere, the stored recordings.
    pub fn ensure_terminal_workspace_hydrated(&mut self, ws_key: &str) -> bool {
        if self.terminal.workspaces.contains_key(ws_key) {
            return false;
        }
        self.take_stored_terminals();
        if let Some((srv, _)) = self.live_terminals() {
            return self.hydrate_from_server(ws_key, &srv);
        }
        let Some(stored) = self.terminal.stored.remove(ws_key) else {
            return false;
        };
        if stored.all.is_empty() {
            return false;
        }

        let theme = theme_from_pal(pal(self.terminal_dark()));
        let (_, directory) = self.terminal_session_dir(ws_key);
        self.terminal.touch_lru(ws_key);
        let waker = self.terminal_waker();

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
                    drag: super::mount::TextDrag::Idle,
                    mount: super::mount::Mount::Live { connection: 0 },
                    fit: std::cell::Cell::new(super::mount::Fit::Unmeasured),
                    preedit: None,
                    cwd: directory.clone(),
                    find: None,
                    focused: std::cell::Cell::new(false),
                    hover: None,
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
        self.terminal.workspaces.insert(
            ws_key.to_owned(),
            TerminalWorkspace { directory, active, tabs, failure: None },
        );
        true
    }

    fn hydrate_from_server(&mut self, ws_key: &str, srv: &opencode_kit::client::Server) -> bool {
        let stored = self.terminal.stored.remove(ws_key).unwrap_or_default();
        let (session_id, directory) = self.terminal_session_dir(ws_key);
        self.terminal.touch_lru(ws_key);
        let mut ws = TerminalWorkspace { directory, ..TerminalWorkspace::default() };
        match srv.terminal_list(&session_id) {
            Err(e) => ws.failure = Some(e.to_string()),
            Ok(mut list) => {
                list.retain(|info| info.status == opencode_kit::pty::PtyStatus::Running);
                let rank = |id: &str| stored.all.iter().position(|p| p.id == id).unwrap_or(usize::MAX);
                list.sort_by_key(|info| rank(&info.id));
                ws.tabs = list.into_iter().filter_map(|info| self.persistent_tab(srv.clone(), info).ok()).collect();
                ws.active = stored
                    .active
                    .filter(|id| ws.tabs.iter().any(|t| &t.id == id))
                    .or_else(|| ws.tabs.first().map(|t| t.id.clone()));
            }
        }
        let loaded = !ws.tabs.is_empty();
        self.terminal.workspaces.insert(ws_key.to_owned(), ws);
        loaded
    }
}

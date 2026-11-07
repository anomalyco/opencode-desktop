//! Per-workspace terminal persistence (`<data dir>/rune-opencode/terminals.json`), matching
//! OpenCode Desktop's `packages/gui-extensions/src/terminal/state.ts` & `model.ts`:
//! `active`, `all` tabs (`id`, `title`, `titleNumber`, `customTitle`, `cols`, `rows`, `scrollY`,
//! `cursor`, and 2,000-row serialized VT `buffer`), lazy workspace hydration, and background saves.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
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

fn file() -> Option<PathBuf> {
    crate::dirs::data().map(|d| d.join("terminals.json"))
}

fn read_disk() -> Value {
    file()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}))
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

impl App {
    /// Loads saved terminal workspaces for `server_base` from `<data dir>/rune-opencode/terminals.json`
    /// without spawning worker threads until a workspace's dock is actually shown.
    pub fn load_persisted_terminals(&mut self, server_base: &str) {
        let root = read_disk();
        self.restore_terminals_from_json(server_base, &root[server_base]);
    }

    /// Populates lazy `stored` workspace entries for `server_base` from a serialized JSON map
    /// (`{ directory: { active, all } }`).
    pub fn restore_terminals_from_json(&mut self, server_base: &str, v: &Value) {
        let Some(obj) = v.as_object() else { return };
        for (dir, ws_val) in obj {
            let sw = StoredWorkspace::from_json(ws_val);
            if sw.all.is_empty() {
                continue;
            }
            let key = workspace_key(server_base, dir);
            self.terminal.touch_lru(&key);
            self.terminal.stored.insert(key, sw);
        }
    }

    /// Captures all workspaces for `server_base` (`{ directory: { active, all } }`), including
    /// 2,000-row serialized VT buffers from active worker threads.
    pub fn terminals_to_json(&self, server_base: &str) -> Value {
        let prefix = format!("{server_base}\0");
        let mut out = serde_json::Map::new();

        for (key, sw) in &self.terminal.stored {
            if let Some(dir) = key.strip_prefix(&prefix)
                && !sw.all.is_empty()
            {
                out.insert(dir.to_owned(), sw.to_json());
            }
        }

        for (key, ws) in &self.terminal.workspaces {
            let Some(dir) = key.strip_prefix(&prefix) else {
                continue;
            };
            if ws.tabs.is_empty() {
                out.remove(dir);
                continue;
            }
            let pending = PendingWorkspaceCapture {
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
            };
            let captured = capture_workspace(&pending, Duration::from_millis(120));
            out.insert(dir.to_owned(), captured.to_json());
        }

        Value::Object(out)
    }

    /// Schedules an off-main-thread snapshot + atomic replace of `terminals.json` on `opencode-saves`.
    pub fn queue_terminal_save(&self) {
        if !self.persists() {
            return;
        }
        let Source::Live { server, .. } = &self.source else {
            return;
        };
        let Some(path) = file() else { return };
        let server_base = server.base.clone();
        let prefix = format!("{server_base}\0");

        let mut unhydrated: HashMap<String, StoredWorkspace> = HashMap::new();
        for (k, sw) in &self.terminal.stored {
            if let Some(dir) = k.strip_prefix(&prefix)
                && !sw.all.is_empty()
            {
                unhydrated.insert(dir.to_owned(), sw.clone());
            }
        }

        let mut live_captures: Vec<PendingWorkspaceCapture> = Vec::new();
        let mut empty_dirs: Vec<String> = Vec::new();
        for (k, ws) in &self.terminal.workspaces {
            let Some(dir) = k.strip_prefix(&prefix) else {
                continue;
            };
            if ws.tabs.is_empty() {
                empty_dirs.push(dir.to_owned());
                continue;
            }
            live_captures.push(PendingWorkspaceCapture {
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

        crate::disk::later("terminals", move || {
            let mut server_map = serde_json::Map::new();
            for (dir, sw) in unhydrated {
                server_map.insert(dir, sw.to_json());
            }
            for pending in &live_captures {
                let sw = capture_workspace(pending, Duration::from_millis(100));
                if !sw.all.is_empty() {
                    server_map.insert(pending.directory.clone(), sw.to_json());
                }
            }
            for empty in empty_dirs {
                server_map.remove(&empty);
            }
            let mut root = read_disk();
            root[server_base] = Value::Object(server_map);
            crate::disk::replace(&path, root.to_string().as_bytes());
        });
    }

    /// Loads a workspace on first dock access: on a live server, the session's running
    /// terminals in the order this window last kept them (others the server has join at the
    /// end, as in Desktop's strip); elsewhere, the stored recordings.
    pub fn ensure_terminal_workspace_hydrated(&mut self, ws_key: &str) -> bool {
        if self.terminal.workspaces.contains_key(ws_key) {
            return false;
        }
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
                    drag_anchor: None,
                    needs_recovery: false,
                    recovery_buffer: Vec::new(),
                    last_fit: std::cell::Cell::new((0, 0, 0, 0)),
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
            Err(e) => ws.failure = Some(e),
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

//! Per-folder `rune` client MCP lifecycle, multi-window connection sharing, and session routing.
//!
//! OpenCode (Rune) attaches one WebSocket per open folder (`name=rune`) on the connected server.
//! Multiple windows open on the same folder share one [`ClientMcp`] connection through
//! [`RuneMcpHub`], and every incoming `tools/call` is routed by `_meta["ai.opencode/sessionID"]`
//! (and its parent chain) to the window and session tab showing it.

pub mod tools;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use opencode_kit::client::{DEFAULT_SERVER, Server};
use opencode_kit::client_mcp::{
    ClientMcp, ClientMcpOptions, McpCall, McpResult, RECONNECT_MAX, RECONNECT_MIN,
    UNSUPPORTED_RETRY,
};
use rune::{Tx, Waker};

use crate::projects::clean_dir;
use crate::{App, Route, Source, flag};

pub const MCP_NAME: &str = "rune";
pub const SESSION_NOT_OPEN_ERROR: &str = "This session isn't open in OpenCode (Rune)";
/// Sent in `initialize.result.instructions` for forward compatibility when OpenCode surfaces
/// instructions from attached client MCP sockets alongside configured MCP servers.
pub const MCP_INSTRUCTIONS: &str = "OpenCode (Rune) UI tools for sessions open in OpenCode (Rune). Load the `rune-ui` skill before writing ```ui outlines.";

static NEXT_WINDOW_ID: AtomicU64 = AtomicU64::new(1);
static GLOBAL_HUB: OnceLock<Arc<RuneMcpHub>> = OnceLock::new();

type ServerKey = (String, u64);
type FolderKey = (ServerKey, String);

fn global_hub() -> Arc<RuneMcpHub> {
    GLOBAL_HUB.get_or_init(RuneMcpHub::new).clone()
}

pub struct RoutedCall {
    pub target_session: String,
    pub call: McpCall,
}

struct WindowEntry {
    server: Option<Server>,
    folders: BTreeSet<String>,
    routed_exact: Option<String>,
    routed_root: Option<String>,
    open_roots: HashSet<String>,
    known_parents: HashMap<String, Option<String>>,
    tx: Sender<RoutedCall>,
    waker: Waker,
}

struct HubInner {
    windows: HashMap<u64, WindowEntry>,
    connections: HashMap<FolderKey, ClientMcp>,
    routers: HashMap<ServerKey, Sender<McpCall>>,
    /// The plugin tools generation the open connections listed (`plugin_host::mcp_tools_generation`).
    tools_generation: u64,
    reconnect_min: Duration,
    reconnect_max: Duration,
    unsupported_retry: Duration,
}

/// Coordinates per-folder `rune` client MCP WebSockets across all open windows in the process.
pub struct RuneMcpHub {
    inner: Mutex<HubInner>,
}

impl RuneMcpHub {
    pub fn new() -> Arc<Self> {
        Self::with_timings(RECONNECT_MIN, RECONNECT_MAX, UNSUPPORTED_RETRY)
    }

    pub fn with_timings(reconnect_min: Duration, reconnect_max: Duration, unsupported_retry: Duration) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(HubInner {
                windows: HashMap::new(),
                connections: HashMap::new(),
                routers: HashMap::new(),
                tools_generation: crate::plugin_host::mcp_tools_generation(),
                reconnect_min,
                reconnect_max,
                unsupported_retry,
            }),
        })
    }

    fn update_window(self: &Arc<Self>, window_id: u64, entry: WindowEntry) {
        let to_drop = {
            let mut inner = lock(&self.inner);
            inner.windows.insert(window_id, entry);
            Self::reconcile_locked(self, &mut inner)
        };
        close_later(to_drop);
    }

    fn remove_window(self: &Arc<Self>, window_id: u64) {
        let to_drop = {
            let mut inner = lock(&self.inner);
            if inner.windows.remove(&window_id).is_none() {
                return;
            }
            Self::reconcile_locked(self, &mut inner)
        };
        close_later(to_drop);
    }

    fn reconcile_locked(hub: &Arc<Self>, inner: &mut HubInner) -> Vec<ClientMcp> {
        let mut wanted: HashMap<FolderKey, Server> = HashMap::new();
        for win in inner.windows.values() {
            let Some(srv) = &win.server else { continue };
            let ep = srv.endpoint_key();
            for dir in &win.folders {
                if !dir.is_empty() {
                    wanted.entry((ep.clone(), dir.clone())).or_insert_with(|| srv.clone());
                }
            }
        }

        let mut removed = Vec::new();
        // A connection lists its tools once, and advertises no `listChanged` (as Desktop's
        // `connectClientMcp`), so a plugin adding, removing or reloading a tool reconnects every
        // folder, as Desktop's client MCP reconnects when what it serves changes.
        let generation = crate::plugin_host::mcp_tools_generation();
        let retool = std::mem::replace(&mut inner.tools_generation, generation) != generation;
        let stale: Vec<FolderKey> = inner.connections.keys().filter(|k| retool || !wanted.contains_key(*k)).cloned().collect();
        for k in stale {
            if let Some(conn) = inner.connections.remove(&k) {
                removed.push(conn);
            }
        }

        inner
            .routers
            .retain(|srv_key, _| wanted.keys().any(|(ep, _)| ep == srv_key));

        let tool_defs = tools::catalog();
        for (key, srv) in wanted {
            if inner.connections.contains_key(&key) {
                continue;
            }
            let srv_key = key.0.clone();
            let call_tx = match inner.routers.get(&srv_key) {
                Some(tx) => tx.clone(),
                None => {
                    let (tx, rx) = channel();
                    let weak = Arc::downgrade(hub);
                    let router_srv = srv.clone();
                    std::thread::Builder::new()
                        .name("desktop-rune-mcp-router".into())
                        .spawn(move || run_router(weak, router_srv, rx))
                        .ok();
                    inner.routers.insert(srv_key, tx.clone());
                    tx
                }
            };

            let dir = key.1.clone();
            let mut opts = ClientMcpOptions::new(MCP_NAME, dir, tool_defs.clone()).with_instructions(MCP_INSTRUCTIONS);
            opts.reconnect_min = inner.reconnect_min;
            opts.reconnect_max = inner.reconnect_max;
            opts.unsupported_retry = inner.unsupported_retry;

            let weak_for_wake = Arc::downgrade(hub);
            let wake_srv = srv.clone();
            let waker: Waker = Arc::new(move || {
                if let Some(h) = weak_for_wake.upgrade() {
                    let wakers: Vec<Waker> = lock(&h.inner)
                        .windows
                        .values()
                        .filter(|w| w.server.as_ref().is_some_and(|s| s.same_target(&wake_srv)))
                        .map(|w| w.waker.clone())
                        .collect();
                    for w in wakers {
                        w();
                    }
                }
            });

            let conn = srv.connect_client_mcp_with(opts, call_tx, waker);
            inner.connections.insert(key, conn);
        }

        removed
    }
}

fn run_router(weak_hub: Weak<RuneMcpHub>, server: Server, rx: Receiver<McpCall>) {
    let mut parent_cache: HashMap<(String, String), Option<String>> = HashMap::new();
    while let Ok(call) = rx.recv() {
        let Some(hub) = weak_hub.upgrade() else {
            call.respond(McpResult::error(SESSION_NOT_OPEN_ERROR));
            break;
        };
        let Some(sid) = call.session_id.clone().filter(|s| !s.is_empty()) else {
            call.respond(McpResult::error(SESSION_NOT_OPEN_ERROR));
            continue;
        };

        if let Some((tx, waker, target_session)) = match_window(&hub, &server, &sid, &parent_cache) {
            if tx.send(RoutedCall { target_session, call }).is_ok() {
                waker();
            }
            continue;
        }

        // Subagent sessions may call a `rune` tool milliseconds before their `SessionInfo` arrives on
        // SSE. Walk `/api/session/<id>` off-thread to discover ancestors if any window has open tabs.
        if should_probe_ancestors(&hub, &server, &sid, &parent_cache) {
            fetch_parent_chain(&server, &sid, &mut parent_cache);
            if let Some((tx, waker, target_session)) = match_window(&hub, &server, &sid, &parent_cache) {
                if tx.send(RoutedCall { target_session, call }).is_ok() {
                    waker();
                }
                continue;
            }
        }

        call.respond(McpResult::error(SESSION_NOT_OPEN_ERROR));
    }
}

fn match_window(
    hub: &RuneMcpHub,
    server: &Server,
    sid: &str,
    extra_parents: &HashMap<(String, String), Option<String>>,
) -> Option<(Sender<RoutedCall>, Waker, String)> {
    let inner = lock(&hub.inner);
    let mut merged_parents: HashMap<&str, Option<&str>> = HashMap::new();
    for ((base, k), v) in extra_parents {
        if base == &server.base {
            merged_parents.insert(k.as_str(), v.as_deref());
        }
    }
    for win in inner.windows.values() {
        if win.server.as_ref().is_some_and(|s| s.same_target(server)) {
            for (k, v) in &win.known_parents {
                merged_parents.insert(k.as_str(), v.as_deref());
            }
        }
    }

    let root = resolve_root(sid, &merged_parents);
    let mut best: Option<(u8, u64, Sender<RoutedCall>, Waker, String)> = None;

    for (&wid, win) in &inner.windows {
        if !win.server.as_ref().is_some_and(|s| s.same_target(server)) {
            continue;
        }
        let score = if win.routed_exact.as_deref() == Some(sid) {
            4
        } else if win.routed_root.as_deref() == Some(root) || win.routed_exact.as_deref() == Some(root) {
            3
        } else if win.open_roots.contains(sid) {
            2
        } else if win.open_roots.contains(root) {
            1
        } else {
            0
        };
        if score == 0 {
            continue;
        }
        let target = if win.routed_exact.as_deref() == Some(sid) || win.open_roots.contains(sid) {
            sid.to_owned()
        } else if let Some(routed) = &win.routed_exact
            && resolve_root(routed, &merged_parents) == root
        {
            routed.clone()
        } else {
            root.to_owned()
        };
        let replace = match &best {
            None => true,
            Some((best_score, best_wid, ..)) => score > *best_score || (score == *best_score && wid < *best_wid),
        };
        if replace {
            best = Some((score, wid, win.tx.clone(), win.waker.clone(), target));
        }
    }

    best.map(|(_, _, tx, waker, target)| (tx, waker, target))
}

fn resolve_root<'a>(sid: &'a str, parents: &HashMap<&'a str, Option<&'a str>>) -> &'a str {
    let mut cur = sid;
    for _ in 0..32 {
        match parents.get(cur).copied().flatten() {
            Some(p) if !p.is_empty() && p != cur => cur = p,
            _ => break,
        }
    }
    cur
}

fn should_probe_ancestors(
    hub: &RuneMcpHub,
    server: &Server,
    sid: &str,
    cache: &HashMap<(String, String), Option<String>>,
) -> bool {
    if cache.contains_key(&(server.base.clone(), sid.to_owned())) {
        return false;
    }
    let inner = lock(&hub.inner);
    let mut has_open = false;
    for win in inner.windows.values() {
        if !win.server.as_ref().is_some_and(|s| s.same_target(server)) {
            continue;
        }
        if win.known_parents.contains_key(sid) {
            return false;
        }
        if !win.open_roots.is_empty() {
            has_open = true;
        }
    }
    has_open
}

fn fetch_parent_chain(server: &Server, sid: &str, cache: &mut HashMap<(String, String), Option<String>>) {
    if cache.len() > 512 {
        cache.clear();
    }
    let mut cur = sid.to_owned();
    for _ in 0..8 {
        let key = (server.base.clone(), cur.clone());
        if let Some(cached) = cache.get(&key) {
            match cached {
                Some(next) => {
                    cur = next.clone();
                    continue;
                }
                None => break,
            }
        }
        let Ok(v) = server.get_timeout(&format!("/api/session/{cur}"), Duration::from_millis(750)) else {
            break;
        };
        let data = v.get("data").unwrap_or(&v);
        let parent = data["parentID"].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        cache.insert(key, parent.clone());
        match parent {
            Some(next) if next != cur => cur = next,
            _ => break,
        }
    }
}

/// Per-window client MCP state stored on [`App`].
pub struct WindowMcp {
    window_id: u64,
    hub: Arc<RuneMcpHub>,
    waker: Waker,
    tx: Sender<RoutedCall>,
    rx: Receiver<RoutedCall>,
    last_stamp: Option<u64>,
    force_enabled: bool,
}

impl Default for WindowMcp {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            window_id: NEXT_WINDOW_ID.fetch_add(1, Ordering::Relaxed),
            hub: global_hub(),
            waker: Arc::new(|| {}),
            tx,
            rx,
            last_stamp: None,
            force_enabled: false,
        }
    }
}

impl Drop for WindowMcp {
    fn drop(&mut self) {
        self.hub.remove_window(self.window_id);
    }
}

impl App {
    pub fn setup_rune_mcp(&mut self, waker: Waker) {
        self.rune_mcp.waker = waker;
        self.rune_mcp.last_stamp = None;
        sync_window(self);
    }

    pub fn close_rune_mcp(&mut self) {
        self.rune_mcp.hub.remove_window(self.rune_mcp.window_id);
        self.rune_mcp.last_stamp = None;
    }

}

pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    sync_window(s);
    let mut changed = false;
    while let Ok(routed) = s.rune_mcp.rx.try_recv() {
        tools::execute(s, &routed.target_session, routed.call, tx);
        changed = true;
    }
    if crate::plugin_host::commands::expire_calls(tx) {
        changed = true;
    }
    changed
}

fn mcp_active(s: &App) -> Option<&Server> {
    if crate::launch::waiting(s) || s.stage.is_some() {
        return None;
    }
    let Source::Live { server, allow_send, .. } = &s.source else { return None };
    if s.rune_mcp.force_enabled {
        return Some(server);
    }
    // Headless `--client-mcp` is strictly for local mock servers, never the default `:4096` server.
    if crate::headless() {
        return (flag("--client-mcp") && server.base != DEFAULT_SERVER).then_some(server);
    }
    (*allow_send).then_some(server)
}

fn sync_stamp(s: &App, active_srv: Option<&Server>) -> u64 {
    let mut h = DefaultHasher::new();
    match active_srv {
        Some(srv) => {
            1u8.hash(&mut h);
            srv.endpoint_key().hash(&mut h);
        }
        None => 0u8.hash(&mut h),
    }
    crate::plugin_host::mcp_tools_generation().hash(&mut h);
    s.route.hash(&mut h);
    s.draft_dir().hash(&mut h);
    s.project_state.used.hash(&mut h);
    s.projects.len().hash(&mut h);
    for p in &s.projects {
        p.id.hash(&mut h);
        p.directory.hash(&mut h);
    }
    s.tabs.len().hash(&mut h);
    for t in &s.tabs {
        t.id.hash(&mut h);
        t.directory.hash(&mut h);
    }
    s.sessions.len().hash(&mut h);
    for info in &s.sessions {
        info.id.hash(&mut h);
        info.parent.hash(&mut h);
        info.directory.hash(&mut h);
    }
    h.finish()
}

fn sync_window(s: &mut App) {
    let active_srv = mcp_active(s).cloned();
    let stamp = sync_stamp(s, active_srv.as_ref());
    if s.rune_mcp.last_stamp == Some(stamp) {
        return;
    }
    s.rune_mcp.last_stamp = Some(stamp);

    let Some(server) = active_srv else {
        s.rune_mcp.hub.remove_window(s.rune_mcp.window_id);
        return;
    };

    let mut folders = BTreeSet::new();
    for p in s.used_projects() {
        let d = clean_dir(&p.directory);
        if !d.is_empty() {
            folders.insert(d);
        }
    }
    for t in &s.tabs {
        let raw = s.session(&t.id).map_or(t.directory.as_str(), |i| i.directory.as_str());
        let d = clean_dir(raw);
        if !d.is_empty() {
            folders.insert(d);
        }
    }
    match &s.route {
        Route::Session { id: sid, .. } => {
            if let Some(info) = s.session(sid) {
                let d = clean_dir(&info.directory);
                if !d.is_empty() {
                    folders.insert(d);
                }
            }
        }
        Route::Draft(_) => {
            let d = clean_dir(s.draft_dir());
            if !d.is_empty() {
                folders.insert(d);
            }
        }
        Route::Home => {}
    }

    let mut open_roots: HashSet<String> = s.tabs.iter().map(|t| t.id.clone()).collect();
    let (routed_exact, routed_root) = match &s.route {
        Route::Session { id: sid, .. } => {
            let root = s.root_of(sid).to_owned();
            open_roots.insert(sid.clone());
            open_roots.insert(root.clone());
            (Some(sid.clone()), Some(root))
        }
        _ => (None, None),
    };

    let known_parents: HashMap<String, Option<String>> =
        s.sessions.iter().map(|i| (i.id.clone(), i.parent.clone())).collect();

    let entry = WindowEntry {
        server: Some(server),
        folders,
        routed_exact,
        routed_root,
        open_roots,
        known_parents,
        tx: s.rune_mcp.tx.clone(),
        waker: s.rune_mcp.waker.clone(),
    };
    s.rune_mcp.hub.update_window(s.rune_mcp.window_id, entry);
}

/// Closes connections a window no longer wants on a thread of their own: a close joins the
/// connection's thread after its WebSocket close, which took 330-820 ms on the UI thread for every
/// tab switch to another server (the window's folders move to that server's connections).
fn close_later(conns: Vec<ClientMcp>) {
    if conns.is_empty() {
        return;
    }
    let _ = std::thread::Builder::new().name("desktop-rune-mcp-close".into()).spawn(move || drop(conns));
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

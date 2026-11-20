//! Per-session `experimental.browser` v4 attachment state, bijective `tab_<uuid> <-> host_id`
//! isolation, per-tab element refs, diagnostics, captures, and ordered FIFO outbound worker.

use std::collections::{BTreeMap, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use browser_pane::tools::diagnostics::TabDiagnostics;
use opencode_kit::browser_rpc::{
    AbortHandle, AttachMachine, BrowserState, LocationRef, Outcome, RpcCallError, Tab,
    failure_code, new_tab_id,
};
use opencode_kit::client::Server;
use opencode_kit::servers::ServerKey;

use super::exec::ActiveCommand;
use crate::App;

const STATE_RETRY_MIN_MS: u64 = 250;
const STATE_RETRY_MAX_MS: u64 = 10_000;

/// Messages sent from the UI thread to one attachment's ordered FIFO HTTP worker.
/// Putting both `State` and `Result` on the same channel guarantees a `Result` naming a newly
/// created tab never reaches the server ahead of the `State` inventory registering it.
pub(super) enum OutboundMsg {
    State(BrowserState),
    Result { request_id: String, outcome: Outcome },
    Stop,
}

/// Element ref mappings for one tab (`eN <-> backendDOMNodeId`). Invalidated whenever the tab's
/// document generation changes; user-picked refs outlive snapshots until navigation.
#[derive(Default)]
pub(super) struct TabRefs {
    pub ref_to_backend: HashMap<String, i64>,
    pub backend_to_ref: HashMap<i64, String>,
    pub picked: HashMap<i64, String>,
}

impl TabRefs {
    pub fn clear_on_nav(&mut self) {
        self.ref_to_backend.clear();
        self.backend_to_ref.clear();
        self.picked.clear();
    }

    pub fn update_from_snapshot(&mut self, refs: BTreeMap<String, i64>) {
        self.ref_to_backend.clear();
        self.backend_to_ref.clear();
        for (k, id) in refs {
            self.backend_to_ref.insert(id, k.clone());
            self.ref_to_backend.insert(k, id);
        }
    }

    pub fn resolve(&self, raw: &str) -> Option<i64> {
        let norm = opencode_kit::browser_rpc::normalize_ref(raw)?;
        self.ref_to_backend.get(&norm).copied()
    }
}

/// One desktop-owned capture artifact (screenshot, trace, CPU profile, heap snapshot) owned by a
/// single tab inside a single session.
#[derive(Clone)]
pub(super) struct CapturedFile {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
    pub resources: Vec<String>,
}

/// Complete per-session state for one `experimental.browser` v4 attachment.
/// Isolation invariant: every tab lookup checks BOTH `wire_to_host` (owned by this session) AND
/// `app.browser_tab_session(host_id) == Some(self.session_id)`.
pub(super) struct SessionAttachment {
    pub server_key: ServerKey,
    pub session_id: String,
    pub location: LocationRef,
    pub server: Server,
    pub attach: AttachMachine,
    pub attach_abort: Option<(u64, AbortHandle)>,
    pub outbound: Option<(String, Sender<OutboundMsg>, AbortHandle)>,
    pub wire_to_host: BTreeMap<String, u64>,
    pub host_to_wire: BTreeMap<u64, String>,
    pub generations: HashMap<u64, u64>,
    pub last_urls: HashMap<u64, String>,
    pub refs: HashMap<u64, TabRefs>,
    pub diagnostics: HashMap<u64, TabDiagnostics>,
    pub captures: HashMap<u64, Vec<CapturedFile>>,
    pub agent_tab_id: Option<String>,
    pub active_commands: Vec<ActiveCommand>,
    pub diag_scope_seed: u64,
}

impl SessionAttachment {
    pub fn new(
        server_key: ServerKey,
        session_id: String,
        location: LocationRef,
        server: Server,
        diag_scope_seed: u64,
    ) -> Self {
        Self {
            server_key,
            session_id,
            location,
            server,
            attach: AttachMachine::new(),
            attach_abort: None,
            outbound: None,
            wire_to_host: BTreeMap::new(),
            host_to_wire: BTreeMap::new(),
            generations: HashMap::new(),
            last_urls: HashMap::new(),
            refs: HashMap::new(),
            diagnostics: HashMap::new(),
            captures: HashMap::new(),
            agent_tab_id: None,
            active_commands: Vec::new(),
            diag_scope_seed,
        }
    }

    pub fn stop_outbound(&mut self) {
        if let Some((_, tx, abort)) = self.outbound.take() {
            abort.abort();
            let _ = tx.send(OutboundMsg::Stop);
        }
    }

    pub fn ensure_outbound(&mut self, connection_id: &str) {
        if self.outbound.as_ref().is_some_and(|(cid, _, _)| cid == connection_id) {
            return;
        }
        self.stop_outbound();
        let (tx, rx) = mpsc::channel();
        let abort = AbortHandle::new();
        spawn_outbound_worker(
            self.server.clone(),
            self.location.clone(),
            self.session_id.clone(),
            connection_id.to_owned(),
            rx,
            abort.clone(),
        );
        self.outbound = Some((connection_id.to_owned(), tx, abort));
    }

    pub fn enqueue_state(&mut self, connection_id: &str, state: BrowserState, hash: u64) {
        self.ensure_outbound(connection_id);
        self.attach.mark_state_published(hash);
        if let Some((_, tx, _)) = &self.outbound {
            let _ = tx.send(OutboundMsg::State(state));
        }
    }

    pub fn enqueue_result(&mut self, connection_id: &str, request_id: String, outcome: Outcome) {
        self.ensure_outbound(connection_id);
        if let Some((_, tx, _)) = &self.outbound {
            let _ = tx.send(OutboundMsg::Result { request_id, outcome });
        }
    }

    /// Reconciles this session's `tab_<uuid> <-> host_tab_id` map against `BrowserHost` strictly
    /// scoped to `self.session_id`, bumping `generation` on document URL changes and returning
    /// `(BrowserState, hash)`.
    pub fn sync_tabs(&mut self, app: &App) -> (BrowserState, u64) {
        let host_ids: Vec<u64> = app
            .workspace_panes
            .browser
            .session_tabs
            .get(&self.session_id)
            .map(|ids| {
                ids.iter()
                    .copied()
                    .filter(|&id| app.workspace_panes.browser.host.tab(id).is_some())
                    .collect()
            })
            .unwrap_or_default();

        // Forget closed tabs owned by this session.
        let stale: Vec<u64> = self
            .host_to_wire
            .keys()
            .copied()
            .filter(|id| !host_ids.contains(id))
            .collect();
        for host_id in stale {
            if let Some(wire) = self.host_to_wire.remove(&host_id) {
                self.wire_to_host.remove(&wire);
                if self.agent_tab_id.as_deref() == Some(wire.as_str()) {
                    self.agent_tab_id = None;
                }
            }
            self.generations.remove(&host_id);
            self.last_urls.remove(&host_id);
            self.refs.remove(&host_id);
            self.diagnostics.remove(&host_id);
            self.captures.remove(&host_id);
        }

        // Register newly opened tabs and track document navigations.
        let mut tabs = Vec::with_capacity(host_ids.len());
        for host_id in &host_ids {
            let wire_id = self
                .host_to_wire
                .entry(*host_id)
                .or_insert_with(|| {
                    let wid = new_tab_id();
                    self.wire_to_host.insert(wid.clone(), *host_id);
                    wid
                })
                .clone();
            let Some(tab) = app.workspace_panes.browser.host.tab(*host_id) else { continue };
            let url_changed = self
                .last_urls
                .get(host_id)
                .is_some_and(|prev| prev != &tab.url && !tab.url.is_empty());
            if url_changed {
                let gen_entry = self.generations.entry(*host_id).or_insert(1);
                *gen_entry = gen_entry.saturating_add(1);
                if let Some(r) = self.refs.get_mut(host_id) {
                    r.clear_on_nav();
                }
            }
            if !self.last_urls.contains_key(host_id) || url_changed {
                self.last_urls.insert(*host_id, tab.url.clone());
            }
            let generation = *self.generations.entry(*host_id).or_insert(1);
            self.diagnostics
                .entry(*host_id)
                .or_insert_with(|| TabDiagnostics::new(self.diag_scope_seed ^ host_id));
            tabs.push(build_wire_tab(&wire_id, tab, generation));
        }

        let active_wire = app
            .active_browser_tab_for_session(&self.session_id)
            .and_then(|hid| self.host_to_wire.get(&hid).cloned());
        let focused_tab_id = self
            .agent_tab_id
            .as_ref()
            .filter(|id| self.wire_to_host.contains_key(id.as_str()))
            .cloned()
            .or(active_wire)
            .or_else(|| tabs.first().map(|t| t.id.clone()));

        let state = BrowserState { tabs, focused_tab_id };
        let hash = hash_state(&state);
        (state, hash)
    }

    /// Resolves `wire_tab_id` strictly within this session, proving cross-session isolation by
    /// construction (`scope.rs` + per-session `wire_to_host`).
    pub fn resolve_tab(&self, app: &App, wire_tab_id: &str) -> Result<u64, String> {
        let Some(&host_id) = self.wire_to_host.get(wire_tab_id) else {
            return Err(
                "Browser tab is unavailable. Call browser.tabs.list({}) and use an existing tabID from this session; a closed tab is not replaced automatically."
                    .into(),
            );
        };
        if app.browser_tab_session(host_id) != Some(self.session_id.as_str()) {
            return Err(
                "Browser tab belongs to another session. Call browser.tabs.list({}) and use a tabID owned by this session."
                    .into(),
            );
        }
        if app.workspace_panes.browser.host.tab(host_id).is_none() {
            return Err(
                "Browser tab is unavailable. Call browser.tabs.list({}) and use an existing tabID from this session."
                    .into(),
            );
        }
        Ok(host_id)
    }

    /// Builds the current wire `Tab` snapshot for `host_id` owned by this session.
    pub fn wire_tab(&self, app: &App, host_id: u64) -> Option<Tab> {
        let wire_id = self.host_to_wire.get(&host_id)?;
        let tab = app.workspace_panes.browser.host.tab(host_id)?;
        let generation = *self.generations.get(&host_id).unwrap_or(&1);
        Some(build_wire_tab(wire_id, tab, generation))
    }

    pub fn generation_of(&self, host_id: u64) -> u64 {
        *self.generations.get(&host_id).unwrap_or(&1)
    }

    pub fn record_capture(&mut self, host_id: u64, file: CapturedFile) {
        let list = self.captures.entry(host_id).or_default();
        list.push(file);
        if list.len() > 64 {
            list.remove(0);
        }
    }
}

pub(super) fn build_wire_tab(wire_id: &str, tab: &browser_pane::BrowserTab, generation: u64) -> Tab {
    Tab {
        id: wire_id.to_owned(),
        url: truncate_str(&tab.url, 16_384),
        title: truncate_str(&tab.title, 2_048),
        loading: tab.loading,
        load_error: tab.load_error.as_deref().map(|s| truncate_str(s, 2_048)),
        can_go_back: tab.can_go_back,
        can_go_forward: tab.can_go_forward,
        generation,
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    s.chars().take(max).collect()
}

pub(super) fn hash_state(state: &BrowserState) -> u64 {
    let mut h = DefaultHasher::new();
    state.focused_tab_id.hash(&mut h);
    state.tabs.len().hash(&mut h);
    for t in &state.tabs {
        t.id.hash(&mut h);
        t.url.hash(&mut h);
        t.title.hash(&mut h);
        t.loading.hash(&mut h);
        t.load_error.hash(&mut h);
        t.can_go_back.hash(&mut h);
        t.can_go_forward.hash(&mut h);
        t.generation.hash(&mut h);
    }
    h.finish()
}

fn spawn_outbound_worker(
    server: Server,
    location: LocationRef,
    session_id: String,
    connection_id: String,
    rx: Receiver<OutboundMsg>,
    abort: AbortHandle,
) {
    std::thread::Builder::new()
        .name("browser-rpc-out".into())
        .spawn(move || {
            while let Ok(msg) = rx.recv() {
                if abort.is_aborted() {
                    break;
                }
                match msg {
                    OutboundMsg::Stop => break,
                    OutboundMsg::State(mut state) => {
                        // Coalesce immediately queued State updates while keeping ordering before
                        // any queued Result message.
                        let mut deferred: Option<OutboundMsg> = None;
                        while let Ok(next) = rx.try_recv() {
                            match next {
                                OutboundMsg::State(newer) => state = newer,
                                other => {
                                    deferred = Some(other);
                                    break;
                                }
                            }
                        }
                        send_state_with_retry(&server, &location, &session_id, &connection_id, &state, &abort);
                        if let Some(d) = deferred {
                            match d {
                                OutboundMsg::Stop => break,
                                OutboundMsg::Result { request_id, outcome } => {
                                    send_result(&server, &location, &session_id, &connection_id, &request_id, &outcome);
                                }
                                OutboundMsg::State(_) => {}
                            }
                        }
                    }
                    OutboundMsg::Result { request_id, outcome } => {
                        send_result(&server, &location, &session_id, &connection_id, &request_id, &outcome);
                    }
                }
            }
        })
        .ok();
}

fn send_state_with_retry(
    server: &Server,
    location: &LocationRef,
    session_id: &str,
    connection_id: &str,
    state: &BrowserState,
    abort: &AbortHandle,
) {
    let mut delay_ms = STATE_RETRY_MIN_MS;
    loop {
        if abort.is_aborted() {
            return;
        }
        match server.browser_rpc_state(location, session_id, connection_id, state) {
            Ok(()) => return,
            Err(
                RpcCallError::Unavailable(_)
                | RpcCallError::Unsupported(_)
                | RpcCallError::InvalidInput(_)
                | RpcCallError::Missing(_)
                | RpcCallError::Aborted,
            ) => return,
            Err(RpcCallError::Internal(_) | RpcCallError::Transport(_)) => {
                std::thread::sleep(Duration::from_millis(delay_ms));
                delay_ms = (delay_ms.saturating_mul(2)).min(STATE_RETRY_MAX_MS);
            }
        }
    }
}

fn send_result(
    server: &Server,
    location: &LocationRef,
    session_id: &str,
    connection_id: &str,
    request_id: &str,
    outcome: &Outcome,
) {
    let res = server.browser_rpc_result(location, session_id, connection_id, request_id, outcome);
    if let Err(RpcCallError::InvalidInput(_)) = res {
        let fallback = Outcome::fail(
            failure_code::OPERATION_FAILED,
            "Browser returned a response this server version could not decode. Do not retry a mutating action blindly.",
        );
        let _ = server.browser_rpc_result(location, session_id, connection_id, request_id, &fallback);
    }
}

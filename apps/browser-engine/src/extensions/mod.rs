//! Chrome MV3 extension host over a private, in-process anonymous CDP pipe.
//!
//! Architecture & trust boundaries:
//! - Starts Chromium with `--disable-extensions` (0 extensions) or
//!   `--disable-extensions-except=<inert 100-byte stub manifests>` (>=1 extension), permanently
//!   blocking Windows Registry (`external_registry_loader_win.cc`) and macOS/Linux external JSON
//!   loaders while keeping `cef::initialize()` at ~170 ms.
//! - Full parallel hardware-accelerated SHA-256 tree verification runs concurrently with
//!   `cef::initialize()`; verified extensions then upgrade asynchronously via
//!   `Extensions.loadUnpacked` (DNR filter extensions first), while any tampered extension has its
//!   inert stub immediately uninstalled and never runs.
//! - Private anonymous OS pipe (`pipe.rs`: `bInheritHandle = FALSE` on Windows, `FD_CLOEXEC` +
//!   reserved FDs 3/4 on Unix) pre-writes `Target.setAutoAttach(waitForDebuggerOnStart: true,
//!   flatten: true)` before `cef::initialize()` — zero TCP ports, zero named pipes, uninheritable.
//! - Only verified extension IDs receive `Runtime.addBinding("__ocExtCall")` and `shim.js`;
//!   regular web tabs and isolated `CefRequestContext` tabs detach immediately; every
//!   `Runtime.bindingCalled` additionally verifies `executionContextId`'s tracked origin equals
//!   `chrome-extension://<ext_id>`.
//! - Every shimmed RPC call and event subscription checks `rpc::is_api_allowed` against the
//!   extension's verified manifest on the Rust side.

#![deny(unsafe_code)]

mod cdp;
pub mod integrity;
pub mod manifest;
pub mod pipe;
mod png;
mod rpc;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use self::integrity::{StartupExtensions, VerifiedExtension, verify_extension_dir};
use self::manifest::{ExtensionManifest, extension_label, extension_page_url, is_private_url};
use self::pipe::CdpPipe;

pub(super) const WINDOW_ID: i32 = 1;
pub(super) const WINDOW_ID_CURRENT: i32 = -2;
const SHIM_TEMPLATE: &str = include_str!("shim.js");

/// Effects the extension host asks `EngineManager` to perform on CEF browsers or the wire.
#[derive(Debug)]
pub enum ExtEffect {
    OpenTab { ext_id: String, url: String, active: bool, reply_to: Option<(String, i64, String)> },
    NavigateTab { engine_tab_id: u64, url: String },
    FocusTab { engine_tab_id: u64 },
    CloseTab { engine_tab_id: u64 },
    ReloadTab { engine_tab_id: u64, ignore_cache: bool },
    OpenSeedProbe { url: String },
    CloseSeedProbe,
    ActionUpdated(ExtensionActionSnapshot),
    ExtensionsSynced(Vec<ExtensionActionSnapshot>),
    FiltersReady,
    IntegrityFailed { ext_id: String },
    PopupResized { ext_id: String, width: u32, height: u32 },
    OpenPopup { ext_id: String, url: String },
    ExtensionUninstalled { ext_id: String, ok: bool, error: Option<String> },
    ExtensionReloaded { ext_id: String, ok: bool, error: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtensionActionSnapshot {
    pub ext_id: String,
    pub name: String,
    pub version: String,
    pub title: String,
    pub popup: Option<String>,
    pub options: Option<String>,
    pub icon: Option<String>,
    pub badge_text: String,
    pub badge_color: Option<String>,
    pub badge_text_color: Option<String>,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ShimMetrics {
    pub sw_boots: u64,
    pub sw_attach_to_resume_us: Vec<u64>,
    pub sw_shim_eval_us: Vec<u64>,
    pub page_boots: u64,
    pub page_attach_to_resume_us: Vec<u64>,
    pub page_shim_eval_us: Vec<u64>,
    pub rpc_calls: u64,
    pub sw_restarts: u64,
}

#[derive(Clone, Debug)]
pub(super) struct ExtTab {
    pub engine_tab_id: u64,
    pub chromium_tab_id: Option<i32>,
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub active: bool,
    pub width: i32,
    pub height: i32,
    pub muted: bool,
    pub audible: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionKind {
    ServiceWorker,
    ExtensionPage,
    PopupPage,
    SeedProbe,
}

#[derive(Debug)]
pub(super) struct AttachedSession {
    pub ext_id: String,
    pub kind: SessionKind,
    pub attach_started: Instant,
    pub pause_started: Option<Instant>,
    pub pending_shim_eval_id: Option<i64>,
    pub was_paused: bool,
    pub shimmed: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) struct ActionValues {
    pub title: Option<String>,
    pub popup: Option<String>,
    pub badge_text: Option<String>,
    pub badge_color: Option<String>,
    pub badge_text_color: Option<String>,
    pub icon: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct ActionState {
    pub all: ActionValues,
    pub tabs: HashMap<i32, ActionValues>,
}

impl ActionState {
    pub(super) fn for_tab(&self, tab_id: Option<i32>) -> ActionValues {
        let mut out = self.all.clone();
        if let Some(tid) = tab_id
            && let Some(ov) = self.tabs.get(&tid)
        {
            if ov.title.is_some() { out.title = ov.title.clone(); }
            if ov.popup.is_some() { out.popup = ov.popup.clone(); }
            if ov.badge_text.is_some() { out.badge_text = ov.badge_text.clone(); }
            if ov.badge_color.is_some() { out.badge_color = ov.badge_color.clone(); }
            if ov.badge_text_color.is_some() { out.badge_text_color = ov.badge_text_color.clone(); }
            if ov.icon.is_some() { out.icon = ov.icon.clone(); }
            if ov.enabled.is_some() { out.enabled = ov.enabled; }
        }
        out
    }

    pub(super) fn apply(&mut self, tab_id: Option<i32>, f: impl FnOnce(&mut ActionValues)) {
        match tab_id {
            None => f(&mut self.all),
            Some(tid) => {
                if self.tabs.contains_key(&tid) || self.tabs.len() < 256 {
                    f(self.tabs.entry(tid).or_default());
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct LoadedExt {
    pub dir: PathBuf,
    pub sha256: String,
    pub manifest: ExtensionManifest,
    pub is_unpacked: bool,
    pub loaded: bool,
    pub boot_count: u64,
}

#[derive(Clone, Debug)]
pub(super) enum ExpectedTarget {
    WebTab,
    Popup { ext_id: String },
    ExtensionPage { ext_id: String },
    SeedProbe { ext_id: String },
    IsolatedTab,
}

pub(super) struct PendingRpc {
    pub session_id: String,
    pub ext_id: String,
    pub req_id: i64,
    pub name: String,
    pub args: Vec<Value>,
}

pub struct ExtensionHost {
    pub(super) pipe: CdpPipe,
    next_cdp_id: i64,
    pub(super) extensions: HashMap<String, LoadedExt>,
    pub(super) verified_startup: HashMap<String, VerifiedExtension>,
    pub(super) sessions: HashMap<String, AttachedSession>,
    /// Tracks `(session_id, executionContextId) -> origin` from `Runtime.executionContextCreated`
    /// so `Runtime.bindingCalled` only accepts calls from `chrome-extension://<ext_id>`.
    pub(super) execution_contexts: HashMap<(String, i64), String>,
    pub(super) subscribers: HashMap<String, HashMap<String, HashSet<String>>>,
    pub(super) tabs: Vec<ExtTab>,
    pub(super) actions: HashMap<String, ActionState>,
    pub(super) context_menus: HashMap<String, Vec<Value>>,
    pub(super) expected_targets: VecDeque<ExpectedTarget>,
    pub(super) seed_session_id: Option<i32>,
    pub(super) seed_probe_started: bool,
    pub(super) seed_eval_id: Option<i64>,
    pub(super) last_known_session_id: i32,
    pub(super) next_probe_id: i64,
    pub(super) pending_rpcs: Vec<PendingRpc>,
    pub(super) probe_in_flight: Option<(i64, String)>,
    pub(super) pending_startup_loads: HashMap<i64, String>,
    pub(super) pending_filter_ids: HashSet<String>,
    pub(super) filters_ready_emitted: bool,
    pub(super) queued_clicks: Vec<String>,
    pub(super) pending_uninstall_upgrade: HashMap<i64, String>,
    pub(super) pending_uninstall_cmd: HashMap<i64, String>,
    pub(super) pending_reload_cmd: HashMap<i64, String>,
    pub metrics: ShimMetrics,
}

impl ExtensionHost {
    pub fn new(pipe: CdpPipe) -> Self {
        Self {
            pipe,
            next_cdp_id: 2, // id=1 was pre-written Target.setAutoAttach
            extensions: HashMap::new(),
            verified_startup: HashMap::new(),
            sessions: HashMap::new(),
            execution_contexts: HashMap::new(),
            subscribers: HashMap::new(),
            tabs: Vec::new(),
            actions: HashMap::new(),
            context_menus: HashMap::new(),
            expected_targets: VecDeque::new(),
            seed_session_id: None,
            seed_probe_started: false,
            seed_eval_id: None,
            last_known_session_id: 0,
            next_probe_id: 1,
            pending_rpcs: Vec::new(),
            probe_in_flight: None,
            pending_startup_loads: HashMap::new(),
            pending_filter_ids: HashSet::new(),
            filters_ready_emitted: false,
            queued_clicks: Vec::new(),
            pending_uninstall_upgrade: HashMap::new(),
            pending_uninstall_cmd: HashMap::new(),
            pending_reload_cmd: HashMap::new(),
            metrics: ShimMetrics::default(),
        }
    }

    /// Delivers the background parallel SHA-256 verification outcome once `cef::initialize()`
    /// finishes: uninstalls inert stubs for tampered extensions, and asynchronously upgrades
    /// verified extensions via `Extensions.loadUnpacked` (DNR filter extensions first).
    pub fn on_startup_verified(&mut self, startup: StartupExtensions) -> Vec<ExtEffect> {
        let mut effects = Vec::new();
        for bad_id in startup.rejected_ids {
            // Remove the inert exemption stub from Chromium immediately.
            let _ = self.send_cdp("Extensions.uninstall", json!({ "id": bad_id }), None);
            effects.push(ExtEffect::IntegrityFailed { ext_id: bad_id });
        }

        // Load DNR filter extensions first so ad-blockers become active ahead of large extensions.
        let mut ordered = startup.verified;
        ordered.sort_by_key(|item| !item.manifest.is_filter_extension());

        for item in ordered {
            let is_filter = item.manifest.is_filter_extension();
            let path_str = item.dir.to_string_lossy().into_owned();
            let id = item.id.clone();
            if is_filter {
                self.pending_filter_ids.insert(id.clone());
            }
            self.verified_startup.insert(id.clone(), item.clone());
            self.register_extension(id.clone(), item.dir, item.sha256, item.manifest, false, false);
            let cmd_id = self.send_cdp("Extensions.loadUnpacked", json!({ "path": path_str }), None);
            self.pending_startup_loads.insert(cmd_id, id);
        }

        if self.pending_filter_ids.is_empty() && !self.filters_ready_emitted {
            self.filters_ready_emitted = true;
            effects.push(ExtEffect::FiltersReady);
        }
        effects
    }

    pub fn filters_ready(&self) -> bool {
        self.filters_ready_emitted
    }

    pub fn close_pipe(&self) {
        self.pipe.close();
    }

    pub fn list_actions(&self) -> Vec<ExtensionActionSnapshot> {
        self.extensions
            .keys()
            .filter_map(|id| self.action_snapshot(id))
            .collect()
    }

    pub fn action_snapshot(&self, ext_id: &str) -> Option<ExtensionActionSnapshot> {
        let ext = self.extensions.get(ext_id)?;
        let active_tid = self.active_chromium_tab_id();
        let vals = self.actions.get(ext_id).map(|s| s.for_tab(active_tid)).unwrap_or_default();
        let def_title = ext
            .manifest
            .action
            .as_ref()
            .and_then(|a| a.default_title.clone())
            .unwrap_or_else(|| ext.manifest.name.clone());
        let def_popup = ext.manifest.action.as_ref().and_then(|a| a.default_popup.clone());
        let popup = vals.popup.or(def_popup).as_deref().and_then(|f| extension_page_url(ext_id, f));
        let options = ext.manifest.options_page.as_deref().and_then(|f| extension_page_url(ext_id, f));
        Some(ExtensionActionSnapshot {
            ext_id: ext_id.to_owned(),
            name: ext.manifest.name.clone(),
            version: ext.manifest.version.clone(),
            title: extension_label(vals.title.as_deref().unwrap_or(&def_title)),
            popup,
            options,
            icon: vals.icon.or_else(|| ext.manifest.icon_data_url.clone()),
            badge_text: extension_label(vals.badge_text.as_deref().unwrap_or("")),
            badge_color: vals.badge_color,
            badge_text_color: vals.badge_text_color,
            enabled: vals.enabled.unwrap_or(true),
        })
    }

    pub fn click_extension(&mut self, ext_id: &str) -> Vec<ExtEffect> {
        let Some(ext_entry) = self.extensions.get(ext_id) else {
            return Vec::new();
        };
        if !ext_entry.loaded {
            if !self.queued_clicks.iter().any(|id| id == ext_id) {
                self.queued_clicks.push(ext_id.to_owned());
            }
            return Vec::new();
        }
        let Some(snap) = self.action_snapshot(ext_id) else {
            return Vec::new();
        };
        if !snap.enabled {
            return Vec::new();
        }
        if let Some(popup_url) = snap.popup {
            return vec![ExtEffect::OpenPopup { ext_id: ext_id.to_owned(), url: popup_url }];
        }
        if let Some(tab) = self.tabs.iter().find(|t| t.active).cloned()
            && !is_private_url(&tab.url)
            && let Some(ext) = self.extensions.get(ext_id).cloned()
        {
            let info = self.tab_json(&tab, &ext.manifest, true);
            self.emit_event("action.onClicked", vec![info], Some(ext_id));
        }
        Vec::new()
    }

    /// Runtime disable / uninstall with ZERO engine restart.
    pub fn uninstall_extension(&mut self, ext_id: &str) -> Vec<ExtEffect> {
        let Some(ext) = self.extensions.get(ext_id).cloned() else {
            return vec![ExtEffect::ExtensionUninstalled {
                ext_id: ext_id.to_owned(),
                ok: false,
                error: Some("Extension not loaded".into()),
            }];
        };
        if ext.is_unpacked {
            let id = self.send_cdp("Extensions.uninstall", json!({ "id": ext_id }), None);
            self.pending_uninstall_cmd.insert(id, ext_id.to_owned());
        } else {
            // Re-verify tree digest before upgrading kCommandLine -> kUnpacked so a tampered dir
            // can never be re-loaded during an uninstall.
            if !verify_extension_dir(&ext.dir, ext_id, &ext.sha256) {
                return vec![ExtEffect::ExtensionUninstalled {
                    ext_id: ext_id.to_owned(),
                    ok: false,
                    error: Some("Extension files changed on disk; restart required.".into()),
                }];
            }
            let path_str = ext.dir.to_string_lossy().into_owned();
            let id = self.send_cdp("Extensions.loadUnpacked", json!({ "path": path_str }), None);
            self.pending_uninstall_upgrade.insert(id, ext_id.to_owned());
        }
        Vec::new()
    }

    /// Runtime reload / re-enable of a previously verified startup-exempt extension by ID.
    /// Never accepts arbitrary filesystem paths over IPC; re-verifies SHA-256 tree digest first.
    pub fn reload_extension(&mut self, ext_id: &str) -> Vec<ExtEffect> {
        let Some(item) = self.verified_startup.get(ext_id).cloned() else {
            return vec![ExtEffect::ExtensionReloaded {
                ext_id: ext_id.to_owned(),
                ok: false,
                error: Some("Extension was not exempted at engine startup; restart required.".into()),
            }];
        };
        if !verify_extension_dir(&item.dir, &item.id, &item.sha256) {
            return vec![ExtEffect::ExtensionReloaded {
                ext_id: ext_id.to_owned(),
                ok: false,
                error: Some("Extension integrity digest mismatch.".into()),
            }];
        }
        let path_str = item.dir.to_string_lossy().into_owned();
        let id = self.send_cdp("Extensions.loadUnpacked", json!({ "path": path_str }), None);
        self.pending_reload_cmd.insert(id, item.id);
        Vec::new()
    }

    pub fn expect_web_tab(&mut self) {
        self.expected_targets.push_back(ExpectedTarget::WebTab);
    }

    pub fn expect_isolated_tab(&mut self) {
        self.expected_targets.push_back(ExpectedTarget::IsolatedTab);
    }

    pub fn expect_popup(&mut self, ext_id: &str) {
        self.expected_targets.push_back(ExpectedTarget::Popup { ext_id: ext_id.to_owned() });
    }

    pub fn expect_extension_page(&mut self, ext_id: &str) {
        self.expected_targets.push_back(ExpectedTarget::ExtensionPage { ext_id: ext_id.to_owned() });
    }

    /// Pops the last pushed expected target if `cef::browser_host_create_browser_sync` returned
    /// `None`, keeping the target queue synchronized.
    pub fn cancel_expected_target(&mut self) {
        let _ = self.expected_targets.pop_back();
    }

    pub fn on_tab_opened(
        &mut self,
        engine_tab_id: u64,
        url: &str,
        width: i32,
        height: i32,
        active: bool,
    ) -> Vec<ExtEffect> {
        if active {
            for t in &mut self.tabs {
                t.active = false;
            }
        }
        self.tabs.push(ExtTab {
            engine_tab_id,
            chromium_tab_id: None,
            url: url.to_owned(),
            title: String::new(),
            loading: true,
            active,
            width,
            height,
            muted: false,
            audible: false,
        });
        let effects = self.maybe_start_seed_probe();
        self.trigger_tab_id_probe();
        effects
    }

    pub fn on_tab_closed(&mut self, engine_tab_id: u64) {
        if let Some(pos) = self.tabs.iter().position(|t| t.engine_tab_id == engine_tab_id) {
            let tab = self.tabs.remove(pos);
            if let Some(cid) = tab.chromium_tab_id {
                self.emit_event(
                    "tabs.onRemoved",
                    vec![json!(cid), json!({ "windowId": WINDOW_ID, "isWindowClosing": false })],
                    None,
                );
            }
        }
    }

    pub fn on_tab_focused(&mut self, engine_tab_id: u64) {
        let mut activated_cid = None;
        for t in &mut self.tabs {
            let is_this = t.engine_tab_id == engine_tab_id;
            if is_this && !t.active {
                activated_cid = t.chromium_tab_id;
            }
            t.active = is_this;
        }
        if let Some(cid) = activated_cid {
            self.broadcast_active_tab(cid);
            self.emit_event("tabs.onActivated", vec![json!({ "tabId": cid, "windowId": WINDOW_ID })], None);
        }
    }

    pub fn on_tab_url_changed(&mut self, engine_tab_id: u64, url: &str) {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.engine_tab_id == engine_tab_id) else {
            return;
        };
        if tab.url == url {
            return;
        }
        tab.url = url.to_owned();
        let tab_clone = tab.clone();
        if let Some(cid) = tab_clone.chromium_tab_id
            && !is_private_url(url)
        {
            self.emit_tab_updated(&tab_clone, json!({ "url": url }), true);
            self.emit_event(
                "webNavigation.onCommitted",
                vec![json!({
                    "tabId": cid, "url": url, "processId": 0, "frameId": 0, "parentFrameId": -1,
                    "transitionType": "link", "transitionQualifiers": [], "timeStamp": now_ms(),
                })],
                None,
            );
        }
    }

    pub fn on_tab_title_changed(&mut self, engine_tab_id: u64, title: &str) {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.engine_tab_id == engine_tab_id) else {
            return;
        };
        if tab.title == title {
            return;
        }
        tab.title = title.to_owned();
        let tab_clone = tab.clone();
        if tab_clone.chromium_tab_id.is_some() && !is_private_url(&tab_clone.url) {
            self.emit_tab_updated(&tab_clone, json!({ "title": title }), true);
        }
    }

    pub fn on_tab_loading_changed(&mut self, engine_tab_id: u64, loading: bool) {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.engine_tab_id == engine_tab_id) else {
            return;
        };
        if tab.loading == loading {
            return;
        }
        tab.loading = loading;
        let tab_clone = tab.clone();
        if let Some(cid) = tab_clone.chromium_tab_id {
            let status = if loading { "loading" } else { "complete" };
            self.emit_tab_updated(&tab_clone, json!({ "status": status }), false);
            if !loading && !is_private_url(&tab_clone.url) {
                self.emit_event(
                    "webNavigation.onCompleted",
                    vec![json!({
                        "tabId": cid, "url": tab_clone.url, "processId": 0, "frameId": 0,
                        "parentFrameId": -1, "timeStamp": now_ms(),
                    })],
                    None,
                );
            }
        }
    }

    pub fn finish_tab_create(&mut self, sid: &str, req_id: i64, ext_id: &str, engine_tab_id: u64) {
        if let (Some(ext), Some(tab)) = (
            self.extensions.get(ext_id),
            self.tabs.iter().find(|t| t.engine_tab_id == engine_tab_id),
        ) {
            let val = self.tab_json(tab, &ext.manifest, false);
            self.reply_ok(sid, req_id, val);
        } else {
            self.reply_ok(sid, req_id, Value::Null);
        }
    }

    pub fn fail_tab_create(&mut self, sid: &str, req_id: i64, error: &str) {
        self.reply_err(sid, req_id, error);
    }

    fn emit_tab_updated(&mut self, tab: &ExtTab, change: Value, needs_access: bool) {
        let Some(cid) = tab.chromium_tab_id else {
            return;
        };
        let exts: Vec<(String, ExtensionManifest)> =
            self.extensions.iter().map(|(id, e)| (id.clone(), e.manifest.clone())).collect();
        for (ext_id, manifest) in exts {
            if needs_access && !manifest.reveals_tab(&tab.url) {
                continue;
            }
            let info = self.tab_json(tab, &manifest, false);
            self.emit_event("tabs.onUpdated", vec![json!(cid), change.clone(), info], Some(&ext_id));
        }
    }

    fn emit_event(&mut self, event: &str, args: Vec<Value>, only_ext: Option<&str>) {
        let Some(by_ext) = self.subscribers.get(event) else {
            return;
        };
        let targets: Vec<String> = by_ext
            .iter()
            .filter(|(eid, _)| only_ext.is_none_or(|wanted| wanted == eid.as_str()))
            .flat_map(|(_, sids)| sids.iter().cloned())
            .collect();
        let msg = json!({ "kind": "event", "name": event, "args": args });
        for sid in targets {
            self.push_to_session(&sid, &msg);
        }
    }

    pub(super) fn broadcast_active_tab(&mut self, cid: i32) {
        let sids: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.shimmed && s.kind != SessionKind::SeedProbe)
            .map(|(sid, _)| sid.clone())
            .collect();
        let msg = json!({ "kind": "active_tab", "tabId": cid });
        for sid in sids {
            self.push_to_session(&sid, &msg);
        }
    }

    pub(super) fn reply_ok(&mut self, sid: &str, req_id: i64, value: Value) {
        self.push_to_session(sid, &json!({ "kind": "reply", "reqId": req_id, "ok": true, "value": value }));
    }

    pub(super) fn reply_err(&mut self, sid: &str, req_id: i64, error: &str) {
        self.push_to_session(sid, &json!({ "kind": "reply", "reqId": req_id, "ok": false, "error": error }));
    }

    pub(super) fn push_to_session(&mut self, sid: &str, msg: &Value) {
        let expr = format!("globalThis.__ocExtRecv?.({msg})");
        self.send_cdp("Runtime.evaluate", json!({ "expression": expr }), Some(sid));
    }

    pub(super) fn active_chromium_tab_id(&self) -> Option<i32> {
        self.tabs.iter().find(|t| t.active).and_then(|t| t.chromium_tab_id)
    }

    pub(super) fn register_extension(
        &mut self,
        id: String,
        dir: PathBuf,
        sha256: String,
        manifest: ExtensionManifest,
        is_unpacked: bool,
        loaded: bool,
    ) {
        self.extensions.insert(
            id,
            LoadedExt { dir, sha256, manifest, is_unpacked, loaded, boot_count: 0 },
        );
    }

    pub(super) fn alloc_id(&mut self) -> i64 {
        let id = self.next_cdp_id;
        self.next_cdp_id += 1;
        id
    }

    pub(super) fn send_cdp(&mut self, method: &str, params: Value, session_id: Option<&str>) -> i64 {
        let id = self.alloc_id();
        let mut payload = json!({ "id": id, "method": method, "params": params });
        if let Some(sid) = session_id {
            payload["sessionId"] = Value::String(sid.to_owned());
        }
        self.pipe.send(&payload);
        id
    }
}

pub(super) fn shim_source(is_popup: bool) -> String {
    SHIM_TEMPLATE.replace("__IS_POPUP__", if is_popup { "true" } else { "false" })
}

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

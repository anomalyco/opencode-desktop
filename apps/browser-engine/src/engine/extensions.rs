//! Extension host effect runner and hidden seed-probe browser for `EngineManager`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cef::{BrowserSettings, CefString, ImplBrowser, ImplBrowserHost, ImplFrame, WindowInfo};
use serde_json::{Value, json};

use super::{BASE_COLOR, EngineManager, ManagedTab};
use crate::handlers::{
    CdpPending, TabDisplayHandler, TabExtPolicy, TabLoadHandler, TabRenderHandler,
    TabRequestHandler, ViewRectState,
};
use crate::output::Output;
use crate::prompts::{
    PromptShared, TabDialogHandler, TabFileDialogHandler, TabLifeSpanHandler,
    TabPermissionHandler, TabPrompts,
};
use crate::tab_client::SidecarTabClient;
use crate::wire::{ContextKind, CursorKind, EngineMsg, TabSnapshot};

const MAX_PENDING_EXT_OPENS: usize = 16;
const MAX_UNMANAGED_EXT_TABS: usize = 16;

impl EngineManager {
    pub fn shutdown_extensions(&mut self) {
        if let Some(probe) = self.seed_probe.take() {
            probe.close_browser(1);
        }
        if let Some(ext) = &self.extensions {
            ext.close_pipe();
        }
        if let Some(root) = self.stub_root.take() {
            crate::extensions::integrity::cleanup_stub_root(&root);
        }
    }

    pub fn on_extensions_verified(&mut self, startup: crate::extensions::integrity::StartupExtensions) {
        let effects = match &mut self.extensions {
            Some(ext) => ext.on_startup_verified(startup),
            None => return,
        };
        self.run_ext_effects(effects);
    }

    pub fn poll_extensions(&mut self) -> bool {
        let effects = match &mut self.extensions {
            Some(ext) => ext.drain_inbound(),
            None => return false,
        };
        let had = !effects.is_empty();
        self.run_ext_effects(effects);
        had
    }

    pub fn on_tab_meta(
        &mut self,
        tab_id: u64,
        url: Option<String>,
        title: Option<String>,
        loading: Option<bool>,
    ) {
        let is_eligible = self.tabs.get(&tab_id).is_some_and(|t| !t.isolated && !t.is_popup);
        if !is_eligible {
            return;
        }
        let Some(ext) = &mut self.extensions else { return };
        if let Some(u) = url {
            ext.on_tab_url_changed(tab_id, &u);
        }
        if let Some(t) = title {
            ext.on_tab_title_changed(tab_id, &t);
        }
        if let Some(l) = loading {
            ext.on_tab_loading_changed(tab_id, l);
        }
    }

    pub(super) fn cmd_click_extension(&mut self, ext_id: &str) {
        let effects = self.extensions.as_mut().map(|e| e.click_extension(ext_id)).unwrap_or_default();
        self.run_ext_effects(effects);
    }

    pub(super) fn cmd_list_extensions(&self, request_id: u64) {
        let extensions = self.extensions.as_ref().map(|e| e.list_actions()).unwrap_or_default();
        let _ = self.out_tx.send(EngineMsg::ExtensionsState { request_id: Some(request_id), extensions });
    }

    pub(super) fn cmd_uninstall_extension(&mut self, ext_id: &str) {
        let effects = self.extensions.as_mut().map(|e| e.uninstall_extension(ext_id)).unwrap_or_default();
        self.run_ext_effects(effects);
    }

    pub(super) fn cmd_reload_extension(&mut self, ext_id: &str) {
        let effects = self.extensions.as_mut().map(|e| e.reload_extension(ext_id)).unwrap_or_default();
        self.run_ext_effects(effects);
    }

    pub(super) fn cmd_cancel_extension_open_tab(&mut self, request_id: u64) {
        if let Some((sid, rpc_id, _)) = self.pending_ext_opens.remove(&request_id)
            && let Some(ext) = &mut self.extensions
        {
            ext.fail_tab_create(&sid, rpc_id, "Tab open refused by browser policy.");
        }
    }

    pub(super) fn cmd_extension_metrics(&self, request_id: u64) {
        let metrics = self.extensions.as_ref().map(|e| e.metrics.clone()).unwrap_or_default();
        let _ = self.out_tx.send(EngineMsg::ExtensionMetrics { request_id, metrics });
    }

    pub(super) fn run_ext_effects(&mut self, effects: Vec<crate::extensions::ExtEffect>) {
        use crate::extensions::ExtEffect;
        for effect in effects {
            match effect {
                ExtEffect::OpenTab { ext_id, url, active, reply_to } => {
                    if self.app_handles_ext_tabs {
                        if self.pending_ext_opens.len() >= MAX_PENDING_EXT_OPENS {
                            if let Some((sid, rpc_id, _)) = reply_to
                                && let Some(ext) = &mut self.extensions
                            {
                                ext.fail_tab_create(&sid, rpc_id, "Too many pending tab requests.");
                            }
                            continue;
                        }
                        let request_id = self.next_ext_open_req;
                        self.next_ext_open_req += 1;
                        if let Some(reply) = reply_to {
                            self.pending_ext_opens.insert(request_id, reply);
                        }
                        let _ = self.out_tx.send(EngineMsg::ExtensionOpenTab {
                            request_id,
                            ext_id,
                            url,
                            active,
                        });
                    } else {
                        if self.unmanaged_ext_tabs >= MAX_UNMANAGED_EXT_TABS {
                            if let Some((sid, rpc_id, _)) = reply_to
                                && let Some(ext) = &mut self.extensions
                            {
                                ext.fail_tab_create(&sid, rpc_id, "Too many extension tabs open.");
                            }
                            continue;
                        }
                        self.unmanaged_ext_tabs += 1;
                        let tab_id = self.next_ext_tab_id;
                        self.next_ext_tab_id += 1;
                        self.open_tab(
                            tab_id,
                            &url,
                            800.0,
                            600.0,
                            1.0,
                            active,
                            Vec::new(),
                            ContextKind::Default,
                            None,
                            false,
                            false,
                            true,
                            false,
                            None,
                        );
                        if let Some((sid, req_id, ext_id)) = reply_to
                            && let Some(ext) = &mut self.extensions
                        {
                            ext.finish_tab_create(&sid, req_id, &ext_id, tab_id);
                        }
                    }
                }
                ExtEffect::NavigateTab { engine_tab_id, url } => {
                    if let Some(t) = self.tabs.get(&engine_tab_id) {
                        if t.prompts.agent_driven.load(Ordering::Relaxed) {
                            continue;
                        }
                        if let Some(frame) = t.browser.main_frame() {
                            t.prompts.set_pending_top_url(Some(url.clone()));
                            frame.load_url(Some(&CefString::from(url.as_str())));
                        }
                    }
                }
                ExtEffect::FocusTab { engine_tab_id } => {
                    for (&id, t) in &self.tabs {
                        let active = id == engine_tab_id;
                        if !t.is_popup {
                            t.host.was_hidden(i32::from(!active));
                        }
                        t.host.set_focus(i32::from(active));
                    }
                    if let Some(ext) = &mut self.extensions {
                        ext.on_tab_focused(engine_tab_id);
                    }
                    let _ = self.out_tx.send(EngineMsg::ExtensionFocusTab { tab_id: engine_tab_id });
                }
                ExtEffect::CloseTab { engine_tab_id } => {
                    if self
                        .tabs
                        .get(&engine_tab_id)
                        .is_some_and(|t| t.prompts.agent_driven.load(Ordering::Relaxed))
                    {
                        continue;
                    }
                    if let Some(tab) = self.tabs.remove(&engine_tab_id) {
                        self.permissions.on_tab_closed(engine_tab_id);
                        tab.menu.cancel_all();
                        tab.prompts.dismiss_all();
                        tab.download_alive.store(false, Ordering::Release);
                        tab.host.close_browser(1);
                        self.downloads.close_tab(engine_tab_id, tab.download_gen);
                        self.contexts.release_tab(engine_tab_id);
                        if engine_tab_id >= 1_000_000 {
                            self.unmanaged_ext_tabs = self.unmanaged_ext_tabs.saturating_sub(1);
                        }
                        if !tab.isolated
                            && !tab.is_popup
                            && let Some(ext) = &mut self.extensions
                        {
                            ext.on_tab_closed(engine_tab_id);
                        }
                        let _ = self.out_tx.send(EngineMsg::TabClosed { tab_id: engine_tab_id });
                    }
                }
                ExtEffect::ReloadTab { engine_tab_id, ignore_cache } => {
                    if let Some(t) = self.tabs.get(&engine_tab_id) {
                        if t.prompts.agent_driven.load(Ordering::Relaxed) {
                            continue;
                        }
                        if ignore_cache {
                            t.browser.reload_ignore_cache();
                        } else {
                            t.browser.reload();
                        }
                    }
                }
                ExtEffect::OpenSeedProbe { url } => self.open_seed_probe(&url),
                ExtEffect::CloseSeedProbe => {
                    if let Some(probe) = self.seed_probe.take() {
                        probe.close_browser(1);
                    }
                }
                ExtEffect::ActionUpdated(action) => {
                    let _ = self.out_tx.send(EngineMsg::ExtensionAction { action });
                }
                ExtEffect::ExtensionsSynced(extensions) => {
                    let _ = self.out_tx.send(EngineMsg::ExtensionsState {
                        request_id: None,
                        extensions,
                    });
                }
                ExtEffect::FiltersReady => {
                    let _ = self.out_tx.send(EngineMsg::ExtensionFiltersReady);
                }
                ExtEffect::IntegrityFailed { ext_id } => {
                    if !self.rejected_exts.contains(&ext_id) {
                        self.rejected_exts.push(ext_id.clone());
                    }
                    let _ = self.out_tx.send(EngineMsg::ExtensionIntegrityFailed { ext_id });
                }
                ExtEffect::PopupResized { ext_id, width, height } => {
                    let _ = self.out_tx.send(EngineMsg::ExtensionPopupResized { ext_id, width, height });
                }
                ExtEffect::OpenPopup { ext_id, url } => {
                    let _ = self.out_tx.send(EngineMsg::ExtensionPopupRequested { ext_id, url });
                }
                ExtEffect::ExtensionUninstalled { ext_id, ok, error } => {
                    if ok {
                        let prefix = format!("chrome-extension://{ext_id}/");
                        let stale_popups: Vec<u64> = self
                            .tabs
                            .iter()
                            .filter(|(_, t)| t.is_popup && t.snap.lock().unwrap().url.starts_with(&prefix))
                            .map(|(&id, _)| id)
                            .collect();
                        for pid in stale_popups {
                            if let Some(tab) = self.tabs.remove(&pid) {
                                tab.menu.cancel_all();
                                tab.prompts.dismiss_all();
                                tab.download_alive.store(false, Ordering::Release);
                                tab.host.close_browser(1);
                                self.downloads.close_tab(pid, tab.download_gen);
                                let _ = self.out_tx.send(EngineMsg::TabClosed { tab_id: pid });
                            }
                        }
                        for t in self.tabs.values() {
                            t.allowed_ext_ids.lock().unwrap().retain(|id| id != &ext_id);
                        }
                    }
                    let _ = self.out_tx.send(EngineMsg::ExtensionUninstalled { ext_id, ok, error });
                }
                ExtEffect::ExtensionReloaded { ext_id, ok, error } => {
                    let _ = self.out_tx.send(EngineMsg::ExtensionReloaded { ext_id, ok, error });
                }
            }
        }
    }

    pub(super) fn open_seed_probe(&mut self, url: &str) {
        if self.seed_probe.is_some() {
            if let Some(ext) = &mut self.extensions {
                ext.cancel_expected_target();
            }
            return;
        }
        let Some(ext_id) = crate::extensions::manifest::extension_id_from_url(url).map(str::to_owned) else {
            if let Some(ext) = &mut self.extensions {
                ext.cancel_expected_target();
            }
            return;
        };
        // Null output sink so this short-lived internal probe never emits wire messages
        // (`tab_id = u64::MAX`) to the desktop app.
        let null_out = Output::null();
        let prompt_shared = PromptShared::default();
        // Do NOT set `agent_driven = true` (which blocks `chrome-extension://` in `on_before_browse`);
        // instead pin `popup_ext_id = Some(ext_id)` so this probe can ONLY load that single origin.
        let prompts = Arc::new(TabPrompts::default());
        let favicon = Arc::new(crate::favicon::TabFavicon::default());
        let diagnostics = Arc::new(crate::diagnostics::TabDiagnostics::default());
        let menu_state = Arc::new(crate::menu::TabMenuState::default());
        let view_state = Arc::new(Mutex::new(ViewRectState { width_pt: 2.0, height_pt: 2.0, scale: 1.0 }));
        let snap = Arc::new(Mutex::new(TabSnapshot {
            tab_id: u64::MAX,
            url: url.to_owned(),
            title: String::new(),
            loading: true,
            can_go_back: false,
            can_go_forward: false,
            zoom: 1.0,
            cursor: CursorKind::Default,
            load_error: None,
            favicon: None,
            context: ContextKind::Default,
            preview: None,
            agent_driven: false,
        }));
        let allowed_ext = vec![ext_id.clone()];
        let file_roots = Arc::new(Mutex::new(Vec::new()));
        let context_ready = Arc::new(AtomicBool::new(true));
        let render = TabRenderHandler::new(
            u64::MAX,
            view_state,
            self.mach_port.clone(),
            null_out.clone(),
            Arc::new(AtomicU64::new(0)),
            #[cfg(windows)]
            None,
            #[cfg(target_os = "linux")]
            None,
            #[cfg(target_os = "macos")]
            (),
            #[cfg(target_os = "linux")]
            self.events.clone(),
            #[cfg(not(target_os = "linux"))]
            (),
        );
        let display = TabDisplayHandler::new(
            u64::MAX,
            None,
            snap.clone(),
            prompts.clone(),
            self.permissions.clone(),
            None,
            favicon,
            null_out.clone(),
            self.events.clone(),
        );
        let load = TabLoadHandler::new(
            u64::MAX,
            file_roots.clone(),
            None,
            snap.clone(),
            prompts.clone(),
            diagnostics.clone(),
            Arc::new(AtomicBool::new(false)),
            null_out.clone(),
            self.events.clone(),
        );
        let request = TabRequestHandler::new(
            u64::MAX,
            file_roots.clone(),
            TabExtPolicy {
                allowed_ext_ids: Arc::new(Mutex::new(allowed_ext)),
                popup_ext_id: Some(ext_id),
                isolated: false,
            },
            None,
            context_ready.clone(),
            snap.clone(),
            prompt_shared.clone(),
            prompts.clone(),
            None,
            diagnostics,
            null_out.clone(),
        );
        let download_gen = self.downloads.alloc_tab_gen();
        let download_alive = Arc::new(AtomicBool::new(true));
        let download = crate::downloads::TabDownloadHandler::new(
            u64::MAX,
            download_gen,
            download_alive,
            true,
            false,
            context_ready.clone(),
            prompts.clone(),
            self.downloads.clone(),
            null_out.clone(),
        );
        let life_span = TabLifeSpanHandler::new(u64::MAX, false, context_ready.clone(), null_out.clone());
        let jsdialog = TabDialogHandler::new(
            u64::MAX,
            prompt_shared.clone(),
            prompts.clone(),
            None,
            null_out.clone(),
        );
        let permission = TabPermissionHandler::new(
            u64::MAX,
            false,
            context_ready,
            prompt_shared.clone(),
            prompts.clone(),
            null_out.clone(),
        );
        let dialog = TabFileDialogHandler::new(u64::MAX, false, prompts, null_out.clone());
        let find = crate::find::TabFindHandler::new(u64::MAX, null_out.clone());
        let context_menu = crate::menu::TabContextMenuHandler::new(
            u64::MAX,
            prompt_shared,
            file_roots,
            None,
            menu_state,
            null_out,
        );
        let mut client = SidecarTabClient::new(
            render,
            display,
            load,
            request,
            download,
            life_span,
            jsdialog,
            permission,
            dialog,
            find,
            context_menu,
        );
        let mut win_info = WindowInfo::default().set_as_windowless(Default::default());
        win_info.shared_texture_enabled = 0;
        let browser_settings = BrowserSettings {
            windowless_frame_rate: 1,
            background_color: BASE_COLOR,
            ..Default::default()
        };
        if let Some(browser) = cef::browser_host_create_browser_sync(
            Some(&win_info),
            Some(&mut client),
            Some(&CefString::from(url)),
            Some(&browser_settings),
            None,
            None,
        ) {
            if let Some(host) = browser.host() {
                host.was_hidden(1);
                self.seed_probe = Some(host);
            } else if let Some(ext) = &mut self.extensions {
                ext.cancel_expected_target();
            }
        } else if let Some(ext) = &mut self.extensions {
            ext.cancel_expected_target();
        }
    }

    pub(super) fn handle_cdp_call(
        &mut self,
        tab_id: u64,
        request_id: u64,
        method: String,
        params: Value,
        session_id: Option<String>,
    ) {
        let Some(t) = self.tabs.get(&tab_id) else {
            return;
        };
        if !self.contexts.is_tab_ready(tab_id) {
            let reason = self.contexts.not_ready_reason(tab_id);
            let _ = self.out_tx.send(EngineMsg::CdpResponse {
                tab_id,
                request_id,
                ok: false,
                result: json!({ "message": reason }),
            });
            return;
        }
        // Reject any cross-session / OOPIF sessionId so external callers can never target
        // extension iframes or non-page targets (`review-rd274.md` §1).
        if session_id.is_some() {
            let _ = self.out_tx.send(EngineMsg::CdpResponse {
                tab_id,
                request_id,
                ok: false,
                result: json!({ "message": "Cross-session DevTools targeting is not permitted." }),
            });
            return;
        }
        // Fail closed on the entire tab whenever ANY frame (main OR sub-frame) is, or is about to
        // become, an extension frame — checked at dispatch time on both agent-driven tabs and
        // unflagged 3P plugin `DevtoolsAsync` tabs (`review-rd274.md` Round-4 ruling).
        if tab_is_extension_surface(t) {
            let _ = self.out_tx.send(EngineMsg::CdpResponse {
                tab_id,
                request_id,
                ok: false,
                result: json!({ "message": "Agent browser tools cannot read or act on extension pages or tabs with extension frames." }),
            });
            return;
        }
        if let Err(err) = browser_policy::check_cdp(&method, &params, true) {
            let _ = self.out_tx.send(EngineMsg::CdpResponse {
                tab_id,
                request_id,
                ok: false,
                result: json!({ "message": err }),
            });
            return;
        }
        let msg_id = self.next_cdp_id.fetch_add(1, Ordering::Relaxed);
        t.cdp_id_map.lock().unwrap().insert(
            msg_id,
            CdpPending::Raw {
                request_id,
                max_bytes: crate::cdp_ledger::max_cdp_response_bytes(&method),
            },
        );
        let payload = json!({
            "id": msg_id,
            "method": method,
            "params": params,
        });
        let raw = payload.to_string();
        if t.host.send_dev_tools_message(Some(raw.as_bytes())) != 1 {
            t.cdp_id_map.lock().unwrap().remove(&msg_id);
            let _ = self.out_tx.send(EngineMsg::CdpResponse {
                tab_id,
                request_id,
                ok: false,
                result: json!({ "error": "send_dev_tools_message rejected" }),
            });
        }
    }

    pub(super) fn handle_set_inspect(&self, tab_id: u64, enabled: bool) {
        let Some(t) = self.tabs.get(&tab_id).filter(|_| self.contexts.is_tab_ready(tab_id)) else {
            return;
        };
        if tab_is_extension_surface(t) {
            return;
        }
        let host = t.host.clone();
        if enabled {
            self.send_internal_cdp(&host, "DOM.enable", json!({}));
            self.send_internal_cdp(&host, "Overlay.enable", json!({}));
            self.send_internal_cdp(
                &host,
                "Overlay.setInspectMode",
                json!({
                    "mode": "searchForNode",
                    "highlightConfig": {
                        "showInfo": true,
                        "contentColor": { "r": 56, "g": 189, "b": 248, "a": 0.24 },
                        "paddingColor": { "r": 52, "g": 211, "b": 153, "a": 0.18 },
                        "borderColor": { "r": 251, "g": 191, "b": 36, "a": 0.90 }
                    }
                }),
            );
        } else {
            self.send_internal_cdp(
                &host,
                "Overlay.setInspectMode",
                json!({ "mode": "none", "highlightConfig": {} }),
            );
            self.send_internal_cdp(&host, "Overlay.hideHighlight", json!({}));
        }
        let _ = self.out_tx.send(EngineMsg::InspectState {
            tab_id,
            active: enabled,
            picked: None,
        });
    }

    pub(super) fn handle_highlight_node(&self, tab_id: u64, backend_node_id: Option<i64>) {
        let Some(t) = self.tabs.get(&tab_id).filter(|_| self.contexts.is_tab_ready(tab_id)) else {
            return;
        };
        if tab_is_extension_surface(t) {
            return;
        }
        let host = t.host.clone();
        match backend_node_id {
            Some(id) => {
                self.send_internal_cdp(&host, "DOM.enable", json!({}));
                self.send_internal_cdp(&host, "Overlay.enable", json!({}));
                self.send_internal_cdp(
                    &host,
                    "Overlay.highlightNode",
                    json!({
                        "backendNodeId": id,
                        "highlightConfig": {
                            "showInfo": false,
                            "contentColor": { "r": 56, "g": 189, "b": 248, "a": 0.25 },
                            "borderColor": { "r": 56, "g": 189, "b": 248, "a": 0.9 }
                        }
                    }),
                );
            }
            None => {
                self.send_internal_cdp(&host, "Overlay.hideHighlight", json!({}));
            }
        }
    }

    pub(super) fn handle_find(
        &self,
        tab_id: u64,
        query: &str,
        forward: bool,
        match_case: bool,
        find_next: bool,
        user_initiated: bool,
    ) {
        let Some(t) = self.tabs.get(&tab_id) else {
            return;
        };
        if !self.contexts.is_tab_ready(tab_id) || (!user_initiated && tab_is_extension_surface(t)) {
            t.host.stop_finding(1);
            return;
        }
        if query.is_empty() {
            t.host.stop_finding(1);
        } else {
            t.host.find(
                Some(&CefString::from(query)),
                i32::from(forward),
                i32::from(match_case),
                i32::from(find_next),
            );
        }
    }

    pub(super) fn handle_set_agent_driven(&mut self, tab_id: u64, agent_driven: bool) {
        if let Some(t) = self.tabs.get(&tab_id) {
            t.prompts.agent_driven.store(agent_driven, Ordering::Release);
            t.prompts.user_reclaimed.store(false, Ordering::Release);
            t.snap.lock().unwrap().agent_driven = agent_driven;
            if agent_driven {
                // Agent-driven tabs may never navigate to `chrome-extension://` pages.
                t.allowed_ext_ids.lock().unwrap().clear();
            }
            if let Some(ext) = &mut self.extensions {
                ext.set_tab_agent_driven(tab_id, agent_driven);
            }
        }
    }

    pub(super) fn expect_ext_tab_target(
        &mut self,
        isolated: bool,
        is_preview: bool,
        is_popup: bool,
        allowed_ext_url: Option<&str>,
    ) {
        let Some(ext) = &mut self.extensions else {
            return;
        };
        if isolated || is_preview {
            ext.expect_isolated_tab();
        } else if let Some(ext_id) = allowed_ext_url {
            if is_popup {
                ext.expect_popup(ext_id);
            } else {
                ext.expect_extension_page(ext_id);
            }
        } else {
            ext.expect_web_tab();
        }
    }

    pub(super) fn notify_ext_tab_opened(
        &mut self,
        tab_id: u64,
        initial_url: &str,
        width_pt: f32,
        height_pt: f32,
        focused: bool,
        ext_open_request_id: Option<u64>,
        agent_driven: bool,
    ) {
        let effects = self
            .extensions
            .as_mut()
            .map(|ext| {
                let fx = ext.on_tab_opened(
                    tab_id,
                    initial_url,
                    width_pt.round() as i32,
                    height_pt.round() as i32,
                    focused,
                );
                if agent_driven {
                    ext.set_tab_agent_driven(tab_id, true);
                }
                fx
            })
            .unwrap_or_default();
        self.run_ext_effects(effects);
        if let Some(req_id) = ext_open_request_id
            && let Some((sid, rpc_id, ext_id)) = self.pending_ext_opens.remove(&req_id)
            && let Some(ext) = &mut self.extensions
        {
            ext.finish_tab_create(&sid, rpc_id, &ext_id, tab_id);
        }
    }
}

/// Fail-closed whole-tab extension guard checked at dispatch AND reply time for `HostMsg::CdpCall`,
/// `HostMsg::Find`, `SetInspect`, and `HighlightNode` (`review-rd274.md` Round-4 & Round-5).
pub(crate) fn tab_is_extension_surface(t: &ManagedTab) -> bool {
    browser_has_extension_surface(Some(&t.browser), t.is_popup, &t.snap, &t.prompts)
}

pub(crate) fn browser_has_extension_surface(
    browser: Option<&cef::Browser>,
    is_popup: bool,
    snap: &Mutex<TabSnapshot>,
    prompts: &TabPrompts,
) -> bool {
    if is_popup {
        return true;
    }
    if crate::extensions::manifest::is_extension_scheme(&snap.lock().unwrap().url) {
        return true;
    }
    if let Some(pending) = prompts.pending_top_url()
        && crate::extensions::manifest::is_extension_scheme(&pending)
    {
        return true;
    }
    if prompts.has_cdp_ext_frames() {
        return true;
    }
    let Some(browser) = browser else {
        return prompts.has_any_pending_ext_subframes();
    };
    if let Some(frame) = browser.main_frame() {
        let live_url = CefString::from(&frame.url()).to_string();
        if crate::extensions::manifest::is_extension_scheme(&live_url) {
            return true;
        }
    }
    let mut ids = cef::CefStringList::new();
    browser.frame_identifiers(Some(&mut ids));
    let mut live_frames: HashMap<String, String> = HashMap::new();
    for id in ids {
        let Some(frame) = browser.frame_by_identifier(Some(&CefString::from(id.as_str()))) else {
            continue;
        };
        let furl = CefString::from(&frame.url()).to_string();
        if crate::extensions::manifest::is_extension_scheme(&furl) {
            return true;
        }
        live_frames.insert(id, furl);
    }
    prompts.has_pending_ext_subframes(&live_frames)
}

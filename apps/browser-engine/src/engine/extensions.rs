//! Extension host effect runner and hidden seed-probe browser for `EngineManager`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cef::{BrowserSettings, CefString, ImplBrowser, ImplBrowserHost, ImplFrame, WindowInfo};
use serde_json::{Value, json};

use super::{BASE_COLOR, EngineManager};
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
                        tab.menu.cancel_all();
                        tab.prompts.dismiss_all();
                        tab.host.close_browser(1);
                        self.downloads.close_tab(engine_tab_id);
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
                                tab.host.close_browser(1);
                                self.downloads.close_tab(pid);
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

    /// Handles `HostMsg::CdpCall` with whole-tab refusal for extension main frames/popups and
    /// narrow sub-frame masking/stripping/hit-testing for web tabs embedding extension sub-frames
    /// (`review-rd274.md` Round-3 §2).
    pub(super) fn handle_cdp_call_guarded(
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
        let refuse = |out_tx: &crate::output::Output, msg: &str| {
            let _ = out_tx.send(EngineMsg::CdpResponse {
                tab_id,
                request_id,
                ok: false,
                result: json!({ "message": msg }),
            });
        };
        // Reject any cross-session / OOPIF sessionId so external callers can never target
        // extension iframes or non-page targets (`review-rd274.md` §1).
        if session_id.is_some() {
            refuse(&self.out_tx, "Cross-session DevTools targeting is not permitted.");
            return;
        }
        // Whole-tab refusal strictly for extension main frames and popups.
        if super::tab_is_extension_main_surface(t) {
            refuse(&self.out_tx, "Agent browser tools cannot read or act on extension pages.");
            return;
        }
        if let Err(err) = browser_policy::check_cdp(&method, &params, true) {
            refuse(&self.out_tx, &err);
            return;
        }

        let subframes = crate::extensions::frame_guard::inspect_subframes(&t.browser, &t.prompts);
        if !subframes.any_ext() {
            self.dispatch_cdp_raw(tab_id, request_id, &method, &params, CdpPending::Raw(request_id));
            return;
        }

        // Reject explicit `frameId` targeting of any committed or pending extension sub-frame.
        if let Some(fid) = params.get("frameId").and_then(Value::as_str) {
            let main_id = t
                .browser
                .main_frame()
                .map(|f| CefString::from(&f.identifier()).to_string())
                .unwrap_or_default();
            let same_main = !main_id.is_empty()
                && crate::extensions::frame_guard::normalize_frame_id(fid)
                    == crate::extensions::frame_guard::normalize_frame_id(&main_id);
            if subframes.matches_frame_id(fid) || (subframes.has_pending && !same_main) {
                refuse(&self.out_tx, "Agent browser tools cannot target extension sub-frames.");
                return;
            }
        }

        // Fail closed on screenshots while extension sub-frames are live/pending until the tracked
        // screenshot-masking follow-up lands (`review-rd274.md` Round-3 §2).
        if method == "Page.captureScreenshot" {
            refuse(
                &self.out_tx,
                "Screenshots on tabs with live extension frames are blocked until screenshot masking lands.",
            );
            return;
        }

        // Keyboard/text input: refuse when focus is inside an extension frame or while pending.
        if crate::extensions::frame_guard::is_keyboard_input_method(&method) {
            if subframes.has_pending
                || crate::extensions::frame_guard::focused_frame_is_extension(&t.browser)
            {
                refuse(&self.out_tx, "Agent browser tools cannot type into extension frames.");
                return;
            }
            self.dispatch_cdp_raw(tab_id, request_id, &method, &params, CdpPending::Raw(request_id));
            return;
        }

        // Pointer/touch input: hit-test against every extension sub-frame viewport bounding box
        // (including closed-ShadowRoot overlays and DNR-redirected frames) before forwarding.
        if crate::extensions::frame_guard::is_pointer_input_method(&method) {
            let points = crate::extensions::frame_guard::extract_input_points(&method, &params);
            if subframes.has_pending || points.is_empty() {
                refuse(
                    &self.out_tx,
                    "Agent browser tools cannot send pointer input while an extension frame is loading.",
                );
                return;
            }
            let job_id = self.next_hit_job;
            self.next_hit_job = self.next_hit_job.wrapping_add(1).max(1);
            self.hit_jobs.insert(
                job_id,
                super::HitTestJob {
                    tab_id,
                    request_id,
                    method,
                    params,
                    points,
                    remaining_boxes: 0,
                    blocked: false,
                },
            );
            let doc_params = json!({ "depth": -1, "pierce": true });
            self.dispatch_cdp_raw(
                tab_id,
                request_id,
                "DOM.getDocument",
                &doc_params,
                CdpPending::HitTestDoc(job_id),
            );
            return;
        }

        // FrameTree / Accessibility / DOM queries: strip extension sub-frames & tokens in reply.
        if let Some(kind) = crate::extensions::frame_guard::reply_filter_for_method(&method) {
            self.dispatch_cdp_raw(
                tab_id,
                request_id,
                &method,
                &params,
                CdpPending::FilteredSubframes {
                    request_id,
                    kind,
                    ext_frame_ids: subframes.ext_frame_ids,
                },
            );
            return;
        }

        self.dispatch_cdp_raw(tab_id, request_id, &method, &params, CdpPending::Raw(request_id));
    }

    pub(super) fn dispatch_cdp_raw(
        &mut self,
        tab_id: u64,
        request_id: u64,
        method: &str,
        params: &Value,
        pending: CdpPending,
    ) {
        let Some(t) = self.tabs.get(&tab_id) else {
            return;
        };
        let msg_id = self.next_cdp_id.fetch_add(1, Ordering::Relaxed);
        t.cdp_id_map.lock().unwrap().insert(msg_id, pending);
        let raw = json!({ "id": msg_id, "method": method, "params": params }).to_string();
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

    /// Advances multi-step CDP hit-testing for `Input.dispatchMouseEvent` / `dispatchTouchEvent`
    /// on web tabs containing extension sub-frames.
    pub(super) fn poll_hit_tests(&mut self) {
        let mut replies = Vec::new();
        for tab in self.tabs.values() {
            replies.extend(tab.hit_events.lock().unwrap().drain(..));
        }
        for reply in replies {
            match reply {
                crate::extensions::frame_guard::HitTestReply::Doc { job_id, ok, result } => {
                    let Some(mut job) = self.hit_jobs.remove(&job_id) else {
                        continue;
                    };
                    let Some(t) = self.tabs.get(&job.tab_id) else {
                        continue;
                    };
                    let fresh = crate::extensions::frame_guard::inspect_subframes(&t.browser, &t.prompts);
                    if !ok || super::tab_is_extension_main_surface(t) || fresh.has_pending {
                        let _ = self.out_tx.send(EngineMsg::CdpResponse {
                            tab_id: job.tab_id,
                            request_id: job.request_id,
                            ok: false,
                            result: json!({ "message": "Agent browser tools cannot interact with extension frames." }),
                        });
                        continue;
                    }
                    let bids = crate::extensions::frame_guard::find_ext_iframe_backend_ids(&result, &fresh);
                    if bids.is_empty() {
                        // All extension sub-frames disappeared before hit-test; safe to forward.
                        if !fresh.any_ext() {
                            self.dispatch_cdp_raw(
                                job.tab_id,
                                job.request_id,
                                &job.method,
                                &job.params,
                                CdpPending::Raw(job.request_id),
                            );
                        } else {
                            // Committed extension sub-frame exists but wasn't found in DOM -> fail closed.
                            let _ = self.out_tx.send(EngineMsg::CdpResponse {
                                tab_id: job.tab_id,
                                request_id: job.request_id,
                                ok: false,
                                result: json!({ "message": "Agent browser tools cannot verify extension frame bounds." }),
                            });
                        }
                        continue;
                    }
                    job.remaining_boxes = bids.len();
                    let tab_id = job.tab_id;
                    let req_id = job.request_id;
                    self.hit_jobs.insert(job_id, job);
                    for bid in bids {
                        self.dispatch_cdp_raw(
                            tab_id,
                            req_id,
                            "DOM.getBoxModel",
                            &json!({ "backendNodeId": bid }),
                            CdpPending::HitTestBox(job_id),
                        );
                    }
                }
                crate::extensions::frame_guard::HitTestReply::Box { job_id, ok, result } => {
                    let Some(job) = self.hit_jobs.get_mut(&job_id) else {
                        continue;
                    };
                    if ok {
                        if let Some(rect) = crate::extensions::frame_guard::parse_box_model_rect(&result)
                            && job
                                .points
                                .iter()
                                .any(|&pt| crate::extensions::frame_guard::point_in_rect(pt, rect))
                        {
                            job.blocked = true;
                        }
                    } else if !crate::extensions::frame_guard::is_unrendered_box_error(&result) {
                        job.blocked = true;
                    }
                    job.remaining_boxes = job.remaining_boxes.saturating_sub(1);
                    if job.remaining_boxes == 0 {
                        let Some(done) = self.hit_jobs.remove(&job_id) else {
                            continue;
                        };
                        let Some(t) = self.tabs.get(&done.tab_id) else {
                            continue;
                        };
                        let fresh = crate::extensions::frame_guard::inspect_subframes(&t.browser, &t.prompts);
                        if done.blocked || super::tab_is_extension_main_surface(t) || fresh.has_pending {
                            let _ = self.out_tx.send(EngineMsg::CdpResponse {
                                tab_id: done.tab_id,
                                request_id: done.request_id,
                                ok: false,
                                result: json!({ "message": "Agent browser tools cannot click or interact with extension frames." }),
                            });
                        } else {
                            self.dispatch_cdp_raw(
                                done.tab_id,
                                done.request_id,
                                &done.method,
                                &done.params,
                                CdpPending::Raw(done.request_id),
                            );
                        }
                    }
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
        }));
        let allowed_ext = vec![ext_id.clone()];
        let file_roots = Arc::new(Mutex::new(Vec::new()));
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
            snap.clone(),
            prompts.clone(),
            favicon,
            null_out.clone(),
            self.events.clone(),
        );
        let load = TabLoadHandler::new(
            u64::MAX,
            file_roots.clone(),
            snap.clone(),
            prompts.clone(),
            diagnostics.clone(),
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
            snap.clone(),
            prompt_shared.clone(),
            prompts.clone(),
            diagnostics,
            null_out.clone(),
        );
        let download = crate::downloads::TabDownloadHandler::new(
            u64::MAX,
            file_roots.clone(),
            true,
            self.downloads.clone(),
            null_out.clone(),
        );
        let life_span = TabLifeSpanHandler::new(u64::MAX, null_out.clone());
        let jsdialog = TabDialogHandler::new(u64::MAX, prompt_shared.clone(), prompts.clone(), null_out.clone());
        let permission = TabPermissionHandler::new(
            u64::MAX,
            prompt_shared.clone(),
            prompts.clone(),
            snap,
            null_out.clone(),
        );
        let dialog = TabFileDialogHandler::new(u64::MAX, prompts, null_out.clone());
        let find = crate::find::TabFindHandler::new(u64::MAX, null_out.clone());
        let context_menu = crate::menu::TabContextMenuHandler::new(
            u64::MAX,
            prompt_shared,
            file_roots,
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
}

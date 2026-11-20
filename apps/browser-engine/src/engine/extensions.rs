//! Extension host effect runner and hidden seed-probe browser for `EngineManager`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cef::{BrowserSettings, CefString, ImplBrowser, ImplBrowserHost, ImplFrame, WindowInfo};

use super::{BASE_COLOR, EngineManager};
use crate::handlers::{
    TabDisplayHandler, TabExtPolicy, TabLoadHandler, TabRenderHandler, TabRequestHandler,
    ViewRectState,
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
                        self.contexts.drop_private_tab(engine_tab_id);
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
                    self.grant_ext_id_to_user_tabs(&action.ext_id);
                    let _ = self.out_tx.send(EngineMsg::ExtensionAction { action });
                }
                ExtEffect::ExtensionsSynced(extensions) => {
                    for act in &extensions {
                        self.grant_ext_id_to_user_tabs(&act.ext_id);
                    }
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
                    if ok && !ext_id.is_empty() {
                        self.grant_ext_id_to_user_tabs(&ext_id);
                    }
                    let _ = self.out_tx.send(EngineMsg::ExtensionReloaded { ext_id, ok, error });
                }
            }
        }
    }

    fn grant_ext_id_to_user_tabs(&self, ext_id: &str) {
        for t in self.tabs.values() {
            if t.allow_extension_pages
                && !t.isolated
                && !t.is_popup
                && !t.prompts.agent_driven.load(Ordering::Relaxed)
            {
                let mut ids = t.allowed_ext_ids.lock().unwrap();
                if !ids.iter().any(|id| id == ext_id) {
                    ids.push(ext_id.to_owned());
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

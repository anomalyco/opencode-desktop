//! Multi-tab CEF OSR manager inside the sidecar process.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cef::{
    Browser, BrowserHost, BrowserSettings, CefString, ImplBrowser, ImplBrowserHost, ImplFrame,
    MouseButtonType, MouseEvent, PaintElementType, Registration, WindowInfo, sys,
};
use serde_json::{Value, json};

use crate::handlers::{
    CdpPending, FrameRing, PendingClear, TabCdpObserver, TabDisplayHandler, TabExtPolicy,
    TabLoadHandler, TabRenderHandler, TabRequestHandler, ViewRectState, cef_mods, event_flag,
};
use crate::keys::send_key;
use crate::tab_client::SidecarTabClient;
use crate::wire::{
    CursorKind, EngineMsg, HostMsg, PickedElement, PointerBtn, TabSnapshot,
};

pub use runtime::CefRuntime;

/// Opaque page background (ARGB) for pages that specify none. macOS uses Rune's dark base.
const BASE_COLOR: u32 = if cfg!(target_os = "macos") { 0xFF11_1318 } else { 0xFFFF_FFFF };

struct ManagedTab {
    browser: Browser,
    host: BrowserHost,
    file_roots: Arc<Mutex<Vec<String>>>,
    allowed_ext_ids: Arc<Mutex<Vec<String>>>,
    allow_extension_pages: bool,
    isolated: bool,
    is_popup: bool,
    view_state: Arc<Mutex<ViewRectState>>,
    snap: Arc<Mutex<TabSnapshot>>,
    cdp_id_map: Arc<Mutex<HashMap<i32, CdpPending>>>,
    internal_picked: Arc<Mutex<Vec<i64>>>,
    pending_clears: Arc<Mutex<Vec<PendingClear>>>,
    prompts: Arc<crate::prompts::TabPrompts>,
    menu: Arc<crate::menu::TabMenuState>,
    context: crate::wire::ContextKind,
    _cdp_reg: Option<Registration>,
    held_buttons: u32,
    /// Linux: whether it's the tab the app shows (`HostMsg::FocusTab`), to open it again so.
    #[cfg(target_os = "linux")]
    shown: bool,
    #[cfg(any(windows, target_os = "linux"))]
    frames: FrameRing,
}

pub struct EngineManager {
    tabs: HashMap<u64, ManagedTab>,
    mach_port: Arc<AtomicU32>,
    out_tx: crate::output::Output,
    events: std::sync::mpsc::Sender<crate::Event>,
    extensions: Option<crate::extensions::ExtensionHost>,
    rejected_exts: Vec<String>,
    stub_root: Option<std::path::PathBuf>,
    app_handles_ext_tabs: bool,
    next_ext_open_req: u64,
    pending_ext_opens: HashMap<u64, (String, i64, String)>,
    unmanaged_ext_tabs: usize,
    seed_probe: Option<BrowserHost>,
    next_ext_tab_id: u64,
    next_cdp_id: AtomicI32,
    next_eref: u64,
    /// The app's scheme, once it said (`HostMsg::SetColorScheme`).
    dark: Option<bool>,
    /// The user's home directory, for filtering overly broad workspace `file_roots`.
    home: Option<String>,
    /// The pages' downloads, in the profile's `opencode-page-downloads` folder.
    downloads: Arc<crate::downloads::Downloads>,
    /// Shared prompt-handling toggle (`Hello.prompts`) and monotonic ID counter.
    prompt_shared: crate::prompts::PromptShared,
    /// Shared pool of isolated `CefRequestContext`s for previews and remote servers.
    contexts: crate::context::ContextPool,
    /// Windows: the D3D11 device frames are copied on; None renders on the CPU instead.
    #[cfg(windows)]
    gpu: Option<Arc<crate::d3d_frames::Gpu>>,
    /// Linux: dmabufs, read-back or CPU paints once they can't work, and where tabs report a
    /// failed frame (`crate::Event::FramesFailed`).
    #[cfg(target_os = "linux")]
    transport: crate::dmabuf_frames::Transport,
}

impl EngineManager {
    /// Windows: `gpu` is the D3D11 device frames are copied on (None renders on the CPU).
    pub fn new(
        out_tx: crate::output::Output,
        downloads: std::path::PathBuf,
        events: std::sync::mpsc::Sender<crate::Event>,
        #[cfg(windows)] gpu: Option<Arc<crate::d3d_frames::Gpu>>,
        #[cfg(target_os = "linux")] transport: crate::dmabuf_frames::Transport,
        extensions: Option<crate::extensions::ExtensionHost>,
        rejected_exts: Vec<String>,
        stub_root: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            tabs: HashMap::new(),
            mach_port: Arc::new(AtomicU32::new(0)),
            out_tx,
            events: events.clone(),
            extensions,
            rejected_exts,
            stub_root,
            app_handles_ext_tabs: false,
            next_ext_open_req: 1,
            pending_ext_opens: HashMap::new(),
            unmanaged_ext_tabs: 0,
            seed_probe: None,
            next_ext_tab_id: 1_000_000,
            next_cdp_id: AtomicI32::new(100),
            next_eref: 1,
            dark: None,
            home: crate::profile::home_dir(),
            downloads: crate::downloads::Downloads::open(&downloads),
            prompt_shared: crate::prompts::PromptShared::default(),
            contexts: crate::context::ContextPool::new(events),
            #[cfg(windows)]
            gpu,
            #[cfg(target_os = "linux")]
            transport,
        }
    }

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
        let is_eligible = self
            .tabs
            .get(&tab_id)
            .is_some_and(|t| !t.isolated && !t.is_popup);
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

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Whether a tab dropped a frame and waits for the app to free a texture (`poll_frames`).
    pub fn waits_for_texture(&self) -> bool {
        #[cfg(windows)]
        return self.tabs.values().any(|t| t.frames.as_ref().is_some_and(|ring| ring.lock().unwrap().dropped));
        // Linux: the app's read repaints the tab (`HostMsg::FrameRead`); nothing waits on a timer.
        #[cfg(not(windows))]
        false
    }

    /// Windows: repaints each tab that dropped a frame while the app had every one unread,
    /// once it has read one.
    pub fn poll_frames(&mut self) {
        #[cfg(windows)]
        for t in self.tabs.values() {
            let Some(ring) = &t.frames else { continue };
            let mut ring = ring.lock().unwrap();
            if ring.dropped && ring.has_free() {
                ring.dropped = false;
                t.host.invalidate(PaintElementType::default());
            }
        }
    }

    /// Linux: runs tab `tab_id`'s frame effects.
    #[cfg(target_os = "linux")]
    fn run_frames(&self, tab_id: u64, effects: Vec<crate::dmabuf_frames::Effect>) {
        let host = self.tabs.get(&tab_id).map(|t| &t.host);
        let events = self.events.clone();
        crate::dmabuf_frames::run(effects, tab_id, &self.out_tx, host, &move |f| {
            let _ = events.send(crate::Event::FramesFailed(f));
        });
    }

    /// Linux: frames can't come the way they do; the engine's transport moves on.
    #[cfg(target_os = "linux")]
    pub fn frames_failed(&mut self, failure: crate::dmabuf_frames::Failure) {
        use crate::dmabuf_frames::{Event, TransportEffect};
        match self.transport.step(failure) {
            Some(TransportEffect::ReadBackTabs) => {
                for (&tab_id, t) in &self.tabs {
                    let Some(ring) = &t.frames else { continue };
                    self.run_frames(tab_id, ring.lock().unwrap().step(Event::ReadBack));
                }
            }
            Some(TransportEffect::ReopenTabs) => {
                let ids: Vec<u64> = self.tabs.keys().copied().collect();
                for tab_id in ids {
                    let Some(tab) = self.tabs.remove(&tab_id) else { continue };
                    tab.host.close_browser(1);
                    let url = tab.snap.lock().unwrap().url.clone();
                    let v = tab.view_state.lock().unwrap().clone();
                    let roots = tab.file_roots.lock().unwrap().clone();
                    let was_agent = tab.prompts.agent_driven.load(Ordering::Relaxed);
                    let allow_ext = tab.allow_extension_pages && !was_agent;
                    let ctx = tab.context.clone();
                    self.open_tab(
                        tab_id,
                        &url,
                        v.width_pt,
                        v.height_pt,
                        v.scale,
                        tab.shown,
                        roots,
                        ctx,
                        tab.isolated,
                        allow_ext,
                        tab.is_popup,
                        None,
                    );
                    if was_agent && let Some(reopened) = self.tabs.get(&tab_id) {
                        reopened.prompts.agent_driven.store(true, Ordering::Relaxed);
                        reopened.allowed_ext_ids.lock().unwrap().clear();
                        if let Some(ext) = &mut self.extensions {
                            ext.set_tab_agent_driven(tab_id, true);
                        }
                    }
                }
            }
            None => {}
        }
    }

    pub fn poll_picks(&mut self) {
        for outcome in self.contexts.drain_ready_loads() {
            match outcome {
                crate::context::DeferredOutcome::Load { tab_id, url } => {
                    if let Some(tab) = self.tabs.get(&tab_id)
                        && let Some(frame) = tab.browser.main_frame()
                    {
                        frame.load_url(Some(&CefString::from(url.as_str())));
                    }
                }
                crate::context::DeferredOutcome::Refuse { tab_id, reason } => {
                    let _ = self.out_tx.send(EngineMsg::Refused {
                        tab_id,
                        reason: reason.to_owned(),
                    });
                }
            }
        }
        let mut ready = Vec::new();
        let mut clears = Vec::new();
        for (&tab_id, tab) in &self.tabs {
            for backend_id in tab.internal_picked.lock().unwrap().drain(..) {
                ready.push((tab_id, backend_id));
            }
            for clear in tab.pending_clears.lock().unwrap().drain(..) {
                clears.push(clear);
            }
        }
        for clear in clears {
            if let Some(tab) = self.tabs.get(&clear.tab_id) {
                for (name, domain, path) in clear.cookies {
                    self.send_internal_cdp(
                        &tab.host,
                        "Network.deleteCookies",
                        json!({ "name": name, "domain": domain, "path": path }),
                    );
                }
                self.send_internal_cdp(
                    &tab.host,
                    "Storage.clearDataForOrigin",
                    json!({
                        "origin": clear.origin,
                        "storageTypes": "local_storage,indexeddb,service_workers,cache_storage,file_systems,shader_cache",
                    }),
                );
                tab.browser.reload();
                let _ = self.out_tx.send(EngineMsg::CdpResponse {
                    tab_id: clear.tab_id,
                    request_id: clear.request_id,
                    ok: true,
                    result: json!({}),
                });
            }
        }
        for (tab_id, backend_id) in ready {
            let eref = format!("@e{}", self.next_eref);
            self.next_eref += 1;
            if let Some(tab) = self.tabs.get(&tab_id) {
                // Turn off inspect mode and highlight the chosen node cleanly.
                self.send_internal_cdp(
                    &tab.host,
                    "Overlay.setInspectMode",
                    json!({ "mode": "none", "highlightConfig": {} }),
                );
                self.send_internal_cdp(
                    &tab.host,
                    "Overlay.highlightNode",
                    json!({
                        "backendNodeId": backend_id,
                        "highlightConfig": {
                            "showInfo": true,
                            "contentColor": { "r": 56, "g": 189, "b": 248, "a": 0.28 },
                            "borderColor": { "r": 56, "g": 189, "b": 248, "a": 0.90 }
                        }
                    }),
                );
            }
            let _ = self.out_tx.send(EngineMsg::InspectState {
                tab_id,
                active: false,
                picked: Some(PickedElement {
                    tab_id,
                    eref: eref.clone(),
                    backend_node_id: backend_id,
                    selector: format!("node#{backend_id}"),
                    label: format!("{eref} (node #{backend_id})"),
                    role: None,
                    name: None,
                    text: None,
                    rect: [0.0; 4],
                }),
            });
        }
    }

    pub fn handle(&mut self, msg: HostMsg) -> bool {
        match msg {
            HostMsg::Hello { mach_service, prompts, extensions, .. } => {
                self.prompt_shared.enabled.store(prompts, Ordering::Relaxed);
                self.app_handles_ext_tabs = extensions;
                #[cfg(not(target_os = "macos"))]
                let _ = mach_service;
                #[cfg(target_os = "macos")]
                if let Some(name) = mach_service {
                    let port = crate::handlers::lookup_mach_port(&name);
                    self.mach_port.store(port, Ordering::Relaxed);
                }
                let _ = self.out_tx.send(EngineMsg::Ready {
                    chromium_major: sys::CHROME_VERSION_MAJOR as u32,
                    pid: std::process::id(),
                    downloads: true,
                    policy: true,
                    prompts,
                    popups: true,
                    find: true,
                    context_menu: prompts,
                    partitions: true,
                    remote_proxy: true,
                    extensions: true,
                });
                for (&tab_id, t) in &self.tabs {
                    t.menu.cancel_all();
                    if prompts {
                        t.prompts.replay_pending(tab_id, &self.out_tx);
                    } else {
                        t.prompts.dismiss_all();
                    }
                }
                for ext_id in &self.rejected_exts {
                    let _ = self.out_tx.send(EngineMsg::ExtensionIntegrityFailed {
                        ext_id: ext_id.clone(),
                    });
                }
                if self.extensions.as_ref().is_none_or(|e| e.filters_ready()) {
                    let _ = self.out_tx.send(EngineMsg::ExtensionFiltersReady);
                }
                if !self.tabs.is_empty() {
                    let list: Vec<TabSnapshot> = self
                        .tabs
                        .values()
                        .map(|t| t.snap.lock().unwrap().clone())
                        .collect();
                    let _ = self.out_tx.send(EngineMsg::TabList { tabs: list });
                    for t in self.tabs.values() {
                        #[cfg(windows)]
                        if let Some(ring) = &t.frames {
                            ring.lock().unwrap().forget_reads();
                        }
                        #[cfg(target_os = "linux")]
                        if let Some(ring) = &t.frames {
                            let _ = ring.lock().unwrap().step(crate::dmabuf_frames::Event::Released);
                        }
                        t.host.invalidate(PaintElementType::default());
                    }
                }
            }
            HostMsg::OpenTab {
                tab_id,
                url,
                width_pt,
                height_pt,
                scale,
                focused,
                file_roots,
                context,
                isolated,
                allow_extension_pages,
                is_popup,
                ext_open_request_id,
            } => {
                // Remote-server tabs never receive local filesystem roots.
                let roots = if matches!(context, crate::wire::ContextKind::RemoteServer { .. }) {
                    Vec::new()
                } else {
                    self.safe_roots(file_roots)
                };
                self.open_tab(
                    tab_id,
                    &url,
                    width_pt,
                    height_pt,
                    scale,
                    focused,
                    roots,
                    context,
                    isolated,
                    allow_extension_pages,
                    is_popup,
                    ext_open_request_id,
                );
            }
            HostMsg::ClickExtension { ext_id } => {
                let effects = self
                    .extensions
                    .as_mut()
                    .map(|e| e.click_extension(&ext_id))
                    .unwrap_or_default();
                self.run_ext_effects(effects);
            }
            HostMsg::ListExtensions { request_id } => {
                let extensions = self
                    .extensions
                    .as_ref()
                    .map(|e| e.list_actions())
                    .unwrap_or_default();
                let _ = self.out_tx.send(EngineMsg::ExtensionsState {
                    request_id: Some(request_id),
                    extensions,
                });
            }
            HostMsg::UninstallExtension { ext_id } => {
                let effects = self
                    .extensions
                    .as_mut()
                    .map(|e| e.uninstall_extension(&ext_id))
                    .unwrap_or_default();
                self.run_ext_effects(effects);
            }
            HostMsg::ReloadExtension { ext_id } => {
                let effects = self
                    .extensions
                    .as_mut()
                    .map(|e| e.reload_extension(&ext_id))
                    .unwrap_or_default();
                self.run_ext_effects(effects);
            }
            HostMsg::CancelExtensionOpenTab { request_id } => {
                if let Some((sid, rpc_id, _)) = self.pending_ext_opens.remove(&request_id)
                    && let Some(ext) = &mut self.extensions
                {
                    ext.fail_tab_create(&sid, rpc_id, "Tab open refused by browser policy.");
                }
            }
            HostMsg::ExtensionMetrics { request_id } => {
                let metrics = self
                    .extensions
                    .as_ref()
                    .map(|e| e.metrics.clone())
                    .unwrap_or_default();
                let _ = self.out_tx.send(EngineMsg::ExtensionMetrics { request_id, metrics });
            }
            HostMsg::SetTabPolicy { tab_id, file_roots } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    if matches!(t.context, crate::wire::ContextKind::RemoteServer { .. }) {
                        t.file_roots.lock().unwrap().clear();
                    } else {
                        let roots = self.safe_roots(file_roots);
                        *t.file_roots.lock().unwrap() = roots;
                    }
                }
            }
            HostMsg::CloseTab { tab_id } => {
                if let Some(tab) = self.tabs.remove(&tab_id) {
                    tab.menu.cancel_all();
                    tab.prompts.dismiss_all();
                    tab.host.close_browser(1);
                    self.downloads.close_tab(tab_id);
                    self.contexts.drop_private_tab(tab_id);
                    if !tab.isolated && !tab.is_popup
                        && let Some(ext) = &mut self.extensions
                    {
                        ext.on_tab_closed(tab_id);
                    }
                    let _ = self.out_tx.send(EngineMsg::TabClosed { tab_id });
                }
            }
            HostMsg::FocusTab { tab_id } => {
                let is_normal = self.tabs.get(&tab_id).is_some_and(|t| !t.isolated && !t.is_popup);
                for (&id, t) in &mut self.tabs {
                    let active = id == tab_id;
                    #[cfg(target_os = "linux")]
                    {
                        t.shown = active;
                        // A hidden tab paints nothing: its buffers go until it's shown again.
                        if let Some(ring) = t.frames.as_ref().filter(|_| !active) {
                            let _ = ring.lock().unwrap().step(crate::dmabuf_frames::Event::Released);
                        }
                    }
                    if !t.is_popup {
                        t.host.was_hidden(i32::from(!active));
                    }
                    t.host.set_focus(i32::from(active));
                    if active {
                        t.host.invalidate(PaintElementType::default());
                    }
                }
                if is_normal && let Some(ext) = &mut self.extensions {
                    ext.on_tab_focused(tab_id);
                }
            }
            HostMsg::Navigate { tab_id, url } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    // Popups stay strictly on their own `chrome-extension://<id>/` origin.
                    if t.is_popup {
                        let own = crate::extensions::manifest::extension_id_from_url(&t.snap.lock().unwrap().url).map(str::to_owned);
                        let target = crate::extensions::manifest::extension_id_from_url(&url);
                        if own.as_deref() != target || target.is_none() {
                            let _ = self.out_tx.send(EngineMsg::Refused {
                                tab_id,
                                reason: browser_policy::Refusal::Blocked.code().to_owned(),
                            });
                            return true;
                        }
                    }
                    let is_agent = t.prompts.agent_driven.load(Ordering::Relaxed);
                    let ext_ok = !is_agent
                        && !t.isolated
                        && crate::extensions::manifest::extension_id_from_url(&url)
                            .is_some_and(|id| t.allowed_ext_ids.lock().unwrap().iter().any(|a| a == id));
                    let roots = t.file_roots.lock().unwrap().clone();
                    if !ext_ok && let Some(why) = browser_policy::frame_guard(&url, true, &roots) {
                        let _ = self.out_tx.send(EngineMsg::Refused {
                            tab_id,
                            reason: why.code().to_owned(),
                        });
                    } else if let Some(frame) = t.browser.main_frame() {
                        frame.load_url(Some(&CefString::from(url.as_str())));
                    }
                }
            }
            HostMsg::Back { tab_id } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    t.browser.go_back();
                }
            }
            HostMsg::Forward { tab_id } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    t.browser.go_forward();
                }
            }
            HostMsg::Reload { tab_id, ignore_cache } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    if ignore_cache {
                        t.browser.reload_ignore_cache();
                    } else {
                        t.browser.reload();
                    }
                }
            }
            HostMsg::Stop { tab_id } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    t.browser.stop_load();
                }
            }
            HostMsg::SetZoom { tab_id, factor } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    let level = factor.max(0.25).ln() / 1.2f64.ln();
                    t.host.set_zoom_level(level);
                    let mut s = t.snap.lock().unwrap();
                    s.zoom = factor;
                    let _ = self.out_tx.send(EngineMsg::TabUpdate { tab: s.clone() });
                }
            }
            // The pages' `prefers-color-scheme` follows the app's scheme, not the system's: the
            // context's Chrome colour scheme doesn't reach windowless pages, DevTools emulation does.
            HostMsg::SetColorScheme { dark } => {
                self.dark = Some(dark);
                for t in self.tabs.values() {
                    self.send_color_scheme(&t.host, dark);
                }
            }
            HostMsg::Emulate { tab_id, device, scale } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    emulation::apply(device.as_ref(), scale, |method, params| self.send_internal_cdp(&t.host, method, params));
                }
            }
            HostMsg::Resize { tab_id, width_pt, height_pt, scale } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    let changed = {
                        let mut v = t.view_state.lock().unwrap();
                        let d = (v.width_pt - width_pt).abs() > 0.5
                            || (v.height_pt - height_pt).abs() > 0.5
                            || (v.scale - scale).abs() > 0.01;
                        if d {
                            v.width_pt = width_pt.max(1.0);
                            v.height_pt = height_pt.max(1.0);
                            v.scale = scale.max(1.0);
                        }
                        d
                    };
                    if changed {
                        t.host.notify_move_or_resize_started();
                        t.host.notify_screen_info_changed();
                        t.host.was_resized();
                    }
                }
            }
            HostMsg::MouseMove { tab_id, x, y, mods, leave } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    let ev = MouseEvent {
                        x: x.round() as i32,
                        y: y.round() as i32,
                        modifiers: cef_mods(mods) | t.held_buttons,
                    };
                    t.host.send_mouse_move_event(Some(&ev), i32::from(leave));
                }
            }
            HostMsg::MouseButton { tab_id, x, y, button, up, clicks, mods } => {
                if let Some(t) = self.tabs.get_mut(&tab_id) {
                    let (btn, flag): (MouseButtonType, u32) = match button {
                        PointerBtn::Secondary => (
                            sys::cef_mouse_button_type_t::MBT_RIGHT.into(),
                            event_flag(sys::cef_event_flags_t::EVENTFLAG_RIGHT_MOUSE_BUTTON),
                        ),
                        PointerBtn::Middle => (
                            sys::cef_mouse_button_type_t::MBT_MIDDLE.into(),
                            event_flag(sys::cef_event_flags_t::EVENTFLAG_MIDDLE_MOUSE_BUTTON),
                        ),
                        PointerBtn::Primary => (
                            sys::cef_mouse_button_type_t::MBT_LEFT.into(),
                            event_flag(sys::cef_event_flags_t::EVENTFLAG_LEFT_MOUSE_BUTTON),
                        ),
                    };
                    if up {
                        t.held_buttons &= !flag;
                    } else {
                        t.held_buttons |= flag;
                        t.host.set_focus(1);
                    }
                    let ev = MouseEvent {
                        x: x.round() as i32,
                        y: y.round() as i32,
                        modifiers: cef_mods(mods) | t.held_buttons,
                    };
                    t.host.send_mouse_click_event(Some(&ev), btn, i32::from(up), clicks as i32);
                }
            }
            HostMsg::Wheel { tab_id, x, y, dx, dy, mods } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    let ev = MouseEvent {
                        x: x.round() as i32,
                        y: y.round() as i32,
                        modifiers: cef_mods(mods) | t.held_buttons,
                    };
                    let (dx, dy) = crate::handlers::wheel_delta(dx, dy);
                    t.host.send_mouse_wheel_event(Some(&ev), dx.round() as i32, dy.round() as i32);
                }
            }
            HostMsg::Key { tab_id, down, text, named, mods } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    send_key(&t.host, down, text.as_deref(), named.as_deref(), cef_mods(mods));
                }
            }
            HostMsg::EditCmd { tab_id, cmd } => {
                if let Some(t) = self.tabs.get(&tab_id)
                    && let Some(frame) = t.browser.main_frame()
                {
                    match cmd.as_str() {
                        "select_all" => frame.select_all(),
                        "copy" => frame.copy(),
                        "paste" => frame.paste(),
                        "cut" => frame.cut(),
                        "undo" => frame.undo(),
                        "redo" => frame.redo(),
                        _ => {}
                    }
                }
            }
            HostMsg::Ime { tab_id, commit, preedit } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    if let Some(c) = commit {
                        t.host.ime_commit_text(Some(&CefString::from(c.as_str())), None, 0);
                    } else if let Some(p) = preedit {
                        if p.is_empty() {
                            t.host.ime_cancel_composition();
                        } else {
                            t.host.ime_set_composition(
                                Some(&CefString::from(p.as_str())),
                                None,
                                None,
                                None,
                            );
                        }
                    }
                }
            }
            HostMsg::SetInspect { tab_id, enabled } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    if tab_is_extension_surface(t) {
                        return true;
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
            }
            HostMsg::HighlightNode { tab_id, backend_node_id } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    if tab_is_extension_surface(t) {
                        return true;
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
            }
            HostMsg::CdpCall { tab_id, request_id, method, params, session_id } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    // Reject any cross-session / OOPIF sessionId so external callers can never
                    // target extension iframes or non-page targets (`review-rd274.md` §1).
                    if session_id.is_some() {
                        let _ = self.out_tx.send(EngineMsg::CdpResponse {
                            tab_id,
                            request_id,
                            ok: false,
                            result: json!({ "message": "Cross-session DevTools targeting is not permitted." }),
                        });
                        return true;
                    }
                    // Agent browser tools must never read, evaluate, or act on extension pages
                    // (checks popup flag, snapshot URL, live main-frame URL, and pending nav URL).
                    if tab_is_extension_surface(t) {
                        let _ = self.out_tx.send(EngineMsg::CdpResponse {
                            tab_id,
                            request_id,
                            ok: false,
                            result: json!({ "message": "Agent browser tools cannot read or act on extension pages." }),
                        });
                        return true;
                    }
                    if let Err(err) = browser_policy::check_cdp(&method, &params, true) {
                        let _ = self.out_tx.send(EngineMsg::CdpResponse {
                            tab_id,
                            request_id,
                            ok: false,
                            result: json!({ "message": err }),
                        });
                        return true;
                    }
                    let msg_id = self.next_cdp_id.fetch_add(1, Ordering::Relaxed);
                    t.cdp_id_map.lock().unwrap().insert(msg_id, CdpPending::Raw(request_id));
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
            }
            HostMsg::SiteCookies { tab_id, request_id } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    let url = t.snap.lock().unwrap().url.clone();
                    if browser_policy::destination_origin(&url).is_none() {
                        let _ = self.out_tx.send(EngineMsg::CdpResponse {
                            tab_id,
                            request_id,
                            ok: true,
                            result: json!({ "cookies": 0 }),
                        });
                        return true;
                    }
                    let msg_id = self.next_cdp_id.fetch_add(1, Ordering::Relaxed);
                    t.cdp_id_map.lock().unwrap().insert(msg_id, CdpPending::CookieCount(request_id));
                    let raw = json!({
                        "id": msg_id,
                        "method": "Network.getCookies",
                        "params": { "urls": [url] },
                    })
                    .to_string();
                    if t.host.send_dev_tools_message(Some(raw.as_bytes())) != 1 {
                        t.cdp_id_map.lock().unwrap().remove(&msg_id);
                        let _ = self.out_tx.send(EngineMsg::CdpResponse {
                            tab_id,
                            request_id,
                            ok: false,
                            result: json!({ "message": "send_dev_tools_message rejected" }),
                        });
                    }
                }
            }
            HostMsg::ClearSite { tab_id, request_id } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    let url = t.snap.lock().unwrap().url.clone();
                    let Some(origin) = browser_policy::destination_origin(&url) else {
                        let _ = self.out_tx.send(EngineMsg::CdpResponse {
                            tab_id,
                            request_id,
                            ok: true,
                            result: json!({}),
                        });
                        return true;
                    };
                    let msg_id = self.next_cdp_id.fetch_add(1, Ordering::Relaxed);
                    t.cdp_id_map
                        .lock()
                        .unwrap()
                        .insert(msg_id, CdpPending::ClearSite { request_id, origin });
                    let raw = json!({
                        "id": msg_id,
                        "method": "Network.getCookies",
                        "params": { "urls": [url] },
                    })
                    .to_string();
                    if t.host.send_dev_tools_message(Some(raw.as_bytes())) != 1 {
                        t.cdp_id_map.lock().unwrap().remove(&msg_id);
                        let _ = self.out_tx.send(EngineMsg::CdpResponse {
                            tab_id,
                            request_id,
                            ok: false,
                            result: json!({ "message": "send_dev_tools_message rejected" }),
                        });
                    }
                }
            }
            #[cfg(target_os = "linux")]
            HostMsg::FrameRead { tab_id, pool, frame_id } => {
                if let Some(ring) = self.tabs.get(&tab_id).and_then(|t| t.frames.as_ref()) {
                    self.run_frames(tab_id, ring.lock().unwrap().step(crate::dmabuf_frames::Event::Read { pool, frame_id }));
                }
            }
            #[cfg(target_os = "linux")]
            HostMsg::DmabufFailed { reason } => self.frames_failed(crate::dmabuf_frames::Failure::Share(format!("the app: {reason}"))),
            #[cfg(not(target_os = "linux"))]
            HostMsg::FrameRead { .. } | HostMsg::DmabufFailed { .. } => {}
            HostMsg::AnswerDialog { tab_id, id, accept, text } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    t.prompts.answer_dialog(id, accept, &text);
                }
            }
            HostMsg::AnswerPermission { tab_id, id, allow } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    t.prompts.answer_permission(id, allow);
                }
            }
            HostMsg::AnswerAuth { tab_id, id, username, password } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    t.prompts.answer_auth(id, username.as_deref(), password.as_deref());
                }
            }
            HostMsg::SetAgentDriven { tab_id, agent_driven } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    t.prompts.agent_driven.store(agent_driven, Ordering::Relaxed);
                    if agent_driven {
                        // Agent-driven tabs may never navigate to `chrome-extension://` pages.
                        t.allowed_ext_ids.lock().unwrap().clear();
                    }
                    if let Some(ext) = &mut self.extensions {
                        ext.set_tab_agent_driven(tab_id, agent_driven);
                    }
                }
            }
            HostMsg::ClearSiteCache { tab_id, request_id } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    let url = t.snap.lock().unwrap().url.clone();
                    if let Some(origin) = browser_policy::destination_origin(&url) {
                        self.send_internal_cdp(
                            &t.host,
                            "Storage.clearDataForOrigin",
                            json!({
                                "origin": origin,
                                "storageTypes": "cache_storage,service_workers,shader_cache",
                            }),
                        );
                        self.send_internal_cdp(&t.host, "Network.clearBrowserCache", json!({}));
                        t.browser.reload_ignore_cache();
                    }
                    let _ = self.out_tx.send(EngineMsg::CdpResponse {
                        tab_id,
                        request_id,
                        ok: true,
                        result: json!({ "cleared": true }),
                    });
                }
            }
            HostMsg::Find { tab_id, query, forward, match_case, find_next } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    if query.is_empty() {
                        t.host.stop_finding(1);
                    } else {
                        t.host.find(
                            Some(&CefString::from(query.as_str())),
                            i32::from(forward),
                            i32::from(match_case),
                            i32::from(find_next),
                        );
                    }
                }
            }
            HostMsg::StopFind { tab_id, clear_selection } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    t.host.stop_finding(i32::from(clear_selection));
                }
            }
            HostMsg::ContextMenuAction { tab_id, id, action } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    let roots = t.file_roots.lock().unwrap().clone();
                    t.menu.execute(id, action, &t.browser, &t.host, &roots);
                }
            }
            HostMsg::SetPartitionProxy { partition, proxy } => {
                if let Ok(applied) = self.contexts.set_partition_proxy(&partition, proxy) {
                    for t in self.tabs.values() {
                        if matches!(&t.context, crate::wire::ContextKind::RemoteServer { partition: p, .. } if p == &partition) {
                            *t.prompts.proxy.lock().unwrap() = applied.clone();
                        }
                    }
                }
            }
            HostMsg::DropContext { context } => {
                self.contexts.drop_context(&context);
            }
            HostMsg::Shutdown => {
                for (_, t) in self.tabs.drain() {
                    t.menu.cancel_all();
                    t.prompts.dismiss_all();
                    t.host.close_browser(1);
                }
                return false;
            }
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn open_tab(
        &mut self,
        tab_id: u64,
        url: &str,
        width_pt: f32,
        height_pt: f32,
        scale: f32,
        focused: bool,
        file_roots: Vec<String>,
        context: crate::wire::ContextKind,
        isolated_flag: bool,
        allow_extension_pages: bool,
        is_popup: bool,
        ext_open_request_id: Option<u64>,
    ) {
        // Normalize legacy `isolated: true` into a dedicated per-tab `ContextKind::PrivateTab`
        // so private tabs NEVER share cookies/storage/cache with previews, artifacts, remote
        // servers, or each other (`review-rd274.md` §5).
        let effective_context = if isolated_flag && matches!(context, crate::wire::ContextKind::Default) {
            crate::wire::ContextKind::PrivateTab
        } else {
            context
        };
        let isolated = !matches!(effective_context, crate::wire::ContextKind::Default);
        // Non-default context tabs (preview, artifact, remote, private) NEVER open extension pages or popups.
        let allow_extension_pages = allow_extension_pages && !isolated;
        let is_popup_valid = is_popup && allow_extension_pages;
        let allowed_ext_list: Vec<String> = if allow_extension_pages {
            self.extensions
                .as_ref()
                .map(|e| e.list_actions().into_iter().map(|a| a.ext_id).collect())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let is_allowed_ext_url = crate::extensions::manifest::extension_id_from_url(url)
            .filter(|id| allowed_ext_list.iter().any(|allowed| allowed == id));
        // If `is_popup` was requested for a non-extension or disallowed URL, fail closed immediately.
        if is_popup && (!is_popup_valid || is_allowed_ext_url.is_none()) {
            let _ = self.out_tx.send(EngineMsg::Refused {
                tab_id,
                reason: browser_policy::Refusal::Blocked.code().to_owned(),
            });
            return;
        }
        let is_popup = is_popup_valid;
        // A popup tab may only ever navigate inside its own extension's origin.
        let allowed_ext_list = if is_popup {
            is_allowed_ext_url.map(|id| vec![id.to_owned()]).unwrap_or_default()
        } else {
            allowed_ext_list
        };
        let refused = if is_allowed_ext_url.is_some() {
            None
        } else {
            browser_policy::frame_guard(url, true, &file_roots)
        };
        let target_url = if refused.is_some() { "about:blank" } else { url };
        // Fail closed if a non-default `CefRequestContext` cannot be created (never falls back to
        // the default profile with user cookies and extensions).
        let resolved_ctx = match self.contexts.resolve(&effective_context, tab_id, target_url) {
            Ok(r) => r,
            Err(reason) => {
                let _ = self.out_tx.send(EngineMsg::Refused {
                    tab_id,
                    reason: reason.to_owned(),
                });
                return;
            }
        };
        let initial_url = if resolved_ctx.defer_initial_load { "about:blank" } else { target_url };
        let mut req_ctx = resolved_ctx.request_context;
        let file_roots = Arc::new(Mutex::new(file_roots));
        let allowed_ext_ids = Arc::new(Mutex::new(allowed_ext_list));
        let view_state = Arc::new(Mutex::new(ViewRectState {
            width_pt: width_pt.max(1.0),
            height_pt: height_pt.max(1.0),
            scale: scale.max(1.0),
        }));
        let snap = Arc::new(Mutex::new(TabSnapshot {
            tab_id,
            url: initial_url.to_owned(),
            title: "New tab".into(),
            loading: refused.is_none(),
            can_go_back: false,
            can_go_forward: false,
            zoom: 1.0,
            cursor: CursorKind::Default,
            load_error: None,
            favicon: None,
        }));
        #[cfg(windows)]
        let frames: FrameRing = self
            .gpu
            .clone()
            .map(|gpu| Arc::new(Mutex::new(crate::d3d_frames::TabFrames::new(gpu, tab_id))));
        #[cfg(target_os = "linux")]
        let frames: FrameRing = self.transport.frames().map(|f| Arc::new(Mutex::new(f)));
        #[cfg(target_os = "macos")]
        let frames: FrameRing = ();
        #[cfg(any(windows, target_os = "linux"))]
        let shared_texture = frames.is_some();
        #[cfg(target_os = "macos")]
        let shared_texture = true;
        let render = TabRenderHandler::new(
            tab_id,
            view_state.clone(),
            self.mach_port.clone(),
            self.out_tx.clone(),
            Arc::new(AtomicU64::new(0)),
            frames.clone(),
            #[cfg(target_os = "linux")]
            self.events.clone(),
            #[cfg(not(target_os = "linux"))]
            (),
        );
        let prompts = Arc::new(crate::prompts::TabPrompts::default());
        *prompts.proxy.lock().unwrap() = resolved_ctx.proxy;
        let favicon = Arc::new(crate::favicon::TabFavicon::default());
        let diagnostics = Arc::new(crate::diagnostics::TabDiagnostics::default());
        let menu_state = Arc::new(crate::menu::TabMenuState::default());
        let display = TabDisplayHandler::new(
            tab_id,
            snap.clone(),
            prompts.clone(),
            favicon,
            self.out_tx.clone(),
            self.events.clone(),
        );
        let load = TabLoadHandler::new(
            tab_id,
            file_roots.clone(),
            snap.clone(),
            prompts.clone(),
            diagnostics.clone(),
            self.out_tx.clone(),
            self.events.clone(),
        );
        let request = TabRequestHandler::new(
            tab_id,
            file_roots.clone(),
            TabExtPolicy {
                allowed_ext_ids: allowed_ext_ids.clone(),
                popup_ext_id: is_popup.then(|| is_allowed_ext_url.map(str::to_owned)).flatten(),
                isolated,
            },
            snap.clone(),
            self.prompt_shared.clone(),
            prompts.clone(),
            diagnostics,
            self.out_tx.clone(),
        );
        let download = crate::downloads::TabDownloadHandler::new(
            tab_id,
            file_roots.clone(),
            is_popup,
            self.downloads.clone(),
            self.out_tx.clone(),
        );
        let life_span = crate::prompts::TabLifeSpanHandler::new(tab_id, self.out_tx.clone());
        let jsdialog = crate::prompts::TabDialogHandler::new(
            tab_id,
            self.prompt_shared.clone(),
            prompts.clone(),
            self.out_tx.clone(),
        );
        let permission = crate::prompts::TabPermissionHandler::new(
            tab_id,
            self.prompt_shared.clone(),
            prompts.clone(),
            snap.clone(),
            self.out_tx.clone(),
        );
        let dialog = crate::prompts::TabFileDialogHandler::new(tab_id, prompts.clone(), self.out_tx.clone());
        let find = crate::find::TabFindHandler::new(tab_id, self.out_tx.clone());
        let context_menu = crate::menu::TabContextMenuHandler::new(
            tab_id,
            self.prompt_shared.clone(),
            file_roots.clone(),
            menu_state.clone(),
            self.out_tx.clone(),
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
        win_info.shared_texture_enabled = i32::from(shared_texture);
        win_info.external_begin_frame_enabled = 0;

        let browser_settings = BrowserSettings {
            windowless_frame_rate: 60,
            background_color: BASE_COLOR,
            ..Default::default()
        };

        // Tell ExtensionHost what kind of OSR target CEF is about to create synchronously.
        if let Some(ext) = &mut self.extensions {
            if isolated {
                ext.expect_isolated_tab();
            } else if let Some(ext_id) = is_allowed_ext_url {
                if is_popup {
                    ext.expect_popup(ext_id);
                } else {
                    ext.expect_extension_page(ext_id);
                }
            } else {
                ext.expect_web_tab();
            }
        }

        let Some(browser) = cef::browser_host_create_browser_sync(
            Some(&win_info),
            Some(&mut client),
            Some(&CefString::from(initial_url)),
            Some(&browser_settings),
            None,
            req_ctx.as_mut(),
        ) else {
            self.contexts.drop_private_tab(tab_id);
            if let Some(ext) = &mut self.extensions {
                ext.cancel_expected_target();
            }
            if let Some(req_id) = ext_open_request_id
                && let Some((sid, rpc_id, _)) = self.pending_ext_opens.remove(&req_id)
                && let Some(ext) = &mut self.extensions
            {
                ext.fail_tab_create(&sid, rpc_id, "Could not create browser tab.");
            }
            return;
        };
        let Some(host) = browser.host() else {
            self.contexts.drop_private_tab(tab_id);
            if let Some(ext) = &mut self.extensions {
                ext.cancel_expected_target();
            }
            return;
        };
        if is_popup {
            host.was_hidden(0);
            host.set_focus(1);
        } else if focused {
            for existing in self.tabs.values() {
                if !existing.is_popup {
                    existing.host.was_hidden(1);
                    existing.host.set_focus(0);
                }
            }
            host.set_focus(1);
        } else {
            host.was_hidden(1);
        }

        let cdp_id_map = Arc::new(Mutex::new(HashMap::new()));
        let internal_picked = Arc::new(Mutex::new(Vec::new()));
        let pending_clears = Arc::new(Mutex::new(Vec::new()));
        let mut observer = TabCdpObserver::new(
            tab_id,
            cdp_id_map.clone(),
            internal_picked.clone(),
            pending_clears.clone(),
            self.out_tx.clone(),
        );
        let cdp_reg = host.add_dev_tools_message_observer(Some(&mut observer));
        if let Some(dark) = self.dark {
            self.send_color_scheme(&host, dark);
        }

        self.tabs.insert(
            tab_id,
            ManagedTab {
                browser,
                host,
                file_roots,
                allowed_ext_ids,
                allow_extension_pages,
                isolated,
                is_popup,
                view_state,
                snap,
                cdp_id_map,
                internal_picked,
                pending_clears,
                prompts,
                menu: menu_state,
                context: effective_context,
                _cdp_reg: cdp_reg,
                held_buttons: 0,
                #[cfg(target_os = "linux")]
                shown: focused,
                #[cfg(any(windows, target_os = "linux"))]
                frames,
            },
        );
        if !isolated && !is_popup {
            let effects = self
                .extensions
                .as_mut()
                .map(|ext| {
                    ext.on_tab_opened(
                        tab_id,
                        initial_url,
                        width_pt.round() as i32,
                        height_pt.round() as i32,
                        focused,
                    )
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
        if let Some(why) = refused {
            let _ = self.out_tx.send(EngineMsg::Refused {
                tab_id,
                reason: why.code().to_owned(),
            });
        }
    }

    fn safe_roots(&self, roots: Vec<String>) -> Vec<String> {
        roots
            .into_iter()
            .filter(|r| browser_policy::is_safe_workspace_root(r, self.home.as_deref()))
            .collect()
    }

    fn send_color_scheme(&self, host: &BrowserHost, dark: bool) {
        let value = if dark { "dark" } else { "light" };
        self.send_internal_cdp(host, "Emulation.setEmulatedMedia", json!({ "features": [{ "name": "prefers-color-scheme", "value": value }] }));
    }

    fn send_internal_cdp(&self, host: &BrowserHost, method: &str, params: Value) {
        let id = self.next_cdp_id.fetch_add(1, Ordering::Relaxed);
        let raw = json!({ "id": id, "method": method, "params": params }).to_string();
        let _ = host.send_dev_tools_message(Some(raw.as_bytes()));
    }
}

/// Whether `t` is currently displaying or navigating to an extension page or popup
/// (checks popup flag, snapshot URL, live CEF main-frame URL, and pending navigation URL to
/// close the navigation TOCTOU race; `review-rd274.md` §1).
fn tab_is_extension_surface(t: &ManagedTab) -> bool {
    if t.is_popup {
        return true;
    }
    if crate::extensions::manifest::is_extension_scheme(&t.snap.lock().unwrap().url) {
        return true;
    }
    if let Some(frame) = t.browser.main_frame() {
        let live_url = CefString::from(&frame.url()).to_string();
        if crate::extensions::manifest::is_extension_scheme(&live_url) {
            return true;
        }
    }
    if let Some(pending) = t.prompts.pending_top_url()
        && crate::extensions::manifest::is_extension_scheme(&pending)
    {
        return true;
    }
    false
}

mod emulation;
mod extensions;
mod runtime;

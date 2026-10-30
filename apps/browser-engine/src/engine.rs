//! Multi-tab CEF OSR manager inside the sidecar process.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use cef::rc::Rc as _;
use cef::{
    App, Browser, BrowserHost, BrowserProcessHandler, BrowserSettings, CefString, Client,
    CommandLine, DisplayHandler, ImplApp, ImplBrowser, ImplBrowserHost, ImplBrowserProcessHandler,
    ImplClient, ImplCommandLine, ImplFrame, KeyEvent, KeyEventType, LoadHandler, MouseButtonType,
    MouseEvent, PaintElementType, Registration, RenderHandler, Settings, WindowInfo, WrapApp,
    WrapBrowserProcessHandler, WrapClient, sys, wrap_app, wrap_browser_process_handler,
    wrap_client,
};
use serde_json::{Value, json};

use crate::handlers::{
    TabCdpObserver, TabDisplayHandler, TabLoadHandler, TabRenderHandler, ViewRectState, ascii_vk,
    cef_mods, mac_key_code, named_codes,
};
use crate::profile::Profile;
use crate::wire::{
    CursorKind, EngineMsg, HostMsg, PickedElement, PointerBtn, TabSnapshot,
};

pub struct CefRuntime {
    profile: Profile,
    #[cfg(target_os = "macos")]
    _loader: cef::library_loader::LibraryLoader,
}

impl CefRuntime {
    pub fn initialize() -> Self {
        #[cfg(target_os = "macos")]
        crate::mac_app::ensure_ns_application();

        #[cfg(target_os = "macos")]
        let loader = {
            let exe = std::env::current_exe().expect("current_exe");
            let loader = cef::library_loader::LibraryLoader::new(&exe, false);
            assert!(loader.load(), "failed to load Chromium Embedded Framework");
            loader
        };

        let _ = cef::api_hash(sys::CEF_API_VERSION_LAST, 0);

        let profile = Profile::open();

        let exe_str = std::env::current_exe()
            .ok()
            .and_then(|p| p.into_os_string().into_string().ok())
            .unwrap_or_else(|| "opencode-browser-engine".into());
        let c_exe = std::ffi::CString::new(exe_str).unwrap();
        let argv = [c_exe.as_ptr()];
        let main_args = cef::MainArgs { argc: 1, argv: argv.as_ptr() as *mut *mut _ };

        let mut app = SidecarCefApp::new();
        let settings = Settings {
            windowless_rendering_enabled: 1,
            external_message_pump: 1,
            no_sandbox: 1,
            root_cache_path: CefString::from(profile.root.to_string_lossy().as_ref()),
            cache_path: CefString::from(profile.cache.to_string_lossy().as_ref()),
            persist_session_cookies: 1,
            log_severity: sys::cef_log_severity_t::LOGSEVERITY_WARNING.into(),
            ..Default::default()
        };
        let ok = cef::initialize(Some(&main_args), Some(&settings), Some(&mut app), std::ptr::null_mut());
        assert_eq!(ok, 1, "cef::initialize failed");

        Self {
            profile,
            #[cfg(target_os = "macos")]
            _loader: loader,
        }
    }
}

impl Drop for CefRuntime {
    fn drop(&mut self) {
        cef::shutdown();
        if self.profile.temporary {
            let _ = std::fs::remove_dir_all(&self.profile.root);
        }
    }
}

wrap_app! {
    struct SidecarCefApp {}
    impl App {
        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&cef::CefStringUtf16>,
            command_line: Option<&mut CommandLine>,
        ) {
            let Some(cmd) = command_line else { return };
            cmd.append_switch(Some(&"mute-audio".into()));
            cmd.append_switch(Some(&"use-mock-keychain".into()));
            cmd.append_switch(Some(&"no-startup-window".into()));
            cmd.append_switch(Some(&"noerrdialogs".into()));
            cmd.append_switch(Some(&"hide-crash-restore-bubble".into()));
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(SidecarProcessHandler::new())
        }
    }
}

wrap_browser_process_handler! {
    struct SidecarProcessHandler {}
    impl BrowserProcessHandler {}
}

wrap_client! {
    struct SidecarTabClient {
        render: RenderHandler,
        display: DisplayHandler,
        load: LoadHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
        }
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.display.clone())
        }
        fn load_handler(&self) -> Option<LoadHandler> {
            Some(self.load.clone())
        }
    }
}

struct ManagedTab {
    browser: Browser,
    host: BrowserHost,
    view_state: Arc<Mutex<ViewRectState>>,
    snap: Arc<Mutex<TabSnapshot>>,
    cdp_id_map: Arc<Mutex<HashMap<i32, u64>>>,
    internal_picked: Arc<Mutex<Vec<i64>>>,
    _cdp_reg: Option<Registration>,
    held_buttons: u32,
}

pub struct EngineManager {
    tabs: HashMap<u64, ManagedTab>,
    mach_port: Arc<AtomicU32>,
    out_tx: Sender<EngineMsg>,
    next_cdp_id: AtomicI32,
    next_eref: u64,
}

impl EngineManager {
    pub fn new(out_tx: Sender<EngineMsg>) -> Self {
        Self {
            tabs: HashMap::new(),
            mach_port: Arc::new(AtomicU32::new(0)),
            out_tx,
            next_cdp_id: AtomicI32::new(100),
            next_eref: 1,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn poll_picks(&mut self) {
        let mut ready = Vec::new();
        for (&tab_id, tab) in &self.tabs {
            for backend_id in tab.internal_picked.lock().unwrap().drain(..) {
                ready.push((tab_id, backend_id));
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
            HostMsg::Hello { mach_service, .. } => {
                #[cfg(target_os = "macos")]
                if let Some(name) = mach_service {
                    let port = crate::handlers::lookup_mach_port(&name);
                    self.mach_port.store(port, Ordering::Relaxed);
                }
                let _ = self.out_tx.send(EngineMsg::Ready {
                    chromium_major: sys::CHROME_VERSION_MAJOR as u32,
                    pid: std::process::id(),
                });
                if !self.tabs.is_empty() {
                    let list: Vec<TabSnapshot> = self
                        .tabs
                        .values()
                        .map(|t| t.snap.lock().unwrap().clone())
                        .collect();
                    let _ = self.out_tx.send(EngineMsg::TabList { tabs: list });
                    for t in self.tabs.values() {
                        t.host.invalidate(PaintElementType::default());
                    }
                }
            }
            HostMsg::OpenTab { tab_id, url, width_pt, height_pt, scale, focused } => {
                self.open_tab(tab_id, &url, width_pt, height_pt, scale, focused);
            }
            HostMsg::CloseTab { tab_id } => {
                if let Some(tab) = self.tabs.remove(&tab_id) {
                    tab.host.close_browser(1);
                    let _ = self.out_tx.send(EngineMsg::TabClosed { tab_id });
                }
            }
            HostMsg::FocusTab { tab_id } => {
                for (&id, t) in &self.tabs {
                    let active = id == tab_id;
                    t.host.was_hidden(i32::from(!active));
                    t.host.set_focus(i32::from(active));
                    if active {
                        t.host.invalidate(PaintElementType::default());
                    }
                }
            }
            HostMsg::Navigate { tab_id, url } => {
                if let Some(t) = self.tabs.get(&tab_id)
                    && let Some(frame) = t.browser.main_frame()
                {
                    frame.load_url(Some(&CefString::from(url.as_str())));
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
            HostMsg::Reload { tab_id } => {
                if let Some(t) = self.tabs.get(&tab_id) {
                    t.browser.reload();
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
                            sys::cef_event_flags_t::EVENTFLAG_RIGHT_MOUSE_BUTTON.0,
                        ),
                        PointerBtn::Middle => (
                            sys::cef_mouse_button_type_t::MBT_MIDDLE.into(),
                            sys::cef_event_flags_t::EVENTFLAG_MIDDLE_MOUSE_BUTTON.0,
                        ),
                        PointerBtn::Primary => (
                            sys::cef_mouse_button_type_t::MBT_LEFT.into(),
                            sys::cef_event_flags_t::EVENTFLAG_LEFT_MOUSE_BUTTON.0,
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
                if let Some(host) = self.tabs.get(&tab_id).map(|t| t.host.clone()) {
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
                if let Some(host) = self.tabs.get(&tab_id).map(|t| t.host.clone()) {
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
                    let msg_id = self.next_cdp_id.fetch_add(1, Ordering::Relaxed);
                    t.cdp_id_map.lock().unwrap().insert(msg_id, request_id);
                    let mut payload = json!({
                        "id": msg_id,
                        "method": method,
                        "params": params,
                    });
                    if let Some(sid) = session_id {
                        payload["sessionId"] = Value::String(sid);
                    }
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
            HostMsg::Shutdown => {
                for (_, t) in self.tabs.drain() {
                    t.host.close_browser(1);
                }
                return false;
            }
        }
        true
    }

    fn open_tab(
        &mut self,
        tab_id: u64,
        url: &str,
        width_pt: f32,
        height_pt: f32,
        scale: f32,
        focused: bool,
    ) {
        let view_state = Arc::new(Mutex::new(ViewRectState {
            width_pt: width_pt.max(1.0),
            height_pt: height_pt.max(1.0),
            scale: scale.max(1.0),
        }));
        let snap = Arc::new(Mutex::new(TabSnapshot {
            tab_id,
            url: url.to_owned(),
            title: "New tab".into(),
            loading: true,
            can_go_back: false,
            can_go_forward: false,
            zoom: 1.0,
            cursor: CursorKind::Default,
        }));
        let render = TabRenderHandler::new(
            tab_id,
            view_state.clone(),
            self.mach_port.clone(),
            self.out_tx.clone(),
            Arc::new(AtomicU64::new(0)),
        );
        let display = TabDisplayHandler::new(snap.clone(), self.out_tx.clone());
        let load = TabLoadHandler::new(snap.clone(), self.out_tx.clone());
        let mut client = SidecarTabClient::new(render, display, load);

        let mut win_info = WindowInfo::default().set_as_windowless(std::ptr::null_mut());
        win_info.shared_texture_enabled = i32::from(cfg!(target_os = "macos"));
        win_info.external_begin_frame_enabled = 0;

        let browser_settings = BrowserSettings {
            windowless_frame_rate: 60,
            background_color: 0xFF11_1318,
            ..Default::default()
        };

        let Some(browser) = cef::browser_host_create_browser_sync(
            Some(&win_info),
            Some(&mut client),
            Some(&CefString::from(url)),
            Some(&browser_settings),
            None,
            None,
        ) else {
            return;
        };
        let Some(host) = browser.host() else { return };
        if focused {
            for existing in self.tabs.values() {
                existing.host.was_hidden(1);
                existing.host.set_focus(0);
            }
            host.set_focus(1);
        } else {
            host.was_hidden(1);
        }

        let cdp_id_map = Arc::new(Mutex::new(HashMap::new()));
        let internal_picked = Arc::new(Mutex::new(Vec::new()));
        let mut observer = TabCdpObserver::new(
            tab_id,
            cdp_id_map.clone(),
            internal_picked.clone(),
            self.out_tx.clone(),
        );
        let cdp_reg = host.add_dev_tools_message_observer(Some(&mut observer));

        self.tabs.insert(
            tab_id,
            ManagedTab {
                browser,
                host,
                view_state,
                snap,
                cdp_id_map,
                internal_picked,
                _cdp_reg: cdp_reg,
                held_buttons: 0,
            },
        );
    }

    fn send_internal_cdp(&self, host: &BrowserHost, method: &str, params: Value) {
        let id = self.next_cdp_id.fetch_add(1, Ordering::Relaxed);
        let raw = json!({ "id": id, "method": method, "params": params }).to_string();
        let _ = host.send_dev_tools_message(Some(raw.as_bytes()));
    }
}

fn send_key(
    host: &BrowserHost,
    down: bool,
    text: Option<&str>,
    named: Option<&str>,
    modifiers: u32,
) {
    if let Some(s) = text {
        for ch in s.chars() {
            let vk = ascii_vk(ch);
            let mc = mac_key_code(ch);
            if down {
                let raw_down = KeyEvent {
                    type_: KeyEventType::from(sys::cef_key_event_type_t::KEYEVENT_RAWKEYDOWN),
                    modifiers,
                    windows_key_code: vk,
                    native_key_code: mc,
                    character: ch as u16,
                    unmodified_character: ch.to_ascii_lowercase() as u16,
                    ..Default::default()
                };
                host.send_key_event(Some(&raw_down));
                let chr = KeyEvent {
                    type_: KeyEventType::from(sys::cef_key_event_type_t::KEYEVENT_CHAR),
                    modifiers,
                    windows_key_code: ch as i32,
                    native_key_code: mc,
                    character: ch as u16,
                    unmodified_character: ch.to_ascii_lowercase() as u16,
                    ..Default::default()
                };
                host.send_key_event(Some(&chr));
            } else {
                let up = KeyEvent {
                    type_: KeyEventType::from(sys::cef_key_event_type_t::KEYEVENT_KEYUP),
                    modifiers,
                    windows_key_code: vk,
                    native_key_code: mc,
                    character: ch as u16,
                    unmodified_character: ch.to_ascii_lowercase() as u16,
                    ..Default::default()
                };
                host.send_key_event(Some(&up));
            }
        }
        return;
    }
    let Some(name) = named else { return };
    let Some((vk, mc, ch)) = named_codes(name) else { return };
    let ty = if down {
        sys::cef_key_event_type_t::KEYEVENT_RAWKEYDOWN
    } else {
        sys::cef_key_event_type_t::KEYEVENT_KEYUP
    };
    let ev = KeyEvent {
        type_: KeyEventType::from(ty),
        modifiers,
        windows_key_code: vk,
        native_key_code: mc,
        character: ch.unwrap_or(0),
        unmodified_character: ch.unwrap_or(0),
        ..Default::default()
    };
    host.send_key_event(Some(&ev));
    if down && let Some(c) = ch {
        let chr = KeyEvent {
            type_: KeyEventType::from(sys::cef_key_event_type_t::KEYEVENT_CHAR),
            modifiers,
            windows_key_code: vk,
            native_key_code: mc,
            character: c,
            unmodified_character: c,
            ..Default::default()
        };
        host.send_key_event(Some(&chr));
    }
}

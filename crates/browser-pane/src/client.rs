//! Sidecar connection, multi-tab state, and one-line Rune `NativeSurface` element builder.

use std::collections::{HashMap, VecDeque};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use rune::native_surface::{ByteFrame, ColorInfo, Format};
use rune::{
    Drag, El, Ime, Key, KeyEvent, NativeSurface, Phase, PointerButton, PointerEvent, SurfaceFit,
    Waker, WheelEvent, el,
};
use serde_json::Value;

use crate::address::resolve_address;
use crate::installer::{self, EngineStatus};
#[cfg(target_os = "macos")]
use crate::mach_pipe::MachReceiver;
use crate::protocol::{
    CursorKind, EngineMsg, HostMsg, PickedElement, PointerBtn, TabSnapshot, pack_mods,
};
use crate::transport::{is_shell_shortcut, named_key_str, spawn_and_attach_bg, wire_stream};

static NEXT_RECEIVER_TAG: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub struct BrowserTab {
    pub id: u64,
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub zoom: f64,
    pub cursor: CursorKind,
    pub inspecting: bool,
    pub surface: NativeSurface,
    viewport_pt: (f32, f32),
    scale: f32,
    sent_resize: Option<(f32, f32, f32)>,
}

pub struct BrowserHost {
    tabs: Vec<BrowserTab>,
    focused_tab: Option<u64>,
    surfaces: Arc<Mutex<HashMap<u64, NativeSurface>>>,
    status: Arc<Mutex<EngineStatus>>,
    outbox: Arc<Mutex<Option<Sender<HostMsg>>>>,
    inbox_tx: Sender<EngineMsg>,
    inbox_rx: Receiver<EngineMsg>,
    waker: Option<Waker>,
    shell_shortcut: Option<fn(&KeyEvent) -> bool>,
    socket_override: Option<PathBuf>,
    next_tab_id: u64,
    next_req_id: u64,
    pending_opens: Vec<(u64, String)>,
    cdp_responses: HashMap<u64, Result<Value, Value>>,
    picked: VecDeque<PickedElement>,
    #[cfg(target_os = "macos")]
    mach: Option<MachReceiver>,
}

impl Default for BrowserHost {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserHost {
    pub fn new() -> Self {
        let (inbox_tx, inbox_rx) = mpsc::channel();
        let initial_status = if installer::resolve_installed_executable().is_some() {
            EngineStatus::Ready
        } else {
            EngineStatus::NotInstalled
        };
        Self {
            tabs: Vec::new(),
            focused_tab: None,
            surfaces: Arc::new(Mutex::new(HashMap::new())),
            status: Arc::new(Mutex::new(initial_status)),
            outbox: Arc::new(Mutex::new(None)),
            inbox_tx,
            inbox_rx,
            waker: None,
            shell_shortcut: None,
            socket_override: None,
            next_tab_id: 1,
            next_req_id: 1,
            pending_opens: Vec::new(),
            cdp_responses: HashMap::new(),
            picked: VecDeque::new(),
            #[cfg(target_os = "macos")]
            mach: None,
        }
    }

    pub fn set_shell_shortcut(&mut self, check: fn(&KeyEvent) -> bool) {
        self.shell_shortcut = Some(check);
    }

    pub fn with_socket(socket: PathBuf) -> Self {
        let mut h = Self::new();
        h.socket_override = Some(socket);
        h
    }

    pub fn set_waker(&mut self, waker: Waker) {
        for tab in &self.tabs {
            tab.surface.set_waker(waker.clone());
        }
        self.waker = Some(waker);
    }

    pub fn status(&self) -> EngineStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn tabs(&self) -> &[BrowserTab] {
        &self.tabs
    }

    pub fn focused_tab(&self) -> Option<&BrowserTab> {
        let id = self.focused_tab?;
        self.tabs.iter().find(|t| t.id == id)
    }

    pub fn focused_tab_id(&self) -> Option<u64> {
        self.focused_tab
    }

    pub fn tab(&self, id: u64) -> Option<&BrowserTab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    /// Creates a synthetic preview tab without spawning CEF, used by deterministic Storybook stages
    /// and headless UI tests.
    pub fn insert_mock_tab(&mut self, url: &str, title: &str, status: EngineStatus) -> u64 {
        let id = self.alloc_tab_id();
        let surface = NativeSurface::new();
        let _ = surface.push_bytes(rune::native_surface::synthetic(640, 360, 0), 1);
        if let Some(w) = &self.waker {
            surface.set_waker(w.clone());
        }
        self.surfaces.lock().unwrap().insert(id, surface.clone());
        self.tabs.push(BrowserTab {
            id,
            url: url.to_owned(),
            title: title.to_owned(),
            loading: false,
            can_go_back: true,
            can_go_forward: false,
            zoom: 1.0,
            cursor: CursorKind::Default,
            inspecting: false,
            surface,
            viewport_pt: (800.0, 600.0),
            scale: 2.0,
            sent_resize: None,
        });
        self.focused_tab = Some(id);
        *self.status.lock().unwrap() = status;
        id
    }

    pub fn open_tab(&mut self, raw_url: &str) -> u64 {
        let url = resolve_address(raw_url);
        let id = self.alloc_tab_id();
        let surface = NativeSurface::new();
        if let Some(w) = &self.waker {
            surface.set_waker(w.clone());
        }
        self.surfaces.lock().unwrap().insert(id, surface.clone());
        self.tabs.push(BrowserTab {
            id,
            url: url.clone(),
            title: "New tab".into(),
            loading: true,
            can_go_back: false,
            can_go_forward: false,
            zoom: 1.0,
            cursor: CursorKind::Default,
            inspecting: false,
            surface,
            viewport_pt: (800.0, 600.0),
            scale: 2.0,
            sent_resize: None,
        });
        self.focused_tab = Some(id);

        if self.is_connected() {
            self.send(HostMsg::OpenTab {
                tab_id: id,
                url,
                width_pt: 800.0,
                height_pt: 600.0,
                scale: 2.0,
                focused: true,
            });
        } else {
            self.pending_opens.push((id, url));
            self.ensure_connected();
        }
        id
    }

    pub fn focus_tab(&mut self, id: u64) {
        if self.tabs.iter().any(|t| t.id == id) {
            self.focused_tab = Some(id);
            self.send(HostMsg::FocusTab { tab_id: id });
        }
    }

    pub fn close_tab(&mut self, id: u64) {
        if let Some(pos) = self.tabs.iter().position(|t| t.id == id) {
            let removed = self.tabs.remove(pos);
            removed.surface.clear();
            self.surfaces.lock().unwrap().remove(&id);
            self.pending_opens.retain(|(tid, _)| *tid != id);
            self.send(HostMsg::CloseTab { tab_id: id });
            if self.focused_tab == Some(id) {
                self.focused_tab = if self.tabs.is_empty() {
                    None
                } else {
                    Some(self.tabs[pos.min(self.tabs.len() - 1)].id)
                };
                if let Some(next_id) = self.focused_tab {
                    self.send(HostMsg::FocusTab { tab_id: next_id });
                }
            }
            if self.tabs.is_empty() {
                self.shutdown_sidecar();
            }
        }
    }

    pub fn navigate(&mut self, tab_id: u64, raw_url: &str) {
        let url = resolve_address(raw_url);
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            t.url = url.clone();
            t.loading = true;
        }
        self.send(HostMsg::Navigate { tab_id, url });
    }

    pub fn back(&self, tab_id: u64) {
        self.send(HostMsg::Back { tab_id });
    }

    pub fn forward(&self, tab_id: u64) {
        self.send(HostMsg::Forward { tab_id });
    }

    pub fn reload(&self, tab_id: u64) {
        self.send(HostMsg::Reload { tab_id });
    }

    pub fn stop(&self, tab_id: u64) {
        self.send(HostMsg::Stop { tab_id });
    }

    pub fn step_zoom(&mut self, tab_id: u64, dir: i32) {
        let next = if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            t.zoom = if dir == 0 {
                1.0
            } else {
                ((t.zoom * 100.0).round() as i32 + dir.signum() * 10).clamp(25, 500) as f64 / 100.0
            };
            t.zoom
        } else {
            return;
        };
        self.send(HostMsg::SetZoom { tab_id, factor: next });
    }

    pub fn toggle_inspect(&mut self, tab_id: u64) {
        let next = if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            t.inspecting = !t.inspecting;
            t.inspecting
        } else {
            return;
        };
        self.send(HostMsg::SetInspect { tab_id, enabled: next });
    }

    pub fn highlight_node(&self, tab_id: u64, backend_node_id: Option<i64>) {
        self.send(HostMsg::HighlightNode { tab_id, backend_node_id });
    }

    pub fn send_cdp(
        &mut self,
        tab_id: u64,
        method: impl Into<String>,
        params: Value,
        session_id: Option<String>,
    ) -> u64 {
        let request_id = self.next_req_id;
        self.next_req_id += 1;
        self.send(HostMsg::CdpCall {
            tab_id,
            request_id,
            method: method.into(),
            params,
            session_id,
        });
        request_id
    }

    pub fn take_cdp_response(&mut self, request_id: u64) -> Option<Result<Value, Value>> {
        self.cdp_responses.remove(&request_id)
    }

    pub fn drain_picked_elements(&mut self) -> impl Iterator<Item = PickedElement> + '_ {
        self.picked.drain(..)
    }

    pub fn retry_install(&mut self) {
        *self.status.lock().unwrap() = EngineStatus::NotInstalled;
        self.ensure_connected();
    }

    /// Processes incoming messages from the sidecar; returns `true` only when visible UI metadata changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(msg) = self.inbox_rx.try_recv() {
            match msg {
                EngineMsg::Ready { .. } => {
                    *self.status.lock().unwrap() = EngineStatus::Ready;
                    for t in &mut self.tabs {
                        t.sent_resize = None;
                    }
                    for (tab_id, url) in std::mem::take(&mut self.pending_opens) {
                        let ((width_pt, height_pt), scale) = self
                            .tabs
                            .iter_mut()
                            .find(|t| t.id == tab_id)
                            .map_or(((800.0, 600.0), 2.0), |t| {
                                t.sent_resize = Some((t.viewport_pt.0, t.viewport_pt.1, t.scale));
                                (t.viewport_pt, t.scale)
                            });
                        self.send(HostMsg::OpenTab {
                            tab_id,
                            url,
                            width_pt,
                            height_pt,
                            scale,
                            focused: self.focused_tab == Some(tab_id),
                        });
                    }
                    changed = true;
                }
                EngineMsg::TabList { tabs } => {
                    for snap in tabs {
                        self.upsert_tab(snap);
                    }
                    if self.focused_tab.is_none() {
                        self.focused_tab = self.tabs.last().map(|t| t.id);
                    }
                    changed = true;
                }
                EngineMsg::TabUpdate { tab } => {
                    changed |= self.upsert_tab(tab);
                }
                EngineMsg::TabClosed { tab_id } => {
                    if let Some(pos) = self.tabs.iter().position(|t| t.id == tab_id) {
                        let removed = self.tabs.remove(pos);
                        removed.surface.clear();
                        self.surfaces.lock().unwrap().remove(&tab_id);
                        if self.focused_tab == Some(tab_id) {
                            self.focused_tab = self.tabs.last().map(|t| t.id);
                        }
                        changed = true;
                    }
                }
                EngineMsg::InspectState { tab_id, active, picked } => {
                    if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        t.inspecting = active;
                    }
                    if let Some(el) = picked {
                        self.picked.push_back(el);
                    }
                    changed = true;
                }
                EngineMsg::CdpResponse { request_id, ok, result, .. } => {
                    self.cdp_responses
                        .insert(request_id, if ok { Ok(result) } else { Err(result) });
                    changed = true;
                }
                EngineMsg::CdpEvent { .. } => {}
                EngineMsg::CpuFrame { tab_id, frame_id, width, height, bgra_b64 } => {
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(bgra_b64) {
                        if let Some(surface) = self.surfaces.lock().unwrap().get(&tab_id).cloned() {
                            let _ = surface.push_bytes(
                                ByteFrame {
                                    width,
                                    height,
                                    format: Format::Bgra,
                                    color: ColorInfo::default(),
                                    planes: [bytes, Vec::new()],
                                },
                                frame_id,
                            );
                        }
                    }
                }
            }
        }
        changed
    }

    pub fn ime(&self, ime: &Ime) -> bool {
        let Some(tab_id) = self.focused_tab else {
            return false;
        };
        match ime {
            Ime::Commit(s) => {
                self.send(HostMsg::Ime { tab_id, commit: Some(s.clone()), preedit: None });
                true
            }
            Ime::Preedit(s, _) => {
                self.send(HostMsg::Ime { tab_id, commit: None, preedit: Some(s.clone()) });
                true
            }
            _ => false,
        }
    }

    /// Builds a focusable Rune `El` displaying `tab_id`'s zero-copy `NativeSurface` and forwarding
    /// layout resizes, pointer clicks/drags/wheel, cursor shapes, and non-shell keys.
    pub fn surface_el<S: 'static>(
        &self,
        key: impl Into<String>,
        tab_id: u64,
        scale: f32,
        access: fn(&mut S) -> &mut BrowserHost,
    ) -> El {
        let Some(tab) = self.tab(tab_id) else {
            return el().key(key.into());
        };
        el().key(key.into())
            .grow(1.0)
            .min_w(0.0)
            .min_h(0.0)
            .w_full()
            .focusable(true)
            .cursor(tab.cursor.to_rune())
            .pointer_buttons([PointerButton::Middle, PointerButton::Back, PointerButton::Forward])
            .native_surface(&tab.surface, SurfaceFit::Fill)
            .on_layout(move |s: &mut S, [_x, _y, w, h]| {
                let width_pt = w.max(1.0);
                let height_pt = h.max(1.0);
                let sc = scale.max(1.0);
                let host = access(s);
                let connected = host.is_connected();
                let mut should_send = false;
                if let Some(t) = host.tabs.iter_mut().find(|t| t.id == tab_id) {
                    t.viewport_pt = (width_pt, height_pt);
                    t.scale = sc;
                    let changed = t.sent_resize.is_none_or(|(pw, ph, ps)| {
                        (pw - width_pt).abs() > 0.5
                            || (ph - height_pt).abs() > 0.5
                            || (ps - sc).abs() > 0.01
                    });
                    if connected && changed {
                        t.sent_resize = Some((width_pt, height_pt, sc));
                        should_send = true;
                    }
                }
                if should_send {
                    host.send(HostMsg::Resize { tab_id, width_pt, height_pt, scale: sc });
                }
            })
            .on_pointer_move_local(move |s: &mut S, p: PointerEvent, tx| {
                access(s).send(HostMsg::MouseMove {
                    tab_id,
                    x: p.x,
                    y: p.y,
                    mods: pack_mods(tx.mods()),
                    leave: false,
                });
                tx.live_only();
            })
            .on_hover_change(move |s: &mut S, inside: bool, tx| {
                if !inside {
                    access(s).send(HostMsg::MouseMove {
                        tab_id,
                        x: -1.0,
                        y: -1.0,
                        mods: pack_mods(tx.mods()),
                        leave: true,
                    });
                }
            })
            .on_drag(move |s: &mut S, d: &mut Drag, tx| {
                let mods = pack_mods(tx.mods());
                let btn = PointerBtn::from(tx.button());
                let host = access(s);
                match d.phase {
                    Phase::Down => host.send(HostMsg::MouseButton {
                        tab_id,
                        x: d.x,
                        y: d.y,
                        button: btn,
                        up: false,
                        clicks: d.clicks.clamp(1, 3),
                        mods,
                    }),
                    Phase::Move => host.send(HostMsg::MouseMove {
                        tab_id,
                        x: d.x,
                        y: d.y,
                        mods,
                        leave: false,
                    }),
                    Phase::Up => host.send(HostMsg::MouseButton {
                        tab_id,
                        x: d.x,
                        y: d.y,
                        button: btn,
                        up: true,
                        clicks: d.clicks.clamp(1, 3),
                        mods,
                    }),
                }
            })
            .on_secondary_press_local(move |s: &mut S, p: PointerEvent, tx| {
                let mods = pack_mods(tx.mods());
                let host = access(s);
                for up in [false, true] {
                    host.send(HostMsg::MouseButton {
                        tab_id,
                        x: p.x,
                        y: p.y,
                        button: PointerBtn::Secondary,
                        up,
                        clicks: 1,
                        mods,
                    });
                }
            })
            .on_wheel(move |s: &mut S, w: WheelEvent, _tx| {
                access(s).send(HostMsg::Wheel {
                    tab_id,
                    x: w.x,
                    y: w.y,
                    dx: w.dx,
                    dy: w.dy,
                    mods: pack_mods(w.mods),
                });
                true
            })
            .on_key(move |s: &mut S, ev: &KeyEvent, _tx| access(s).forward_key(tab_id, ev, true))
            .on_key_up(move |s: &mut S, ev: &KeyEvent, _tx| access(s).forward_key(tab_id, ev, false))
    }

    pub fn shutdown_sidecar(&mut self) {
        self.send(HostMsg::Shutdown);
        *self.outbox.lock().unwrap() = None;
    }

    fn forward_key(&self, tab_id: u64, ev: &KeyEvent, down: bool) -> bool {
        if self.shell_shortcut.map_or_else(|| is_shell_shortcut(ev), |f| f(ev)) {
            return false;
        }
        if down && let Key::Command(c) = ev.key {
            let cmd = match c {
                'a' => Some("select_all"),
                'c' => Some("copy"),
                'v' => Some("paste"),
                'x' => Some("cut"),
                'z' if ev.mods.shift => Some("redo"),
                'z' => Some("undo"),
                'r' => {
                    self.reload(tab_id);
                    return true;
                }
                _ => None,
            };
            if let Some(cmd) = cmd {
                self.send(HostMsg::EditCmd { tab_id, cmd: cmd.into() });
                return true;
            }
        }
        let (text, named) = match &ev.key {
            Key::Text(s) => (Some(s.clone()), None),
            other => (None, named_key_str(other).map(str::to_owned)),
        };
        if text.is_none() && named.is_none() {
            return false;
        }
        self.send(HostMsg::Key {
            tab_id,
            down,
            text,
            named,
            mods: pack_mods(ev.mods),
        });
        true
    }

    fn alloc_tab_id(&mut self) -> u64 {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        id
    }

    fn is_connected(&self) -> bool {
        self.outbox.lock().unwrap().is_some()
    }

    fn send(&self, msg: HostMsg) {
        if let Some(tx) = self.outbox.lock().unwrap().as_ref() {
            let _ = tx.send(msg);
        }
    }

    fn upsert_tab(&mut self, snap: TabSnapshot) -> bool {
        self.next_tab_id = self.next_tab_id.max(snap.tab_id + 1);
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == snap.tab_id) {
            let changed = t.url != snap.url
                || t.title != snap.title
                || t.loading != snap.loading
                || t.can_go_back != snap.can_go_back
                || t.can_go_forward != snap.can_go_forward
                || (t.zoom - snap.zoom).abs() > 0.001
                || t.cursor != snap.cursor;
            t.url = snap.url;
            t.title = snap.title;
            t.loading = snap.loading;
            t.can_go_back = snap.can_go_back;
            t.can_go_forward = snap.can_go_forward;
            t.zoom = snap.zoom;
            t.cursor = snap.cursor;
            return changed;
        }
        let surface = NativeSurface::new();
        if let Some(w) = &self.waker {
            surface.set_waker(w.clone());
        }
        self.surfaces.lock().unwrap().insert(snap.tab_id, surface.clone());
        self.tabs.push(BrowserTab {
            id: snap.tab_id,
            url: snap.url,
            title: snap.title,
            loading: snap.loading,
            can_go_back: snap.can_go_back,
            can_go_forward: snap.can_go_forward,
            zoom: snap.zoom,
            cursor: snap.cursor,
            inspecting: false,
            surface,
            viewport_pt: (800.0, 600.0),
            scale: 2.0,
            sent_resize: None,
        });
        true
    }

    fn ensure_connected(&mut self) {
        if self.is_connected() {
            return;
        }
        let cur_status = self.status();
        if matches!(cur_status, EngineStatus::Downloading { .. } | EngineStatus::Connecting) {
            return;
        }

        #[cfg(target_os = "macos")]
        if self.mach.is_none() {
            let tag = NEXT_RECEIVER_TAG.fetch_add(1, Ordering::Relaxed);
            self.mach = MachReceiver::start(self.surfaces.clone(), tag).ok();
        }
        #[cfg(target_os = "macos")]
        let mach_service = self.mach.as_ref().map(|m| m.service_name.clone());
        #[cfg(not(target_os = "macos"))]
        let mach_service: Option<String> = None;

        let sock = self
            .socket_override
            .clone()
            .unwrap_or_else(installer::socket_path);

        // 1. Reconnect to an already-running sidecar first (survives UI hot-reloads).
        if let Ok(stream) = UnixStream::connect(&sock) {
            self.attach_stream(stream, mach_service);
            return;
        }

        // 2. Spawn installed sidecar binary if present on disk.
        if let Some(bin) = installer::resolve_installed_executable() {
            self.spawn_and_connect(bin, sock, mach_service);
            return;
        }

        // 3. Otherwise kick off on-demand background download and connect when ready.
        let status = self.status.clone();
        let waker = self.waker.clone();
        let outbox = self.outbox.clone();
        let inbox_tx = self.inbox_tx.clone();
        installer::start_download(
            status.clone(),
            waker.clone(),
            Box::new(move |bin| {
                spawn_and_attach_bg(bin, sock, mach_service, status, outbox, inbox_tx, waker);
            }),
        );
    }

    fn spawn_and_connect(&mut self, bin: PathBuf, sock: PathBuf, mach_service: Option<String>) {
        *self.status.lock().unwrap() = EngineStatus::Connecting;
        let status = self.status.clone();
        let outbox = self.outbox.clone();
        let inbox_tx = self.inbox_tx.clone();
        let waker = self.waker.clone();
        std::thread::Builder::new()
            .name("opencode-browser-connect".into())
            .spawn(move || {
                spawn_and_attach_bg(bin, sock, mach_service, status, outbox, inbox_tx, waker);
            })
            .ok();
    }

    fn attach_stream(&mut self, stream: UnixStream, mach_service: Option<String>) {
        wire_stream(
            stream,
            mach_service,
            &self.status,
            &self.outbox,
            self.inbox_tx.clone(),
            self.waker.clone(),
        );
    }
}


//! Sidecar connection, multi-tab state, and one-line Rune `NativeSurface` element builder.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use rune::native_surface::{ByteFrame, ColorInfo, Format};
use rune::{Ime, Key, KeyEvent, NativeSurface, Waker};
use serde_json::Value;

use crate::address::resolve_address;
use crate::installer::EngineStatus;
use crate::keys::{is_shell_shortcut, named_key_str};
#[cfg(all(feature = "engine", target_os = "macos"))]
use crate::mach_pipe::MachReceiver;
use crate::site_zoom::{SiteZoom, site_of};
use crate::protocol::{
    CursorKind, EngineMsg, HostMsg, PickedElement, TabSnapshot, pack_mods,
};

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
    /// Why the page shows no document (`TabSnapshot::load_error`).
    pub load_error: Option<String>,
    pub inspecting: bool,
    pub surface: NativeSurface,
    pub(crate) viewport_pt: (f32, f32),
    pub(crate) scale: f32,
    pub(crate) sent_resize: Option<(f32, f32, f32)>,
}

impl BrowserTab {
    /// The pixel size the engine renders this tab's frames at: its viewport in whole points (the
    /// engine's view rect) times its scale.
    pub fn frame_px(&self) -> (u32, u32) {
        let px = |pt: f32| (pt.round().max(1.0) * self.scale).ceil() as u32;
        (px(self.viewport_pt.0), px(self.viewport_pt.1))
    }
}

pub struct BrowserHost {
    pub(crate) tabs: Vec<BrowserTab>,
    focused_tab: Option<u64>,
    pub(crate) surfaces: Arc<Mutex<HashMap<u64, NativeSurface>>>,
    pub(crate) status: Arc<Mutex<EngineStatus>>,
    last_polled_status: EngineStatus,
    pub(crate) outbox: Arc<Mutex<Option<Sender<HostMsg>>>>,
    #[cfg_attr(not(feature = "engine"), allow(dead_code))]
    pub(crate) inbox_tx: Sender<EngineMsg>,
    inbox_rx: Receiver<EngineMsg>,
    pub(crate) waker: Option<Waker>,
    shell_shortcut: Option<fn(&KeyEvent) -> bool>,
    pub(crate) socket_override: Option<PathBuf>,
    #[cfg_attr(not(feature = "engine"), allow(dead_code))]
    pub(crate) profile_dir: Option<PathBuf>,
    next_tab_id: u64,
    next_req_id: u64,
    pending_opens: Vec<(u64, String)>,
    cdp_responses: HashMap<u64, Result<Value, Value>>,
    picked: VecDeque<PickedElement>,
    site_zoom: SiteZoom,
    keep_engine: bool,
    /// The scheme pages draw in: 0 not given yet, 1 light, 2 dark.
    color_scheme: std::sync::atomic::AtomicU8,
    /// The last laid-out page size, so a new tab opens at it instead of 800 × 600 and a resize.
    pub(crate) last_viewport: ((f32, f32), f32),
    #[cfg(all(feature = "engine", target_os = "macos"))]
    pub(crate) mach: Option<MachReceiver>,
    /// The engine download job, background or asked for.
    #[cfg(feature = "engine")]
    pub(crate) prefetch: Arc<crate::prefetch::Prefetch>,
}

impl Default for BrowserHost {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserHost {
    pub fn new() -> Self {
        let (inbox_tx, inbox_rx) = mpsc::channel();
        let initial_status = Self::initial_status();
        Self {
            tabs: Vec::new(),
            focused_tab: None,
            surfaces: Arc::new(Mutex::new(HashMap::new())),
            last_polled_status: initial_status.clone(),
            status: Arc::new(Mutex::new(initial_status)),
            outbox: Arc::new(Mutex::new(None)),
            inbox_tx,
            inbox_rx,
            waker: None,
            shell_shortcut: None,
            socket_override: None,
            profile_dir: None,
            next_tab_id: 1,
            next_req_id: 1,
            pending_opens: Vec::new(),
            cdp_responses: HashMap::new(),
            picked: VecDeque::new(),
            site_zoom: SiteZoom::default(),
            keep_engine: false,
            color_scheme: std::sync::atomic::AtomicU8::new(0),
            last_viewport: ((800.0, 600.0), 2.0),
            #[cfg(all(feature = "engine", target_os = "macos"))]
            mach: None,
            #[cfg(feature = "engine")]
            prefetch: Arc::default(),
        }
    }

    /// Remembers each site's page zoom in `store` (the app's, as JSON text) and opens its pages at it again.
    pub fn remember_site_zoom(&mut self, store: crate::ZoomStore) {
        self.site_zoom = SiteZoom::remember(store);
    }

    /// Pages draw in the app's scheme (light or dark); the engine hears of it once it's up and at
    /// each change. Cheap to call every frame.
    pub fn set_color_scheme(&self, dark: bool) {
        let want = if dark { 2 } else { 1 };
        if self.color_scheme.swap(want, std::sync::atomic::Ordering::Relaxed) != want {
            self.send(HostMsg::SetColorScheme { dark });
        }
    }

    /// Leaves the engine running when the last tab closes, so the next page skips its start and
    /// reuses its warm connections; the engine still quits on its own after a minute with no tabs.
    pub fn set_keep_engine(&mut self, keep: bool) {
        self.keep_engine = keep;
    }

    /// Keeps the engine's cookies and sign-ins in `dir` instead of this app's own profile
    /// (`OPENCODE_BROWSER_PROFILE` in the environment still wins). Takes effect at the next spawn.
    pub fn set_profile_dir(&mut self, dir: PathBuf) {
        self.profile_dir = Some(dir);
    }

    /// Marks a mock tab loading or done (stories and tests).
    pub fn set_mock_loading(&mut self, id: u64, loading: bool) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
            t.loading = loading;
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

    pub fn has_socket_override(&self) -> bool {
        self.socket_override.is_some()
    }

    pub fn has_live_sidecar(&self) -> bool {
        self.is_connected()
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
            load_error: None,
            inspecting: false,
            surface,
            viewport_pt: (800.0, 600.0),
            scale: 2.0,
            sent_resize: None,
        });
        self.focused_tab = Some(id);
        self.last_polled_status = status.clone();
        *self.status.lock().unwrap() = status;
        id
    }

    /// Shows `url` titled `title` in mock tab `id`, loaded at once, as an engine would report a
    /// finished navigation (stories, captures and tests have no engine).
    pub fn set_mock_page(&mut self, id: u64, url: &str, title: &str) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
            // The page it left is history, unless it was a blank one.
            t.can_go_back |= !t.url.is_empty() && t.url != "about:blank";
            t.url = url.to_owned();
            t.title = title.to_owned();
            t.loading = false;
        }
    }

    /// Sets whether mock tab `id` has an earlier and a later page.
    pub fn set_mock_history(&mut self, id: u64, back: bool, forward: bool) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
            t.can_go_back = back;
            t.can_go_forward = forward;
        }
    }

    /// Sends every message to the returned receiver instead of an engine (tests).
    pub fn capture_sent(&mut self) -> Receiver<HostMsg> {
        let (tx, rx) = mpsc::channel();
        *self.outbox.lock().unwrap() = Some(tx);
        rx
    }

    pub fn set_mock_status(&mut self, status: EngineStatus) {
        self.last_polled_status = status.clone();
        *self.status.lock().unwrap() = status;
    }

    pub fn open_tab(&mut self, raw_url: &str) -> u64 {
        rune::marks::mark("browser.tab.open");
        let url = resolve_address(raw_url);
        let id = self.alloc_tab_id();
        let surface = NativeSurface::new();
        if let Some(w) = &self.waker {
            surface.set_waker(w.clone());
        }
        self.surfaces.lock().unwrap().insert(id, surface.clone());
        self.site_zoom.prepare();
        let ((width_pt, height_pt), scale) = self.last_viewport;
        let connected = self.is_connected();
        self.tabs.push(BrowserTab {
            id,
            url: url.clone(),
            title: "New tab".into(),
            loading: true,
            can_go_back: false,
            can_go_forward: false,
            zoom: 1.0,
            cursor: CursorKind::Default,
            load_error: None,
            inspecting: false,
            surface,
            viewport_pt: (width_pt, height_pt),
            scale,
            sent_resize: connected.then_some((width_pt, height_pt, scale)),
        });
        self.focused_tab = Some(id);

        if connected {
            self.send(HostMsg::OpenTab { tab_id: id, url, width_pt, height_pt, scale, focused: true });
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
            if self.tabs.is_empty() && !self.keep_engine {
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
        if self.is_connected() {
            self.send(HostMsg::Navigate { tab_id, url });
        } else if let Some((_, pending_url)) = self.pending_opens.iter_mut().find(|(id, _)| *id == tab_id) {
            *pending_url = url;
        } else {
            self.pending_opens.push((tab_id, url));
        }
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
            self.site_zoom.set(&t.url, t.zoom);
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
        self.start_install();
    }

    /// Processes incoming messages from the sidecar; returns `true` only when visible UI metadata changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(msg) = self.inbox_rx.try_recv() {
            match msg {
                EngineMsg::Ready { .. } => {
                    rune::marks::mark("browser.engine.ready");
                    *self.status.lock().unwrap() = EngineStatus::Ready;
                    // Before any page loads, so the first draws in the app's scheme.
                    match self.color_scheme.load(std::sync::atomic::Ordering::Relaxed) {
                        0 => {}
                        s => self.send(HostMsg::SetColorScheme { dark: s == 2 }),
                    }
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
                // Windows pushes these to the tab's surface on the socket's reader thread.
                EngineMsg::SharedFrame { .. } => {}
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
        // A download finished while a tab waited for it (and one still running hurries).
        if !self.pending_opens.is_empty()
            && !self.is_connected()
            && matches!(
                self.status(),
                EngineStatus::Installed
                    | EngineStatus::Ready
                    | EngineStatus::Downloading { .. }
                    | EngineStatus::Verifying
                    | EngineStatus::Paused(_)
            )
        {
            self.ensure_connected();
        }
        {
            let guard = self.status.lock().unwrap();
            if *guard != self.last_polled_status {
                self.last_polled_status = guard.clone();
                changed = true;
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
    pub fn shutdown_sidecar(&mut self) {
        self.send(HostMsg::Shutdown);
        *self.outbox.lock().unwrap() = None;
    }

    pub(crate) fn forward_key(&self, tab_id: u64, ev: &KeyEvent, down: bool) -> bool {
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

    pub(crate) fn is_connected(&self) -> bool {
        self.outbox.lock().unwrap().is_some()
    }

    pub(crate) fn send(&self, msg: HostMsg) {
        if let Some(tx) = self.outbox.lock().unwrap().as_ref() {
            let _ = tx.send(msg);
        }
    }

    pub(crate) fn upsert_tab(&mut self, snap: TabSnapshot) -> bool {
        self.next_tab_id = self.next_tab_id.max(snap.tab_id + 1);
        let want_zoom = self.site_zoom.get(&snap.url);
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == snap.tab_id) {
            let new_site = site_of(&snap.url).is_some() && site_of(&t.url) != site_of(&snap.url);
            let changed = t.url != snap.url
                || t.title != snap.title
                || t.loading != snap.loading
                || t.can_go_back != snap.can_go_back
                || t.can_go_forward != snap.can_go_forward
                || (t.zoom - snap.zoom).abs() > 0.001
                || t.cursor != snap.cursor
                || t.load_error != snap.load_error;
            t.url = snap.url;
            t.title = snap.title;
            t.loading = snap.loading;
            t.can_go_back = snap.can_go_back;
            t.can_go_forward = snap.can_go_forward;
            t.zoom = snap.zoom;
            t.cursor = snap.cursor;
            t.load_error = snap.load_error;
            if new_site {
                let changed = changed || (t.zoom - want_zoom).abs() > 0.001;
                t.zoom = want_zoom;
                self.send(HostMsg::SetZoom { tab_id: snap.tab_id, factor: want_zoom });
                return changed;
            }
            return changed;
        }
        let surface = NativeSurface::new();
        if let Some(w) = &self.waker {
            surface.set_waker(w.clone());
        }
        self.surfaces.lock().unwrap().insert(snap.tab_id, surface.clone());
        let is_site = site_of(&snap.url).is_some();
        if is_site && (snap.zoom - want_zoom).abs() > 0.001 {
            self.send(HostMsg::SetZoom { tab_id: snap.tab_id, factor: want_zoom });
        }
        self.tabs.push(BrowserTab {
            id: snap.tab_id,
            url: snap.url,
            title: snap.title,
            loading: snap.loading,
            can_go_back: snap.can_go_back,
            can_go_forward: snap.can_go_forward,
            zoom: if is_site { want_zoom } else { snap.zoom },
            cursor: snap.cursor,
            load_error: snap.load_error,
            inspecting: false,
            surface,
            viewport_pt: (800.0, 600.0),
            scale: 2.0,
            sent_resize: None,
        });
        true
    }
}

#[cfg(not(feature = "engine"))]
impl BrowserHost {
    pub(crate) fn initial_status() -> EngineStatus {
        EngineStatus::Unsupported
    }

    pub fn status_checked(&self) -> EngineStatus {
        self.status()
    }

    /// No engine runs on this platform yet: tabs stay mock-only and the status stays as it is.
    pub fn start_install(&mut self) {}

    pub fn prefetch(&mut self) {}

    pub fn stop_prefetch(&mut self) {}

    pub(crate) fn ensure_connected(&mut self) {}
}

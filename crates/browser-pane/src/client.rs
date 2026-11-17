//! Sidecar connection and multi-tab state, including the Rune frame handle for each tab.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use rune::{Ime, Key, KeyEvent, Waker};
use serde_json::Value;

use crate::address::resolve_address;
use crate::frame::{self, TabSurface};
use crate::installer::EngineStatus;
use crate::keys::{is_shell_shortcut, named_key_str, page_zoom_dir};
#[cfg(all(feature = "engine", target_os = "macos"))]
use crate::mach_pipe::MachReceiver;
use crate::site_zoom::{SiteZoom, site_of};
use crate::protocol::{
    CursorKind, DownloadFile, EngineMsg, HostMsg, PickedElement, TabSnapshot, pack_mods,
};

#[derive(Clone, Debug)]
pub struct BrowserTab {
    pub device: Option<crate::device::Device>,
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
    /// Why the last navigation on this page was refused while keeping the document (`policy::Refusal`).
    pub refusal: Option<String>,
    /// Count of refused navigations on this page, so repeated refusals end the address field's hold.
    pub rejected: u64,
    pub inspecting: bool,
    /// What the page downloaded, oldest first (`DownloadFile`), for the agent's tools.
    pub files: Vec<DownloadFile>,
    pub surface: TabSurface,
    pub(crate) file_roots: Vec<String>,
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

/// The engine profile's downloads folder (the engine's `downloads::DIR_NAME`).
#[cfg(feature = "engine")]
const DOWNLOADS_DIR: &str = "opencode-page-downloads";

/// A tab's downloads at most (the engine's `downloads::MAX_TAB_FILES`).
pub const MAX_TAB_FILES: usize = 64;

pub struct BrowserHost {
    pub(crate) tabs: Vec<BrowserTab>,
    focused_tab: Option<u64>,
    pub(crate) surfaces: Arc<Mutex<HashMap<u64, TabSurface>>>,
    pub(crate) status: Arc<Mutex<EngineStatus>>,
    last_polled_status: EngineStatus,
    pub(crate) outbox: Arc<Mutex<Option<Sender<HostMsg>>>>,
    #[cfg_attr(not(feature = "engine"), allow(dead_code))]
    pub(crate) inbox_tx: Sender<EngineMsg>,
    inbox_rx: Receiver<EngineMsg>,
    /// Messages quiet read off the inbox, which poll handles first.
    held: std::cell::RefCell<std::collections::VecDeque<EngineMsg>>,
    pub(crate) waker: Option<Waker>,
    shell_shortcut: Option<fn(&KeyEvent) -> bool>,
    pub(crate) socket_override: Option<PathBuf>,
    #[cfg_attr(not(feature = "engine"), allow(dead_code))]
    pub(crate) profile_dir: Option<PathBuf>,
    /// The running engine keeps a page's downloads (its `Ready` said so).
    engine_downloads: bool,
    /// The running engine enforces URL and workspace `file://` policy in its request handlers.
    engine_policy: bool,
    next_tab_id: u64,
    next_req_id: u64,
    pending_opens: Vec<(u64, String)>,
    cdp_responses: HashMap<u64, Result<Value, Value>>,
    picked: VecDeque<PickedElement>,
    site_zoom: SiteZoom,
    pub(crate) keep_engine: bool,
    /// The scheme pages draw in: 0 not given yet, 1 light, 2 dark.
    color_scheme: std::sync::atomic::AtomicU8,
    /// The last laid-out page size, so a new tab opens at it instead of 800 × 600 and a resize.
    pub(crate) last_viewport: ((f32, f32), f32),
    #[cfg(all(feature = "engine", target_os = "macos"))]
    pub(crate) mach: Option<MachReceiver>,
    /// The engine download job, background or asked for.
    #[cfg(feature = "engine")]
    pub(crate) prefetch: Arc<crate::prefetch::Prefetch>,
    /// Whether this run looked for a newer engine yet (`check_for_update_once`).
    #[cfg(feature = "engine")]
    pub(crate) update_checked: bool,
    /// The engine this app started, while it runs (`transport::end_engine`).
    #[cfg(feature = "engine")]
    pub(crate) engine: Arc<Mutex<Option<u32>>>,
    /// The waiting tabs a connect sent itself (`flush_pending_opens` skips them).
    pub(crate) early: EarlyOpens,
    /// Tabs opened so far, and `prepare` calls: an engine started for a page that didn't come
    /// stops once its hold ends with neither changed.
    pub(crate) opened: Arc<std::sync::atomic::AtomicU64>,
    #[cfg(feature = "engine")]
    pub(crate) prepared: Arc<std::sync::atomic::AtomicU64>,
    /// Monotonic connection generation so a shutting-down engine's reader/watcher cannot clobber a newer connection.
    #[cfg_attr(not(feature = "engine"), allow(dead_code))]
    pub(crate) conn_gen: Arc<std::sync::atomic::AtomicU64>,
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
            held: Default::default(),
            waker: None,
            shell_shortcut: None,
            socket_override: None,
            profile_dir: None,
            engine_downloads: false,
            engine_policy: false,
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
            #[cfg(feature = "engine")]
            update_checked: false,
            #[cfg(feature = "engine")]
            engine: Arc::default(),
            early: Arc::default(),
            opened: Arc::default(),
            #[cfg(feature = "engine")]
            prepared: Arc::default(),
            conn_gen: Arc::default(),
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
    /// The engine that runs (or last ran) keeps a page's downloads, for the agent's tools.
    pub fn engine_downloads(&self) -> bool {
        self.engine_downloads
    }

    pub fn engine_policy(&self) -> bool {
        self.engine_policy
    }

    /// Where tab `tab_id`'s download `file` is, when it's where the engine keeps it: the engine
    /// profile's `opencode-page-downloads\<tab>\<file id>\` (or a throwaway profile's beside it).
    /// None for any other path the engine named: the app opens no file another process points at.
    pub fn download_path(&self, tab_id: u64, file: &DownloadFile) -> Option<PathBuf> {
        #[cfg(feature = "engine")]
        {
            let profile = std::fs::canonicalize(crate::profile::for_spawn(self.profile_dir.as_deref())?).ok()?;
            let path = std::fs::canonicalize(&file.path).ok()?;
            let folder = path.parent()?;
            let tab = folder.parent()?;
            let dir = tab.parent()?;
            let root = dir.parent()?;
            let named = folder.file_name()? == file.id.as_str() && tab.file_name()? == tab_id.to_string().as_str() && dir.file_name()? == DOWNLOADS_DIR;
            let throwaway = profile.file_name().map(|n| format!("{}-throwaway", n.to_string_lossy()));
            let ours = root == profile || (root.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()) == throwaway && root.parent()?.parent() == profile.parent());
            (named && ours && path.is_file()).then_some(path)
        }
        #[cfg(not(feature = "engine"))]
        {
            let _ = (tab_id, file);
            None
        }
    }

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

    /// Whether this build has any way to run an engine: a download source, a trust rule for an
    /// installed version, or an explicit engine/socket override in the environment.
    pub fn engine_configured() -> bool {
        #[cfg(feature = "engine")]
        {
            let set = |k| std::env::var_os(k).is_some_and(|v| !v.is_empty());
            crate::source::configured()
                || crate::source::has_trust()
                || set("OPENCODE_BROWSER_ENGINE_BIN")
                || set("OPENCODE_BROWSER_ENGINE_APP")
                || set("OPENCODE_BROWSER_SOCKET")
        }
        #[cfg(not(feature = "engine"))]
        {
            false
        }
    }

    pub fn with_socket(socket: PathBuf) -> Self {
        let mut h = Self::new();
        h.socket_override = Some(socket);
        #[cfg(feature = "engine")]
        {
            h.last_polled_status = EngineStatus::Unchecked;
            *h.status.lock().unwrap() = EngineStatus::Unchecked;
        }
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
            frame::wake_window(&tab.surface, &waker);
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
        let surface = frame::preview();
        if let Some(w) = &self.waker {
            frame::wake_window(&surface, w);
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
            refusal: None,
            rejected: 0,
            inspecting: false,
            files: Vec::new(),
            surface,
            file_roots: Vec::new(),
            viewport_pt: (800.0, 600.0),
            device: None,
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
            t.refusal = None;
        }
    }

    pub fn set_mock_refusal(&mut self, id: u64, reason: &str) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
            t.loading = false;
            t.refusal = Some(reason.to_owned());
            t.rejected += 1;
        }
    }

    pub fn clear_refusal(&mut self, id: u64) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
            t.refusal = None;
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
        self.open_tab_with_roots(raw_url, Vec::new(), true)
    }

    pub fn open_tab_with_roots(&mut self, raw_url: &str, file_roots: Vec<String>, from_address: bool) -> u64 {
        rune::marks::mark("browser.tab.open");
        self.opened.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let candidate = if from_address { resolve_address(raw_url) } else { crate::policy::with_scheme(raw_url) };
        let connected = self.is_connected();
        // An older engine without request-level `file://` confinement cannot safely host a `file://` origin.
        let effective_roots: &[String] = if connected && !self.engine_policy { &[] } else { &file_roots };
        let refused = crate::policy::refusal(&candidate, effective_roots);
        let url = if refused.is_some() { "about:blank".to_owned() } else { candidate };
        let id = self.alloc_tab_id();
        let surface = TabSurface::new();
        if let Some(w) = &self.waker {
            frame::wake_window(&surface, w);
        }
        self.surfaces.lock().unwrap().insert(id, surface.clone());
        self.site_zoom.prepare();
        let ((width_pt, height_pt), scale) = self.last_viewport;
        self.tabs.push(BrowserTab {
            id,
            url: url.clone(),
            title: "New tab".into(),
            loading: refused.is_none(),
            can_go_back: false,
            can_go_forward: false,
            zoom: 1.0,
            cursor: CursorKind::Default,
            load_error: None,
            refusal: refused.map(|r| r.code().to_owned()),
            rejected: u64::from(refused.is_some()),
            inspecting: false,
            files: Vec::new(),
            surface,
            file_roots: file_roots.clone(),
            viewport_pt: (width_pt, height_pt),
            device: None,
            scale,
            sent_resize: connected.then_some((width_pt, height_pt, scale)),
        });
        self.focused_tab = Some(id);

        if connected {
            self.send(HostMsg::OpenTab {
                tab_id: id,
                url,
                width_pt,
                height_pt,
                scale,
                focused: true,
                file_roots: if self.engine_policy { file_roots } else { Vec::new() },
            });
        } else {
            self.pending_opens.push((id, url));
            self.ensure_connected();
        }
        id
    }

    pub fn set_tab_policy(&mut self, tab_id: u64, file_roots: Vec<String>) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id)
            && t.file_roots != file_roots
        {
            t.file_roots = file_roots.clone();
            if self.is_connected() && self.engine_policy {
                self.send(HostMsg::SetTabPolicy { tab_id, file_roots });
            }
        }
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
        self.navigate_with_policy(tab_id, raw_url, true);
    }

    pub fn navigate_with_policy(&mut self, tab_id: u64, raw_url: &str, from_address: bool) {
        rune::marks::mark("browser.page.navigate");
        #[cfg(feature = "engine")]
        crate::transport::paint_event(crate::transport::PaintEvent::Navigate);
        let url = if from_address { resolve_address(raw_url) } else { crate::policy::with_scheme(raw_url) };
        let connected = self.is_connected();
        let engine_policy = self.engine_policy;
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) else {
            return;
        };
        let effective_roots: &[String] = if connected && !engine_policy { &[] } else { &t.file_roots };
        if let Some(why) = crate::policy::refusal(&url, effective_roots) {
            t.loading = false;
            t.refusal = Some(why.code().to_owned());
            t.rejected += 1;
            return;
        }
        t.refusal = None;
        // The page shows where it is until the engine says it went: the address field holds what
        // was submitted meanwhile, as Desktop's does (an address set here at once ended that hold,
        // and the engine's next report, still the old document, emptied the field).
        t.loading = true;
        if connected {
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
        // Chromium's 17 zoom presets (`packages/gui-plugins/src/browser/chromium.ts:118, 144-150`).
        const STEPS: [f64; 17] = [
            0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0,
        ];
        let next = if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            let cur = t.zoom;
            let z = if dir == 0 {
                1.0
            } else if dir > 0 {
                STEPS.into_iter().find(|&s| s > cur + 0.001).unwrap_or(cur)
            } else {
                STEPS.into_iter().rev().find(|&s| s < cur - 0.001).unwrap_or(cur)
            };
            if (z - cur).abs() < 0.0005 {
                return;
            }
            t.zoom = z;
            self.site_zoom.set(&t.url, t.zoom);
            t.zoom
        } else {
            return;
        };
        self.send(HostMsg::SetZoom { tab_id, factor: next });
        if let Some(w) = &self.waker {
            w();
        }
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
    ) -> Result<u64, String> {
        let method = method.into();
        crate::policy::check_cdp(&method, &params, self.engine_policy)?;
        let request_id = self.next_req_id;
        self.next_req_id += 1;
        self.send(HostMsg::CdpCall {
            tab_id,
            request_id,
            method,
            params,
            session_id,
        });
        Ok(request_id)
    }

    /// Counts cookies for tab `tab_id`'s HTTP(S) destination without exposing cookie values.
    pub fn site_cookies(&mut self, tab_id: u64) -> Result<u64, String> {
        let url = self
            .tabs
            .iter()
            .find(|t| t.id == tab_id)
            .map(|t| t.url.clone())
            .ok_or_else(|| format!("No browser tab with tabID {tab_id}"))?;
        let request_id = self.next_req_id;
        self.next_req_id += 1;
        if crate::policy::destination_origin(&url).is_none() {
            self.cdp_responses.insert(request_id, Ok(serde_json::json!({ "cookies": 0 })));
            return Ok(request_id);
        }
        if self.engine_policy {
            self.send(HostMsg::SiteCookies { tab_id, request_id });
        } else {
            // Older engine fallback: request in Rust; `native/browser.rs` returns only the integer count.
            self.send(HostMsg::CdpCall {
                tab_id,
                request_id,
                method: "Network.getCookies".into(),
                params: serde_json::json!({ "urls": [url] }),
                session_id: None,
            });
        }
        Ok(request_id)
    }

    /// Deletes cookies and origin storage for tab `tab_id`'s HTTP(S) destination and reloads.
    pub fn clear_site(&mut self, tab_id: u64) -> Result<u64, String> {
        if !self.tabs.iter().any(|t| t.id == tab_id) {
            return Err(format!("No browser tab with tabID {tab_id}"));
        }
        if !self.engine_policy {
            return Err("Clearing site data requires an updated browser engine.".into());
        }
        let request_id = self.next_req_id;
        self.next_req_id += 1;
        self.send(HostMsg::ClearSite { tab_id, request_id });
        Ok(request_id)
    }

    pub fn take_cdp_response(&mut self, request_id: u64) -> Option<Result<Value, Value>> {
        self.cdp_responses.remove(&request_id)
    }

    /// Whether the page picked an element since the last drain.
    pub fn has_picked(&self) -> bool {
        !self.picked.is_empty()
    }

    pub fn drain_picked_elements(&mut self) -> impl Iterator<Item = PickedElement> + '_ {
        self.picked.drain(..)
    }

    pub fn retry_install(&mut self) {
        self.start_install();
    }

    /// The engine went away with pages open: they open again in the next one, where they were.
    #[cfg_attr(not(feature = "engine"), allow(dead_code))]
    pub(crate) fn requeue_tabs(&mut self) {
        for t in &self.tabs {
            if !self.pending_opens.iter().any(|(id, _)| *id == t.id) {
                self.pending_opens.push((t.id, t.url.clone()));
            }
        }
    }

    /// Sends the tabs that waited for the engine, as soon as it's connected: the engine opens
    /// them right after it says it's ready, without a turn of the app in between.
    fn flush_pending_opens(&mut self) {
        if !self.is_connected() {
            return;
        }
        let ready = *self.status.lock().unwrap() == EngineStatus::Ready;
        let engine_policy = self.engine_policy;
        // Those the connect sent already (`transport::Link::opens`): one closed meanwhile closes;
        // one given another address or laid out at another size meanwhile gets them.
        let early = std::mem::take(&mut *self.early.lock().unwrap());
        for (&tab_id, (sent_url, sent_size)) in &early {
            let pending_url = self.pending_opens.iter().find(|(id, _)| *id == tab_id).map(|(_, u)| u.clone());
            let (size, roots_to_send, nav_url) = {
                let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) else {
                    self.send(HostMsg::CloseTab { tab_id });
                    continue;
                };
                let size = (t.viewport_pt.0, t.viewport_pt.1, t.scale);
                t.sent_resize = Some(size);
                let roots = (engine_policy && !t.file_roots.is_empty()).then(|| t.file_roots.clone());
                let nav = match pending_url {
                    Some(url) if &url != sent_url => {
                        if crate::policy::is_file_scheme(&url) && !engine_policy {
                            t.loading = false;
                            t.refusal = Some(crate::policy::Refusal::Web.code().to_owned());
                            t.rejected += 1;
                            None
                        } else {
                            Some(url)
                        }
                    }
                    _ => None,
                };
                (size, roots, nav)
            };
            if size != *sent_size {
                self.send(HostMsg::Resize { tab_id, width_pt: size.0, height_pt: size.1, scale: size.2 });
            }
            if let Some(file_roots) = roots_to_send {
                self.send(HostMsg::SetTabPolicy { tab_id, file_roots });
            }
            if let Some(url) = nav_url {
                self.send(HostMsg::Navigate { tab_id, url });
            }
        }
        let mut deferred = Vec::new();
        for (tab_id, url) in std::mem::take(&mut self.pending_opens).into_iter().filter(|(id, _)| !early.contains_key(id)) {
            if crate::policy::is_file_scheme(&url) && !ready {
                deferred.push((tab_id, url));
                continue;
            }
            let (open_url, ((width_pt, height_pt), scale), file_roots) =
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                    t.sent_resize = Some((t.viewport_pt.0, t.viewport_pt.1, t.scale));
                    let open_url = if crate::policy::is_file_scheme(&url) && !engine_policy {
                        t.url = "about:blank".into();
                        t.loading = false;
                        t.refusal = Some(crate::policy::Refusal::Web.code().to_owned());
                        t.rejected += 1;
                        "about:blank".into()
                    } else {
                        url
                    };
                    let roots = if engine_policy { t.file_roots.clone() } else { Vec::new() };
                    (open_url, (t.viewport_pt, t.scale), roots)
                } else {
                    (url, ((800.0, 600.0), 2.0), Vec::new())
                };
            self.send(HostMsg::OpenTab {
                tab_id,
                url: open_url,
                width_pt,
                height_pt,
                scale,
                focused: self.focused_tab == Some(tab_id),
                file_roots,
            });
        }
        self.pending_opens = deferred;
    }

    /// What `flush_pending_opens` would send for the tabs that wait now, for a connect to send
    /// itself. `file:` tabs wait for `Ready` so an older engine without `policy` never opens one.
    #[cfg_attr(not(feature = "engine"), allow(dead_code))]
    pub(crate) fn pending_open_msgs(&self) -> Vec<(u64, HostMsg)> {
        let focused = self.focused_tab;
        self.pending_opens
            .iter()
            .filter(|(_, url)| !crate::policy::is_file_scheme(url))
            .cloned()
            .map(|(tab_id, url)| {
                let (((width_pt, height_pt), scale), file_roots) = self
                    .tabs
                    .iter()
                    .find(|t| t.id == tab_id)
                    .map_or((((800.0, 600.0), 2.0), Vec::new()), |t| ((t.viewport_pt, t.scale), t.file_roots.clone()));
                (
                    tab_id,
                    HostMsg::OpenTab {
                        tab_id,
                        url,
                        width_pt,
                        height_pt,
                        scale,
                        focused: focused == Some(tab_id),
                        file_roots,
                    },
                )
            })
            .collect()
    }

    /// Processes incoming messages from the sidecar; returns `true` only when visible UI metadata changed.
    /// Whether poll would find nothing to do: no message, no download to connect for, the
    /// status as last polled. Reads only, so a caller that polls every frame writes nothing.
    pub fn quiet(&self) -> bool {
        if let Ok(msg) = self.inbox_rx.try_recv() {
            self.held.borrow_mut().push_back(msg);
        }
        self.held.borrow().is_empty() && self.pending_opens.is_empty() && *self.status.lock().unwrap() == self.last_polled_status
    }

    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        self.flush_pending_opens();
        let mut mail = std::mem::take(&mut *self.held.borrow_mut());
        mail.extend(std::iter::from_fn(|| self.inbox_rx.try_recv().ok()));
        for msg in mail {
            match msg {
                EngineMsg::Ready { downloads, policy, .. } => {
                    self.engine_downloads = downloads;
                    self.engine_policy = policy;
                    // A new engine starts with an empty downloads folder and numbers its files
                    // from 1 again: the last one's files are gone.
                    for t in &mut self.tabs {
                        t.files.clear();
                    }
                    rune::marks::mark("browser.engine.ready");
                    *self.status.lock().unwrap() = EngineStatus::Ready;
                    // Before any page loads, so the first draws in the app's scheme.
                    match self.color_scheme.load(std::sync::atomic::Ordering::Relaxed) {
                        0 => {}
                        s => self.send(HostMsg::SetColorScheme { dark: s == 2 }),
                    }

                    // Tabs opened before it: their size and policy roots again (a reconnect forgets them).
                    let roots_to_sync: Vec<(u64, Vec<String>)> = self
                        .tabs
                        .iter_mut()
                        .filter(|t| !self.pending_opens.iter().any(|(id, _)| *id == t.id))
                        .map(|t| {
                            t.sent_resize = None;
                            (t.id, t.file_roots.clone())
                        })
                        .collect();
                    if policy {
                        for (tab_id, file_roots) in roots_to_sync {
                            if !file_roots.is_empty() {
                                self.send(HostMsg::SetTabPolicy { tab_id, file_roots });
                            }
                        }
                    }
                    self.flush_pending_opens();
                    changed = true;
                }
                EngineMsg::Refused { tab_id, reason } => {
                    if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        t.loading = false;
                        t.refusal = Some(reason);
                        t.rejected += 1;
                        changed = true;
                    }
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
                EngineMsg::Download { tab_id, file } => {
                    if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        match t.files.iter().position(|f| f.id == file.id) {
                            Some(known) => t.files[known] = file,
                            // The engine keeps at most MAX_TAB_FILES a tab; the list holds those and
                            // one more place for the latest it refused, which the pane names.
                            None if t.files.len() <= MAX_TAB_FILES => t.files.push(file),
                            None => {
                                if let Some(last) = t.files.last_mut() {
                                    *last = file;
                                }
                            }
                        }
                        changed = true;
                    }
                }
                EngineMsg::CdpEvent { .. } => {}
                // The socket reader delivers frames directly to the tab's retained Rune surface.
                EngineMsg::SharedFrame { .. } | EngineMsg::CpuFrame { .. } => {}
            }
        }
        #[cfg(feature = "engine")]
        self.check_for_update_once();
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
            Ime::Preedit(s, ..) => {
                self.send(HostMsg::Ime { tab_id, commit: None, preedit: Some(s.clone()) });
                true
            }
            _ => false,
        }
    }

    /// Builds a focusable Rune `El` displaying `tab_id`'s zero-copy `NativeSurface` and forwarding
    /// layout resizes, pointer clicks/drags/wheel, cursor shapes, and non-shell keys.
    pub fn shutdown_sidecar(&mut self) {
        self.conn_gen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        #[cfg(feature = "engine")]
        if let Ok(sock) = self.engine_socket() {
            let _ = std::fs::remove_file(sock);
        }
        self.send(HostMsg::Shutdown);
        *self.outbox.lock().unwrap() = None;
        let mut status = self.status.lock().unwrap();
        if *status == EngineStatus::Ready || *status == EngineStatus::Connecting {
            *status = EngineStatus::Installed;
        }
    }

    pub(crate) fn forward_key(&mut self, tab_id: u64, ev: &KeyEvent, down: bool) -> bool {
        if self.shell_shortcut.map_or_else(|| is_shell_shortcut(ev), |f| f(ev)) {
            return false;
        }
        if let Some(dir) = page_zoom_dir(ev) {
            if down {
                self.step_zoom(tab_id, dir);
            }
            return true;
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
        if !snap.loading && (snap.url.starts_with("http") || snap.url.starts_with("file:")) {
            rune::marks::mark("browser.page.loaded");
        }
        self.next_tab_id = self.next_tab_id.max(snap.tab_id + 1);
        let want_zoom = self.site_zoom.get(&snap.url);
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == snap.tab_id) {
            let new_site = site_of(&snap.url).is_some() && site_of(&t.url) != site_of(&snap.url);
            let clear_refusal = (t.url != snap.url || snap.load_error.is_some()) && t.refusal.is_some();
            let changed = t.url != snap.url
                || t.title != snap.title
                || t.loading != snap.loading
                || t.can_go_back != snap.can_go_back
                || t.can_go_forward != snap.can_go_forward
                || (t.zoom - snap.zoom).abs() > 0.001
                || t.cursor != snap.cursor
                || t.load_error != snap.load_error
                || clear_refusal;
            if clear_refusal {
                t.refusal = None;
            }
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
        let surface = self.surfaces.lock().unwrap().entry(snap.tab_id).or_insert_with(TabSurface::new).clone();
        if let Some(w) = &self.waker {
            frame::wake_window(&surface, w);
        }
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
            refusal: None,
            rejected: 0,
            inspecting: false,
            files: Vec::new(),
            surface,
            file_roots: Vec::new(),
            viewport_pt: (800.0, 600.0),
            device: None,
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

    /// No engine runs on this platform yet: tabs stay mock-only and the status stays as it is.
    pub fn start_install(&mut self) {}

    pub(crate) fn ensure_connected(&mut self) {}

    pub fn prepare(&mut self, _hold: std::time::Duration) {}
}

/// Tabs a connect sent itself, by id: the address and size (width, height in points, scale)
/// each opened at.
pub(crate) type EarlyOpens = Arc<Mutex<HashMap<u64, (String, (f32, f32, f32))>>>;

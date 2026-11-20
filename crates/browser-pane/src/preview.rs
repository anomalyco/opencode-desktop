//! App-side `oc-preview://` tab orchestration and async remote/artifact proxy queue on
//! [`BrowserHost`], with fail-closed fallback for older engines lacking `Ready.preview`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use crate::client::{BrowserHost, BrowserTab};
use crate::frame::TabSurface;
use crate::machine::SharedEngine;
use crate::protocol::{ContextKind, CursorKind, HostMsg, PreviewKindTag, PreviewSpec};

/// Hard cap on unread `HostMsg::PreviewResponseChunk` frames buffered in the host's outbound
/// socket writer queue (`32 * 64 KiB = 2 MiB` max queued across all active preview fetches).
const MAX_OUTBOX_PREVIEW_CHUNKS: usize = 32;

/// Bounded back-pressure semaphore shared between `PreviewResponder` workers and `rune-browser-tx`
/// so streaming 128 MiB files never floods the outbound channel or starves UI control messages.
#[derive(Default)]
pub(crate) struct PreviewChunkBudget {
    in_flight: Mutex<usize>,
    ready: Condvar,
}

impl PreviewChunkBudget {
    pub(crate) fn acquire(&self, alive: &AtomicBool, shared: &Mutex<SharedEngine>) -> bool {
        let mut count = self.in_flight.lock().unwrap_or_else(PoisonError::into_inner);
        while *count >= MAX_OUTBOX_PREVIEW_CHUNKS {
            if !alive.load(Ordering::Acquire)
                || !shared.lock().unwrap_or_else(PoisonError::into_inner).is_connected()
            {
                return false;
            }
            let (next, _) = self
                .ready
                .wait_timeout(count, Duration::from_millis(50))
                .unwrap_or_else(PoisonError::into_inner);
            count = next;
        }
        if !alive.load(Ordering::Acquire) {
            return false;
        }
        *count += 1;
        true
    }

    pub(crate) fn release_one(&self) {
        let mut count = self.in_flight.lock().unwrap_or_else(PoisonError::into_inner);
        *count = count.saturating_sub(1);
        self.ready.notify_one();
    }

    pub(crate) fn reset(&self) {
        let mut count = self.in_flight.lock().unwrap_or_else(PoisonError::into_inner);
        *count = 0;
        self.ready.notify_all();
    }
}

/// Thread-safe handle allowing background remote/artifact preview HTTP readers to stream
/// response headers and 64 KiB chunks directly to the engine sidecar with bounded back-pressure.
#[derive(Clone)]
pub struct PreviewResponder {
    shared: Arc<Mutex<SharedEngine>>,
    budget: Arc<PreviewChunkBudget>,
}

impl PreviewResponder {
    fn send(&self, msg: HostMsg) -> bool {
        let guard = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        guard.outbox().is_some_and(|tx| tx.send(msg).is_ok())
    }

    pub fn send_head(
        &self,
        req_id: u64,
        ok: bool,
        status: u16,
        mime: String,
        content_length: Option<u64>,
        content_range: Option<String>,
    ) -> bool {
        self.send(HostMsg::PreviewResponseHead {
            req_id,
            ok,
            status,
            mime,
            content_length,
            content_range,
        })
    }

    /// Streams one base64 chunk with bounded outbox back-pressure; returns `false` immediately if
    /// `alive` flips `false` or the engine disconnects.
    pub fn send_chunk(&self, req_id: u64, data_b64: String, alive: &AtomicBool) -> bool {
        if !self.budget.acquire(alive, &self.shared) {
            return false;
        }
        if !self.send(HostMsg::PreviewResponseChunk { req_id, data_b64 }) {
            self.budget.release_one();
            return false;
        }
        true
    }

    pub fn send_end(&self, req_id: u64, ok: bool) -> bool {
        self.send(HostMsg::PreviewResponseEnd { req_id, ok })
    }
}

/// Pending remote workspace or artifact fetch emitted by the engine (`EngineMsg::PreviewFetch`),
/// answered by the desktop app host using strictly the owning session's server credentials.
#[derive(Clone, Debug)]
pub struct PreviewFetchRequest {
    pub req_id: u64,
    pub tab_id: u64,
    pub kind: PreviewKindTag,
    pub path: String,
    pub method: String,
    pub range: Option<String>,
}

impl PreviewSpec {
    pub fn tag(&self) -> PreviewKindTag {
        match self {
            Self::Local { .. } => PreviewKindTag::Local,
            Self::Remote { .. } => PreviewKindTag::Remote,
            Self::Artifact { .. } => PreviewKindTag::Artifact,
        }
    }

    /// App-side isolated [`ContextKind`] tag matching the engine's isolated preview/artifact scope
    /// so `t.context.is_isolated()` is always `true` for preview and artifact tabs.
    pub fn context_kind(&self, tab_id: u64) -> ContextKind {
        match self {
            Self::Local { root, .. } => ContextKind::Preview {
                workspace: root.clone(),
                grant: format!("tab-{tab_id}"),
            },
            Self::Remote { root, session_prefix, .. } => ContextKind::Preview {
                workspace: format!("{session_prefix}{root}"),
                grant: format!("tab-{tab_id}"),
            },
            Self::Artifact { artifact_id, .. } => ContextKind::Artifact {
                artifact_id: format!("{artifact_id}:tab-{tab_id}"),
            },
        }
    }
}

impl BrowserHost {
    /// Opens a new isolated preview tab (`local`, `remote`, or `artifact`).
    ///
    /// Fail-closed capability gating (`notes/review-rd281.md` §9):
    /// - While the engine is not yet `Ready`, the tab waits in `pending_opens` until `Ready` arrives.
    /// - When `Ready.preview == true`, the engine opens an isolated `oc-preview://` context.
    /// - When connected to an older engine (`Ready.preview == false`), NEVER falls back to `file://`;
    ///   refuses cleanly with `"browser.preview.unsupported"`.
    pub fn open_preview_tab(&mut self, display_url: &str, spec: PreviewSpec) -> u64 {
        rune::marks::mark("browser.tab.open");
        let id = self.alloc_tab_id();
        let surface = TabSurface::new();
        if let Some(w) = &self.waker {
            crate::frame::wake_window(&surface, w);
        }
        self.surfaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, surface.clone());

        let ((width_pt, height_pt), scale) = self.last_viewport;
        let connected = self.is_connected();
        let ready = !matches!(self.caps, crate::machine::EngineCaps::Unknown);
        let can_send = connected && ready;
        let tag = spec.tag();
        let ctx_kind = spec.context_kind(id);

        let (initial_url, loading, refusal, send_msg) = if can_send {
            if self.engine_preview() {
                (
                    display_url.to_owned(),
                    true,
                    None,
                    Some(HostMsg::OpenTab {
                        tab_id: id,
                        url: "about:blank".into(),
                        width_pt,
                        height_pt,
                        scale,
                        focused: true,
                        file_roots: Vec::new(),
                        context: ctx_kind.clone(),
                        preview: Some(spec.clone()),
                    }),
                )
            } else {
                ("about:blank".into(), false, Some("browser.preview.unsupported".to_owned()), None)
            }
        } else {
            (display_url.to_owned(), true, None, None)
        };

        self.tabs.push(BrowserTab {
            id,
            url: initial_url,
            title: String::new(),
            loading,
            can_go_back: false,
            can_go_forward: false,
            zoom: 1.0,
            cursor: CursorKind::Default,
            load_error: None,
            favicon: None,
            preview: Some(tag),
            preview_spec: Some(spec),
            refusal: refusal.clone(),
            rejected: u64::from(refusal.is_some()),
            inspecting: false,
            files: Vec::new(),
            prompts: Default::default(),
            find: Default::default(),
            surface,
            surface_origin: (0.0, 0.0),
            view_zoom: 1.0,
            context: ctx_kind,
            file_roots: Vec::new(),
            viewport_pt: (width_pt, height_pt),
            device: None,
            scale,
            sent_resize: can_send.then_some((width_pt, height_pt, scale)),
        });
        self.focused_tab = Some(id);

        if can_send {
            if let Some(msg) = send_msg {
                self.send(msg);
            }
        } else {
            self.pending_opens.push((id, display_url.to_owned()));
            self.ensure_connected();
        }
        id
    }

    /// Replaces the CEF browser behind `tab_id` when navigating between a preview tab and a normal
    /// web tab (or vice versa), so web pages always run in the user's normal signed-in context and
    /// previews always run in a fresh isolated preview context.
    pub fn reopen_tab_as_web(&mut self, tab_id: u64, url: &str, context: ContextKind) {
        let focused = self.focused_tab == Some(tab_id);
        let connected = self.is_connected();
        let ready = !matches!(self.caps, crate::machine::EngineCaps::Unknown);
        let ((width_pt, height_pt), scale) = {
            let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) else { return };
            t.preview = None;
            t.preview_spec = None;
            t.context = context.clone();
            t.file_roots.clear();
            t.url = url.to_owned();
            t.loading = true;
            t.load_error = None;
            t.refusal = None;
            (t.viewport_pt, t.scale)
        };
        if connected && (context.is_default() || ready) {
            *self.reopening_tabs.entry(tab_id).or_default() += 1;
            self.send(HostMsg::CloseTab { tab_id });
            self.send(HostMsg::OpenTab {
                tab_id,
                url: url.to_owned(),
                width_pt,
                height_pt,
                scale,
                focused,
                file_roots: Vec::new(),
                context,
                preview: None,
            });
        } else {
            self.pending_opens.retain(|(id, _)| *id != tab_id);
            self.pending_opens.push((tab_id, url.to_owned()));
            self.ensure_connected();
        }
    }

    /// Replaces the CEF browser behind `tab_id` with a fresh isolated preview context.
    pub fn reopen_tab_as_preview(&mut self, tab_id: u64, display_url: &str, spec: PreviewSpec) {
        let focused = self.focused_tab == Some(tab_id);
        let connected = self.is_connected();
        let ready = !matches!(self.caps, crate::machine::EngineCaps::Unknown);
        let engine_preview = self.engine_preview();
        let ctx_kind = spec.context_kind(tab_id);
        let ((width_pt, height_pt), scale) = {
            let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) else { return };
            t.preview = Some(spec.tag());
            t.preview_spec = Some(spec.clone());
            t.context = ctx_kind.clone();
            t.file_roots.clear();
            t.url = display_url.to_owned();
            t.loading = true;
            t.load_error = None;
            t.refusal = None;
            (t.viewport_pt, t.scale)
        };
        if connected && ready {
            if engine_preview {
                *self.reopening_tabs.entry(tab_id).or_default() += 1;
                self.send(HostMsg::CloseTab { tab_id });
                self.send(HostMsg::OpenTab {
                    tab_id,
                    url: "about:blank".into(),
                    width_pt,
                    height_pt,
                    scale,
                    focused,
                    file_roots: Vec::new(),
                    context: ctx_kind,
                    preview: Some(spec),
                });
            } else if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                t.url = "about:blank".into();
                t.loading = false;
                t.refusal = Some("browser.preview.unsupported".to_owned());
                t.rejected += 1;
            }
        } else {
            self.pending_opens.retain(|(id, _)| *id != tab_id);
            self.pending_opens.push((tab_id, display_url.to_owned()));
            self.ensure_connected();
        }
    }

    pub fn has_preview_fetches(&self) -> bool {
        !self.preview_fetches.is_empty()
    }

    /// Drains pending remote/artifact preview fetch requests queued by the engine.
    pub fn drain_preview_fetches(&mut self) -> Vec<PreviewFetchRequest> {
        std::mem::take(&mut self.preview_fetches)
    }

    /// Drains `(tab_id, document)` pairs for local preview tabs that waited for an older engine
    /// lacking `Ready.preview` so the app can switch them to the Files source viewer instead of
    /// ever falling back to `file://`.
    pub fn drain_unsupported_local_previews(&mut self) -> Vec<(u64, String)> {
        std::mem::take(&mut self.unsupported_local_previews)
    }

    /// Inserts a locally refused tab (e.g., `browser.preview.unsupported` or policy refusal)
    /// WITHOUT sending any `HostMsg::OpenTab` to the engine (`notes/review-rd281.md` PR 2 Blocker #4).
    pub fn open_refused_tab(&mut self, code: &str, context: ContextKind) -> u64 {
        let id = self.alloc_tab_id();
        let surface = TabSurface::new();
        self.surfaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, surface.clone());
        let ((width_pt, height_pt), scale) = self.last_viewport;
        self.tabs.push(BrowserTab {
            id,
            url: "about:blank".into(),
            title: String::new(),
            loading: false,
            can_go_back: false,
            can_go_forward: false,
            zoom: 1.0,
            cursor: CursorKind::Default,
            load_error: None,
            favicon: None,
            preview: None,
            preview_spec: None,
            refusal: Some(code.to_owned()),
            rejected: 1,
            inspecting: false,
            files: Vec::new(),
            prompts: Default::default(),
            find: Default::default(),
            surface,
            surface_origin: (0.0, 0.0),
            view_zoom: 1.0,
            context,
            file_roots: Vec::new(),
            viewport_pt: (width_pt, height_pt),
            device: None,
            scale,
            sent_resize: None,
        });
        self.focused_tab = Some(id);
        id
    }

    /// Returns a cloneable responder that streams preview responses directly to the engine outbox
    /// with bounded chunk back-pressure.
    pub fn preview_responder(&self) -> PreviewResponder {
        PreviewResponder {
            shared: Arc::clone(&self.shared),
            budget: Arc::clone(&self.preview_chunk_budget),
        }
    }

    /// Sets an explicit load error on a tab (for example `browser.artifact.missing`).
    pub fn set_tab_load_error(&mut self, id: u64, error: Option<String>) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
            t.load_error = error;
            t.loading = false;
        }
    }

    pub fn set_mock_preview_clear(&mut self, id: u64) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
            t.preview = None;
            t.preview_spec = None;
        }
    }

    pub fn send_preview_response_head(
        &self,
        req_id: u64,
        ok: bool,
        status: u16,
        mime: String,
        content_length: Option<u64>,
        content_range: Option<String>,
    ) {
        self.send(HostMsg::PreviewResponseHead {
            req_id,
            ok,
            status,
            mime,
            content_length,
            content_range,
        });
    }

    pub fn send_preview_response_chunk(&self, req_id: u64, data_b64: String) {
        self.send(HostMsg::PreviewResponseChunk { req_id, data_b64 });
    }

    pub fn send_preview_response_end(&self, req_id: u64, ok: bool) {
        self.send(HostMsg::PreviewResponseEnd { req_id, ok });
    }

    /// Marks a mock tab as a preview (stories and fixture tests).
    pub fn set_mock_preview(&mut self, id: u64, kind: PreviewKindTag, spec: PreviewSpec) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
            t.preview = Some(kind);
            t.preview_spec = Some(spec);
        }
    }
}

//! Explicit per-tab in-page search state machine (`packages/gui-plugins/src/browser/find-machine.ts`).

use std::hash::{Hash, Hasher};

/// Explicit state of a tab's in-page find session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FindState {
    Closed { query: String },
    Editing { query: String },
    Searching { query: String },
    Found { query: String, active: u32, total: u32 },
}

impl Default for FindState {
    fn default() -> Self {
        Self::Closed { query: String::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FindEvent {
    Open,
    Close,
    Query(String),
    Issued,
    Result { active: u32, total: u32, final_update: bool },
    DocumentChanged,
}

impl FindState {
    pub fn is_open(&self) -> bool {
        !matches!(self, Self::Closed { .. })
    }

    pub fn query(&self) -> &str {
        match self {
            Self::Closed { query }
            | Self::Editing { query }
            | Self::Searching { query }
            | Self::Found { query, .. } => query,
        }
    }

    pub fn matches(&self) -> Option<(u32, u32)> {
        match self {
            Self::Found { active, total, .. } => Some((*active, *total)),
            _ => None,
        }
    }

    pub fn step(&mut self, event: FindEvent) -> bool {
        let next = match (&*self, event) {
            (Self::Closed { query }, FindEvent::Open) => Self::Editing { query: query.clone() },
            (Self::Closed { .. }, _) => return false,
            (_, FindEvent::Close) => Self::Closed { query: self.query().to_owned() },
            (_, FindEvent::Query(query)) => Self::Editing { query },
            (_, FindEvent::Issued) => Self::Searching { query: self.query().to_owned() },
            (Self::Searching { query } | Self::Found { query, .. }, FindEvent::Result { active, total, final_update }) => {
                if !final_update {
                    return false;
                }
                Self::Found { query: query.clone(), active, total }
            }
            (_, FindEvent::DocumentChanged) => Self::Editing { query: self.query().to_owned() },
            _ => return false,
        };
        let changed = *self != next;
        *self = next;
        changed
    }

    pub fn hash_state(&self, h: &mut impl Hasher) {
        match self {
            Self::Closed { .. } => 0u8.hash(h),
            Self::Editing { query } => (1u8, query).hash(h),
            Self::Searching { query } => (2u8, query).hash(h),
            Self::Found { query, active, total } => (3u8, query, *active, *total).hash(h),
        }
    }
}

/// A privacy-safe structured diagnostic event emitted by the engine (`UP-110` `#65096`).
#[derive(Clone, Debug, PartialEq)]
pub struct PageDiagnostic {
    pub tab_id: u64,
    pub level: String,
    pub event: String,
    pub fields: serde_json::Value,
}

impl crate::BrowserHost {
    pub fn engine_find(&self) -> bool {
        self.caps.find()
    }

    pub fn engine_context_menu(&self) -> bool {
        self.caps.context_menu()
    }

    pub fn engine_partitions(&self) -> bool {
        self.caps.partitions()
    }

    pub fn engine_remote_proxy(&self) -> bool {
        self.caps.remote_proxy()
    }

    pub fn engine_agent_tools(&self) -> bool {
        self.caps.agent_tools()
    }

    /// Whether `tab_id` has been sent to a connected engine (`OpenTab` already flushed).
    pub fn tab_open_in_engine(&self, tab_id: u64) -> bool {
        self.is_connected()
            && self.tabs.iter().any(|t| t.id == tab_id)
            && !self.pending_opens.iter().any(|(id, _)| *id == tab_id)
    }

    /// Discards any buffered or future CDP response for `request_id` so cancelled commands never
    /// leak orphaned entries in `cdp_responses`.
    pub fn cancel_cdp(&mut self, request_id: u64) {
        self.cdp_responses.remove(&request_id);
        if let Some(pos) = self.cdp_response_order.iter().position(|&id| id == request_id) {
            self.cdp_response_order.remove(pos);
        }
        if self.cancelled_cdp.len() >= 1_024 {
            self.cancelled_cdp.pop_front();
        }
        self.cancelled_cdp.push_back(request_id);
    }

    pub fn has_cdp_events(&self) -> bool {
        !self.cdp_events.is_empty()
    }

    pub fn drain_cdp_events(&mut self) -> impl Iterator<Item = crate::client::CdpEvent> + '_ {
        self.cdp_events.drain(..)
    }

    /// Sends a typed CDP call allowed by [`crate::policy::check_typed_cdp`] exclusively for native
    /// Rust `experimental.browser` executors (never exposed to untrusted Luau plugins). Falls back
    /// to `HostMsg::CdpCall` for methods already permitted by `check_cdp`.
    pub fn send_typed_cdp(
        &mut self,
        tab_id: u64,
        method: impl Into<String>,
        params: serde_json::Value,
        session_id: Option<String>,
    ) -> Result<u64, String> {
        let method = method.into();
        crate::policy::check_typed_cdp(&method, &params, self.engine_policy())?;
        let request_id = self.next_req_id;
        self.next_req_id += 1;
        if self.engine_agent_tools() {
            self.send(crate::protocol::HostMsg::TypedCdpCall {
                tab_id,
                request_id,
                method,
                params,
                session_id,
            });
            return Ok(request_id);
        }
        crate::policy::check_cdp(&method, &params, self.engine_policy())?;
        self.send(crate::protocol::HostMsg::CdpCall {
            tab_id,
            request_id,
            method,
            params,
            session_id,
        });
        Ok(request_id)
    }

    /// Returns the currently registered loopback tunnel proxy for `server_id`, if any.
    pub fn remote_proxy(&self, server_id: &str) -> Option<&crate::protocol::RemoteProxyConfig> {
        self.remote_proxies.get(server_id.trim())
    }

    /// Drains `(tab_id, fatal_close)` for tabs closed by the engine (`EngineMsg::TabClosed`) so the
    /// desktop shell can prune `session_tabs`/strip and show a notice only on fatal proxy/context loss.
    pub fn drain_closed_tabs(&mut self) -> impl Iterator<Item = (u64, bool)> + '_ {
        self.closed_tabs.drain(..)
    }

    /// Resolves & validates a `RemoteServer` context's loopback proxy before opening a tab;
    /// when a valid explicit `Some(cfg)` updates `remote_proxies`, clears suspended banners on
    /// existing tabs for that server.
    pub(crate) fn resolve_remote_context(
        &mut self,
        context: &mut crate::protocol::ContextKind,
    ) -> Option<&'static str> {
        let crate::protocol::ContextKind::RemoteServer { server_id, proxy } = context else {
            return None;
        };
        let sid = server_id.trim().to_owned();
        *server_id = sid.clone();
        if sid.is_empty() {
            return Some("browser.context.invalid");
        }
        let explicit = proxy.is_some();
        if !explicit {
            *proxy = self.remote_proxies.get(&sid).cloned();
        }
        let Some(cfg) = proxy.as_ref() else {
            return Some("browser.proxy.required");
        };
        if let Err(reason) = crate::policy::validate_proxy_endpoint(
            &cfg.host,
            cfg.port,
            cfg.username.as_deref(),
            cfg.password.as_deref(),
        ) {
            return Some(reason);
        }
        if explicit {
            self.remote_proxies.insert(sid.clone(), cfg.clone());
            for t in &mut self.tabs {
                if let crate::protocol::ContextKind::RemoteServer { server_id: id, proxy: cur } = &mut t.context
                    && id.trim() == sid
                {
                    *cur = Some(cfg.clone());
                    if t.refusal.as_deref() == Some("browser.proxy.required") {
                        t.refusal = None;
                    }
                }
            }
        }
        None
    }

    /// Updates or suspends the local loopback tunnel proxy on a remote server partition.
    /// Rejects invalid proxy configs (must be a strict loopback IP literal + non-empty Basic creds).
    /// Clearing (`None`) suspends live tabs in the engine without destroying unsaved user state;
    /// supplying `Some(cfg)` clears the suspended refusal banner on matching tabs.
    pub fn set_partition_proxy(
        &mut self,
        server_id: &str,
        proxy: Option<crate::protocol::RemoteProxyConfig>,
    ) -> bool {
        let sid = server_id.trim();
        if sid.is_empty() {
            return false;
        }
        if let Some(cfg) = &proxy {
            if crate::policy::validate_proxy_endpoint(
                &cfg.host,
                cfg.port,
                cfg.username.as_deref(),
                cfg.password.as_deref(),
            )
            .is_err()
            {
                return false;
            }
            self.remote_proxies.insert(sid.to_owned(), cfg.clone());
        } else {
            self.remote_proxies.remove(sid);
        }
        for t in &mut self.tabs {
            if let crate::protocol::ContextKind::RemoteServer { server_id: id, proxy: cur } = &mut t.context
                && id.trim() == sid
            {
                *cur = proxy.clone();
                if proxy.is_some() && t.refusal.as_deref() == Some("browser.proxy.required") {
                    t.refusal = None;
                }
            }
        }
        if self.is_connected() && self.engine_partitions() && self.engine_remote_proxy() {
            self.send(crate::protocol::HostMsg::SetPartitionProxy {
                server_id: sid.to_owned(),
                proxy,
            });
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open_tab_full(
        &mut self,
        raw_url: &str,
        _file_roots: Vec<String>,
        context: crate::protocol::ContextKind,
        from_address: bool,
        agent_driven: bool,
        allow_extension_pages: bool,
        ext_open_request_id: Option<u64>,
    ) -> u64 {
        rune::marks::mark("browser.tab.open");
        self.opened.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut context = context;
        let proxy_err = self.resolve_remote_context(&mut context);
        let candidate = if from_address {
            crate::address::resolve_address(raw_url)
        } else {
            crate::policy::with_scheme(raw_url)
        };
        let connected = self.is_connected();
        let ready = self.status() == crate::installer::EngineStatus::Ready;
        let ctx_ok = !ready || context.is_supported(self.engine_partitions(), self.engine_remote_proxy());
        let allow_extension_pages = allow_extension_pages && !context.is_isolated() && !agent_driven;
        let allowed_ext = allow_extension_pages
            && self.is_loaded_extension_url(&candidate)
            && (self.engine_extensions() || !connected);
        let refused = if let Some(err) = proxy_err {
            Some(err.to_owned())
        } else if !ctx_ok {
            Some("browser.partitions.unsupported".to_owned())
        } else if allowed_ext {
            None
        } else {
            crate::policy::refusal(&candidate, &[]).map(|r| r.code().to_owned())
        };
        let url = if refused.is_some() { "about:blank".to_owned() } else { candidate };
        let can_open_in_engine = proxy_err.is_none() && ctx_ok;
        let id = self.alloc_tab_id();
        let surface = crate::frame::TabSurface::new();
        if let Some(w) = &self.waker {
            crate::frame::wake_window(&surface, w);
        }
        self.surfaces
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, surface.clone());
        if !context.is_isolated() {
            self.site_zoom.prepare();
        }
        let ((width_pt, height_pt), scale) = self.last_viewport;
        let mut prompts = crate::prompts::TabPromptState::default();
        prompts.agent_driven = agent_driven;
        self.tabs.push(crate::client::BrowserTab {
            id,
            url: url.clone(),
            title: "New tab".into(),
            loading: refused.is_none(),
            can_go_back: false,
            can_go_forward: false,
            zoom: 1.0,
            cursor: crate::protocol::CursorKind::Default,
            load_error: None,
            favicon: None,
            preview: None,
            preview_spec: None,
            refusal: refused.clone(),
            rejected: u64::from(refused.is_some()),
            inspecting: false,
            files: Vec::new(),
            prompts,
            find: Default::default(),
            surface,
            surface_origin: (0.0, 0.0),
            view_zoom: 1.0,
            context: context.clone(),
            allow_extension_pages,
            file_roots: Vec::new(),
            viewport_pt: (width_pt, height_pt),
            device: None,
            scale,
            sent_resize: (can_open_in_engine && connected && (context.is_default() || ready))
                .then_some((width_pt, height_pt, scale)),
            has_engine_tab: can_open_in_engine,
            engine_confirmed: false,
        });
        self.focused_tab = Some(id);

        if can_open_in_engine {
            if connected && (context.is_default() || ready) {
                self.send_open_tab_with_ext(
                    id,
                    url,
                    width_pt,
                    height_pt,
                    scale,
                    true,
                    Vec::new(),
                    context,
                    None,
                    agent_driven,
                    allow_extension_pages,
                    ext_open_request_id,
                );
            } else {
                self.pending_opens.push((id, url));
                self.ensure_connected();
            }
        }
        id
    }

    /// Sends `HostMsg::OpenTab` (carrying `agent_driven`) and mirrors `HostMsg::SetAgentDriven`
    /// when `agent_driven` is true so older engines without `OpenTab.agent_driven` still guard the tab.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn send_open_tab(
        &mut self,
        tab_id: u64,
        url: String,
        width_pt: f32,
        height_pt: f32,
        scale: f32,
        focused: bool,
        file_roots: Vec<String>,
        context: crate::protocol::ContextKind,
        preview: Option<crate::protocol::PreviewSpec>,
        agent_driven: bool,
    ) {
        self.send_open_tab_with_ext(
            tab_id,
            url,
            width_pt,
            height_pt,
            scale,
            focused,
            file_roots,
            context,
            preview,
            agent_driven,
            false,
            None,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn send_open_tab_with_ext(
        &mut self,
        tab_id: u64,
        url: String,
        width_pt: f32,
        height_pt: f32,
        scale: f32,
        focused: bool,
        _file_roots: Vec<String>,
        context: crate::protocol::ContextKind,
        preview: Option<crate::protocol::PreviewSpec>,
        agent_driven: bool,
        allow_extension_pages: bool,
        ext_open_request_id: Option<u64>,
    ) {
        let initial_url = if !context.is_isolated()
            && preview.is_none()
            && self.extensions.filter_gate.maybe_hold(tab_id, &url)
        {
            "about:blank".to_owned()
        } else {
            url
        };
        let allow_extension_pages = allow_extension_pages && !context.is_isolated() && !agent_driven;
        self.send(crate::protocol::HostMsg::OpenTab {
            tab_id,
            url: initial_url,
            width_pt,
            height_pt,
            scale,
            focused,
            file_roots: Vec::new(),
            context,
            preview,
            agent_driven,
            allow_extension_pages,
            is_popup: false,
            ext_open_request_id,
        });
        if agent_driven {
            self.send(crate::protocol::HostMsg::SetAgentDriven { tab_id, agent_driven: true });
        }
    }

    /// What `flush_pending_opens` would send for the tabs that wait now, for a connect to send
    /// itself. `file:`, `chrome-extension:`, and isolated/remote/preview tabs wait for `Ready`
    /// so an older engine never opens one.
    #[cfg_attr(not(feature = "engine"), allow(dead_code))]
    pub(crate) fn pending_open_msgs(&mut self) -> Vec<(u64, crate::protocol::HostMsg)> {
        let focused = self.focused_tab;
        let candidates: Vec<(u64, String)> = self
            .pending_opens
            .iter()
            .filter(|(tab_id, url)| {
                !crate::policy::is_file_scheme(url)
                    && !crate::policy::extension_origin(url)
                    && self
                        .tabs
                        .iter()
                        .any(|t| t.id == *tab_id && t.context.is_default() && t.preview_spec.is_none())
            })
            .cloned()
            .collect();
        let mut out = Vec::with_capacity(candidates.len());
        for (tab_id, url) in candidates {
            let (((width_pt, height_pt), scale), agent_driven, allow_extension_pages) = self
                .tabs
                .iter()
                .find(|t| t.id == tab_id)
                .map_or((((800.0, 600.0), 2.0), false, false), |t| {
                    (
                        (t.viewport_pt, t.scale),
                        t.prompts.agent_driven,
                        t.allow_extension_pages && !t.prompts.agent_driven,
                    )
                });
            let initial_url = if self.extensions.filter_gate.maybe_hold(tab_id, &url) {
                "about:blank".to_owned()
            } else {
                url
            };
            out.push((
                tab_id,
                crate::protocol::HostMsg::OpenTab {
                    tab_id,
                    url: initial_url,
                    width_pt,
                    height_pt,
                    scale,
                    focused: focused == Some(tab_id),
                    file_roots: Vec::new(),
                    context: crate::protocol::ContextKind::Default,
                    preview: None,
                    agent_driven,
                    allow_extension_pages,
                    is_popup: false,
                    ext_open_request_id: None,
                },
            ));
        }
        out
    }

    /// Closes connections and releases a non-default preview or remote server context in the engine.
    pub fn drop_context(&mut self, context: crate::protocol::ContextKind) {
        if let crate::protocol::ContextKind::RemoteServer { server_id, .. } = &context {
            let sid = server_id.trim();
            self.remote_proxies.remove(sid);
        }
        if !context.is_default() {
            self.send(crate::protocol::HostMsg::DropContext { context });
        }
    }

    /// Hard reload bypassing cached files (`menu.hardReload` / `Ctrl+F5` / `Mod+Shift+R`).
    pub fn reload_ignore_cache(&mut self, tab_id: u64) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            t.prompts.on_user_nav();
        }
        self.send(crate::protocol::HostMsg::Reload { tab_id, ignore_cache: true });
    }

    /// Clears origin-scoped cache storage, service workers, and HTTP cache while keeping cookies
    /// (`site.clearCache`, "You stay signed in.").
    pub fn clear_site_cache(&mut self, tab_id: u64) -> Result<u64, String> {
        if !self.tabs.iter().any(|t| t.id == tab_id) {
            return Err(format!("No browser tab with tabID {tab_id}"));
        }
        if !self.engine_policy() {
            return Err("Clearing saved files requires an updated browser engine.".into());
        }
        let request_id = self.next_req_id;
        self.next_req_id += 1;
        self.send(crate::protocol::HostMsg::ClearSiteCache { tab_id, request_id });
        Ok(request_id)
    }

    pub fn send_cdp(
        &mut self,
        tab_id: u64,
        method: impl Into<String>,
        params: serde_json::Value,
        session_id: Option<String>,
    ) -> Result<u64, String> {
        if session_id.is_some() {
            return Err("DevTools CdpCall does not permit targeting a child sessionId.".into());
        }
        if self
            .tabs
            .iter()
            .find(|t| t.id == tab_id)
            .is_some_and(|t| crate::policy::extension_origin(&t.url))
        {
            return Err("DevTools calls are not permitted on extension pages.".into());
        }
        let method = method.into();
        crate::policy::check_cdp(&method, &params, self.engine_policy())?;
        let request_id = self.next_req_id;
        self.next_req_id += 1;
        self.send(crate::protocol::HostMsg::CdpCall {
            tab_id,
            request_id,
            method,
            params,
            session_id: None,
        });
        Ok(request_id)
    }

    /// Starts or advances an in-page text search (`FindHandler`).
    /// `user_initiated` MUST be `true` ONLY when triggered directly by the user via the bundled
    /// built-in browser find bar (`Half::is_builtin("browser") && tool_sid.is_none()`).
    pub fn find(
        &mut self,
        tab_id: u64,
        query: &str,
        forward: bool,
        match_case: bool,
        find_next: bool,
        user_initiated: bool,
    ) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            t.find.step(FindEvent::Open);
            if t.find.query() != query {
                t.find.step(FindEvent::Query(query.to_owned()));
            }
            if !query.is_empty() {
                t.find.step(FindEvent::Issued);
            }
        }
        self.send(crate::protocol::HostMsg::Find {
            tab_id,
            query: query.to_owned(),
            forward,
            match_case,
            find_next,
            user_initiated,
        });
    }

    /// Ends in-page text search and clears match highlights.
    pub fn stop_find(&mut self, tab_id: u64, clear_selection: bool) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            t.find.step(FindEvent::Close);
        }
        self.send(crate::protocol::HostMsg::StopFind { tab_id, clear_selection });
    }

    pub fn has_context_menus(&self) -> bool {
        !self.context_menus.is_empty()
    }

    pub fn drain_context_menus(&mut self) -> impl Iterator<Item = (u64, crate::protocol::PageContextMenu)> + '_ {
        self.context_menus.drain(..)
    }

    pub fn run_context_menu_action(
        &mut self,
        tab_id: u64,
        id: u64,
        action: Option<crate::protocol::ContextMenuCommand>,
    ) {
        if matches!(
            action,
            Some(
                crate::protocol::ContextMenuCommand::Back
                    | crate::protocol::ContextMenuCommand::Forward
                    | crate::protocol::ContextMenuCommand::Reload
                    | crate::protocol::ContextMenuCommand::HardReload
            )
        ) && let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id)
        {
            t.prompts.on_user_nav();
        }
        self.send(crate::protocol::HostMsg::ContextMenuAction { tab_id, id, action });
    }

    pub fn has_diagnostics(&self) -> bool {
        !self.diagnostics.is_empty()
    }

    pub fn drain_diagnostics(&mut self) -> impl Iterator<Item = PageDiagnostic> + '_ {
        self.diagnostics.drain(..)
    }
}

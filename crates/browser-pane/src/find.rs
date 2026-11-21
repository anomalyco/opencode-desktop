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

    /// Sends `HostMsg::OpenTab` (carrying `agent_driven`) and mirrors `HostMsg::SetAgentDriven`
    /// when `agent_driven` is true so older engines without `OpenTab.agent_driven` still guard the tab.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn send_open_tab(
        &self,
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
    ) {
        self.send(crate::protocol::HostMsg::OpenTab {
            tab_id,
            url,
            width_pt,
            height_pt,
            scale,
            focused,
            file_roots: Vec::new(),
            context,
            preview,
            agent_driven,
        });
        if agent_driven {
            self.send(crate::protocol::HostMsg::SetAgentDriven { tab_id, agent_driven: true });
        }
    }

    /// What `flush_pending_opens` would send for the tabs that wait now, for a connect to send
    /// itself. `file:` and isolated/remote/preview tabs wait for `Ready` so an older engine never opens one.
    #[cfg_attr(not(feature = "engine"), allow(dead_code))]
    pub(crate) fn pending_open_msgs(&self) -> Vec<(u64, crate::protocol::HostMsg)> {
        let focused = self.focused_tab;
        self.pending_opens
            .iter()
            .filter(|(tab_id, url)| {
                !crate::policy::is_file_scheme(url)
                    && self.tabs.iter().any(|t| t.id == *tab_id && t.context.is_default() && t.preview_spec.is_none())
            })
            .cloned()
            .map(|(tab_id, url)| {
                let (((width_pt, height_pt), scale), agent_driven) = self
                    .tabs
                    .iter()
                    .find(|t| t.id == tab_id)
                    .map_or((((800.0, 600.0), 2.0), false), |t| {
                        ((t.viewport_pt, t.scale), t.prompts.agent_driven)
                    });
                (
                    tab_id,
                    crate::protocol::HostMsg::OpenTab {
                        tab_id,
                        url,
                        width_pt,
                        height_pt,
                        scale,
                        focused: focused == Some(tab_id),
                        file_roots: Vec::new(),
                        context: crate::protocol::ContextKind::Default,
                        preview: None,
                        agent_driven,
                    },
                )
            })
            .collect()
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

    pub fn has_download_save_requests(&self) -> bool {
        !self.download_save_requests.is_empty()
    }

    pub fn pop_download_save_request(&mut self) -> Option<crate::protocol::DownloadSaveRequest> {
        self.download_save_requests.pop_front()
    }

    pub fn answer_download_save(&mut self, tab_id: u64, id: u64, path: Option<String>) {
        self.send(crate::protocol::HostMsg::AnswerDownloadSave { tab_id, id, path });
    }

    /// Cancels and removes all queued human Save dialogs belonging to `tab_id`.
    pub(crate) fn cancel_download_save_requests(&mut self, tab_id: u64) {
        let mut cancel_ids = Vec::new();
        self.download_save_requests.retain(|r| {
            if r.tab_id == tab_id {
                cancel_ids.push(r.id);
                false
            } else {
                true
            }
        });
        for req_id in cancel_ids {
            self.send(crate::protocol::HostMsg::AnswerDownloadSave {
                tab_id,
                id: req_id,
                path: None,
            });
        }
    }

    /// Confines agent-readable downloads to the exact per-context staging root owned by this app:
    /// - `Default` tabs: `<profile>/opencode-page-downloads/<tab_id>/<file_id>/<file>` (or throwaway
    ///   `<profile>-throwaway/pid-*/opencode-page-downloads/<tab_id>/<file_id>/<file>`).
    /// - Isolated (`RemoteServer` / `Private`) tabs:
    ///   `<profile>-isolated-downloads/pid-<digits>/<tab_id>/<file_id>/<file>` (strictly off `<profile>`).
    /// Rejects non-`agent_driven` tabs, `Preview`/`Artifact` tabs, symlinks/reparse points, and
    /// any path outside the tab's own context staging directory.
    pub fn download_path(
        &self,
        tab_id: u64,
        file: &crate::protocol::DownloadFile,
    ) -> Option<std::path::PathBuf> {
        #[cfg(feature = "engine")]
        {
            const DOWNLOADS_DIR: &str = "opencode-page-downloads";
            let tab = self.tabs.iter().find(|t| t.id == tab_id)?;
            if !tab.prompts.agent_driven
                || tab.preview.is_some()
                || matches!(
                    tab.context,
                    crate::protocol::ContextKind::Preview { .. }
                        | crate::protocol::ContextKind::Artifact { .. }
                )
            {
                return None;
            }
            let lmeta = std::fs::symlink_metadata(&file.path).ok()?;
            if !lmeta.is_file() || is_reparse_or_symlink(&lmeta) {
                return None;
            }
            let profile = std::fs::canonicalize(crate::profile::for_spawn(self.profile_dir.as_deref())?).ok()?;
            let path = std::fs::canonicalize(&file.path).ok()?;
            let folder = path.parent()?;
            let tab_dir = folder.parent()?;
            let dir = tab_dir.parent()?;
            let root = dir.parent()?;
            if folder.file_name()? != file.id.as_str()
                || tab_dir.file_name()? != tab_id.to_string().as_str()
                || !path.is_file()
            {
                return None;
            }
            let profile_name = profile.file_name()?.to_string_lossy();
            let ours = if tab.context.is_isolated() {
                let iso_name = format!("{profile_name}-isolated-downloads");
                let is_pid = dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_prefix("pid-"))
                    .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
                is_pid
                    && root.file_name().map(|n| n.to_string_lossy()) == Some(iso_name.into())
                    && root.parent() == profile.parent()
            } else {
                let throwaway = format!("{profile_name}-throwaway");
                dir.file_name()? == DOWNLOADS_DIR
                    && (root == profile
                        || (root.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy())
                            == Some(throwaway.into())
                            && root.parent()?.parent() == profile.parent()))
            };
            ours.then_some(path)
        }
        #[cfg(not(feature = "engine"))]
        {
            let _ = (tab_id, file);
            None
        }
    }
}

#[cfg(feature = "engine")]
fn is_reparse_or_symlink(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        if (meta.file_attributes() & 0x0400) != 0 {
            return true;
        }
    }
    false
}

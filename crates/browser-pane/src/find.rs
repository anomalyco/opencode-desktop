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

    /// Returns the currently registered loopback tunnel proxy for `server_id`, if any.
    pub fn remote_proxy(&self, server_id: &str) -> Option<&crate::protocol::RemoteProxyConfig> {
        self.remote_proxies.get(server_id.trim())
    }

    /// Drains `(tab_id, last_refusal)` closed by the engine (`EngineMsg::TabClosed`) so the desktop
    /// shell can prune `session_tabs`, close dead side-panel tabs, and surface any refusal reason.
    pub fn drain_closed_tabs(&mut self) -> impl Iterator<Item = (u64, Option<String>)> + '_ {
        self.closed_tabs.drain(..)
    }

    /// Updates or suspends the local loopback tunnel proxy on a remote server partition.
    /// Rejects invalid proxy configs (must be a strict loopback IP literal + non-empty Basic creds).
    /// Clearing (`None`) suspends live tabs in the engine without destroying unsaved user state.
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

    /// Closes connections and releases a non-default preview or remote server context in the engine.
    pub fn drop_context(&mut self, context: crate::protocol::ContextKind) {
        if let crate::protocol::ContextKind::RemoteServer { server_id, .. } = &context {
            self.remote_proxies.remove(server_id.trim());
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

    /// Starts or advances an in-page text search (`FindHandler`).
    pub fn find(&mut self, tab_id: u64, query: &str, forward: bool, match_case: bool, find_next: bool) {
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

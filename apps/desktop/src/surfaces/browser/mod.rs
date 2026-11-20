//! The window's embedded Chromium 156 browser (`browser_pane::BrowserHost`): its pages, the
//! session each belongs to, the engine's background download, and the DevTools calls in flight.
//! The browser plugin (`plugins/browser`) draws the pane, its address bar and its menus over
//! this through the `Browser` service and `ui.browserSurface` (`plugin_host::native::browser`).
//!
//! On `wasm32`, no CEF sidecar engine exists (`EngineStatus::Unsupported`), so `browser-pane` is
//! omitted and this module exposes zero-sized no-op stubs.

pub mod links;
#[cfg(not(target_arch = "wasm32"))]
pub mod context_menu;
#[cfg(not(target_arch = "wasm32"))]
pub mod prompts;
#[cfg(not(target_arch = "wasm32"))]
pub mod rpc;
#[cfg(not(target_arch = "wasm32"))]
mod scene;
#[cfg(not(target_arch = "wasm32"))]
pub mod view;
#[cfg(not(target_arch = "wasm32"))]
pub use prompts::BrowserFeature;
#[cfg(not(target_arch = "wasm32"))]
pub use rpc::BrowserRpcFeature;

/// The browser plugin's id: its tabs key as `browser:<page id>`.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub const PLUGIN: &str = "browser";

#[cfg(target_arch = "wasm32")]
mod web {
    use rune::{Tx, Waker};

    use crate::App;
    use crate::panels::SessionPane;

    #[derive(Default)]
    pub struct BrowserHost;

    impl BrowserHost {
        pub fn set_color_scheme(&self, _: bool) {}
    }

    #[derive(Default)]
    pub struct BrowserPane {
        pub host: BrowserHost,
    }

    impl App {
        pub fn browser_tab_external_url(&self, _: u64) -> Option<String> {
            None
        }

        pub fn open_browser_tab_externally(&mut self, _: u64, _: &mut Tx) {}

        pub fn setup_browser(&mut self, _: Waker) {}

        pub fn opens_as_page(&self, _: &str) -> bool {
            false
        }

        pub fn open_new_browser_tab_for_session(&mut self, _: Option<&str>, _: &str, _: Option<SessionPane>, _: bool) -> u64 {
            0
        }

        pub fn browser_links_enabled(&self) -> bool {
            false
        }

        pub fn browser_agent_pages_enabled(&self) -> bool {
            true
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::BrowserPane;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::collections::HashMap;
    use rune::time::Instant;

    use browser_pane::{BrowserHost, EngineStatus, bare_url};
    use rune::{Key, KeyEvent, Tx, Waker};

    use super::PLUGIN;
    use crate::App;
    use crate::keymap::Bind;
    use crate::panels::{SessionPane, panel_key, split_panel_key};

    /// One DevTools call in flight for a plugin or MCP tool.
    pub struct PendingDevtools {
        pub done: rune_luau::Completer,
        pub deadline: Instant,
        pub tab_id: u64,
        pub is_dialog: bool,
    }

    #[derive(Default)]
    pub struct BrowserPane {
        pub host: BrowserHost,
        /// Each session's pages, oldest first.
        pub session_tabs: HashMap<String, Vec<u64>>,
        /// DevTools calls a plugin waits on, by the engine's request id.
        pub devtools: HashMap<u64, PendingDevtools>,
        /// Last observed Luau store version for `plugin.browser/rune.sitePermissions`.
        pub site_permissions_version: u64,
        /// Conversation app-scheme links waiting to display inside the browser pane's guarded
        /// `RefusalBar` (`(tab_id, href)`).
        pub pending_app_links: std::cell::RefCell<std::collections::VecDeque<(u64, String)>>,
        /// Persistent SQLite browsing history store (`history.sqlite`, lazily opened).
        pub history: std::cell::RefCell<Option<browser_pane::history::HistoryStore>>,
    }

    /// App-level shortcuts that a focused web page must yield to OpenCode (Rune) rather than swallow,
    /// matching Desktop's `packages/gui-plugins/src/browser/keys.ts` (`pageShortcut`): in-page chords
    /// such as ⌘K, ⌘B, ⌘F and ⌘G stay with the page.
    pub fn claims_app_shortcut(ev: &KeyEvent) -> bool {
        if PAGE_KEYS.iter().any(|c| c.matches(ev)) {
            return false;
        }
        if crate::shortcuts::claims_from_browser(ev) {
            return true;
        }
        if [Bind::Rename, Bind::Background].iter().any(|b| b.matches(ev)) || crate::keymap::TERMINAL_TOGGLE.matches(ev) {
            return true;
        }
        let Key::Command(c) = ev.key else { return false };
        let ch = c.to_ascii_lowercase();
        if ev.mods.alt {
            return !ev.mods.shift && matches!(ch, 'r' | 'b' | 'p');
        }
        if ev.mods.shift {
            if ch == 'm' && matches!(crate::updater::build_channel(), "dev" | "local") { return true; }
            return matches!(ch, 't' | '[' | ']' | '{' | '}' | 'r' | 'p' | 'l' | 'e' | 'o' | 's' | 'c');
        }
        matches!(ch, 'l' | 't' | 'n' | 'w' | ',' | '\\' | '[' | ']')
    }

    static PAGE_KEYS: std::sync::LazyLock<Vec<rune::Chord>> = std::sync::LazyLock::new(|| {
        ["mod+a", "mod+c", "mod+v", "mod+x", "mod+z", "mod+r", "mod+shift+z"]
            .into_iter()
            .filter_map(|c| rune::Chord::parse(c).ok())
            .collect()
    });

    /// `url` when the default browser can open it: an `http:` or `https:` page, not `about:blank`,
    /// `chrome:` or `data:`.
    pub fn external_url(url: &str) -> Option<&str> {
        let scheme = url.split_once("://")?.0;
        (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")).then_some(url)
    }

    impl App {
        /// The page browser tab `id` shows, when the default browser can open it.
        pub fn browser_tab_external_url(&self, id: u64) -> Option<String> {
            external_url(&self.workspace_panes.browser.host.tab(id)?.url).map(str::to_owned)
        }

        /// Opens browser tab `id`'s page in the default browser.
        pub fn open_browser_tab_externally(&mut self, id: u64, tx: &mut Tx) {
            if let Some(url) = self.browser_tab_external_url(id) {
                crate::open_in::open_external(tx, &url);
            }
        }

        pub fn setup_browser(&mut self, waker: Waker) {
            self.workspace_panes.browser.host.set_waker(waker);
            self.workspace_panes.browser.host.set_shell_shortcut(claims_app_shortcut);
            // The levels live in the app's state (`plugin.browser`, Desktop's browser plugin namespace),
            // in memory once the storage is up; a change goes behind like any other.
            if self.persists() {
                self.workspace_panes.browser.host.remember_site_zoom(browser_pane::ZoomStore {
                    read: Box::new(|| crate::storage::text("plugin.browser", "rune.siteZoom").map(|t| t.to_string())),
                    write: Box::new(|levels| crate::storage::set("plugin.browser", "rune.siteZoom", levels)),
                });
                self.workspace_panes.browser.host.remember_site_permissions(browser_pane::PermissionStore {
                    read: Box::new(|| crate::storage::text("plugin.browser", "rune.sitePermissions").map(|t| t.to_string())),
                    write: Box::new(|json| {
                        crate::storage::set("plugin.browser", "rune.sitePermissions", json);
                        crate::plugin_host::services::store::store_changed("browser", "rune.sitePermissions");
                    }),
                });
                self.workspace_panes.browser.site_permissions_version =
                    crate::plugin_host::services::store::store_version("browser", "rune.sitePermissions");
            }
        }

        /// Whether "Open links in OpenCode" (`Settings > Browser`) is enabled (defaults to `true`).
        pub fn browser_links_enabled(&self) -> bool {
            crate::storage::text("plugin.browser", "preferences")
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                .and_then(|v| v.get("links").and_then(serde_json::Value::as_bool))
                .unwrap_or(true)
        }

        /// Whether "Open browser automatically" (`Settings > Browser`) is enabled (defaults to `true`).
        pub fn browser_agent_pages_enabled(&self) -> bool {
            crate::storage::text("plugin.browser", "preferences")
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                .and_then(|v| v.get("agentPages").and_then(serde_json::Value::as_bool))
                .unwrap_or(true)
        }

        /// No engine drives live tabs in this build: they show the unsupported card and have no address
        /// bar to focus. Mock tabs (headless, tests, stories) still work.
        pub(crate) fn browser_unsupported(&self) -> bool {
            !self.use_mock_browser() && self.workspace_panes.browser.host.status() == EngineStatus::Unsupported
        }

        pub(crate) fn use_mock_browser(&self) -> bool {
            (crate::headless() || self.fixture_run.stage.is_some())
                && !self.workspace_panes.browser.host.has_socket_override()
                && std::env::var_os("OPENCODE_BROWSER_LIVE_TEST").as_deref() != Some(std::ffi::OsStr::new("1"))
        }

        /// A link to `path`, inside the session's folder, opens as a page in the browser pane rather
        /// than as a file tab, as Desktop's file plugin decides (`renderer.tsx` `fileOrPage`): an
        /// HTML file, the browser on and able to run, the app's own server (which shares this disk:
        /// the pane loads `file://` inside the workspace only) and a wide window.
        pub fn opens_as_page(&self, path: &str) -> bool {
            let html = std::path::Path::new(path).extension().is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"));
            html && !self.browser_unsupported()
                && !self.narrow()
                && crate::servers::runtime::active(self).is_sidecar()
                && self.plugins.list.iter().any(|p| p.manifest.id == PLUGIN && p.running())
        }

        /// Zero-allocation lookup of the active browser tab ID in the routed session.
        pub fn active_browser_tab_id(&self) -> Option<u64> {
            self.active_browser_tab_for_session(self.active_session_id()?)
        }

        /// Active browser tab ID in `sid`'s side panel, if a browser tab is currently active there.
        pub fn active_browser_tab_for_session(&self, sid: &str) -> Option<u64> {
            let side = self.kernel.slots.get(&crate::panels::SIDE);
            let st = side.sessions.get(sid)?;
            let act = st.tab_state.tabs.active.as_deref()?;
            let (plugin, tid) = split_panel_key(act)?;
            if plugin != PLUGIN {
                return None;
            }
            let pane = crate::panels::tabs::session_tab_pane(&st.tab_state.tabs, act);
            if side.adding.iter().any(|n| n.session_id == sid && n.pane == pane && n.over.as_deref() == Some(act)) {
                return None;
            }
            tid.parse::<u64>().ok()
        }

        /// Best default browser tab for `sid`: its active browser tab, else its most recent live tab,
        /// else the host's focused tab.
        pub fn default_browser_tab_for_session(&self, sid: &str) -> Option<u64> {
            self.active_browser_tab_for_session(sid)
                .or_else(|| self.workspace_panes.browser.session_tabs.get(sid)?.iter().rev().copied().find(|&id| self.workspace_panes.browser.host.tab(id).is_some()))
                .or_else(|| self.workspace_panes.browser.host.focused_tab_id())
        }

        /// The pane of the routed session's selected browser tab while the keyboard is in it: its
        /// page, or a field of the browser plugin (its address field) when `field` says one has it.
        pub fn focused_browser_pane(&self, field: bool) -> Option<SessionPane> {
            if self.composer.focused {
                return None;
            }
            let sid = self.active_session_id()?;
            let key = panel_key(PLUGIN, &self.active_browser_tab_id()?.to_string());
            let side = self.kernel.slots.get(&crate::panels::SIDE);
            let st = side.sessions.get(sid)?;
            let pane = crate::panels::tabs::session_tab_pane(&st.tab_state.tabs, &key);
            let in_pane = side.focus.is_focused() && side.active_pane_focus == pane;
            (field || in_pane).then_some(pane)
        }

        /// A press on page `id`: the keyboard is in its pane now, as a click inside Desktop's side
        /// region makes it, so ⌘W, ⌘L and ⌘T act on the browser rather than the session's tab.
        pub fn focus_browser_page(&mut self, id: u64) {
            let Some(sid) = self.browser_tab_session(id).map(str::to_owned) else { return };
            let key = panel_key(PLUGIN, &id.to_string());
            let side = self.kernel.slots.get_mut(&crate::panels::SIDE);
            let Some(st) = side.sessions.get(&sid) else { return };
            side.active_pane_focus = crate::panels::tabs::session_tab_pane(&st.tab_state.tabs, &key);
            side.focus.focus();
            self.composer.focused = false;
        }

        /// Workspace directories whose `file://` documents may open in session `sid` (`pane.ts:162`):
        /// the session's folder when the active server shares this disk (`is_sidecar`) and the
        /// directory is narrower than a filesystem root, drive root, or home folder
        /// (`review-browser-policy.md` §5), else empty. Tabs with no owning session get no roots.
        pub fn browser_file_roots(&self, sid: Option<&str>) -> Vec<String> {
            if !crate::servers::runtime::active(self).is_sidecar() {
                return Vec::new();
            }
            let Some(id) = sid else {
                return Vec::new();
            };
            let dir = self.link_directory(id);
            let home = (!self.home.is_empty()).then_some(self.home.as_str());
            if browser_pane::policy::is_safe_workspace_root(&dir, home) {
                vec![dir]
            } else {
                Vec::new()
            }
        }

        /// Default browser isolation context for tabs opened on the active server: local sidecar
        /// uses `ContextKind::Default`, while remote servers ALWAYS use a dedicated `RemoteServer`
        /// context keyed by canonical `ServerKey` identity (never falling back to `Default`).
        pub fn default_browser_context(&self) -> browser_pane::ContextKind {
            let server = crate::servers::runtime::active(self);
            let same_machine = server.is_sidecar()
                && match &*self.source {
                    crate::Source::Live { server, .. } => server.is_local(),
                    crate::Source::Fixture => true,
                };
            if same_machine {
                browser_pane::ContextKind::Default
            } else {
                browser_pane::ContextKind::RemoteServer {
                    server_id: server.as_str().to_owned(),
                    proxy: None,
                }
            }
        }

        pub fn open_new_browser_tab(&mut self, url: &str) -> u64 {
            self.open_new_browser_tab_in(url, None)
        }

        pub fn open_new_browser_tab_in(&mut self, url: &str, target_pane: Option<SessionPane>) -> u64 {
            let sid = self.active_session_id().map(str::to_owned);
            self.open_new_browser_tab_for_session(sid.as_deref(), url, target_pane, true)
        }

        pub fn open_new_browser_tab_for_session(&mut self, sid: Option<&str>, url: &str, target_pane: Option<SessionPane>, steal_focus: bool) -> u64 {
            self.open_new_browser_tab_for_session_with(sid, url, target_pane, steal_focus, false)
        }

        pub fn open_new_browser_tab_for_session_with(
            &mut self,
            sid: Option<&str>,
            url: &str,
            target_pane: Option<SessionPane>,
            steal_focus: bool,
            from_address: bool,
        ) -> u64 {
            let context = self.default_browser_context();
            self.open_new_browser_tab_with_context(sid, url, target_pane, steal_focus, from_address, context)
        }

        pub fn open_new_browser_tab_with_context(
            &mut self,
            sid: Option<&str>,
            url: &str,
            target_pane: Option<SessionPane>,
            steal_focus: bool,
            from_address: bool,
            context: browser_pane::ContextKind,
        ) -> u64 {
            let roots = if context.is_isolated() {
                Vec::new()
            } else {
                self.browser_file_roots(sid)
            };
            let id = if self.use_mock_browser() {
                let candidate = if from_address { browser_pane::resolve_address(url) } else { browser_pane::with_scheme(url) };
                let refused = browser_pane::refusal(&candidate, &roots);
                let resolved = if refused.is_some() { "about:blank".into() } else { candidate };
                let title = if resolved == "about:blank" { String::new() } else { bare_url(&resolved) };
                let id = self.workspace_panes.browser.host.insert_mock_tab(&resolved, &title, EngineStatus::Ready);
                self.workspace_panes.browser.host.set_mock_history(id, false, false);
                if let Some(why) = refused {
                    self.workspace_panes.browser.host.set_mock_refusal(id, why.code());
                }
                id
            } else {
                self.workspace_panes.browser.host.open_tab_with_context(url, roots, context, from_address)
            };
            if let Some(sid) = sid {
                self.workspace_panes.browser.session_tabs.entry(sid.to_owned()).or_default().push(id);
                let key = panel_key(PLUGIN, &id.to_string());
                if steal_focus {
                    self.open_side_tab_for_session(sid, &key, target_pane, false, false);
                } else {
                    self.keeping_keyboard(|s| s.open_side_tab_for_session(sid, &key, target_pane, false, false));
                }
            }
            id
        }

        /// Loads `url` (or a `:port`) in page `id`. A mock page shows it loaded at once.
        pub fn navigate_browser_tab(&mut self, id: u64, url: &str) {
            let sid = self.browser_tab_session(id).map(str::to_owned);
            let roots = self.browser_file_roots(sid.as_deref());
            let resolved = browser_pane::resolve_address(url);
            if self.use_mock_browser() {
                if let Some(why) = browser_pane::refusal(&resolved, &roots) {
                    self.workspace_panes.browser.host.set_mock_refusal(id, why.code());
                    return;
                }
                let title = if resolved == "about:blank" { String::new() } else { bare_url(&resolved) };
                self.workspace_panes.browser.host.set_mock_page(id, &resolved, &title);
                return;
            }
            self.workspace_panes.browser.host.set_tab_policy(id, roots);
            self.workspace_panes.browser.host.navigate_with_policy(id, &resolved, false);
        }

        pub fn register_existing_browser_tab(&mut self, id: u64) {
            if let Some(sid) = self.active_session_id().map(str::to_owned) {
                self.register_existing_browser_tab_for_session(&sid, id, true);
            }
        }

        pub fn register_existing_browser_tab_for_session(&mut self, sid: &str, id: u64, steal_focus: bool) {
            let vec = self.workspace_panes.browser.session_tabs.entry(sid.to_owned()).or_default();
            if !vec.contains(&id) {
                vec.push(id);
            }
            let key = panel_key(PLUGIN, &id.to_string());
            if steal_focus {
                self.open_side_tab_for_session(sid, &key, None, false, false);
            } else {
                self.keeping_keyboard(|s| s.open_side_tab_for_session(sid, &key, None, false, false));
            }
        }

        /// The session page `id` belongs to.
        pub fn browser_tab_session(&self, id: u64) -> Option<&str> {
            self.workspace_panes.browser.session_tabs.iter().find(|(_, ids)| ids.contains(&id)).map(|(sid, _)| sid.as_str())
        }

        pub fn close_browser_tab_for_session(&mut self, sid: &str, id: u64, tx: &mut Tx) {
            let (composer, side) = (self.composer.focused, self.kernel.slots.get(&crate::panels::SIDE).focus.mode());
            let key = panel_key(PLUGIN, &id.to_string());
            let routed = self.active_session_id() == Some(sid);
            if routed {
                self.close_side_tab(&key, tx);
            } else {
                let st = self.session_side_mut(sid);
                st.tab_state = crate::panels::tabs::close_session_tab(&st.tab_state, &key);
                if st.tab_state.tabs.all.is_empty() {
                    st.visibility.step(false);
                }
            }
            self.forget_browser_tab(id);
            // The keyboard stays where it was, unless it was in the panel this just closed: then it
            // leaves with the panel, and the next ⌘W is held as after the strip's last tab closes,
            // so one press too many doesn't close the session.
            self.composer.focused = composer;
            if routed && side.is_focused() && !self.side_open() {
                let s = self.kernel.slots.get_mut(&crate::panels::SIDE);
                s.focus.blur();
                s.close_guard.step(crate::panels::CloseTabGuardEvent::Emptied);
            } else {
                self.kernel.slots.get_mut(&crate::panels::SIDE).focus.restore(side);
            }
        }

        /// Page `id`'s tab left its strip: the page closes.
        pub fn forget_browser_tab(&mut self, id: u64) {
            for ids in self.workspace_panes.browser.session_tabs.values_mut() {
                ids.retain(|&x| x != id);
            }
            if self.workspace_panes.browser.host.tab(id).is_some() {
                self.workspace_panes.browser.host.close_tab(id);
            }
        }

        pub fn start_browser_install(&mut self) {
            self.workspace_panes.browser.host.start_install();
        }

        /// The engine's messages and the page the engine paints: the
        /// routed session's selected one. True when what the pane shows changed.
        pub fn poll_browser(&mut self) -> bool {
            // Every poll comes here: the pane is written only when the engine has something.
            let mut changed = !self.workspace_panes.browser.host.quiet() && self.workspace_panes.browser.host.poll();
            if self.workspace_panes.browser.host.has_popup_opens() {
                let popups: Vec<_> = self.workspace_panes.browser.host.drain_popup_opens().collect();
                for popup in popups {
                    let sid = self.browser_tab_session(popup.opener_tab_id).map(str::to_owned);
                    let (agent, opener_ctx) = self
                        .workspace_panes
                        .browser
                        .host
                        .tab(popup.opener_tab_id)
                        .map_or((false, self.default_browser_context()), |t| {
                            let ctx = match &t.context {
                                browser_pane::ContextKind::RemoteServer { .. } => t.context.clone(),
                                browser_pane::ContextKind::Preview { .. }
                                | browser_pane::ContextKind::Artifact { .. }
                                | browser_pane::ContextKind::Private => browser_pane::ContextKind::Private,
                                browser_pane::ContextKind::Default => self.default_browser_context(),
                            };
                            (t.prompts.agent_driven, ctx)
                        });
                    let new_id = self.open_new_browser_tab_with_context(
                        sid.as_deref(),
                        &popup.url,
                        None,
                        !popup.background,
                        false,
                        opener_ctx,
                    );
                    if agent {
                        self.workspace_panes.browser.host.set_agent_driven(new_id, true);
                    }
                    changed = true;
                }
            }
            // Reload persisted site permissions when Settings > Browser edits `rune.sitePermissions`.
            let perm_ver = crate::plugin_host::services::store::store_version("browser", "rune.sitePermissions");
            if perm_ver != self.workspace_panes.browser.site_permissions_version {
                self.workspace_panes.browser.site_permissions_version = perm_ver;
                if self.persists() {
                    self.workspace_panes.browser.host.remember_site_permissions(browser_pane::PermissionStore {
                        read: Box::new(|| crate::storage::text("plugin.browser", "rune.sitePermissions").map(|t| t.to_string())),
                        write: Box::new(|json| {
                            crate::storage::set("plugin.browser", "rune.sitePermissions", json);
                            crate::plugin_host::services::store::store_changed("browser", "rune.sitePermissions");
                        }),
                    });
                }
            }
            // The engine paints only its focused tab, so the page on screen must be that tab (after a
            // session switch, say, which shows another session's page without selecting it).
            if self.side_open()
                && let Some(id) = self.active_browser_tab_id()
                && self.workspace_panes.browser.host.focused_tab_id() != Some(id)
            {
                self.workspace_panes.browser.host.focus_tab(id);
                changed = true;
            }
            changed
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{BrowserPane, PendingDevtools};

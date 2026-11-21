//! The window's embedded Chromium 156 browser (`browser_pane::BrowserHost`): its pages, the
//! session each belongs to, the engine's background download, and the DevTools calls in flight.
//! The browser plugin (`plugins/browser`) draws the pane, its address bar and its menus over
//! this through the `Browser` service and `ui.browserSurface` (`plugin_host::native::browser`).
//!
//! On `wasm32`, no CEF sidecar engine exists (`EngineStatus::Unsupported`), so `browser-pane` is
//! omitted and this module exposes zero-sized no-op stubs.

pub mod links;
#[cfg(not(target_arch = "wasm32"))]
mod activity;
#[cfg(not(target_arch = "wasm32"))]
pub mod app_icons;
#[cfg(not(target_arch = "wasm32"))]
pub mod comment;
#[cfg(not(target_arch = "wasm32"))]
mod comment_describe;
#[cfg(not(target_arch = "wasm32"))]
pub mod context_menu;
#[cfg(not(target_arch = "wasm32"))]
pub mod cookie_import;
#[cfg(not(target_arch = "wasm32"))]
mod keys;
#[cfg(not(target_arch = "wasm32"))]
pub mod extensions;
#[cfg(not(target_arch = "wasm32"))]
mod openers;
#[cfg(not(target_arch = "wasm32"))]
pub mod preview;
#[cfg(not(target_arch = "wasm32"))]
mod preview_pool;
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

        pub fn external_url_allowed_for_session(&self, _: Option<&str>, _: &str) -> bool {
            true
        }

        pub fn open_browser_tab_externally(&mut self, _: u64, _: &mut Tx) {}

        pub fn setup_browser(&mut self, _: Waker) {}

        pub fn opens_as_page(&self, _: &str) -> bool {
            false
        }

        pub fn open_workspace_preview_tab(&mut self, _: &str, _: &str, _: Option<SessionPane>, _: bool) -> u64 {
            0
        }

        pub fn try_open_artifact_link(&mut self, _: &str, _: &str) -> bool {
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
    use rune::{Tx, Waker};

    use super::PLUGIN;
    use crate::App;
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
        /// Settings > Browser's cookie-file import waiting on the engine (`cookie_import`).
        pub cookie_import: Option<super::cookie_import::CookieImport>,
        /// Each other browser's icon for Settings > Browser, read off the UI thread (`app_icons`).
        pub app_icons: super::app_icons::AppIcons,
        /// The other browsers and their profiles, listed off the UI thread (`app_icons::ImportSources`).
        pub import_sources: super::app_icons::ImportSources,
        /// Last observed Luau store version for `plugin.browser/rune.sitePermissions`.
        pub site_permissions_version: u64,
        /// Async preview proxy errors `(tab_id, binding_generation, code)` (e.g. artifact 404).
        pub preview_errors: Option<(
            std::sync::mpsc::Sender<(u64, u64, String)>,
            std::sync::mpsc::Receiver<(u64, u64, String)>,
        )>,
        /// Per-tab server & grant-liveness bindings for `PreviewFetch` (strictly bound by `tab_id`
        /// to the owning session's live server — never consulting the window's active server).
        pub(crate) preview_bindings: HashMap<u64, super::preview::PreviewTabBinding>,
        /// Monotonic generation assigned to each `PreviewTabBinding` to guard `preview_errors`.
        pub(crate) next_preview_gen: u64,
        /// Active host-owned app-scheme link reviews keyed by `tab_id` (rendered natively above
        /// the browser panel with a 128-bit CSPRNG token and 500 ms visible+stationary guard).
        pub active_app_reviews: std::cell::RefCell<HashMap<u64, super::links::AppLinkReview>>,
        /// Persistent SQLite browsing history store (`history.sqlite`, lazily opened).
        pub history: std::cell::RefCell<Option<browser_pane::history::HistoryStore>>,
        /// Serializes human download OS Save dialogs (`EngineMsg::DownloadSaveRequest`) one at a time.
        pub save_dialog_open: bool,
        /// Tabs an agent opened or drove while hidden (`machine.ts` `unseen`, blue `tab.activity` dot).
        pub unseen_tabs: std::collections::HashSet<u64>,
        /// Tabs whose strip close is in flight (`UP-148` `strip.closing`) so reconciliation never re-adds them.
        pub closing_tabs: std::collections::HashSet<u64>,
        /// Tabs a page opened (`PopupOpen`): one whose first navigation only became a download
        /// closes itself once the download has its place (official `closesUnshown`).
        pub page_opened_tabs: std::collections::HashSet<u64>,
        /// Chrome-style opener chains and initial `"New tab"` title holds (`UP-118`).
        pub openers: super::openers::TabOpeners,
        /// Floating element-comment spotlight/editor and whole-page note editor (`panel.tsx:1525-1665`).
        pub overlay: super::comment::BrowserOverlay,
        /// True while Site information popover is on View 2 ("Site settings") so Escape steps back.
        pub site_detailed: bool,
        /// Live Shift key state tracked in `home_modifiers` for synchronous Shift+Click checks.
        pub shift_held: bool,
    }

    pub use super::keys::claims_app_shortcut;

    /// `url` when the default browser can open it: a credential-free `http:` or `https:` page,
    /// not `about:blank`, `chrome:`, `data:`, or embedded credentials.
    pub fn external_url(url: &str) -> Option<&str> {
        browser_pane::destination_origin(url).map(|_| url.trim())
    }

    /// Whether `url` is a local/loopback/unspecified HTTP(S) address (`127.0.0.0/8`, `0.0.0.0/8`,
    /// `localhost`, `*.localhost`, `[::1]`, `[::]`, IPv4-mapped IPv6, octal/hex/decimal forms).
    fn is_local_or_loopback_http_url(url: &str) -> bool {
        let Some(origin) = browser_pane::destination_origin(url) else { return false };
        let Some((_, authority)) = origin.split_once("://") else { return false };
        let host = if let Some(rest) = authority.strip_prefix('[') {
            rest.split(']').next().unwrap_or("")
        } else {
            authority.split(':').next().unwrap_or("")
        };
        browser_pane::policy::is_local_or_loopback_host(host)
    }

    impl App {
        /// Resolves the canonical `ServerKey` that owns session `sid` (checking open tabs,
        /// `BROWSER_RPC`, and parked server snapshots before falling back to `active`).
        pub fn browser_server_for_session(&self, sid: Option<&str>) -> opencode_kit::servers::ServerKey {
            if let Some(id) = sid {
                if let Some(k) = crate::servers::runtime::server_for_session(self, id) {
                    return k;
                }
                if let Some(k) = crate::surfaces::browser::rpc::server_for_session(self, id) {
                    return k;
                }
            }
            crate::servers::runtime::active(self).clone()
        }

        /// Whether `url` clicked in session `sid` may be handed to the local OS default browser:
        /// refuses non-HTTP(S), credentialed URLs, and any local/loopback URL on a remote server.
        pub fn external_url_allowed_for_session(&self, sid: Option<&str>, url: &str) -> bool {
            let Some(clean) = external_url(url) else { return false };
            let remote = !self.browser_server_for_session(sid).is_sidecar();
            !(remote && is_local_or_loopback_http_url(clean))
        }

        /// Whether `url` shown in tab `id` may be handed to the local OS default browser:
        /// refuses non-HTTP(S), credentialed URLs, and any local/loopback URL from remote-server tabs.
        pub fn external_url_allowed_for_tab(&self, id: u64, url: &str) -> bool {
            let Some(clean) = external_url(url) else { return false };
            let sid = self.browser_tab_session(id).map(str::to_owned);
            let remote = self
                .workspace_panes
                .browser
                .host
                .tab(id)
                .is_some_and(|t| matches!(t.context, browser_pane::ContextKind::RemoteServer { .. }))
                || !self.browser_server_for_session(sid.as_deref()).is_sidecar();
            !(remote && is_local_or_loopback_http_url(clean))
        }

        /// The page browser tab `id` shows, when the default browser can safely open it.
        pub fn browser_tab_external_url(&self, id: u64) -> Option<String> {
            let url = self.workspace_panes.browser.host.tab(id)?.url.clone();
            self.external_url_allowed_for_tab(id, &url).then(|| url.trim().to_owned())
        }

        /// Opens browser tab `id`'s page in the default browser.
        pub fn open_browser_tab_externally(&mut self, id: u64, tx: &mut Tx) {
            if let Some(url) = self.browser_tab_external_url(id) {
                crate::open_in::open_external(tx, &url);
            }
        }

        pub fn setup_browser(&mut self, waker: Waker) {
            self.workspace_panes.browser.overlay.set_waker(waker.clone());
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

        /// No engine can drive live tabs in this build (`Unsupported` or `Fault::NotPublished`).
        pub(crate) fn browser_unsupported(&self) -> bool {
            !self.use_mock_browser() && matches!(
                self.workspace_panes.browser.host.status(),
                EngineStatus::Unsupported | EngineStatus::Failed(browser_pane::Failure { fault: browser_pane::Fault::NotPublished, .. })
            )
        }

        pub(crate) fn use_mock_browser(&self) -> bool {
            (crate::headless() || self.fixture_run.stage.is_some())
                && !self.workspace_panes.browser.host.has_socket_override()
                && std::env::var_os("OPENCODE_BROWSER_LIVE_TEST").as_deref() != Some(std::ffi::OsStr::new("1"))
        }

        pub fn browser_plugin_ready(&self) -> bool {
            !self.browser_unsupported()
                && !self.narrow()
                && self.plugins.plugin_available(PLUGIN)
        }

        /// Whether `path`, inside the session's folder, opens as a preview page in the browser pane
        /// rather than as a file tab (`renderer.tsx` `fileOrPage`): an HTML file, the browser
        /// running in a wide window, AND an engine supporting isolated `oc-preview://` (`Ready.preview`).
        /// Older engines lacking `Ready.preview` ALWAYS return `false` so local HTML opens in the
        /// Files source viewer instead of ever falling back to native `file://`.
        pub fn opens_as_page(&self, path: &str) -> bool {
            browser_pane::policy::preview::is_html_path(path)
                && self.browser_plugin_ready()
                && self.browser_preview_supported()
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
            self.workspace_panes.browser.host.close_extension_popup();
            let Some(sid) = self.browser_tab_session(id).map(str::to_owned) else { return };
            let key = panel_key(PLUGIN, &id.to_string());
            let side = self.kernel.slots.get_mut(&crate::panels::SIDE);
            let Some(st) = side.sessions.get(&sid) else { return };
            side.active_pane_focus = crate::panels::tabs::session_tab_pane(&st.tab_state.tabs, &key);
            side.focus.focus();
            self.composer.focused = false;
        }

        /// Safe local workspace root for classifying local `file:///` paths into isolated
        /// `PreviewSpec::Local` grants (`#63002`); never forwarded as engine `file_roots`.
        pub fn browser_file_roots(&self, sid: Option<&str>) -> Vec<String> {
            if !self.session_server_is_local(sid) {
                return Vec::new();
            }
            let Some(id) = sid else {
                return Vec::new();
            };
            let dir = self.link_directory(id);
            self.pinned_sidecar_root(&dir).into_iter().collect()
        }

        /// Default browser isolation context for tabs opened in session `sid`: derived from `sid`'s
        /// owning server (NEVER solely from whichever server happens to be active in the window).
        pub fn browser_context_for_session(&self, sid: Option<&str>) -> browser_pane::ContextKind {
            let server = self.browser_server_for_session(sid);
            let same_machine = server.is_sidecar()
                && if server == *crate::servers::runtime::active(self) {
                    match &*self.source {
                        crate::Source::Live { server: live, .. } => live.is_local(),
                        crate::Source::Fixture => true,
                    }
                } else {
                    crate::servers::runtime::own_server().is_none_or(|own| own.is_local())
                };
            if same_machine {
                browser_pane::ContextKind::Default
            } else {
                let id = server.as_str();
                browser_pane::ContextKind::RemoteServer {
                    server_id: id.to_owned(),
                    proxy: self.workspace_panes.browser.host.remote_proxy(id).cloned(),
                }
            }
        }

        /// Default browser isolation context for tabs opened on the active server.
        pub fn default_browser_context(&self) -> browser_pane::ContextKind {
            self.browser_context_for_session(None)
        }

        /// Context and optional `TabDriver` inherited by a pop-up or "Open link/image in new tab"
        /// triggered from `opener_tab_id`:
        /// - Live `RemoteServer` opener -> same `RemoteServer` with current registered loopback proxy.
        /// - Live `Preview` / `Artifact` / `Private` opener -> fresh `ContextKind::Private`.
        /// - Live `Default` opener -> `ContextKind::Default` (never active server).
        /// - Already-closed opener -> `None` (refuse the open; NEVER downgrade to direct `Private`).
        pub fn browser_context_for_opener(
            &self,
            opener_tab_id: u64,
        ) -> Option<(Option<browser_pane::TabDriver>, browser_pane::ContextKind)> {
            let t = self.workspace_panes.browser.host.tab(opener_tab_id)?;
            let ctx = match &t.context {
                browser_pane::ContextKind::RemoteServer { server_id, proxy } => {
                    let sid = server_id.trim();
                    browser_pane::ContextKind::RemoteServer {
                        server_id: sid.to_owned(),
                        proxy: self
                            .workspace_panes
                            .browser
                            .host
                            .remote_proxy(sid)
                            .cloned()
                            .or_else(|| proxy.clone()),
                    }
                }
                browser_pane::ContextKind::Preview { .. }
                | browser_pane::ContextKind::Artifact { .. }
                | browser_pane::ContextKind::Private => browser_pane::ContextKind::Private,
                browser_pane::ContextKind::Default => {
                    if t.preview.is_some() {
                        browser_pane::ContextKind::Private
                    } else {
                        browser_pane::ContextKind::Default
                    }
                }
            };
            let driver = if t.prompts.agent_driven {
                Some(t.prompts.driver.clone().unwrap_or(browser_pane::TabDriver::Agent))
            } else {
                None
            };
            Some((driver, ctx))
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
            let context = self.browser_context_for_session(sid);
            self.open_new_browser_tab_with_context(sid, url, target_pane, steal_focus, from_address, context, None)
        }

        pub fn open_new_agent_browser_tab_for_session(
            &mut self,
            sid: Option<&str>,
            url: &str,
            target_pane: Option<SessionPane>,
            steal_focus: bool,
        ) -> u64 {
            self.open_new_controlled_browser_tab_for_session(
                sid,
                url,
                target_pane,
                steal_focus,
                browser_pane::TabDriver::Agent,
            )
        }

        pub fn open_new_controlled_browser_tab_for_session(
            &mut self,
            sid: Option<&str>,
            url: &str,
            target_pane: Option<SessionPane>,
            steal_focus: bool,
            driver: browser_pane::TabDriver,
        ) -> u64 {
            let context = self.browser_context_for_session(sid);
            self.open_new_browser_tab_with_context(sid, url, target_pane, steal_focus, false, context, Some(driver))
        }

        #[allow(clippy::too_many_arguments)]
        pub fn open_new_browser_tab_with_context(
            &mut self,
            sid: Option<&str>,
            url: &str,
            target_pane: Option<SessionPane>,
            steal_focus: bool,
            from_address: bool,
            context: browser_pane::ContextKind,
            driver: Option<browser_pane::TabDriver>,
        ) -> u64 {
            let agent_driven = driver.is_some();
            let candidate = if from_address {
                browser_pane::resolve_address(url)
            } else {
                browser_pane::with_scheme(url)
            };
            match self.resolve_preview_target(sid, &candidate, agent_driven) {
                super::preview::PreviewTarget::Preview { display_url, spec } => {
                    return self.open_resolved_preview_tab(
                        sid,
                        &display_url,
                        spec,
                        target_pane,
                        steal_focus,
                        driver,
                    );
                }
                super::preview::PreviewTarget::Refused(code) => {
                    let id = self.open_refused_browser_tab(sid, code, target_pane, steal_focus);
                    if let Some(driver) = driver {
                        self.workspace_panes.browser.host.set_tab_driver(id, driver);
                    }
                    return id;
                }
                super::preview::PreviewTarget::NotPreview => {}
            }
            let roots: Vec<String> = Vec::new();
            let id = if self.use_mock_browser() {
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
                self.workspace_panes
                    .browser
                    .host
                    .open_tab_with_context(url, roots, context, from_address, agent_driven)
            };
            let is_agent = matches!(driver, Some(browser_pane::TabDriver::Agent));
            if let Some(driver) = driver {
                self.workspace_panes.browser.host.set_tab_driver(id, driver);
            }
            self.attach_browser_tab_to_session(sid, id, target_pane, steal_focus);
            if is_agent {
                self.mark_browser_tab_unseen(id);
            }
            id
        }

        /// Loads `url` (or a `:port`) in page `id`. Switches context cleanly between isolated
        /// `oc-preview://` tabs and normal web tabs; bans raw `file://` unconditionally.
        pub fn navigate_browser_tab(&mut self, id: u64, url: &str) {
            self.navigate_browser_tab_with_agent(id, url, false);
        }

        pub fn navigate_browser_tab_with_agent(&mut self, id: u64, url: &str, agent: bool) {
            if !agent {
                self.note_browser_addressed(id);
                crate::plugin_host::services::browser_contract::revoke_tab(self, id);
            }
            let sid = self.browser_tab_session(id).map(str::to_owned);
            let resolved = browser_pane::resolve_address(url);
            match self.resolve_preview_target(sid.as_deref(), &resolved, agent) {
                super::preview::PreviewTarget::Preview { display_url, spec } => {
                    self.reopen_resolved_preview_tab(id, sid.as_deref(), &display_url, spec);
                    return;
                }
                super::preview::PreviewTarget::Refused(code) => {
                    self.workspace_panes.browser.host.set_mock_refusal(id, code);
                    return;
                }
                super::preview::PreviewTarget::NotPreview => {}
            }
            let was_preview = self
                .workspace_panes
                .browser
                .host
                .tab(id)
                .is_some_and(|t| t.preview.is_some());
            if self.use_mock_browser() {
                if let Some(why) = browser_pane::refusal(&resolved, &[]) {
                    self.workspace_panes.browser.host.set_mock_refusal(id, why.code());
                    return;
                }
                if was_preview {
                    self.revoke_preview_binding(id);
                    self.workspace_panes.browser.host.set_mock_preview_clear(id);
                }
                let title = if resolved == "about:blank" { String::new() } else { bare_url(&resolved) };
                self.workspace_panes.browser.host.set_mock_page(id, &resolved, &title);
                return;
            }
            if was_preview {
                if let Some(why) = browser_pane::refusal(&resolved, &[]) {
                    self.workspace_panes.browser.host.set_mock_refusal(id, why.code());
                    return;
                }
                self.revoke_preview_binding(id);
                let ctx = self.browser_context_for_session(sid.as_deref());
                self.workspace_panes.browser.host.reopen_tab_as_web(id, &resolved, ctx);
                return;
            }
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

        /// Page `id`'s tab left its strip: the page closes and any preview server binding revokes.
        pub fn forget_browser_tab(&mut self, id: u64) {
            self.workspace_panes.browser.closing_tabs.remove(&id);
            self.workspace_panes.browser.active_app_reviews.borrow_mut().remove(&id);
            self.workspace_panes.browser.openers.on_close(id);
            crate::plugin_host::services::browser_contract::revoke_tab(self, id);
            self.revoke_preview_binding(id);
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
            changed |= self.pump_preview_fetches();
            changed |= self.pump_cookie_import();
            let fallback_sources = self.workspace_panes.browser.host.drain_unsupported_local_previews();
            for (tab_id, doc) in fallback_sources {
                // Non-agent local HTML clicks on an older engine (`!Ready.preview`) open in the
                // Files code viewer ("show source", never `file://`). Agent tabs stay refused.
                let is_agent = self
                    .workspace_panes
                    .browser
                    .host
                    .tab(tab_id)
                    .is_some_and(|t| t.prompts.agent_driven);
                if !is_agent
                    && let Some(sid) = self.browser_tab_session(tab_id).map(str::to_owned)
                {
                    self.later(Box::new(move |s: &mut App, tx: &mut Tx| {
                        s.close_browser_tab_for_session(&sid, tab_id, tx);
                        s.open_workspace_file_source(&sid, &doc, false, None, true, tx);
                    }));
                    changed = true;
                }
            }
            // Prune `session_tabs` and side-panel strip entries for any tab closed by the engine,
            // surfacing a plain-language toast only for fatal proxy/context teardown (never `DropContext`).
            let engine_closed: Vec<(u64, bool)> =
                self.workspace_panes.browser.host.drain_closed_tabs().collect();
            for (closed_id, fatal_close) in engine_closed {
                self.workspace_panes.browser.closing_tabs.remove(&closed_id);
                self.workspace_panes.browser.page_opened_tabs.remove(&closed_id);
                self.workspace_panes.browser.active_app_reviews.borrow_mut().remove(&closed_id);
                self.revoke_preview_binding(closed_id);
                let owner_sid = self.browser_tab_session(closed_id).map(str::to_owned);
                for ids in self.workspace_panes.browser.session_tabs.values_mut() {
                    ids.retain(|&x| x != closed_id);
                }
                if let Some(sid) = owner_sid {
                    let key = panel_key(PLUGIN, &closed_id.to_string());
                    let st = self.session_side_mut(&sid);
                    st.tab_state = crate::panels::tabs::close_session_tab(&st.tab_state, &key);
                    if st.tab_state.tabs.all.is_empty() {
                        st.visibility.step(false);
                    }
                    changed = true;
                }
                if fatal_close {
                    crate::status_toast::fail(
                        self,
                        opencode_i18n::t!("rune.browser.proxy.closed").to_owned(),
                        String::new(),
                    );
                    changed = true;
                }
            }
            changed |= self.drain_browser_popups();
            changed |= self.poll_app_link_reviews();
            changed |= self.close_download_only_tabs();
            self.copy_browser_images();
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
            changed |= self.prune_browser_unseen();
            changed |= self.sync_browser_openers();
            changed
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{BrowserPane, PendingDevtools};

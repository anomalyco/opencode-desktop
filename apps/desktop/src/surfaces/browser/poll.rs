//! The browser's part of every poll (`App::poll_browser`): the engine's messages, the tabs it
//! closed, popups and app-link reviews, and the settings it reloads.

use rune::Tx;

use super::PLUGIN;
use crate::App;
use crate::panels::panel_key;

impl App {
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

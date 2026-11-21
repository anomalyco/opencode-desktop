//! Strip status dot truth for browser tabs (`renderer.tsx:536-538` at `b9a60d9d`):
//! amber `prompt.tab` when waiting on the user while hidden, and blue `tab.activity` when an
//! agent opened or drove the tab while hidden.

#[cfg(not(target_arch = "wasm32"))]
use crate::{App, panels::panel_key};

#[cfg(not(target_arch = "wasm32"))]
use super::PLUGIN;

#[cfg(not(target_arch = "wasm32"))]
impl App {
    /// True when tab `id` is waiting on user input while not visible in the side panel
    /// (`prompt.tab` amber strip dot, `model.ts:900-904`).
    pub fn browser_tab_asking(&self, id: u64) -> bool {
        let key = panel_key(PLUGIN, &id.to_string());
        self.workspace_panes.browser.host.tab(id).is_some_and(|t| t.prompts.waiting_on_user())
            && !self.side_tab_visible(&key)
    }

    /// True when an agent opened or used tab `id` while it was not visible (`tab.activity` blue dot).
    pub fn browser_tab_unseen(&self, id: u64) -> bool {
        let key = panel_key(PLUGIN, &id.to_string());
        self.workspace_panes.browser.unseen_tabs.contains(&id) && !self.side_tab_visible(&key)
    }

    /// Marks tab `id` as having unseen agent activity when it is not currently visible to the user.
    pub fn mark_browser_tab_unseen(&mut self, id: u64) {
        let key = panel_key(PLUGIN, &id.to_string());
        if !self.side_tab_visible(&key) {
            self.workspace_panes.browser.unseen_tabs.insert(id);
        }
    }

    /// Drops unseen marks once a tab closes or becomes visible to the user.
    pub(super) fn prune_browser_unseen(&mut self) -> bool {
        if self.workspace_panes.browser.unseen_tabs.is_empty() {
            return false;
        }
        let ids: Vec<u64> = self.workspace_panes.browser.unseen_tabs.iter().copied().collect();
        let mut changed = false;
        for id in ids {
            let key = panel_key(PLUGIN, &id.to_string());
            if self.workspace_panes.browser.host.tab(id).is_none() || self.side_tab_visible(&key) {
                self.workspace_panes.browser.unseen_tabs.remove(&id);
                changed = true;
            }
        }
        changed
    }
}

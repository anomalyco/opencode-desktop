//! Desktop Tier 2 CDP Freeze scheduler and extension wake-tab handling for browser tabs.

use std::time::Duration;

use rune::time::Instant;

use crate::App;

/// How long an eligible hidden browser tab stays unfrozen before Tier 2 CDP Freeze triggers.
pub const FREEZE_AFTER: Duration = Duration::from_secs(120);

/// Pure eligibility predicate for Tier 2 CDP Freeze on `tab_id`.
pub fn can_freeze_tab(app: &App, tab_id: u64) -> bool {
    let browser = &app.workspace_panes.browser;
    let host = &browser.host;
    let Some(t) = host.tab(tab_id) else {
        return false;
    };
    if host.visible_tabs().contains(&tab_id) || host.is_tab_unthrottled(tab_id) {
        return false;
    }
    if !t.has_engine_tab() || !t.engine_confirmed() || t.loading || t.audible || t.inspecting {
        return false;
    }
    if t.prompts.waiting_on_user() {
        return false;
    }
    if t.files.iter().any(|f| f.state == browser_pane::DownloadState::Pending) {
        return false;
    }
    if browser.save_dialog_open {
        return false;
    }
    if browser.devtools.values().any(|p| p.tab_id == tab_id) {
        return false;
    }
    if browser.overlay.active_for(tab_id) {
        return false;
    }
    if browser.active_app_reviews.borrow().contains_key(&tab_id) {
        return false;
    }
    if host.has_extension_popup_for(tab_id) {
        return false;
    }
    let has_media_grant = t.prompts.visit_permissions.iter().any(|((_, kind), setting)| {
        *setting == browser_pane::SiteSetting::Allow
            && matches!(kind, browser_pane::PermissionKind::Camera | browser_pane::PermissionKind::Microphone)
    });
    !has_media_grant
}

impl App {
    /// Freezes eligible `Hidden` tabs whose hidden duration has reached [`FREEZE_AFTER`].
    /// Read-only fast exit when no tab is due so `Tracked<BrowserPane>` is never dirtied needlessly.
    pub fn tick_browser_residency(&mut self) {
        if !self.workspace_panes.browser.host.engine_tab_hibernation() {
            return;
        }
        let now = Instant::now();
        let to_freeze: Vec<u64> = self
            .workspace_panes
            .browser
            .host
            .tabs()
            .iter()
            .filter_map(|t| {
                let since = t.residency.hidden_since()?;
                (now.saturating_duration_since(since) >= FREEZE_AFTER && can_freeze_tab(self, t.id))
                    .then_some(t.id)
            })
            .collect();
        if to_freeze.is_empty() {
            return;
        }
        for tab_id in to_freeze {
            self.workspace_panes.browser.host.freeze_tab(tab_id, true);
        }
    }

    /// Earliest deadline at which an eligible hidden browser tab reaches [`FREEZE_AFTER`].
    pub fn browser_residency_wake(&self) -> Option<Instant> {
        if !self.workspace_panes.browser.host.engine_tab_hibernation() {
            return None;
        }
        self.workspace_panes
            .browser
            .host
            .tabs()
            .iter()
            .filter_map(|t| {
                let since = t.residency.hidden_since()?;
                can_freeze_tab(self, t.id).then_some(since + FREEZE_AFTER)
            })
            .min()
    }

    /// Processes queued `EngineMsg::ExtensionWakeTab` requests emitted when an extension targets
    /// a discarded (hibernated) tab via `chrome.tabs.{update, reload, highlight}`.
    pub fn drain_extension_wake_tabs(&mut self) -> bool {
        if !self.workspace_panes.browser.host.has_pending_ext_wakes() {
            return false;
        }
        let wakes: Vec<_> = self
            .workspace_panes
            .browser
            .host
            .drain_extension_wake_tabs()
            .collect();
        let mut changed = false;
        for req in wakes {
            let state = self
                .workspace_panes
                .browser
                .host
                .tab(req.tab_id)
                .map(|t| (t.residency.is_hibernated(), t.context.clone()));
            let Some((hibernated, ctx)) = state else {
                continue;
            };
            if hibernated {
                if let Some(url) = req.url {
                    self.workspace_panes
                        .browser
                        .host
                        .wake_tab_to_url(req.tab_id, &url, ctx, req.focus);
                } else {
                    self.workspace_panes
                        .browser
                        .host
                        .wake_tab(req.tab_id, req.focus);
                }
            } else if let Some(url) = req.url {
                self.workspace_panes
                    .browser
                    .host
                    .navigate_with_policy(req.tab_id, &url, false);
            } else if req.reload {
                self.workspace_panes.browser.host.reload(req.tab_id);
            }
            if req.focus {
                self.workspace_panes.browser.host.focus_tab(req.tab_id);
            }
            changed = true;
        }
        changed
    }
}

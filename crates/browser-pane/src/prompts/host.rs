//! `BrowserHost` prompt, permission, HTTP auth, and pop-up coordination methods.

use rune::time::Instant;

use super::{
    AuthGate, BlockedGuardReason, PermissionDecision, PermissionScope, PermissionStore, PopupGate,
    PopupRequest, SitePermissions, SiteSetting, TabDriver,
};
use crate::protocol::{EngineMsg, HostMsg, PermissionKind, SitePermissionEntry};

pub const INHERITED_PERMISSION_REASON: &str = "browser.controlled.inherited_permission";
pub const INHERITED_PERMISSION_MSG: &str =
    "Controlled browser actions are blocked on this site because it holds a profile permission.";

impl crate::BrowserHost {
    pub fn remember_site_permissions(&mut self, store: PermissionStore) {
        let changed = self.site_permissions.reload_profile(store);
        // Clear any revoked/changed visit cache on Default tabs so revocations take effect immediately.
        if !changed.is_empty() {
            for t in &mut self.tabs {
                if t.context.is_default() {
                    for pair in &changed {
                        t.prompts.visit_permissions.remove(pair);
                    }
                }
            }
        }
        self.sync_site_permissions();
    }

    pub fn engine_permission_sync(&self) -> bool {
        self.caps.permission_sync()
    }

    /// Syncs Chromium's `Default` `HostContentSettingsMap` to match Rune's persisted
    /// `rune.sitePermissions` (`Default` `"Always allow"` / `"Never allow"` decisions).
    pub(crate) fn sync_site_permissions(&mut self) {
        if !self.is_connected() || !self.caps.permission_sync() {
            return;
        }
        let _ = self.site_permissions.drain_pending_resets();
        let entries: Vec<SitePermissionEntry> = self
            .site_permissions
            .list()
            .iter()
            .map(|d| SitePermissionEntry {
                origin: d.origin.clone(),
                kind: d.permission,
                allow: d.setting == SiteSetting::Allow,
            })
            .collect();
        self.send(HostMsg::SyncSitePermissions { entries });
    }

    pub fn site_permissions(&self) -> &SitePermissions {
        &self.site_permissions
    }

    pub fn site_permissions_mut(&mut self) -> &mut SitePermissions {
        &mut self.site_permissions
    }

    pub fn engine_prompts(&self) -> bool {
        self.caps.prompts()
    }

    pub fn engine_popups(&self) -> bool {
        self.caps.popups()
    }

    pub fn set_agent_driven(&mut self, tab_id: u64, agent_driven: bool) {
        if agent_driven {
            self.set_tab_driver(tab_id, TabDriver::Agent);
            return;
        }
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) else { return };
        if !t.prompts.agent_driven {
            return;
        }
        t.prompts.agent_driven = false;
        t.prompts.driver = None;
        self.send(HostMsg::SetAgentDriven { tab_id, agent_driven: false });
    }

    /// Marks `tab_id` controlled by `driver` (`Agent` or third-party `Plugin(name)`), notifying
    /// the engine if `agent_driven` was not yet enabled. `Agent` takes precedence over `Plugin`.
    pub fn set_tab_driver(&mut self, tab_id: u64, driver: TabDriver) {
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) else { return };
        if t.prompts.driver != Some(TabDriver::Agent) {
            t.prompts.driver = Some(driver);
        }
        if !t.prompts.agent_driven {
            t.prompts.agent_driven = true;
            self.send(HostMsg::SetAgentDriven { tab_id, agent_driven: true });
        }
    }

    pub fn dismiss_blocked_guard_notice(&mut self, tab_id: u64) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            t.prompts.blocked_guard_notice = None;
        }
    }

    /// App-side mirror of the engine's inherited-permission guard (`Default` profile only).
    /// Fails closed when `url` is empty/unparseable or when connected to an older engine without
    /// `permission_sync` and the profile permission store is not loaded.
    pub(crate) fn url_has_inherited_default_grant(&self, url: &str) -> bool {
        let trimmed = url.trim();
        if trimmed.eq_ignore_ascii_case("about:blank") {
            return false;
        }
        if !self.caps.permission_sync() && !self.site_permissions.is_loaded() {
            return true;
        }
        let Some(orig) = crate::policy::destination_origin(trimmed) else {
            return true;
        };
        if self.site_permissions.has_inherited_allow(&orig) {
            return true;
        }
        self.tabs.iter().any(|other| {
            other.context.is_default()
                && other.preview.is_none()
                && other.prompts.visit_permissions.iter().any(|((o, k), s)| {
                    *s == SiteSetting::Allow
                        && matches!(
                            k,
                            PermissionKind::ClipboardRead
                                | PermissionKind::Location
                                | PermissionKind::Notifications
                        )
                        && crate::policy::destination_origin(o).as_deref() == Some(orig.as_str())
                })
        })
    }

    /// True (fail-closed) when controlled actions on `tab_id` must be refused because its `Default`
    /// profile top-level origin (or `target_url`) holds an inherited grant or cannot be determined.
    pub(crate) fn controlled_origin_blocked(&self, tab_id: u64, target_url: Option<&str>) -> bool {
        let Some(t) = self.tabs.iter().find(|t| t.id == tab_id) else {
            return false;
        };
        if !t.prompts.agent_driven || !t.context.is_default() || t.preview.is_some() {
            return false;
        }
        let target = target_url.map(str::trim).filter(|s| !s.is_empty());
        let current = (!t.url.trim().is_empty()).then_some(t.url.as_str());
        if target.is_none() && current.is_none() {
            return true;
        }
        [target, current]
            .into_iter()
            .flatten()
            .any(|u| self.url_has_inherited_default_grant(u))
    }

    pub(crate) fn reject_if_inherited(&mut self, tab_id: u64, target_url: Option<&str>) -> bool {
        if !self.controlled_origin_blocked(tab_id, target_url) {
            return false;
        }
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            t.prompts.blocked_guard_notice = Some(BlockedGuardReason::InheritedPermission);
        }
        true
    }

    pub fn answer_dialog(
        &mut self,
        tab_id: u64,
        id: u64,
        accept: bool,
        text: String,
        stop: bool,
    ) -> bool {
        let now = Instant::now();
        let replies = self
            .tabs
            .iter_mut()
            .find(|t| t.id == tab_id)
            .map(|t| t.prompts.dialog.answer(id, accept, text, stop, now))
            .unwrap_or_default();
        if replies.is_empty() {
            return false;
        }
        for (reply_id, accept, text) in replies {
            self.send(HostMsg::AnswerDialog { tab_id, id: reply_id, accept, text });
        }
        true
    }

    pub fn answer_permission(
        &mut self,
        tab_id: u64,
        id: u64,
        decision: PermissionDecision,
    ) -> bool {
        let now = Instant::now();
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) else { return false };
        let Some((item, ids)) = t.prompts.permission.take_active(id, now) else { return false };
        let scope = PermissionScope::from_context(&t.context);
        let origin = item.request.origin;
        let kinds = item.request.kinds;
        let allow = matches!(decision, PermissionDecision::AllowOnce | PermissionDecision::AllowAlways);
        let can_keep = !item.agent_driven
            && !matches!(scope, PermissionScope::VisitOnly)
            && t.preview.is_none()
            && crate::policy::destination_origin(&origin).is_some();
        let permanent_block = matches!(decision, PermissionDecision::BlockAlways) && can_keep;
        // Agent-driven and isolated/preview tabs never persist permissions into global `sitePermissions`,
        // and an allow on an agent-driven tab applies only once (`page-prompts.ts:503`).
        if !(item.agent_driven && allow) {
            match decision {
                PermissionDecision::AllowAlways if can_keep => {
                    for kind in kinds {
                        self.site_permissions.set_scoped(&scope, origin.clone(), kind, SiteSetting::Allow);
                    }
                }
                PermissionDecision::BlockAlways if can_keep => {
                    for kind in kinds {
                        self.site_permissions.set_scoped(&scope, origin.clone(), kind, SiteSetting::Block);
                    }
                }
                PermissionDecision::AllowOnce | PermissionDecision::AllowAlways => {
                    for kind in kinds {
                        t.prompts.visit_permissions.insert((origin.clone(), kind), SiteSetting::Allow);
                    }
                }
                PermissionDecision::BlockOnce | PermissionDecision::BlockAlways => {
                    for kind in kinds {
                        t.prompts.visit_permissions.insert((origin.clone(), kind), SiteSetting::Block);
                    }
                }
            }
        }
        // Use `CEF_PERMISSION_RESULT_DISMISS` for non-permanent denies (`BlockOnce`, agent tabs,
        // visit-only contexts) so Chromium does not permanently block the origin.
        let dismiss = !allow && !permanent_block;
        let persist = can_keep
            && matches!(decision, PermissionDecision::AllowAlways | PermissionDecision::BlockAlways);
        for req_id in ids {
            self.send(HostMsg::AnswerPermission { tab_id, id: req_id, allow, dismiss, persist });
        }
        true
    }

    pub fn answer_auth(
        &mut self,
        tab_id: u64,
        id: u64,
        credentials: Option<(String, String)>,
    ) -> bool {
        let now = Instant::now();
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) else { return false };
        let Some((item, ids)) = t.prompts.auth.take_active(id, now) else { return false };
        let (username, password) = match credentials {
            Some((u, p)) => (Some(u), Some(p)),
            None => {
                t.prompts.auth_dismissed.insert(item.key);
                (None, None)
            }
        };
        for challenge_id in ids {
            self.send(HostMsg::AnswerAuth {
                tab_id,
                id: challenge_id,
                username: username.clone(),
                password: password.clone(),
            });
        }
        true
    }

    pub fn dismiss_blocked_popup(&mut self, tab_id: u64) -> bool {
        self.tabs
            .iter_mut()
            .find(|t| t.id == tab_id)
            .is_some_and(|t| t.prompts.popup.dismiss())
    }

    pub fn take_blocked_popup_url(&mut self, tab_id: u64) -> Option<String> {
        let t = self.tabs.iter_mut().find(|t| t.id == tab_id)?;
        let PopupGate::Blocked { url, input, .. } = &t.prompts.popup else { return None };
        if !input.is_live() {
            return None;
        }
        let url = url.clone();
        t.prompts.popup = PopupGate::Idle;
        Some(url)
    }

    pub fn has_popup_opens(&self) -> bool {
        !self.popup_opens.is_empty()
    }

    pub fn drain_popup_opens(&mut self) -> impl Iterator<Item = PopupRequest> + '_ {
        self.popup_opens.drain(..)
    }

    /// Arms guarded prompt buttons and expires timed-out agent permission prompts.
    /// Returns `(changed, next_wake_at)` so idle frames wake zero times.
    pub fn tick_prompts(&mut self, now: Instant) -> (bool, Option<Instant>) {
        let mut changed = false;
        let mut expired: Vec<(u64, u64)> = Vec::new();
        for t in &mut self.tabs {
            changed |= t.prompts.dialog.tick(now);
            let (perm_changed, timed_out) = t.prompts.permission.tick(now);
            changed |= perm_changed;
            for id in timed_out {
                expired.push((t.id, id));
            }
            changed |= t.prompts.auth.tick(now);
            changed |= t.prompts.popup.tick(now);
        }
        for (tab_id, id) in expired {
            self.send(HostMsg::AnswerPermission { tab_id, id, allow: false, dismiss: true, persist: false });
        }
        let next = self.tabs.iter().filter_map(|t| t.prompts.wake_at()).min();
        (changed, next)
    }

    pub(crate) fn handle_prompt_msg(&mut self, msg: EngineMsg) -> bool {
        let now = Instant::now();
        match msg {
            EngineMsg::Dialog { tab_id, dialog } => {
                let auto = self
                    .tabs
                    .iter_mut()
                    .find(|t| t.id == tab_id)
                    .and_then(|t| t.prompts.dialog.opened(dialog, now));
                if let Some((id, accept, text)) = auto {
                    self.send(HostMsg::AnswerDialog { tab_id, id, accept, text });
                    false
                } else {
                    true
                }
            }
            EngineMsg::DialogsReset { tab_id } => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                    let had = t.prompts.dialog.open().is_some();
                    t.prompts.dialog.reset();
                    had
                } else {
                    false
                }
            }
            EngineMsg::PermissionRequest { tab_id, request } => {
                let id = request.id;
                let auto = if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                    if t.preview.is_some() {
                        Some((false, true, false))
                    } else {
                        let agent = t.prompts.agent_driven;
                        let scope = PermissionScope::from_context(&t.context);
                        t.prompts.permission.evaluate(
                            request,
                            agent,
                            &scope,
                            &self.site_permissions,
                            &t.prompts.visit_permissions,
                            now,
                        )
                    }
                } else {
                    Some((false, true, false))
                };
                if let Some((allow, dismiss, persist)) = auto {
                    self.send(HostMsg::AnswerPermission { tab_id, id, allow, dismiss, persist });
                    false
                } else {
                    true
                }
            }
            EngineMsg::PermissionDismissed { tab_id, id } => self
                .tabs
                .iter_mut()
                .find(|t| t.id == tab_id)
                .is_some_and(|t| t.prompts.permission.dismiss_by_id(id, now)),
            EngineMsg::AuthRequest { tab_id, challenge } => {
                let (cancel_id, ui_changed) = if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                    if t.prompts.agent_driven {
                        // Defense in depth against older engines where `OpenTab` doesn't carry
                        // `agent_driven` yet and a fast 401 on `TID_IO` races `SetAgentDriven`.
                        t.prompts.agent_auth_required = Some(challenge.origin);
                        t.prompts.blocked_guard_notice = Some(BlockedGuardReason::Action);
                        (Some(challenge.id), true)
                    } else {
                        let cancel = t.prompts.auth_challenge(challenge, now);
                        (cancel, cancel.is_none())
                    }
                } else {
                    (Some(challenge.id), false)
                };
                if let Some(id) = cancel_id {
                    self.send(HostMsg::AnswerAuth { tab_id, id, username: None, password: None });
                }
                ui_changed
            }
            EngineMsg::AuthReset { tab_id } => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                    let had = t.prompts.auth.active().is_some();
                    t.prompts.auth = AuthGate::Idle;
                    had
                } else {
                    false
                }
            }
            EngineMsg::AuthAgentRequired { tab_id, origin } => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                    t.prompts.agent_auth_required = Some(origin);
                    t.prompts.blocked_guard_notice = Some(BlockedGuardReason::Action);
                    true
                } else {
                    false
                }
            }
            EngineMsg::PopupOpen { tab_id, url, background } => {
                self.popup_opens.push_back(PopupRequest { opener_tab_id: tab_id, url, background });
                true
            }
            EngineMsg::PopupBlocked { tab_id, url } => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                    t.prompts.popup.record(url, now);
                    true
                } else {
                    false
                }
            }
            EngineMsg::FileChooserBlocked { tab_id } => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                    t.prompts.file_chooser_blocked = t.prompts.file_chooser_blocked.saturating_add(1);
                    t.prompts.blocked_guard_notice = Some(BlockedGuardReason::Action);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

//! `BrowserHost` prompt, permission, HTTP auth, and pop-up coordination methods.

use rune::time::Instant;

use super::{
    AuthGate, PermissionDecision, PermissionStore, PopupGate, PopupRequest, SitePermissions,
    SiteSetting,
};
use crate::protocol::{EngineMsg, HostMsg};

impl crate::BrowserHost {
    pub fn remember_site_permissions(&mut self, store: PermissionStore) {
        self.site_permissions = SitePermissions::remember(store);
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
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) else { return };
        if agent_driven {
            t.allow_extension_pages = false;
        }
        if t.prompts.agent_driven == agent_driven {
            return;
        }
        t.prompts.agent_driven = agent_driven;
        self.send(HostMsg::SetAgentDriven { tab_id, agent_driven });
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
        let origin = item.request.origin;
        let kinds = item.request.kinds;
        let allow = matches!(decision, PermissionDecision::AllowOnce | PermissionDecision::AllowAlways);
        // Agent-driven tabs never persist permissions and an allow applies only once (`page-prompts.ts:503`).
        if !(item.agent_driven && allow) {
            match decision {
                PermissionDecision::AllowAlways if !item.agent_driven => {
                    for kind in kinds {
                        self.site_permissions.set(origin.clone(), kind, SiteSetting::Allow);
                    }
                }
                PermissionDecision::BlockAlways if !item.agent_driven => {
                    for kind in kinds {
                        self.site_permissions.set(origin.clone(), kind, SiteSetting::Block);
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
        for req_id in ids {
            self.send(HostMsg::AnswerPermission { tab_id, id: req_id, allow });
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
            self.send(HostMsg::AnswerPermission { tab_id, id, allow: false });
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
                    let agent = t.prompts.agent_driven;
                    t.prompts.permission.evaluate(
                        request,
                        agent,
                        &self.site_permissions,
                        &t.prompts.visit_permissions,
                        now,
                    )
                } else {
                    Some(false)
                };
                if let Some(allow) = auto {
                    self.send(HostMsg::AnswerPermission { tab_id, id, allow });
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
                let cancel_id = if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                    t.prompts.auth_challenge(challenge, now)
                } else {
                    Some(challenge.id)
                };
                if let Some(id) = cancel_id {
                    self.send(HostMsg::AnswerAuth { tab_id, id, username: None, password: None });
                    false
                } else {
                    true
                }
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
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

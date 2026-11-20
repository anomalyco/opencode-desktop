//! Input activation delay, prompt text sanitization, and persisted per-site permission store
//! (`packages/gui-plugins/src/browser/{activation-delay,prompt-machine,site-permissions}.ts`).

use std::collections::HashMap;
use std::time::Duration;

use rune::time::Instant;
use serde::{Deserialize, Serialize};

use crate::protocol::{ContextKind, PermissionKind};

/// How long prompt buttons ignore presses after appearing (`activation-delay.ts`).
pub const ACTIVATION_DELAY_MS: u64 = 500;
/// Repeat scripted dialog threshold (`dialog-machine.ts` `REPEAT_DIALOG_MS`).
pub const REPEAT_DIALOG_MS: u64 = 1_000;
/// Maximum text shown from a page dialog (`ipc.ts` `PROMPT_TEXT_MAX`).
pub const PROMPT_TEXT_MAX: usize = 2_000;
/// Maximum realm length shown on an HTTP auth prompt (`page-prompts.ts:404`).
pub const REALM_TEXT_MAX: usize = 256;
/// How long an agent-driven tab's permission prompt waits before auto-denying (`page-prompts.ts`).
pub const AGENT_PERMISSION_TIMEOUT_MS: u64 = 20_000;
/// Maximum HTTP auth attempts per `(origin, scheme)` in a visit (`site-auth.ts`).
pub const MAX_SIGN_IN_ATTEMPTS: u8 = 3;

/// Whether a prompt's buttons accept presses yet (`activation-delay.ts`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputGuard {
    Guarded { until: Instant },
    Live,
}

impl InputGuard {
    pub fn arm(now: Instant) -> Self {
        Self::Guarded { until: now + Duration::from_millis(ACTIVATION_DELAY_MS) }
    }

    pub fn is_live(self) -> bool {
        self == Self::Live
    }

    pub fn wake_at(self) -> Option<Instant> {
        match self {
            Self::Guarded { until } => Some(until),
            Self::Live => None,
        }
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        if let Self::Guarded { until } = *self
            && now >= until
        {
            *self = Self::Live;
            return true;
        }
        false
    }
}

/// Strips control and bidirectional reordering characters and bounds length (`prompt-machine.ts`).
pub fn sanitize_prompt_text(text: &str, max: usize) -> String {
    text.chars()
        .filter(|&c| {
            let n = c as u32;
            !matches!(
                n,
                0x00..=0x08
                    | 0x0b..=0x1f
                    | 0x7f..=0x9f
                    | 0x061c
                    | 0x200e
                    | 0x200f
                    | 0x202a..=0x202e
                    | 0x2066..=0x2069
            )
        })
        .take(max)
        .collect()
}

/// Saved per-site permission decision (`site-permissions.ts` `SiteDecision`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteSetting {
    Allow,
    Block,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteDecision {
    pub origin: String,
    pub permission: PermissionKind,
    pub setting: SiteSetting,
}

/// Partition scope for site permission decisions (`site-permissions.ts` `DecisionScope`):
/// - `Profile`: persistent decisions for `ContextKind::Default` (`plugin.browser/rune.sitePermissions`).
/// - `RemoteServer(server_id)`: in-memory decisions strictly for one remote server for the current run
///   (never reads or writes `Profile`, never persisted to disk, dropped on `DropContext`).
/// - `VisitOnly`: `Preview`, `Artifact`, and `Private` tabs never read or persist kept decisions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionScope {
    Profile,
    RemoteServer(String),
    VisitOnly,
}

impl PermissionScope {
    pub fn from_context(context: &ContextKind) -> Self {
        match context {
            ContextKind::Default => Self::Profile,
            ContextKind::RemoteServer { server_id, .. } => {
                let sid = server_id.trim();
                if sid.is_empty() {
                    Self::VisitOnly
                } else {
                    Self::RemoteServer(sid.to_owned())
                }
            }
            ContextKind::Preview { .. } | ContextKind::Artifact { .. } | ContextKind::Private => {
                Self::VisitOnly
            }
        }
    }
}

/// Host persistence hook for profile-wide site permission decisions (`Settings › Browser`).
pub struct PermissionStore {
    pub read: Box<dyn Fn() -> Option<String> + Send + Sync>,
    pub write: Box<dyn Fn(String) + Send + Sync>,
}

/// Context-partitioned site permission decisions (`site-permissions.ts`).
#[derive(Default)]
pub struct SitePermissions {
    decisions: Vec<SiteDecision>,
    remote: HashMap<(String, String, PermissionKind), SiteSetting>,
    pending_resets: Vec<(String, PermissionKind)>,
    store: Option<PermissionStore>,
}

impl SitePermissions {
    pub fn remember(store: PermissionStore) -> Self {
        let decisions = (store.read)()
            .and_then(|raw| serde_json::from_str::<Vec<SiteDecision>>(&raw).ok())
            .unwrap_or_default();
        Self {
            decisions,
            remote: HashMap::new(),
            pending_resets: Vec::new(),
            store: Some(store),
        }
    }

    /// Reloads the persistent `Default` profile decisions (e.g. after `Settings > Browser` edits
    /// `rune.sitePermissions`) while preserving live in-memory `RemoteServer` decisions, and
    /// queues Chromium `HostContentSettingsMap` resets for every removed or changed entry.
    pub fn reload_profile(&mut self, store: PermissionStore) -> Vec<(String, PermissionKind)> {
        let next = (store.read)()
            .and_then(|raw| serde_json::from_str::<Vec<SiteDecision>>(&raw).ok())
            .unwrap_or_default();
        let mut changed = Vec::new();
        for old in &self.decisions {
            let still_same = next.iter().any(|n| {
                n.origin == old.origin && n.permission == old.permission && n.setting == old.setting
            });
            if !still_same {
                changed.push((old.origin.clone(), old.permission));
            }
        }
        for pair in &changed {
            if !self.pending_resets.contains(pair) {
                self.pending_resets.push(pair.clone());
            }
        }
        self.decisions = next;
        self.store = Some(store);
        changed
    }

    pub fn drain_pending_resets(&mut self) -> Vec<(String, PermissionKind)> {
        std::mem::take(&mut self.pending_resets)
    }

    pub fn list(&self) -> &[SiteDecision] {
        &self.decisions
    }

    pub fn get(&self, origin: &str, permission: PermissionKind) -> Option<SiteSetting> {
        self.decisions
            .iter()
            .find(|d| d.origin == origin && d.permission == permission)
            .map(|d| d.setting)
    }

    /// Looks up a decision strictly inside `scope` (never crosses between `Profile`, different
    /// `RemoteServer` partitions, or `VisitOnly`).
    pub fn get_scoped(
        &self,
        scope: &PermissionScope,
        origin: &str,
        permission: PermissionKind,
    ) -> Option<SiteSetting> {
        match scope {
            PermissionScope::Profile => self.get(origin, permission),
            PermissionScope::RemoteServer(sid) => {
                self.remote.get(&(sid.clone(), origin.to_owned(), permission)).copied()
            }
            PermissionScope::VisitOnly => None,
        }
    }

    pub fn set(&mut self, origin: String, permission: PermissionKind, setting: SiteSetting) {
        if origin.is_empty() || origin.to_ascii_lowercase().starts_with("oc-preview:") {
            return;
        }
        if let Some(existing) = self
            .decisions
            .iter_mut()
            .find(|d| d.origin == origin && d.permission == permission)
        {
            if existing.setting != setting {
                let pair = (origin.clone(), permission);
                if !self.pending_resets.contains(&pair) {
                    self.pending_resets.push(pair);
                }
            }
            existing.setting = setting;
        } else {
            self.decisions.push(SiteDecision { origin, permission, setting });
        }
        self.flush();
    }

    /// Records a decision strictly inside `scope`: `Profile` persists to disk, `RemoteServer` stays
    /// strictly in memory per server identity, and `VisitOnly` is ignored.
    pub fn set_scoped(
        &mut self,
        scope: &PermissionScope,
        origin: String,
        permission: PermissionKind,
        setting: SiteSetting,
    ) {
        if origin.is_empty() || origin.to_ascii_lowercase().starts_with("oc-preview:") {
            return;
        }
        match scope {
            PermissionScope::Profile => self.set(origin, permission, setting),
            PermissionScope::RemoteServer(sid) => {
                self.remote.insert((sid.clone(), origin, permission), setting);
            }
            PermissionScope::VisitOnly => {}
        }
    }

    /// Drops all ephemeral in-memory decisions for remote server `server_id` (on `DropContext`).
    pub fn clear_remote(&mut self, server_id: &str) {
        let sid = server_id.trim();
        self.remote.retain(|(id, _, _), _| id != sid);
    }

    pub fn remove(&mut self, origin: &str, permission: PermissionKind) {
        let before = self.decisions.len();
        self.decisions.retain(|d| !(d.origin == origin && d.permission == permission));
        if self.decisions.len() != before {
            let pair = (origin.to_owned(), permission);
            if !self.pending_resets.contains(&pair) {
                self.pending_resets.push(pair);
            }
            self.flush();
        }
    }

    pub fn clear(&mut self) {
        if !self.decisions.is_empty() {
            for d in self.decisions.drain(..) {
                let pair = (d.origin, d.permission);
                if !self.pending_resets.contains(&pair) {
                    self.pending_resets.push(pair);
                }
            }
            self.flush();
        }
    }

    fn flush(&self) {
        if let Some(store) = &self.store
            && let Ok(json) = serde_json::to_string(&self.decisions)
        {
            (store.write)(json);
        }
    }
}

//! Input activation delay, prompt text sanitization, and persisted per-site permission store
//! (`packages/gui-plugins/src/browser/{activation-delay,prompt-machine,site-permissions}.ts`).

use std::time::Duration;

use rune::time::Instant;
use serde::{Deserialize, Serialize};

use crate::protocol::PermissionKind;

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

/// Host persistence hook for profile-wide site permission decisions (`Settings › Browser`).
pub struct PermissionStore {
    pub read: Box<dyn Fn() -> Option<String> + Send + Sync>,
    pub write: Box<dyn Fn(String) + Send + Sync>,
}

/// Profile-wide persisted site permission decisions (`site-permissions.ts`).
#[derive(Default)]
pub struct SitePermissions {
    decisions: Vec<SiteDecision>,
    store: Option<PermissionStore>,
}

impl SitePermissions {
    pub fn remember(store: PermissionStore) -> Self {
        let decisions = (store.read)()
            .and_then(|raw| serde_json::from_str::<Vec<SiteDecision>>(&raw).ok())
            .unwrap_or_default();
        Self { decisions, store: Some(store) }
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

    pub fn set(&mut self, origin: String, permission: PermissionKind, setting: SiteSetting) {
        if origin.is_empty() || origin.to_ascii_lowercase().starts_with("oc-preview:") {
            return;
        }
        if let Some(existing) = self
            .decisions
            .iter_mut()
            .find(|d| d.origin == origin && d.permission == permission)
        {
            existing.setting = setting;
        } else {
            self.decisions.push(SiteDecision { origin, permission, setting });
        }
        self.flush();
    }

    pub fn remove(&mut self, origin: &str, permission: PermissionKind) {
        let before = self.decisions.len();
        self.decisions.retain(|d| !(d.origin == origin && d.permission == permission));
        if self.decisions.len() != before {
            self.flush();
        }
    }

    pub fn clear(&mut self) {
        if !self.decisions.is_empty() {
            self.decisions.clear();
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

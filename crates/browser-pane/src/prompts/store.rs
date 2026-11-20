//! Input activation delay, prompt text sanitization, and persisted per-site permission store
//! (`packages/gui-plugins/src/browser/{activation-delay,prompt-machine,site-permissions}.ts`).

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
/// - `VisitOnly`: `RemoteServer`, `Preview`, `Artifact`, and `Private` tabs never read or persist
///   kept decisions (per-visit only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionScope {
    Profile,
    VisitOnly,
}

impl PermissionScope {
    pub fn from_context(context: &ContextKind) -> Self {
        match context {
            ContextKind::Default => Self::Profile,
            ContextKind::RemoteServer { .. }
            | ContextKind::Preview { .. }
            | ContextKind::Artifact { .. }
            | ContextKind::Private => Self::VisitOnly,
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
            pending_resets: Vec::new(),
            store: Some(store),
        }
    }

    /// Reloads the persistent `Default` profile decisions (e.g. after `Settings > Browser` edits
    /// `rune.sitePermissions`), queuing Chromium `HostContentSettingsMap` resets for every removed
    /// or changed entry.
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

    pub fn is_loaded(&self) -> bool {
        self.store.is_some()
    }

    /// True if `Default` profile holds `"Always allow"` for any permission a controlled tab would
    /// inherit (`clipboard-read`, `geolocation`, `notifications`) on `origin`.
    pub fn has_inherited_allow(&self, origin: &str) -> bool {
        self.decisions.iter().any(|d| {
            d.setting == SiteSetting::Allow
                && matches!(
                    d.permission,
                    PermissionKind::ClipboardRead | PermissionKind::Location | PermissionKind::Notifications
                )
                && crate::policy::destination_origin(&d.origin).as_deref() == Some(origin)
        })
    }

    pub fn get(&self, origin: &str, permission: PermissionKind) -> Option<SiteSetting> {
        self.decisions
            .iter()
            .find(|d| d.origin == origin && d.permission == permission)
            .map(|d| d.setting)
    }

    /// Looks up a decision strictly inside `scope` (`Profile` only; `VisitOnly` never inherits).
    pub fn get_scoped(
        &self,
        scope: &PermissionScope,
        origin: &str,
        permission: PermissionKind,
    ) -> Option<SiteSetting> {
        match scope {
            PermissionScope::Profile => self.get(origin, permission),
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

    /// Records a decision strictly inside `scope`: `Profile` persists to disk; `VisitOnly` is ignored.
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
        if matches!(scope, PermissionScope::Profile) {
            self.set(origin, permission, setting);
        }
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

#[cfg(feature = "engine")]
static PREFS_SCRUB_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(feature = "engine")]
static PREFS_SCRUB_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Before spawning a browser engine on an unlocked `profile`, strips any exact-origin site
/// permission entries in Chromium's `Default/Preferences` that are absent from (or disagree with)
/// Rune's `rune.sitePermissions` so revocations and edits in `Settings > Browser` take effect on
/// engine restart even when connected to an older engine (`permission_sync == false`).
///
/// Holds both an in-process mutex (serializing multi-window spawns in the same process) and the
/// exclusive OS `engine.lock` file lock across the entire read-modify-rename, using a unique
/// per-call temp filename (`Preferences.tmp-{pid}-{seq}`).
#[cfg(feature = "engine")]
pub(crate) fn scrub_chromium_preferences_for_older_engine(
    profile: &std::path::Path,
    kept: &[SiteDecision],
) {
    let prefs_path = profile.join("Default").join("Preferences");
    if !prefs_path.exists() {
        return;
    }
    let _in_proc = PREFS_SCRUB_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(_engine_lock) = crate::profile::try_lock_exclusive(profile) else {
        return;
    };
    let Ok(meta) = std::fs::symlink_metadata(&prefs_path) else { return };
    if !meta.is_file() || meta.file_type().is_symlink() {
        return;
    }
    let Ok(raw) = std::fs::read_to_string(&prefs_path) else { return };
    let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&raw) else { return };
    let Some(exceptions) = root
        .get_mut("profile")
        .and_then(|v| v.get_mut("content_settings"))
        .and_then(|v| v.get_mut("exceptions"))
        .and_then(serde_json::Value::as_object_mut)
    else {
        return;
    };
    let tables: &[(&str, PermissionKind)] = &[
        ("notifications", PermissionKind::Notifications),
        ("clipboard", PermissionKind::ClipboardRead),
        ("geolocation", PermissionKind::Location),
        ("geolocation_with_options", PermissionKind::Location),
        ("media_stream_camera", PermissionKind::Camera),
        ("media_stream_mic", PermissionKind::Microphone),
    ];
    let mut dirty = false;
    for &(table_name, kind) in tables {
        let Some(table) = exceptions
            .get_mut(table_name)
            .and_then(serde_json::Value::as_object_mut)
        else {
            continue;
        };
        let before = table.len();
        table.retain(|entry_key, val| {
            let origin_raw = entry_key.split(',').next().unwrap_or(entry_key);
            let Some(origin) = crate::policy::destination_origin(origin_raw) else {
                // Keep non-origin / wildcard keys untouched.
                return true;
            };
            let setting_num = val.get("setting").and_then(serde_json::Value::as_i64);
            kept.iter().any(|d| {
                d.permission == kind
                    && crate::policy::destination_origin(&d.origin).as_deref()
                        == Some(origin.as_str())
                    && matches!(
                        (d.setting, setting_num),
                        (SiteSetting::Allow, Some(1)) | (SiteSetting::Block, Some(2))
                    )
            })
        });
        if table.len() != before {
            dirty = true;
        }
    }
    if !dirty {
        return;
    }
    let Ok(updated) = serde_json::to_vec(&root) else { return };
    let seq = PREFS_SCRUB_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = prefs_path.with_file_name(format!("Preferences.tmp-{}-{seq}", std::process::id()));
    if std::fs::write(&tmp, &updated).is_err() || std::fs::rename(&tmp, &prefs_path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

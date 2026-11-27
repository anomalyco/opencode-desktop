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
                    | 0xfeff
            )
        })
        .take(max)
        .collect()
}

/// Single-line sanitization for auth realms, hostnames, and origins (also strips `\n`/`\r`).
pub fn sanitize_single_line_prompt(text: &str, max: usize) -> String {
    sanitize_prompt_text(text, max)
        .chars()
        .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
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

/// Whether `origin` is a secure context eligible for site permissions (`site-permissions.ts:57-65`):
/// `https:`, `oc-preview:`, or loopback `http:` (`localhost`, `<label>.localhost`, `127.*`, `[::1]`).
/// Rejects userinfo (`@`), bare `".localhost"`, and non-loopback `http:` origins.
pub fn trustworthy_origin(origin: &str) -> bool {
    let trimmed = origin.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else { return false };
    if scheme.eq_ignore_ascii_case("oc-preview") {
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        return !host.is_empty() && !host.contains('@');
    }
    let Some(norm) = crate::policy::destination_origin(trimmed) else { return false };
    let Some((norm_scheme, norm_rest)) = norm.split_once("://") else { return false };
    if norm_scheme.eq_ignore_ascii_case("https") {
        return true;
    }
    if !norm_scheme.eq_ignore_ascii_case("http") {
        return false;
    }
    let host = if norm_rest.starts_with('[') {
        norm_rest.split_once(']').map_or(norm_rest, |(h, _)| &norm_rest[..=h.len()])
    } else {
        norm_rest.split(':').next().unwrap_or(norm_rest)
    };
    let lower = host.to_ascii_lowercase();
    lower == "localhost"
        || (lower.ends_with(".localhost") && lower.len() > ".localhost".len() && !lower.starts_with('.'))
        || lower == "[::1]"
        || is_ipv4_loopback(&lower)
}

fn is_ipv4_loopback(host: &str) -> bool {
    let mut parts = host.split('.');
    if parts.next() != Some("127") {
        return false;
    }
    let mut count = 1;
    for part in parts {
        count += 1;
        if count > 4 || part.is_empty() || part.len() > 3 || part.parse::<u8>().is_err() {
            return false;
        }
    }
    count == 4
}

/// Partition scope for site permission decisions (`site-permissions.ts` `DecisionScope`):
/// - `Profile`: persistent decisions for `ContextKind::Default` (`plugin.browser/rune.sitePermissions`).
/// - `Session(server_id)`: in-memory decisions for one `ContextKind::RemoteServer` kept until app quit,
///   never persisted to disk and never crossing into `Profile` or other remote servers.
/// - `VisitOnly`: `Preview`, `Artifact`, and `Private` tabs never read or persist kept decisions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionScope {
    Profile,
    Session(String),
    VisitOnly,
}

impl PermissionScope {
    pub fn from_context(context: &ContextKind) -> Self {
        match context {
            ContextKind::Default => Self::Profile,
            ContextKind::RemoteServer { server_id, .. } => {
                let sid = server_id.trim();
                if sid.is_empty() { Self::VisitOnly } else { Self::Session(sid.to_owned()) }
            }
            ContextKind::Preview { .. }
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
    session_decisions: std::collections::HashMap<(String, String, PermissionKind), SiteSetting>,
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
            session_decisions: std::collections::HashMap::new(),
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

    pub fn get(&self, origin: &str, permission: PermissionKind) -> Option<SiteSetting> {
        self.decisions
            .iter()
            .find(|d| d.origin == origin && d.permission == permission)
            .map(|d| d.setting)
    }

    /// Looks up a decision strictly inside `scope` (`Profile` persisted, `Session` in-RAM per remote
    /// server, `VisitOnly` never inherits).
    pub fn get_scoped(
        &self,
        scope: &PermissionScope,
        origin: &str,
        permission: PermissionKind,
    ) -> Option<SiteSetting> {
        match scope {
            PermissionScope::Profile => self.get(origin, permission),
            PermissionScope::Session(server_id) => self
                .session_decisions
                .get(&(server_id.clone(), origin.to_owned(), permission))
                .copied(),
            PermissionScope::VisitOnly => None,
        }
    }

    pub fn set(&mut self, origin: String, permission: PermissionKind, setting: SiteSetting) {
        if origin.is_empty() || origin.to_ascii_lowercase().starts_with("oc-preview:") || !trustworthy_origin(&origin) {
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

    /// Records a decision strictly inside `scope`: `Profile` persists to disk; `Session(server_id)`
    /// stays in RAM until app quit; `VisitOnly` is ignored.
    pub fn set_scoped(
        &mut self,
        scope: &PermissionScope,
        origin: String,
        permission: PermissionKind,
        setting: SiteSetting,
    ) {
        if origin.is_empty() || origin.to_ascii_lowercase().starts_with("oc-preview:") || !trustworthy_origin(&origin) {
            return;
        }
        match scope {
            PermissionScope::Profile => self.set(origin, permission, setting),
            PermissionScope::Session(server_id) => {
                self.session_decisions.insert((server_id.clone(), origin, permission), setting);
            }
            PermissionScope::VisitOnly => {}
        }
    }

    /// Drops all RAM-only session permission grants/denials for `server_id` (called when that
    /// remote server disconnects or its isolated context is dropped).
    pub fn clear_session(&mut self, server_id: &str) {
        let sid = server_id.trim();
        if !sid.is_empty() {
            self.session_decisions.retain(|(k, _, _), _| k != sid);
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

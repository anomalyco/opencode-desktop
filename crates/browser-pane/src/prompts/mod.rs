//! Explicit state machines for a browser tab's in-pane prompts, permissions, HTTP auth, and
//! pop-up blocking, matching upstream `#63002` (`packages/gui-plugins/src/browser/{dialog-machine,
//! prompt-machine,activation-delay,site-permissions,site-auth}.ts`).
//!
//! Every interactive button waits [`ACTIVATION_DELAY_MS`] (500 ms) after appearing or changing
//! before accepting presses (`InputGuard::Guarded` -> `InputGuard::Live`), so clicks or held keys
//! already in flight cannot accidentally accept a dialog, grant a permission, or open a pop-up.
//! Queued permission and auth gates coalesce concurrent requests by key so every held CEF callback
//! settles exactly once.

mod host;
mod store;

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::time::Duration;

use rune::time::Instant;

#[cfg(feature = "engine")]
pub(crate) use store::scrub_chromium_preferences_for_older_engine;
pub use store::{
    ACTIVATION_DELAY_MS, AGENT_PERMISSION_TIMEOUT_MS, InputGuard, MAX_SIGN_IN_ATTEMPTS,
    PROMPT_TEXT_MAX, PermissionScope, PermissionStore, REALM_TEXT_MAX, REPEAT_DIALOG_MS,
    SiteDecision, SitePermissions, SiteSetting, sanitize_prompt_text,
};

use crate::protocol::{AuthChallenge, DialogKind, PageDialog, PermissionKind, PermissionPrompt};

/// A tab's JavaScript dialog state (`dialog-machine.ts`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogGate {
    Idle { stopped: bool, closed_at: Option<Instant> },
    Open {
        dialog: PageDialog,
        queue: VecDeque<PageDialog>,
        repeat: bool,
        input: InputGuard,
    },
}

impl Default for DialogGate {
    fn default() -> Self {
        Self::Idle { stopped: false, closed_at: None }
    }
}

impl DialogGate {
    pub fn open(&self) -> Option<&PageDialog> {
        match self {
            Self::Open { dialog, .. } => Some(dialog),
            Self::Idle { .. } => None,
        }
    }

    pub fn armed(&self) -> bool {
        matches!(self, Self::Open { input: InputGuard::Live, .. })
    }

    pub fn repeat(&self) -> bool {
        matches!(self, Self::Open { repeat: true, .. })
    }

    pub fn waiting_count(&self) -> usize {
        match self {
            Self::Open { queue, .. } => queue.len(),
            Self::Idle { .. } => 0,
        }
    }

    pub fn wake_at(&self) -> Option<Instant> {
        match self {
            Self::Open { input, .. } => input.wake_at(),
            Self::Idle { .. } => None,
        }
    }

    pub fn opened(&mut self, mut dialog: PageDialog, now: Instant) -> Option<(u64, bool, String)> {
        let scripted = dialog.kind != DialogKind::Leave;
        dialog.message = sanitize_prompt_text(&dialog.message, PROMPT_TEXT_MAX);
        dialog.default_text = sanitize_prompt_text(&dialog.default_text, PROMPT_TEXT_MAX);
        match self {
            Self::Idle { stopped: true, .. } if scripted => {
                Some((dialog.id, false, String::new()))
            }
            Self::Idle { closed_at, .. } => {
                let repeat = scripted
                    && closed_at.is_some_and(|at| {
                        now.saturating_duration_since(at) <= Duration::from_millis(REPEAT_DIALOG_MS)
                    });
                *self = Self::Open {
                    dialog,
                    queue: VecDeque::new(),
                    repeat,
                    input: InputGuard::arm(now),
                };
                None
            }
            Self::Open { queue, .. } => {
                queue.push_back(dialog);
                None
            }
        }
    }

    /// Answers the active dialog, returning the engine reply plus any auto-dismissed queued dialogs
    /// if the user chose to stop further scripted dialogs from this document.
    pub fn answer(
        &mut self,
        id: u64,
        accept: bool,
        text: String,
        stop: bool,
        now: Instant,
    ) -> Vec<(u64, bool, String)> {
        let Self::Open { dialog, queue, repeat, .. } = self else { return Vec::new() };
        if dialog.id != id {
            return Vec::new();
        }
        let kind = dialog.kind;
        let was_repeat = *repeat;
        let stopped = stop && was_repeat && kind != DialogKind::Leave;
        let reply = if kind == DialogKind::Prompt && accept { text } else { String::new() };
        let mut replies = vec![(id, accept, reply)];
        let mut remaining = std::mem::take(queue);
        let closed_at = (kind != DialogKind::Leave).then_some(now);
        while let Some(next) = remaining.pop_front() {
            if stopped && next.kind != DialogKind::Leave {
                replies.push((next.id, false, String::new()));
                continue;
            }
            let next_repeat = next.kind != DialogKind::Leave && closed_at.is_some();
            *self = Self::Open {
                dialog: next,
                queue: remaining,
                repeat: next_repeat,
                input: InputGuard::arm(now),
            };
            return replies;
        }
        *self = Self::Idle { stopped, closed_at };
        replies
    }

    /// A new document committed in the main frame: allow dialogs again (`dialog-machine.ts:182`).
    pub fn on_document_committed(&mut self) {
        if let Self::Idle { stopped, closed_at } = self {
            *stopped = false;
            *closed_at = None;
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        match self {
            Self::Open { input, .. } => input.tick(now),
            Self::Idle { .. } => false,
        }
    }
}

/// How the user answered a permission prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionDecision {
    AllowOnce,
    AllowAlways,
    BlockOnce,
    BlockAlways,
}

/// One queued site permission prompt; concurrent requests with the same `(origin, kinds, agent)`
/// coalesce into `extra_ids` so every held CEF callback settles when answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedPermission {
    pub key: String,
    pub request: PermissionPrompt,
    pub extra_ids: Vec<u64>,
    pub asked: u32,
    pub agent_driven: bool,
    pub input: InputGuard,
    pub expires_at: Option<Instant>,
}

impl QueuedPermission {
    fn all_ids(&self) -> Vec<u64> {
        let mut ids = Vec::with_capacity(1 + self.extra_ids.len());
        ids.push(self.request.id);
        ids.extend_from_slice(&self.extra_ids);
        ids
    }
}

/// A tab's queued site permission prompt state (`site-permissions.ts`, `prompt-machine.ts`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum PermissionGate {
    #[default]
    Idle,
    Prompting { queue: VecDeque<QueuedPermission> },
}

fn permission_key(request: &PermissionPrompt, agent_driven: bool) -> String {
    let mut key = request.origin.clone();
    key.push('\n');
    for (i, kind) in request.kinds.iter().enumerate() {
        if i > 0 {
            key.push(',');
        }
        key.push_str(kind.as_str());
    }
    if agent_driven {
        key.push_str("\nagent");
    }
    key
}

impl PermissionGate {
    pub fn active(&self) -> Option<&QueuedPermission> {
        match self {
            Self::Prompting { queue } => queue.front(),
            Self::Idle => None,
        }
    }

    pub fn armed(&self) -> bool {
        self.active().is_some_and(|q| q.input.is_live())
    }

    pub fn waiting_count(&self) -> usize {
        match self {
            Self::Prompting { queue } => queue.len().saturating_sub(1),
            Self::Idle => 0,
        }
    }

    pub fn wake_at(&self) -> Option<Instant> {
        let Self::Prompting { queue } = self else { return None };
        let head_guard = queue.front().and_then(|q| q.input.wake_at());
        queue
            .iter()
            .filter_map(|q| q.expires_at)
            .chain(head_guard)
            .min()
    }

    /// Evaluates an incoming permission request against scope-partitioned kept + visit decisions.
    /// Returns `Some((allow, dismiss, persist))` to answer immediately without showing a bar, else enqueues.
    pub fn evaluate(
        &mut self,
        request: PermissionPrompt,
        agent_driven: bool,
        scope: &PermissionScope,
        kept: &SitePermissions,
        visit: &HashMap<(String, PermissionKind), SiteSetting>,
        now: Instant,
    ) -> Option<(bool, bool, bool)> {
        if request.kinds.is_empty() || request.origin != request.top_origin {
            return Some((false, true, false));
        }
        let is_profile = matches!(scope, PermissionScope::Profile);
        let mut all_allowed = true;
        let mut all_kept = true;
        for &kind in &request.kinds {
            let kept_val = kept.get_scoped(scope, &request.origin, kind);
            let visit_val = visit.get(&(request.origin.clone(), kind)).copied();
            if kept_val == Some(SiteSetting::Block) {
                return Some((false, false, is_profile));
            }
            if visit_val == Some(SiteSetting::Block) {
                return Some((false, true, false));
            }
            if kept_val != Some(SiteSetting::Allow) {
                all_kept = false;
            }
            let allowed = if agent_driven {
                // Agent tabs never inherit kept user permissions (`site-permissions.ts`).
                false
            } else {
                kept_val == Some(SiteSetting::Allow) || visit_val == Some(SiteSetting::Allow)
            };
            if !allowed {
                all_allowed = false;
            }
        }
        if all_allowed {
            return Some((true, false, all_kept && is_profile));
        }
        let key = permission_key(&request, agent_driven);
        let expires_at = agent_driven.then(|| now + Duration::from_millis(AGENT_PERMISSION_TIMEOUT_MS));
        match self {
            Self::Idle => {
                let mut queue = VecDeque::new();
                queue.push_back(QueuedPermission {
                    key,
                    request,
                    extra_ids: Vec::new(),
                    asked: 1,
                    agent_driven,
                    input: InputGuard::arm(now),
                    expires_at,
                });
                *self = Self::Prompting { queue };
            }
            Self::Prompting { queue } => {
                if let Some((idx, item)) = queue.iter_mut().enumerate().find(|(_, q)| q.key == key) {
                    item.extra_ids.push(request.id);
                    item.asked = item.asked.saturating_add(1);
                    if idx == 0 {
                        item.input = InputGuard::arm(now);
                    }
                } else {
                    queue.push_back(QueuedPermission {
                        key,
                        request,
                        extra_ids: Vec::new(),
                        asked: 1,
                        agent_driven,
                        input: InputGuard::arm(now),
                        expires_at,
                    });
                }
            }
        }
        None
    }

    pub fn take_active(&mut self, id: u64, now: Instant) -> Option<(QueuedPermission, Vec<u64>)> {
        let Self::Prompting { queue } = self else { return None };
        if queue.front().is_none_or(|q| q.request.id != id) {
            return None;
        }
        let item = queue.pop_front()?;
        let ids = item.all_ids();
        if let Some(next) = queue.front_mut() {
            next.input = InputGuard::arm(now);
        } else {
            *self = Self::Idle;
        }
        Some((item, ids))
    }

    pub fn dismiss_by_id(&mut self, id: u64, now: Instant) -> bool {
        let Self::Prompting { queue } = self else { return false };
        let mut changed = false;
        let mut remove_idx = None;
        for (idx, item) in queue.iter_mut().enumerate() {
            if item.request.id == id {
                changed = true;
                if let Some(next_id) = item.extra_ids.first().copied() {
                    item.extra_ids.remove(0);
                    item.request.id = next_id;
                } else {
                    remove_idx = Some(idx);
                }
                break;
            }
            if let Some(pos) = item.extra_ids.iter().position(|&eid| eid == id) {
                item.extra_ids.remove(pos);
                changed = true;
                break;
            }
        }
        if let Some(idx) = remove_idx {
            queue.remove(idx);
            if queue.is_empty() {
                *self = Self::Idle;
            } else if idx == 0
                && let Some(next) = queue.front_mut()
            {
                next.input = InputGuard::arm(now);
            }
        }
        changed
    }

    pub fn drain_all_ids(&mut self) -> Vec<u64> {
        let Self::Prompting { queue } = std::mem::take(self) else { return Vec::new() };
        queue.into_iter().flat_map(|q| q.all_ids()).collect()
    }

    pub fn tick(&mut self, now: Instant) -> (bool, Vec<u64>) {
        let Self::Prompting { queue } = self else { return (false, Vec::new()) };
        let mut expired = Vec::new();
        let mut head_changed = false;
        let mut idx = 0;
        while idx < queue.len() {
            if queue[idx].expires_at.is_some_and(|exp| now >= exp) {
                if let Some(item) = queue.remove(idx) {
                    if idx == 0 {
                        head_changed = true;
                    }
                    expired.extend(item.all_ids());
                }
            } else {
                idx += 1;
            }
        }
        if queue.is_empty() {
            *self = Self::Idle;
            return (!expired.is_empty(), expired);
        }
        let mut changed = !expired.is_empty();
        if let Some(head) = queue.front_mut() {
            if head_changed {
                head.input = InputGuard::arm(now);
            } else {
                changed |= head.input.tick(now);
            }
        }
        (changed, expired)
    }
}

/// One queued HTTP auth challenge; concurrent challenges for the same `(origin, scheme)` coalesce
/// into `extra_ids` so every held CEF `AuthCallback` settles when answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedAuth {
    pub key: String,
    pub challenge: AuthChallenge,
    pub extra_ids: Vec<u64>,
    pub attempt: u8,
    pub asked: u32,
    pub input: InputGuard,
}

impl QueuedAuth {
    fn all_ids(&self) -> Vec<u64> {
        let mut ids = Vec::with_capacity(1 + self.extra_ids.len());
        ids.push(self.challenge.id);
        ids.extend_from_slice(&self.extra_ids);
        ids
    }
}

/// A tab's queued HTTP authentication prompt state (`site-auth.ts`, `prompt-machine.ts`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum AuthGate {
    #[default]
    Idle,
    Prompting { queue: VecDeque<QueuedAuth> },
}

impl AuthGate {
    pub fn active(&self) -> Option<&QueuedAuth> {
        match self {
            Self::Prompting { queue } => queue.front(),
            Self::Idle => None,
        }
    }

    pub fn armed(&self) -> bool {
        self.active().is_some_and(|a| a.input.is_live())
    }

    pub fn failed(&self) -> bool {
        self.active().is_some_and(|a| a.attempt > 1)
    }

    pub fn waiting_count(&self) -> usize {
        match self {
            Self::Prompting { queue } => queue.len().saturating_sub(1),
            Self::Idle => 0,
        }
    }

    pub fn wake_at(&self) -> Option<Instant> {
        self.active().and_then(|a| a.input.wake_at())
    }

    pub fn push(&mut self, key: String, challenge: AuthChallenge, attempt: u8, now: Instant) {
        match self {
            Self::Idle => {
                let mut queue = VecDeque::new();
                queue.push_back(QueuedAuth {
                    key,
                    challenge,
                    extra_ids: Vec::new(),
                    attempt,
                    asked: 1,
                    input: InputGuard::arm(now),
                });
                *self = Self::Prompting { queue };
            }
            Self::Prompting { queue } => {
                if let Some((idx, item)) = queue.iter_mut().enumerate().find(|(_, a)| a.key == key) {
                    item.extra_ids.push(challenge.id);
                    item.attempt = item.attempt.max(attempt);
                    item.asked = item.asked.saturating_add(1);
                    if idx == 0 {
                        item.input = InputGuard::arm(now);
                    }
                } else {
                    queue.push_back(QueuedAuth {
                        key,
                        challenge,
                        extra_ids: Vec::new(),
                        attempt,
                        asked: 1,
                        input: InputGuard::arm(now),
                    });
                }
            }
        }
    }

    pub fn take_active(&mut self, id: u64, now: Instant) -> Option<(QueuedAuth, Vec<u64>)> {
        let Self::Prompting { queue } = self else { return None };
        if queue.front().is_none_or(|a| a.challenge.id != id) {
            return None;
        }
        let item = queue.pop_front()?;
        let ids = item.all_ids();
        if let Some(next) = queue.front_mut() {
            next.input = InputGuard::arm(now);
        } else {
            *self = Self::Idle;
        }
        Some((item, ids))
    }

    pub fn drain_all_ids(&mut self) -> Vec<u64> {
        let Self::Prompting { queue } = std::mem::take(self) else { return Vec::new() };
        queue.into_iter().flat_map(|a| a.all_ids()).collect()
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        match self {
            Self::Prompting { queue } => queue.front_mut().is_some_and(|a| a.input.tick(now)),
            Self::Idle => false,
        }
    }
}

/// A tab's blocked pop-up banner state (`activation-delay.ts`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum PopupGate {
    #[default]
    Idle,
    Blocked {
        url: String,
        count: u32,
        input: InputGuard,
    },
}

impl PopupGate {
    pub fn blocked(&self) -> Option<(&str, u32, bool)> {
        match self {
            Self::Blocked { url, count, input } => Some((url.as_str(), *count, input.is_live())),
            Self::Idle => None,
        }
    }

    pub fn record(&mut self, url: String, now: Instant) {
        let count = match self {
            Self::Blocked { count, .. } => count.saturating_add(1),
            Self::Idle => 1,
        };
        *self = Self::Blocked { url, count, input: InputGuard::arm(now) };
    }

    pub fn dismiss(&mut self) -> bool {
        let had = !matches!(self, Self::Idle);
        *self = Self::Idle;
        had
    }

    pub fn wake_at(&self) -> Option<Instant> {
        match self {
            Self::Blocked { input, .. } => input.wake_at(),
            Self::Idle => None,
        }
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        match self {
            Self::Blocked { input, .. } => input.tick(now),
            Self::Idle => false,
        }
    }
}

/// Who is driving a controlled browser tab (for honest UI copy in permission and notice bars).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TabDriver {
    /// An AI agent (native MCP browser RPC or built-in browser MCP tools).
    Agent,
    /// A third-party Luau plugin, carrying its sanitized display name.
    Plugin(String),
}

impl TabDriver {
    pub fn plugin(name: &str) -> Self {
        let clean: String = sanitize_prompt_text(name, 64)
            .chars()
            .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
            .collect();
        let trimmed = clean.trim();
        Self::Plugin(if trimmed.is_empty() { "Plugin".to_owned() } else { trimmed.to_owned() })
    }
}

/// All prompt, permission, auth, and pop-up states owned by one `BrowserTab`.
#[derive(Clone, Debug, Default)]
pub struct TabPromptState {
    pub agent_driven: bool,
    /// Honest attribution for why this tab is controlled (`Agent` vs third-party `Plugin(name)`).
    pub driver: Option<TabDriver>,
    pub dialog: DialogGate,
    pub permission: PermissionGate,
    pub auth: AuthGate,
    pub popup: PopupGate,
    /// Per-visit temporary permission choices (cleared on top-level origin change or reset).
    pub visit_permissions: HashMap<(String, PermissionKind), SiteSetting>,
    /// Per-visit HTTP auth attempt counts keyed by `origin\nscheme` (`site-auth.ts`).
    pub auth_attempts: HashMap<String, u8>,
    /// Per-visit dismissed HTTP auth sites keyed by `origin\nscheme`.
    pub auth_dismissed: HashSet<String>,
    /// Last origin where an agent tab hit an HTTP auth challenge (`sign_in_required`).
    pub agent_auth_required: Option<String>,
    /// Count of blocked native file choosers on agent-driven tabs.
    pub file_chooser_blocked: u64,
    /// True after a sign-in prompt or file picker was blocked on this document (dismissible UI notice).
    pub blocked_guard_notice: bool,
}

impl TabPromptState {
    pub fn has_blocking_modal(&self) -> bool {
        self.dialog.open().is_some() || self.auth.active().is_some()
    }

    /// The tab waits on the user: a page dialog, a permission request or a sign-in (official
    /// `PromptView`, `packages/gui-plugins/src/browser/ipc.ts:97-145`). A blocked pop-up is not one.
    pub fn waiting_on_user(&self) -> bool {
        self.has_blocking_modal() || self.permission.active().is_some()
    }

    pub fn wake_at(&self) -> Option<Instant> {
        [
            self.dialog.wake_at(),
            self.permission.wake_at(),
            self.auth.wake_at(),
            self.popup.wake_at(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    pub fn reset_for_engine(&mut self) {
        self.dialog.reset();
        self.permission = PermissionGate::Idle;
        self.auth = AuthGate::Idle;
        self.popup = PopupGate::Idle;
    }

    /// Context switched (`reopen_tab_as_preview` / `reopen_tab_as_web`): clears visit-scoped
    /// permissions, auth state, and notices while keeping `agent_driven` and `driver`.
    pub fn reset_for_context_switch(&mut self) {
        self.visit_permissions.clear();
        self.auth_attempts.clear();
        self.auth_dismissed.clear();
        self.agent_auth_required = None;
        self.blocked_guard_notice = false;
        self.reset_for_engine();
    }

    /// User started a navigation/reload/back/forward: dismissed auth sites ask again (`prompt-machine.ts:182`).
    pub fn on_user_nav(&mut self) {
        self.auth_attempts.clear();
        self.auth_dismissed.clear();
        self.popup.dismiss();
    }

    /// Top-level URL changed: allow scripted dialogs on the new document, clear visit permissions
    /// and auth/permission prompts on origin change (`prompt-machine.ts:189`, `dialog-machine.ts:182`).
    /// Returns `(cancel_permission_ids, cancel_auth_ids)` to settle in the engine.
    pub fn on_document_committed(&mut self, old_url: &str, new_url: &str) -> (Vec<u64>, Vec<u64>) {
        self.dialog.on_document_committed();
        self.popup.dismiss();
        self.agent_auth_required = None;
        self.blocked_guard_notice = false;
        let same_origin = crate::policy::destination_origin(old_url)
            == crate::policy::destination_origin(new_url);
        if same_origin {
            return (Vec::new(), Vec::new());
        }
        self.visit_permissions.clear();
        self.auth_attempts.clear();
        self.auth_dismissed.clear();
        (self.permission.drain_all_ids(), self.auth.drain_all_ids())
    }

    pub fn auth_challenge(&mut self, mut challenge: AuthChallenge, now: Instant) -> Option<u64> {
        let key = format!("{}\n{}", challenge.origin, challenge.scheme.to_ascii_lowercase());
        if self.auth_dismissed.contains(&key) {
            return Some(challenge.id);
        }
        let next = self.auth_attempts.get(&key).copied().unwrap_or(0).saturating_add(1);
        if next > MAX_SIGN_IN_ATTEMPTS {
            return Some(challenge.id);
        }
        self.auth_attempts.insert(key.clone(), next);
        challenge.realm = sanitize_prompt_text(&challenge.realm, REALM_TEXT_MAX);
        self.auth.push(key, challenge, next, now);
        None
    }

    /// Zero-allocation state hash for `Dep::Browser` invalidation (`ours.md` §3.2).
    pub fn hash_state(&self, h: &mut impl Hasher) {
        self.agent_driven.hash(h);
        self.driver.hash(h);
        self.blocked_guard_notice.hash(h);
        match &self.dialog {
            DialogGate::Idle { stopped, .. } => (0u8, *stopped).hash(h),
            DialogGate::Open { dialog, queue, repeat, input } => {
                (1u8, dialog.id, dialog.kind as u8, queue.len(), *repeat, input.is_live()).hash(h);
            }
        }
        match &self.permission {
            PermissionGate::Idle => 0u8.hash(h),
            PermissionGate::Prompting { queue } => {
                if let Some(q) = queue.front() {
                    (1u8, q.request.id, q.asked, queue.len(), q.agent_driven, q.input.is_live()).hash(h);
                }
            }
        }
        match &self.auth {
            AuthGate::Idle => 0u8.hash(h),
            AuthGate::Prompting { queue } => {
                if let Some(a) = queue.front() {
                    (1u8, a.challenge.id, a.attempt, a.asked, queue.len(), a.input.is_live()).hash(h);
                }
            }
        }
        match &self.popup {
            PopupGate::Idle => 0u8.hash(h),
            PopupGate::Blocked { url, count, input } => (1u8, url, *count, input.is_live()).hash(h),
        }
    }
}

/// A managed pop-up requested by a user-activated `window.open` / `target="_blank"` link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PopupRequest {
    pub opener_tab_id: u64,
    pub url: String,
    pub background: bool,
}

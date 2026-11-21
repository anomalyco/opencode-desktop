//! CEF prompt, permission, pop-up, HTTP auth, and file-chooser handlers for one managed tab
//! (`packages/gui-plugins/src/browser/{page-prompts,dialog-machine,site-permissions,site-auth,file-choosers}.ts`).
//!
//! Security invariants:
//! - Pop-ups never spawn unmanaged CEF windows: `on_before_popup` always returns `1`. Disallowed
//!   schemes, `file://`, and credentialed URLs emit `EngineMsg::Refused`; user-activated `http(s)` /
//!   `about:blank` pop-ups emit `EngineMsg::PopupOpen`; non-user-gesture pop-ups emit
//!   `EngineMsg::PopupBlocked`.
//! - Permissions outside `{Camera, Microphone, Location, Notifications, ClipboardRead}`, or from
//!   cross-origin frames or insecure origins, or when connected to an older app (`!enabled`), are
//!   denied immediately (never left hanging on `"prompt"`).
//! - HTTP auth only accepts same-origin/top-level `http(s)` challenges with Basic/Digest/NTLM/
//!   Negotiate; agent-driven tabs cancel immediately and emit `AuthAgentRequired` (`sign_in_required`).
//! - Native file choosers on agent-driven tabs are cancelled immediately (`FileChooserBlocked`).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cef::rc::Rc as _;
use cef::{
    AuthCallback, Browser, BrowserSettings, CefString, CefStringList, Client, DialogHandler,
    DictionaryValue, FileDialogCallback, FileDialogMode, Frame, ImplAuthCallback, ImplBrowser, ImplDialogHandler,
    ImplFileDialogCallback, ImplFrame, ImplJsdialogCallback, ImplJsdialogHandler,
    ImplLifeSpanHandler, ImplMediaAccessCallback, ImplPermissionHandler,
    ImplPermissionPromptCallback, JsdialogCallback, JsdialogHandler, JsdialogType, LifeSpanHandler,
    MediaAccessCallback, PermissionHandler, PermissionPromptCallback, PermissionRequestResult,
    PopupFeatures, RequestContext, WindowInfo, WindowOpenDisposition, WrapDialogHandler,
    WrapJsdialogHandler, WrapLifeSpanHandler, WrapPermissionHandler, sys, wrap_dialog_handler,
    wrap_jsdialog_handler, wrap_life_span_handler, wrap_permission_handler,
};

use crate::output::Output;
use crate::wire::{
    AuthChallenge, DialogKind, EngineMsg, PageDialog, PermissionKind, PermissionPrompt,
    RemoteProxyConfig, TabSnapshot,
};

/// Shared across all tabs in the engine: whether the connected app handles prompts (`Hello.prompts`)
/// and a monotonic prompt ID generator.
#[derive(Clone, Default)]
pub struct PromptShared {
    pub enabled: Arc<AtomicBool>,
    next_id: Arc<AtomicU64>,
}

impl PromptShared {
    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed) + 1
    }
}

enum HeldPermission {
    Media {
        prompt: PermissionPrompt,
        requested_bits: u32,
        callback: MediaAccessCallback,
    },
    Prompt {
        prompt: PermissionPrompt,
        cef_prompt_id: u64,
        callback: PermissionPromptCallback,
    },
}

/// Per-tab held CEF prompt callbacks and agent-driven isolation flag.
#[derive(Default)]
pub struct TabPrompts {
    pub agent_driven: AtomicBool,
    /// One pop-up per input event, as Chromium's blocker allows per user activation (it is off:
    /// `--disable-popup-blocking`). Set only by the person's click/key; the agent's `Input.*` clears
    /// it, so a page cannot open a tab off the agent's input (official `window-open.ts:78-80,106`).
    pub popup_credit: AtomicBool,
    /// Set by the agent's `Input.*`, cleared by the person's next press or key: until then the page
    /// gets no native context menu (official `chromium.ts:2500-2501`, `nativeMenu`).
    pub menu_suppressed: AtomicBool,
    /// The person's own click or key on a controlled tab hands its downloads back to them until
    /// the agent acts again (official `policy.ts:121-125, 175-177` `control: "reclaimed"`).
    pub user_reclaimed: AtomicBool,
    /// Authoritative OS PID of this tab's current main-frame renderer process (`0` until reported
    /// by `opencode-browser-engine-helper`). Used to isolate browser-wide CDP traces to this tab.
    pub renderer_pid: AtomicU32,
    /// Renderer OS PID locked when `Tracing.start` succeeded on this tab (`0` when not tracing).
    pub trace_pid: AtomicU32,
    pub proxy: Mutex<Option<RemoteProxyConfig>>,
    dialogs: Mutex<Vec<(PageDialog, JsdialogCallback)>>,
    permissions: Mutex<Vec<HeldPermission>>,
    auth: Mutex<Vec<(AuthChallenge, AuthCallback)>>,
    pending_top_url: Mutex<Option<String>>,
    /// Tracks in-flight (pre-commit) `chrome-extension://` sub-frame navigations keyed by CEF
    /// frame identifier until commit, cancel/error, main-frame nav, or renderer crash — NO
    /// time-out that fails open (`review-rd274.md` Round-4 ruling).
    pending_ext_subframes: Mutex<HashSet<String>>,
}

impl TabPrompts {
    /// The agent sent the page input: no pop-up and no native menu off it until the person's own press or key.
    pub fn agent_input(&self) {
        self.popup_credit.store(false, Ordering::Release);
        self.menu_suppressed.store(true, Ordering::Release);
    }

    /// Whether the agent owns the tab's native dialogs now: driven by the agent and not taken back
    /// by the person's own input since its last command (official `policy.ts:175-177`
    /// `activityOwner`; `control: "reclaimed"` is `user_reclaimed`). File pickers follow this, not
    /// `agent_driven`, which stays set for the restrictions that outlive a takeover (passkeys,
    /// inherited permissions: `policy.ts:132-135`).
    pub fn agent_owns_dialogs(&self) -> bool {
        self.agent_driven.load(Ordering::Acquire) && !self.user_reclaimed.load(Ordering::Acquire)
    }

    pub fn set_pending_top_url(&self, url: Option<String>) {
        *self.pending_top_url.lock().unwrap() = url;
    }

    pub fn pending_top_url(&self) -> Option<String> {
        self.pending_top_url.lock().unwrap().clone()
    }

    pub fn mark_pending_ext_subframe(&self, frame_id: String) {
        if !frame_id.is_empty() {
            self.pending_ext_subframes.lock().unwrap().insert(frame_id);
        }
    }

    pub fn clear_pending_ext_subframe(&self, frame_id: &str) {
        if !frame_id.is_empty() {
            self.pending_ext_subframes.lock().unwrap().remove(frame_id);
        }
    }

    pub fn clear_all_ext_frame_tracking(&self) {
        self.pending_ext_subframes.lock().unwrap().clear();
    }

    /// Retains only pending extension sub-frames whose `RenderFrameHost` is still alive and has
    /// not yet committed as an extension scheme (zero fail-open TTL). Returns `true` if any
    /// pre-commit extension sub-frame navigation is still in flight.
    pub fn has_pending_ext_subframes(&self, live_frames: &HashMap<String, String>) -> bool {
        let mut set = self.pending_ext_subframes.lock().unwrap();
        set.retain(|id| {
            live_frames
                .get(id)
                .is_some_and(|url| !crate::extensions::manifest::is_extension_scheme(url))
        });
        !set.is_empty()
    }


    pub fn answer_dialog(&self, id: u64, accept: bool, text: &str) -> bool {
        let cb = {
            let mut list = self.dialogs.lock().unwrap();
            let Some(pos) = list.iter().position(|(d, _)| d.id == id) else { return false };
            list.remove(pos).1
        };
        cb.cont(i32::from(accept), Some(&CefString::from(text)));
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub fn answer_permission(
        &self,
        tab_id: u64,
        id: u64,
        allow: bool,
        dismiss: bool,
        persist: bool,
        ctx_scope: Option<&str>,
        req_ctx: Option<&RequestContext>,
        tracker: &crate::permissions::PermissionTracker,
    ) -> bool {
        let entry = {
            let mut list = self.permissions.lock().unwrap();
            let Some(pos) = list.iter().position(|p| match p {
                HeldPermission::Media { prompt, .. } | HeldPermission::Prompt { prompt, .. } => {
                    prompt.id == id
                }
            }) else {
                return false;
            };
            list.remove(pos)
        };
        let is_default = ctx_scope.is_none();
        let effective_persist = persist && is_default;
        match entry {
            HeldPermission::Media { prompt, requested_bits, callback } => {
                if allow {
                    callback.cont(requested_bits);
                } else {
                    callback.cancel();
                }
                if let Some(ctx) = req_ctx {
                    for &kind in &prompt.kinds {
                        if effective_persist {
                            crate::permissions::set_content_setting(ctx, &prompt.origin, kind, allow);
                            tracker.record_persistent(&prompt.origin, kind, allow);
                        } else if allow {
                            crate::permissions::set_content_setting(ctx, &prompt.origin, kind, true);
                            tracker.record_visit(
                                ctx_scope.map(str::to_owned),
                                tab_id,
                                &prompt.origin,
                                kind,
                                ctx.clone(),
                            );
                        }
                    }
                }
            }
            HeldPermission::Prompt { prompt, callback, .. } => {
                let res = if allow {
                    sys::cef_permission_request_result_t::CEF_PERMISSION_RESULT_ACCEPT
                } else if effective_persist && !dismiss {
                    sys::cef_permission_request_result_t::CEF_PERMISSION_RESULT_DENY
                } else {
                    sys::cef_permission_request_result_t::CEF_PERMISSION_RESULT_DISMISS
                };
                if let Some(ctx) = req_ctx {
                    for &kind in &prompt.kinds {
                        if effective_persist && (allow || !dismiss) {
                            tracker.record_persistent(&prompt.origin, kind, allow);
                        } else if allow {
                            tracker.record_visit(
                                ctx_scope.map(str::to_owned),
                                tab_id,
                                &prompt.origin,
                                kind,
                                ctx.clone(),
                            );
                        }
                    }
                }
                callback.cont(res.into());
            }
        }
        true
    }

    pub fn answer_auth(&self, id: u64, username: Option<&str>, password: Option<&str>) -> bool {
        let cb = {
            let mut list = self.auth.lock().unwrap();
            let Some(pos) = list.iter().position(|(c, _)| c.id == id) else { return false };
            list.remove(pos).1
        };
        match (username, password) {
            (Some(u), Some(p)) => cb.cont(Some(&CefString::from(u)), Some(&CefString::from(p))),
            _ => cb.cancel(),
        }
        true
    }

    pub fn clear_auth_on_nav(&self, tab_id: u64, out_tx: &Output) {
        let old = std::mem::take(&mut *self.auth.lock().unwrap());
        let had = !old.is_empty();
        for (_, cb) in old {
            cb.cancel();
        }
        if had {
            let _ = out_tx.send(EngineMsg::AuthReset { tab_id });
        }
    }

    pub fn clear_permissions_on_origin_change(&self, tab_id: u64, new_origin: Option<&str>, out_tx: &Output) {
        let mut removed = Vec::new();
        {
            let mut list = self.permissions.lock().unwrap();
            let mut idx = 0;
            while idx < list.len() {
                let origin_matches = match &list[idx] {
                    HeldPermission::Media { prompt, .. } | HeldPermission::Prompt { prompt, .. } => {
                        new_origin == Some(prompt.origin.as_str())
                    }
                };
                if origin_matches {
                    idx += 1;
                } else {
                    removed.push(list.remove(idx));
                }
            }
        }
        for p in removed {
            let id = match p {
                HeldPermission::Media { prompt, callback, .. } => {
                    callback.cancel();
                    prompt.id
                }
                HeldPermission::Prompt { prompt, callback, .. } => {
                    callback.cont(sys::cef_permission_request_result_t::CEF_PERMISSION_RESULT_DISMISS.into());
                    prompt.id
                }
            };
            let _ = out_tx.send(EngineMsg::PermissionDismissed { tab_id, id });
        }
    }

    pub fn reset_on_crash(&self, tab_id: u64, out_tx: &Output) {
        self.renderer_pid.store(0, Ordering::Release);
        self.set_pending_top_url(None);
        self.clear_all_ext_frame_tracking();
        let dialogs = std::mem::take(&mut *self.dialogs.lock().unwrap());
        let had_dialogs = !dialogs.is_empty();
        for (_, cb) in dialogs {
            cb.cont(0, None);
        }
        if had_dialogs {
            let _ = out_tx.send(EngineMsg::DialogsReset { tab_id });
        }
        self.clear_permissions_on_origin_change(tab_id, None, out_tx);
        self.clear_auth_on_nav(tab_id, out_tx);
    }

    pub fn dismiss_all(&self) {
        self.trace_pid.store(0, Ordering::Release);
        for (_, cb) in std::mem::take(&mut *self.dialogs.lock().unwrap()) {
            cb.cont(0, None);
        }
        for p in std::mem::take(&mut *self.permissions.lock().unwrap()) {
            match p {
                HeldPermission::Media { callback, .. } => callback.cancel(),
                HeldPermission::Prompt { callback, .. } => {
                    callback.cont(sys::cef_permission_request_result_t::CEF_PERMISSION_RESULT_DISMISS.into());
                }
            }
        }
        for (_, cb) in std::mem::take(&mut *self.auth.lock().unwrap()) {
            cb.cancel();
        }
    }

    pub fn replay_pending(&self, tab_id: u64, out_tx: &Output) {
        for (dialog, _) in self.dialogs.lock().unwrap().iter() {
            let _ = out_tx.send(EngineMsg::Dialog { tab_id, dialog: dialog.clone() });
        }
        for p in self.permissions.lock().unwrap().iter() {
            let request = match p {
                HeldPermission::Media { prompt, .. } | HeldPermission::Prompt { prompt, .. } => {
                    prompt.clone()
                }
            };
            let _ = out_tx.send(EngineMsg::PermissionRequest { tab_id, request });
        }
        for (challenge, _) in self.auth.lock().unwrap().iter() {
            let _ = out_tx.send(EngineMsg::AuthRequest { tab_id, challenge: challenge.clone() });
        }
    }
}

/// Evaluates an HTTP authentication challenge from `TabRequestHandler::auth_credentials`
/// (`site-auth.ts`). Always returns `1` for site challenges so CEF never falls back to a native
/// system dialog; returns `0` for proxy auth.
#[allow(clippy::too_many_arguments)]
pub fn handle_auth_credentials(
    tab_id: u64,
    shared: &PromptShared,
    prompts: &TabPrompts,
    snap: &Arc<Mutex<TabSnapshot>>,
    out_tx: &Output,
    origin_url: Option<&CefString>,
    is_proxy: i32,
    host: Option<&CefString>,
    port: i32,
    realm: Option<&CefString>,
    scheme: Option<&CefString>,
    callback: Option<&mut AuthCallback>,
) -> i32 {
    if is_proxy != 0 {
        let Some(callback) = callback else { return 1 };
        let scheme_str = scheme.map(CefString::to_string).unwrap_or_default();
        let host_str = host.map(CefString::to_string).unwrap_or_default();
        let realm_str = realm.map(CefString::to_string).unwrap_or_default();
        if let Some(cfg) = prompts.proxy.lock().unwrap().as_ref()
            && crate::context::proxy_matches_challenge(cfg, &scheme_str, &host_str, port, &realm_str)
            && let (Some(u), Some(p)) = (&cfg.username, &cfg.password)
        {
            callback.cont(Some(&CefString::from(u.as_str())), Some(&CefString::from(p.as_str())));
            return 1;
        }
        callback.cancel();
        return 1;
    }
    let Some(callback) = callback else { return 1 };
    let scheme_str = scheme.map(CefString::to_string).unwrap_or_default();
    let scheme_lower = scheme_str.to_ascii_lowercase();
    if !matches!(scheme_lower.as_str(), "basic" | "digest" | "ntlm" | "negotiate") {
        callback.cancel();
        return 1;
    }
    let raw_origin = origin_url.map(CefString::to_string).unwrap_or_default();
    let Some(req_origin) = browser_policy::destination_origin(&raw_origin) else {
        callback.cancel();
        return 1;
    };
    let top_url = snap.lock().unwrap().url.clone();
    let top_origin = browser_policy::destination_origin(&top_url);
    let pending_origin = prompts
        .pending_top_url
        .lock()
        .unwrap()
        .as_deref()
        .and_then(browser_policy::destination_origin);
    let same_origin = top_origin.as_deref() == Some(req_origin.as_str())
        || pending_origin.as_deref() == Some(req_origin.as_str());
    if !same_origin {
        callback.cancel();
        return 1;
    }
    if prompts.agent_driven.load(Ordering::Relaxed) {
        callback.cancel();
        let _ = out_tx.send(EngineMsg::AuthAgentRequired { tab_id, origin: req_origin });
        return 1;
    }
    if !shared.enabled.load(Ordering::Relaxed) {
        callback.cancel();
        return 1;
    }
    let id = shared.next_id();
    let challenge = AuthChallenge {
        id,
        origin: req_origin,
        host: sanitize_wire_line(&host.map(CefString::to_string).unwrap_or_default(), 256),
        port: u16::try_from(port).unwrap_or(0),
        realm: sanitize_wire_line(&realm.map(CefString::to_string).unwrap_or_default(), 256),
        scheme: scheme_lower,
    };
    prompts.auth.lock().unwrap().push((challenge.clone(), callback.clone()));
    let _ = out_tx.send(EngineMsg::AuthRequest { tab_id, challenge });
    1
}

/// Whether `origin` is a trustworthy context for browser permissions (`site-permissions.ts`).
fn is_secure_origin(origin: &str) -> bool {
    let trimmed = origin.trim();
    if trimmed.len() >= 8 && trimmed.as_bytes()[..8].eq_ignore_ascii_case(b"https://") {
        return browser_policy::destination_origin(trimmed).is_some();
    }
    if trimmed.len() >= 13 && trimmed.as_bytes()[..13].eq_ignore_ascii_case(b"oc-preview://") {
        return true;
    }
    if trimmed.len() >= 7 && trimmed.as_bytes()[..7].eq_ignore_ascii_case(b"http://") {
        let Some(orig) = browser_policy::destination_origin(trimmed) else { return false };
        let authority = &orig[7..];
        let host = if let Some(rest) = authority.strip_prefix('[') {
            rest.split(']').next().unwrap_or("")
        } else {
            authority.split(':').next().unwrap_or("")
        };
        return browser_policy::loopback_host(host);
    }
    false
}

pub fn origin_of(url: &str) -> Option<String> {
    if let Some(orig) = browser_policy::destination_origin(url) {
        return Some(orig);
    }
    let trimmed = url.trim();
    if trimmed.len() >= 13 && trimmed.as_bytes()[..13].eq_ignore_ascii_case(b"oc-preview://") {
        let host = trimmed[13..].split(['/', '?', '#']).next().unwrap_or("");
        if !host.is_empty() {
            return Some(format!("oc-preview://{}", host.to_ascii_lowercase()));
        }
    }
    None
}

/// Bounds untrusted text before sending it over the engine IPC socket.
pub(crate) fn cap_wire_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Sanitizes a single-line header/origin/realm field before sending over IPC: strips newlines,
/// control characters, and Unicode bidi overrides/isolates, and bounds character length.
pub(crate) fn sanitize_wire_line(s: &str, max: usize) -> String {
    s.chars()
        .filter(|&c| {
            let n = c as u32;
            !matches!(
                n,
                0x00..=0x1f
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

/// Maximum URL byte length emitted in `EngineMsg::PopupBlocked` (prevents multi-MiB query
/// exfiltration behind a truncated `short_host` popup notice).
pub const MAX_POPUP_URL_BYTES: usize = 16_384;

wrap_life_span_handler! {
    pub struct TabLifeSpanHandler {
        tab_id: u64,
        is_preview: bool,
        context_ready: Arc<AtomicBool>,
        prompts: Arc<TabPrompts>,
        out_tx: Output,
    }

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: ::std::os::raw::c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            target_disposition: WindowOpenDisposition,
            user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            if !self.context_ready.load(Ordering::Acquire) {
                return 1;
            }
            let raw = target_url.map(CefString::to_string).unwrap_or_default();
            let url = if raw.len() > MAX_POPUP_URL_BYTES {
                let _ = self.out_tx.send(EngineMsg::Refused {
                    tab_id: self.tab_id,
                    reason: browser_policy::Refusal::Web.code().to_owned(),
                });
                return 1;
            } else if raw.trim().is_empty() {
                "about:blank".to_owned()
            } else {
                raw
            };
            // Pop-ups are never permitted to open `file://` or non-HTTP(S) schemes (`chromium.ts:448-468`).
            if let Some(why) = browser_policy::frame_guard(&url, true, &[]) {
                let _ = self.out_tx.send(EngineMsg::Refused {
                    tab_id: self.tab_id,
                    reason: why.code().to_owned(),
                });
                return 1;
            }
            // Preview tabs never auto-spawn pop-ups (even with user gesture); user-gesture http(s)
            // links within the URL size cap surface as a blocked-popup notice, while non-gesture
            // or oversized attempts emit `Refused`.
            if self.is_preview {
                if user_gesture != 0
                    && url.len() <= MAX_POPUP_URL_BYTES
                    && browser_policy::destination_origin(&url).is_some()
                {
                    let _ = self.out_tx.send(EngineMsg::PopupBlocked { tab_id: self.tab_id, url });
                } else {
                    let _ = self.out_tx.send(EngineMsg::Refused {
                        tab_id: self.tab_id,
                        reason: browser_policy::Refusal::Web.code().to_owned(),
                    });
                }
                return 1;
            }
            // A gesture opens one pop-up; a page that opens more from the same click gets the
            // "Pop-up blocked" notice for the rest.
            if user_gesture != 0 && self.prompts.popup_credit.swap(false, Ordering::AcqRel) {
                let background = *target_disposition.as_ref()
                    == sys::cef_window_open_disposition_t::CEF_WOD_NEW_BACKGROUND_TAB;
                let _ = self.out_tx.send(EngineMsg::PopupOpen {
                    tab_id: self.tab_id,
                    url,
                    background,
                });
            } else {
                let _ = self.out_tx.send(EngineMsg::PopupBlocked {
                    tab_id: self.tab_id,
                    url,
                });
            }
            // Always cancel CEF's native popup window; the app opens a managed tab instead.
            1
        }

        fn on_before_dev_tools_popup(
            &self,
            _browser: Option<&mut Browser>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            use_default_window: Option<&mut ::std::os::raw::c_int>,
        ) {
            if let Some(flag) = use_default_window {
                *flag = 0;
            }
        }
    }
}

wrap_jsdialog_handler! {
    pub struct TabDialogHandler {
        tab_id: u64,
        shared: PromptShared,
        prompts: Arc<TabPrompts>,
        preview: Option<Arc<crate::preview::PreviewGrant>>,
        out_tx: Output,
    }

    impl JsdialogHandler {
        fn on_jsdialog(
            &self,
            _browser: Option<&mut Browser>,
            origin_url: Option<&CefString>,
            dialog_type: JsdialogType,
            message_text: Option<&CefString>,
            default_prompt_text: Option<&CefString>,
            callback: Option<&mut JsdialogCallback>,
            _suppress_message: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            use sys::cef_jsdialog_type_t as T;
            let kind = match *dialog_type.as_ref() {
                T::JSDIALOGTYPE_CONFIRM => DialogKind::Confirm,
                T::JSDIALOGTYPE_PROMPT => DialogKind::Prompt,
                _ => DialogKind::Alert,
            };
            let s = |v: Option<&CefString>| v.map(CefString::to_string).unwrap_or_default();
            self.hold(kind, s(origin_url), s(message_text), s(default_prompt_text), callback)
        }

        fn on_before_unload_dialog(
            &self,
            _browser: Option<&mut Browser>,
            _message_text: Option<&CefString>,
            _is_reload: ::std::os::raw::c_int,
            callback: Option<&mut JsdialogCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(callback) = callback else { return 0 };
            // Agent-driven tabs skip "Leave site?" on navigation (`#63002`).
            if self.prompts.agent_driven.load(Ordering::Relaxed) {
                callback.cont(1, None);
                return 1;
            }
            self.hold(DialogKind::Leave, String::new(), String::new(), String::new(), Some(callback))
        }

        fn on_reset_dialog_state(&self, _browser: Option<&mut Browser>) {
            self.forget();
        }

        fn on_dialog_closed(&self, _browser: Option<&mut Browser>) {
            self.forget();
        }
    }
}

impl TabDialogHandler {
    fn forget(&self) {
        let had = !std::mem::take(&mut *self.prompts.dialogs.lock().unwrap()).is_empty();
        if had {
            let _ = self.out_tx.send(EngineMsg::DialogsReset { tab_id: self.tab_id });
        }
    }

    fn hold(
        &self,
        kind: DialogKind,
        origin: String,
        message: String,
        default_text: String,
        callback: Option<&mut JsdialogCallback>,
    ) -> ::std::os::raw::c_int {
        let Some(callback) = callback else { return 0 };
        if !self.shared.enabled.load(Ordering::Relaxed) {
            // Suppress scripted dialogs and let `beforeunload` proceed when connected to an older
            // app so CEF never pops an unmanaged native OS dialog box.
            callback.cont(i32::from(kind == DialogKind::Leave), None);
            return 1;
        }
        let origin = sanitize_wire_line(
            &self.preview.as_ref().map_or(origin.clone(), |g| g.sanitize_reported_url(&origin)),
            512,
        );
        let id = self.shared.next_id();
        let dialog = PageDialog {
            id,
            kind,
            origin,
            message: cap_wire_chars(&message, 4096),
            default_text: cap_wire_chars(&default_text, 4096),
        };
        self.prompts.dialogs.lock().unwrap().push((dialog.clone(), callback.clone()));
        let _ = self.out_tx.send(EngineMsg::Dialog { tab_id: self.tab_id, dialog });
        1
    }
}

wrap_permission_handler! {
    pub struct TabPermissionHandler {
        tab_id: u64,
        is_preview: bool,
        context_ready: Arc<AtomicBool>,
        shared: PromptShared,
        prompts: Arc<TabPrompts>,
        out_tx: Output,
    }

    impl PermissionHandler {
        fn on_request_media_access_permission(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut MediaAccessCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(callback) = callback else { return 1 };
            if self.is_preview || !self.context_ready.load(Ordering::Acquire) {
                callback.cancel();
                return 1;
            }
            let audio = sys::cef_media_access_permission_types_t::CEF_MEDIA_PERMISSION_DEVICE_AUDIO_CAPTURE as u32;
            let video = sys::cef_media_access_permission_types_t::CEF_MEDIA_PERMISSION_DEVICE_VIDEO_CAPTURE as u32;
            let allowed_mask = audio | video;
            if requested_permissions == 0 || (requested_permissions & !allowed_mask) != 0 {
                callback.cancel();
                return 1;
            }
            let raw_origin = requesting_origin.map(CefString::to_string).unwrap_or_default();
            let top_url = browser
                .as_ref()
                .and_then(|b| b.main_frame())
                .map(|f| CefString::from(&f.url()).to_string())
                .unwrap_or_default();
            let frame_ok = frame
                .as_ref()
                .is_some_and(|f| origin_of(&CefString::from(&f.url()).to_string()) == origin_of(&raw_origin));
            let (Some(req_orig), Some(top_orig)) = (origin_of(&raw_origin), origin_of(&top_url)) else {
                callback.cancel();
                return 1;
            };
            if !frame_ok
                || !self.shared.enabled.load(Ordering::Relaxed)
                || !is_secure_origin(&req_orig)
                || req_orig != top_orig
            {
                callback.cancel();
                return 1;
            }
            let mut kinds = Vec::new();
            if requested_permissions & video != 0 {
                kinds.push(PermissionKind::Camera);
            }
            if requested_permissions & audio != 0 {
                kinds.push(PermissionKind::Microphone);
            }
            let id = self.shared.next_id();
            let prompt = PermissionPrompt {
                id,
                origin: req_orig,
                top_origin: top_orig,
                kinds,
                is_media: true,
            };
            self.prompts.permissions.lock().unwrap().push(HeldPermission::Media {
                prompt: prompt.clone(),
                requested_bits: requested_permissions,
                callback: callback.clone(),
            });
            let _ = self.out_tx.send(EngineMsg::PermissionRequest { tab_id: self.tab_id, request: prompt });
            1
        }

        fn on_show_permission_prompt(
            &self,
            browser: Option<&mut Browser>,
            prompt_id: u64,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut PermissionPromptCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(callback) = callback else { return 1 };
            let deny = || {
                // `DISMISS` rejects the JS request cleanly without writing a permanent `setting: 2`
                // entry into `Default/Preferences`.
                callback.cont(sys::cef_permission_request_result_t::CEF_PERMISSION_RESULT_DISMISS.into());
                1
            };
            if self.is_preview || !self.context_ready.load(Ordering::Acquire) {
                return deny();
            }
            use sys::cef_permission_request_types_t as P;
            let cam = P::CEF_PERMISSION_TYPE_CAMERA_STREAM as u32;
            let mic = P::CEF_PERMISSION_TYPE_MIC_STREAM as u32;
            let geo = P::CEF_PERMISSION_TYPE_GEOLOCATION as u32;
            let notif = P::CEF_PERMISSION_TYPE_NOTIFICATIONS as u32;
            let clip = P::CEF_PERMISSION_TYPE_CLIPBOARD as u32;
            let known_mask = cam | mic | geo | notif | clip;
            if requested_permissions == 0 || (requested_permissions & !known_mask) != 0 {
                return deny();
            }
            let raw_origin = requesting_origin.map(CefString::to_string).unwrap_or_default();
            let top_url = browser
                .as_ref()
                .and_then(|b| b.main_frame())
                .map(|f| CefString::from(&f.url()).to_string())
                .unwrap_or_default();
            let (Some(req_orig), Some(top_orig)) = (origin_of(&raw_origin), origin_of(&top_url)) else {
                return deny();
            };
            if !self.shared.enabled.load(Ordering::Relaxed)
                || !is_secure_origin(&req_orig)
                || req_orig != top_orig
            {
                return deny();
            }
            let mut kinds = Vec::new();
            if requested_permissions & cam != 0 {
                kinds.push(PermissionKind::Camera);
            }
            if requested_permissions & mic != 0 {
                kinds.push(PermissionKind::Microphone);
            }
            if requested_permissions & geo != 0 {
                kinds.push(PermissionKind::Location);
            }
            if requested_permissions & notif != 0 {
                kinds.push(PermissionKind::Notifications);
            }
            if requested_permissions & clip != 0 {
                kinds.push(PermissionKind::ClipboardRead);
            }
            let id = self.shared.next_id();
            let prompt = PermissionPrompt {
                id,
                origin: req_orig,
                top_origin: top_orig,
                kinds,
                is_media: false,
            };
            self.prompts.permissions.lock().unwrap().push(HeldPermission::Prompt {
                prompt: prompt.clone(),
                cef_prompt_id: prompt_id,
                callback: callback.clone(),
            });
            let _ = self.out_tx.send(EngineMsg::PermissionRequest { tab_id: self.tab_id, request: prompt });
            1
        }

        fn on_dismiss_permission_prompt(
            &self,
            _browser: Option<&mut Browser>,
            prompt_id: u64,
            _result: PermissionRequestResult,
        ) {
            let removed_id = {
                let mut list = self.prompts.permissions.lock().unwrap();
                list.iter()
                    .position(|p| matches!(p, HeldPermission::Prompt { cef_prompt_id, .. } if *cef_prompt_id == prompt_id))
                    .map(|idx| match list.remove(idx) {
                        HeldPermission::Media { prompt, .. } | HeldPermission::Prompt { prompt, .. } => prompt.id,
                    })
            };
            if let Some(id) = removed_id {
                let _ = self.out_tx.send(EngineMsg::PermissionDismissed { tab_id: self.tab_id, id });
            }
        }
    }
}

wrap_dialog_handler! {
    pub struct TabFileDialogHandler {
        tab_id: u64,
        is_preview: bool,
        prompts: Arc<TabPrompts>,
        out_tx: Output,
    }

    impl DialogHandler {
        fn on_file_dialog(
            &self,
            _browser: Option<&mut Browser>,
            _mode: FileDialogMode,
            _title: Option<&CefString>,
            _default_file_path: Option<&CefString>,
            _accept_filters: Option<&mut CefStringList>,
            _accept_extensions: Option<&mut CefStringList>,
            _accept_descriptions: Option<&mut CefStringList>,
            callback: Option<&mut FileDialogCallback>,
        ) -> ::std::os::raw::c_int {
            if self.is_preview || self.prompts.agent_owns_dialogs() {
                if let Some(cb) = callback {
                    cb.cancel();
                }
                let _ = self.out_tx.send(EngineMsg::FileChooserBlocked { tab_id: self.tab_id });
                return 1;
            }
            0
        }
    }
}

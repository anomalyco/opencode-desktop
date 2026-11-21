//! Site-permission lifecycle tracker and hardware-device / File System Access guards for
//! `opencode-browser-engine` (`notes/review-rd-engine.md` Fix PR 3 `#305`).
//!
//! Lifecycle invariants:
//! - `"Always allow"` / `"Never allow"` (`Default` profile only) keep Chromium's
//!   `HostContentSettingsMap` live so `new Notification()`, `navigator.permissions.query()`, and
//!   `geolocation.watchPosition()` work natively until revoked or changed in `Settings > Browser`
//!   (`HostMsg::SyncSitePermissions` / `HostMsg::ResetPermission`).
//! - `"Allow this time"` (`Default` and isolated `VisitOnly` contexts) keeps `ALLOW` live for the
//!   tab's current origin lifetime, then resets `(ctx, origin, kind)` to `DEFAULT` on committed
//!   cross-origin navigation (`on_address_change`), tab close (`close_managed_tab`,
//!   `ExtEffect::CloseTab`, `ReopenTabs`), or `Shutdown` (before `cef::shutdown()`) when no other
//!   live tab in that context or `Default` persistent grant holds it.
//! - Startup `scrub_legacy_content_settings` wipes all 6 exact-origin permission tables in
//!   `Default/Preferences` before Rune syncs `rune.sitePermissions`, guaranteeing crashed one-time
//!   grants never persist across restarts.
//! - All 7 hardware-device and File System Access guards (`USB`, `SERIAL`, `HID`, `BLUETOOTH`,
//!   `BLUETOOTH_SCANNING`, `FILE_SYSTEM_READ`, `FILE_SYSTEM_WRITE`) are locked to `BLOCK` on both
//!   `Default` and every isolated `CefRequestContext` (via prefs and explicit-origin settings).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::sync::atomic::Ordering;

use cef::rc::Rc as _;
use cef::{
    Browser, BrowserHost, CefString, CefStringList, ImplBrowser, ImplBrowserHost,
    ImplDictionaryValue, ImplFrame, ImplNavigationEntry, ImplNavigationEntryVisitor,
    ImplPreferenceManager, ImplRequestContext, ImplValue, NavigationEntry, NavigationEntryVisitor,
    RequestContext, WrapNavigationEntryVisitor, sys, wrap_navigation_entry_visitor,
};

use crate::wire::{ContextKind, PermissionKind, SitePermissionEntry, TabSnapshot};

pub const INHERITED_PERMISSION_MSG: &str =
    "Controlled browser actions are blocked on this site because it holds a profile permission.";

fn is_inherited_permission(kind: PermissionKind) -> bool {
    matches!(
        kind,
        PermissionKind::ClipboardRead | PermissionKind::Location | PermissionKind::Notifications
    )
}

enum OriginClass {
    Web(String),
    Opaque,
    Invalid,
}

/// Canonicalizes `raw` using the `url` crate (`Url::origin().ascii_serialization()`), stripping
/// default ports (`:80`/`:443`), canonicalizing IPv6, punycode-encoding Unicode IDNs, and mapping
/// `blob:http(s)://...` to its creator's `http(s)` tuple origin. Opaque origins (`about:blank`,
/// `chrome-error://chromewebdata/`, `data:`, etc.) hold no profile grant (`Opaque`), whereas
/// unparseable strings fail closed (`Invalid`).
fn classify_origin(raw: &str) -> OriginClass {
    match url::Url::parse(raw.trim()) {
        Ok(u) => match u.origin() {
            url::Origin::Tuple(scheme, _, _) if matches!(scheme.as_str(), "http" | "https") => {
                OriginClass::Web(u.origin().ascii_serialization())
            }
            _ => OriginClass::Opaque,
        },
        Err(_) => OriginClass::Invalid,
    }
}

/// Returns the canonical ASCII serialization of an `http(s):` origin (or `None` for non-web/invalid).
fn origin_of(raw: &str) -> Option<String> {
    match classify_origin(raw) {
        OriginClass::Web(o) => Some(o),
        OriginClass::Opaque | OriginClass::Invalid => None,
    }
}

const DEVICE_GUARD_TYPES: [sys::cef_content_setting_types_t; 7] = [
    sys::cef_content_setting_types_t::CEF_CONTENT_SETTING_TYPE_USB_GUARD,
    sys::cef_content_setting_types_t::CEF_CONTENT_SETTING_TYPE_SERIAL_GUARD,
    sys::cef_content_setting_types_t::CEF_CONTENT_SETTING_TYPE_HID_GUARD,
    sys::cef_content_setting_types_t::CEF_CONTENT_SETTING_TYPE_BLUETOOTH_GUARD,
    sys::cef_content_setting_types_t::CEF_CONTENT_SETTING_TYPE_BLUETOOTH_SCANNING,
    sys::cef_content_setting_types_t::CEF_CONTENT_SETTING_TYPE_FILE_SYSTEM_READ_GUARD,
    sys::cef_content_setting_types_t::CEF_CONTENT_SETTING_TYPE_FILE_SYSTEM_WRITE_GUARD,
];

/// Default-scope content-setting pref names corresponding to `DEVICE_GUARD_TYPES` (`2 == BLOCK`).
pub const DEVICE_GUARD_PREFS: [&str; 7] = [
    "profile.default_content_setting_values.usb_guard",
    "profile.default_content_setting_values.serial_guard",
    "profile.default_content_setting_values.hid_guard",
    "profile.default_content_setting_values.bluetooth_guard",
    "profile.default_content_setting_values.bluetooth_scanning",
    "profile.default_content_setting_values.file_system_read_guard",
    "profile.default_content_setting_values.file_system_write_guard",
];

fn content_types(kind: PermissionKind) -> &'static [sys::cef_content_setting_types_t] {
    use sys::cef_content_setting_types_t as C;
    match kind {
        PermissionKind::Camera => &[C::CEF_CONTENT_SETTING_TYPE_MEDIASTREAM_CAMERA],
        PermissionKind::Microphone => &[C::CEF_CONTENT_SETTING_TYPE_MEDIASTREAM_MIC],
        PermissionKind::Location => &[C::CEF_CONTENT_SETTING_TYPE_GEOLOCATION],
        PermissionKind::Notifications => &[C::CEF_CONTENT_SETTING_TYPE_NOTIFICATIONS],
        PermissionKind::ClipboardRead => &[C::CEF_CONTENT_SETTING_TYPE_CLIPBOARD_READ_WRITE],
    }
}

/// Sets Chromium's `HostContentSettingsMap` entry for exact `(origin, origin, kind)` to `ALLOW` or `BLOCK`.
pub fn set_content_setting(ctx: &RequestContext, origin: &str, kind: PermissionKind, allow: bool) {
    let Some(orig) = origin_of(origin) else { return };
    let url = CefString::from(orig.as_str());
    let val = if allow {
        sys::cef_content_setting_values_t::CEF_CONTENT_SETTING_VALUE_ALLOW
    } else {
        sys::cef_content_setting_values_t::CEF_CONTENT_SETTING_VALUE_BLOCK
    };
    for &ct in content_types(kind) {
        ctx.set_content_setting(Some(&url), Some(&url), ct.into(), val.into());
    }
    // Clear any companion `GEOLOCATION_WITH_OPTIONS` dictionary override so Chromium observes `GEOLOCATION`.
    if kind == PermissionKind::Location {
        ctx.set_website_setting(
            Some(&url),
            Some(&url),
            sys::cef_content_setting_types_t::CEF_CONTENT_SETTING_TYPE_GEOLOCATION_WITH_OPTIONS.into(),
            None,
        );
    }
}

/// Resets Chromium's `HostContentSettingsMap` entry for exact `(origin, origin, kind)` to `DEFAULT`.
pub fn reset_content_setting(ctx: &RequestContext, origin: &str, kind: PermissionKind) {
    let Some(orig) = origin_of(origin) else { return };
    let url = CefString::from(orig.as_str());
    let default = sys::cef_content_setting_values_t::CEF_CONTENT_SETTING_VALUE_DEFAULT;
    for &ct in content_types(kind) {
        ctx.set_content_setting(Some(&url), Some(&url), ct.into(), default.into());
    }
    if kind == PermissionKind::Location {
        ctx.set_website_setting(
            Some(&url),
            Some(&url),
            sys::cef_content_setting_types_t::CEF_CONTENT_SETTING_TYPE_GEOLOCATION_WITH_OPTIONS.into(),
            None,
        );
    }
}

/// Locks all 7 hardware-device and File System Access guards to `BLOCK` on `Default`'s global context.
pub fn block_device_permission_guards(ctx: &RequestContext) {
    let block = sys::cef_content_setting_values_t::CEF_CONTENT_SETTING_VALUE_BLOCK;
    for ct in DEVICE_GUARD_TYPES {
        ctx.set_content_setting(None, None, ct.into(), block.into());
    }
}

/// Content settings for Chrome services official's Electron does not contain (READER-7's audit,
/// `notes/review-rd274.md`): FedCM (N2), the anti-abuse Privacy Sandbox setting (N6a), payment
/// handlers (N7), and the minor locks (protocol handlers, window management, idle detection,
/// local fonts). Default-scope prefs so isolated contexts, where a `None` origin is a no-op, get
/// them too (`context::lock_chrome_services`).
const CHROME_SERVICE_SETTINGS: [(sys::cef_content_setting_types_t, &str); 8] = [
    (sys::cef_content_setting_types_t::CEF_CONTENT_SETTING_TYPE_FEDERATED_IDENTITY_API, "webid_api"),
    (
        sys::cef_content_setting_types_t::CEF_CONTENT_SETTING_TYPE_FEDERATED_IDENTITY_AUTO_REAUTHN_PERMISSION,
        "webid_auto_reauthn",
    ),
    (sys::cef_content_setting_types_t::CEF_CONTENT_SETTING_TYPE_ANTI_ABUSE, "anti_abuse"),
    (sys::cef_content_setting_types_t::CEF_CONTENT_SETTING_TYPE_PAYMENT_HANDLER, "payment_handler"),
    (sys::cef_content_setting_types_t::CEF_CONTENT_SETTING_TYPE_PROTOCOL_HANDLERS, "protocol_handler"),
    (sys::cef_content_setting_types_t::CEF_CONTENT_SETTING_TYPE_WINDOW_MANAGEMENT, "window_placement"),
    (sys::cef_content_setting_types_t::CEF_CONTENT_SETTING_TYPE_IDLE_DETECTION, "idle_detection"),
    (sys::cef_content_setting_types_t::CEF_CONTENT_SETTING_TYPE_LOCAL_FONTS, "local_fonts"),
];

pub fn chrome_service_setting_prefs() -> impl Iterator<Item = String> {
    CHROME_SERVICE_SETTINGS.iter().map(|(_, name)| format!("profile.default_content_setting_values.{name}"))
}

/// Locks `CHROME_SERVICE_SETTINGS` to `BLOCK` on `ctx`'s default scope.
pub fn block_chrome_service_settings(ctx: &RequestContext) {
    let block = sys::cef_content_setting_values_t::CEF_CONTENT_SETTING_VALUE_BLOCK;
    for (ct, _) in CHROME_SERVICE_SETTINGS {
        ctx.set_content_setting(None, None, ct.into(), block.into());
    }
}

/// Locks all 7 hardware-device and File System Access guards to `BLOCK` for an explicit `origin`
/// on an isolated/incognito `CefRequestContext` (where `set_content_setting(None, None, ...)` is a
/// CEF no-op).
pub fn block_device_permission_guards_for_origin(ctx: &RequestContext, origin: &str) {
    let Some(orig) = guard_origin(origin) else { return };
    let url = CefString::from(orig.as_str());
    let block = sys::cef_content_setting_values_t::CEF_CONTENT_SETTING_VALUE_BLOCK;
    for ct in DEVICE_GUARD_TYPES {
        ctx.set_content_setting(Some(&url), Some(&url), ct.into(), block.into());
    }
}

fn guard_origin(raw: &str) -> Option<String> {
    if let Some(http) = origin_of(raw) {
        return Some(http);
    }
    let trimmed = raw.trim();
    if trimmed.len() > 13 && trimmed[..13].eq_ignore_ascii_case("oc-preview://") {
        let rest = &trimmed[13..];
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if !host.is_empty() {
            return Some(format!("oc-preview://{}", host.to_ascii_lowercase()));
        }
    }
    None
}

/// Scrubs all exact-origin entries across the 6 site-permission tables in `Default/Preferences` at
/// startup so crashed one-time grants and stale pre-sync overrides never survive a restart. Rune
/// then restores only its own `rune.sitePermissions` (`Default` `"Always"`/`"Never"`) via
/// `HostMsg::SyncSitePermissions`.
pub fn scrub_legacy_content_settings(ctx: &RequestContext) {
    let tables: &[(&str, PermissionKind)] = &[
        ("profile.content_settings.exceptions.notifications", PermissionKind::Notifications),
        ("profile.content_settings.exceptions.clipboard", PermissionKind::ClipboardRead),
        ("profile.content_settings.exceptions.geolocation", PermissionKind::Location),
        ("profile.content_settings.exceptions.geolocation_with_options", PermissionKind::Location),
        ("profile.content_settings.exceptions.media_stream_camera", PermissionKind::Camera),
        ("profile.content_settings.exceptions.media_stream_mic", PermissionKind::Microphone),
    ];
    for &(pref, kind) in tables {
        let key = CefString::from(pref);
        let Some(val) = ctx.preference(Some(&key)) else { continue };
        let Some(dict) = val.dictionary() else { continue };
        let mut raw_keys = CefStringList::new();
        if dict.keys(Some(&mut raw_keys)) == 0 {
            continue;
        }
        for entry_key in crate::menu::read_cef_string_list(&raw_keys) {
            // Keys in `Preferences` have the form `"https://host:port,*"` or `"https://host:port,https://host:port"`.
            let origin = entry_key.split(',').next().unwrap_or(&entry_key);
            reset_content_setting(ctx, origin, kind);
        }
    }
}

/// Stable partition key identifying which `CefRequestContext` a tab belongs to (`None` = `Default`).
pub fn context_scope_id(kind: &ContextKind, tab_id: u64) -> Option<String> {
    match kind {
        ContextKind::Default => None,
        ContextKind::Preview { workspace, grant } => {
            Some(format!("preview:{}:{}", workspace.trim(), grant.trim()))
        }
        ContextKind::Artifact { artifact_id } => Some(format!("artifact:{}", artifact_id.trim())),
        ContextKind::Private => Some(format!("private:{tab_id}")),
        ContextKind::RemoteServer { server_id, .. } => {
            Some(format!("remote:{}", server_id.trim()))
        }
    }
}

#[derive(Default)]
struct TrackerState {
    /// Persistent `"Always allow"` (`true`) / `"Never allow"` (`false`) entries in `Default`.
    persistent: HashMap<(String, PermissionKind), bool>,
    /// Active `"Allow this time"` visit grants keyed by `(ctx_scope, tab_id) -> ((origin, kind) -> ctx)`.
    visits: HashMap<(Option<String>, u64), HashMap<(String, PermissionKind), RequestContext>>,
}

/// Engine-wide tracker keeping `"Always"` and `"Allow this time"` Chromium content settings in
/// exact step with tab lifetimes and Rune's `rune.sitePermissions` store.
#[derive(Default)]
pub struct PermissionTracker {
    state: Mutex<TrackerState>,
}

impl PermissionTracker {
    /// Records a persistent `Default`-profile `"Always allow"` or `"Never allow"` decision.
    pub fn record_persistent(&self, origin: &str, kind: PermissionKind, allow: bool) {
        let Some(orig) = origin_of(origin) else { return };
        self.state.lock().unwrap().persistent.insert((orig, kind), allow);
    }

    /// Records an active `"Allow this time"` visit grant for `(ctx_scope, tab_id, origin, kind)`.
    pub fn record_visit(
        &self,
        ctx_scope: Option<String>,
        tab_id: u64,
        origin: &str,
        kind: PermissionKind,
        ctx: RequestContext,
    ) {
        let Some(orig) = origin_of(origin) else { return };
        self.state
            .lock()
            .unwrap()
            .visits
            .entry((ctx_scope, tab_id))
            .or_default()
            .insert((orig, kind), ctx);
    }

    /// Syncs Chromium's `Default` `HostContentSettingsMap` to exactly match Rune's persisted
    /// `rune.sitePermissions` (`Default` `"Always allow"` / `"Never allow"` only).
    pub fn sync_profile_permissions(&self, entries: &[SitePermissionEntry]) {
        let Some(global) = cef::request_context_get_global_context() else { return };
        let mut next: HashMap<(String, PermissionKind), bool> = HashMap::new();
        for e in entries {
            if let Some(orig) = origin_of(&e.origin) {
                next.insert((orig, e.kind), e.allow);
            }
        }
        let to_reset: Vec<(String, PermissionKind)> = {
            let mut st = self.state.lock().unwrap();
            let mut removed = Vec::new();
            let mut changed = Vec::new();
            for (pair, old_allow) in &st.persistent {
                match next.get(pair) {
                    None => removed.push(pair.clone()),
                    Some(new_allow) if new_allow != old_allow => changed.push(pair.clone()),
                    _ => {}
                }
            }
            for pair in removed.iter().chain(changed.iter()) {
                for ((scope, _), tab_map) in &mut st.visits {
                    if scope.is_none() {
                        tab_map.remove(pair);
                    }
                }
            }
            st.persistent = next.clone();
            removed
        };
        for (origin, kind) in to_reset {
            reset_content_setting(&global, &origin, kind);
        }
        for ((origin, kind), allow) in next {
            set_content_setting(&global, &origin, kind, allow);
        }
    }

    /// Explicitly revokes `(origin, kind)` in `Default` (e.g. `Settings > Browser` removal).
    pub fn reset_profile_permission(&self, origin: &str, kind: PermissionKind) {
        let Some(orig) = origin_of(origin) else { return };
        {
            let mut st = self.state.lock().unwrap();
            let key = (orig.clone(), kind);
            st.persistent.remove(&key);
            for ((scope, _), tab_map) in &mut st.visits {
                if scope.is_none() {
                    tab_map.remove(&key);
                }
            }
        }
        if let Some(global) = cef::request_context_get_global_context() {
            reset_content_setting(&global, &orig, kind);
        }
    }

    /// Committed top-level navigation changed `tab_id`'s origin: drops every `"Allow this time"`
    /// grant belonging to prior origins and resets Chromium when no other tab or persistent rule holds it.
    pub fn on_origin_changed(
        &self,
        ctx_scope: Option<&str>,
        tab_id: u64,
        new_origin: Option<&str>,
    ) {
        let scope_owned = ctx_scope.map(str::to_owned);
        let new_norm = new_origin.and_then(origin_of);
        let actions = {
            let mut st = self.state.lock().unwrap();
            let Some(tab_map) = st.visits.get_mut(&(scope_owned.clone(), tab_id)) else {
                return;
            };
            let mut dropped = Vec::new();
            tab_map.retain(|(orig, kind), ctx| {
                if new_norm.as_deref() == Some(orig.as_str()) {
                    true
                } else {
                    dropped.push((orig.clone(), *kind, ctx.clone()));
                    false
                }
            });
            if tab_map.is_empty() {
                st.visits.remove(&(scope_owned.clone(), tab_id));
            }
            compute_reset_actions(&st, scope_owned.as_deref(), dropped)
        };
        apply_reset_actions(actions);
    }

    /// Tab closed (`CloseTab`, `close_managed_tab`, `ExtEffect::CloseTab`, duplicate `open_tab`,
    /// or Linux `ReopenTabs`): releases all `"Allow this time"` grants held by `tab_id`.
    pub fn on_tab_closed(&self, tab_id: u64) {
        let actions = {
            let mut st = self.state.lock().unwrap();
            let matching_keys: Vec<(Option<String>, u64)> = st
                .visits
                .keys()
                .filter(|(_, tid)| *tid == tab_id)
                .cloned()
                .collect();
            let mut all_actions = Vec::new();
            for key in matching_keys {
                if let Some(tab_map) = st.visits.remove(&key) {
                    let dropped: Vec<(String, PermissionKind, RequestContext)> = tab_map
                        .into_iter()
                        .map(|((orig, kind), ctx)| (orig, kind, ctx))
                        .collect();
                    all_actions.extend(compute_reset_actions(&st, key.0.as_deref(), dropped));
                }
            }
            all_actions
        };
        apply_reset_actions(actions);
    }

    pub fn has_any_default_inherited_grant(&self) -> bool {
        let Ok(st) = self.state.lock() else {
            return true;
        };
        st.persistent.iter().any(|((_, k), &allow)| allow && is_inherited_permission(*k))
            || st.visits.iter().any(|((scope, _), m)| {
                scope.is_none() && m.keys().any(|(_, k)| is_inherited_permission(*k))
            })
    }

    /// Returns `true` if `url` is an `http(s):` origin holding a live `Default` `ALLOW` (persistent
    /// or active one-time) for `clipboard-read`, `geolocation`, or `notifications`, OR if `url`
    /// fails to parse or tracker state is poisoned (fail-closed). Opaque origins (`about:blank`,
    /// `chrome-error://chromewebdata/`, `data:`, etc.) return `false`.
    pub fn has_inherited_default_grant(&self, url: &str) -> bool {
        let orig = match classify_origin(url) {
            OriginClass::Web(o) => o,
            OriginClass::Opaque => return false,
            OriginClass::Invalid => return true,
        };
        let Ok(st) = self.state.lock() else {
            return true;
        };
        st.persistent.iter().any(|((o, k), &allow)| allow && o == &orig && is_inherited_permission(*k))
            || st.visits.iter().any(|((scope, _), m)| {
                scope.is_none() && m.keys().any(|(o, k)| o == &orig && is_inherited_permission(*k))
            })
    }

    /// Guard 1 (committed-origin check): returns `Some(url_opt)` when a controlled `Default` tab's
    /// committed origin holds an inherited grant or cannot be determined; `None` otherwise.
    /// Never inspects destination URLs, so navigating away from a granted origin always succeeds.
    pub fn tab_blocked_url(
        &self,
        browser: Option<&Browser>,
        isolated: bool,
        is_preview: bool,
        snap: &Mutex<TabSnapshot>,
        prompts: &crate::prompts::TabPrompts,
    ) -> Option<Option<String>> {
        if isolated || is_preview || !prompts.agent_driven.load(Ordering::Acquire) {
            return None;
        }
        let main_url = browser
            .and_then(|b| b.main_frame())
            .map(|f| CefString::from(&f.url()).to_string())
            .filter(|s| !s.trim().is_empty());
        let Ok(snap_url) = snap.lock().map(|s| (!s.url.trim().is_empty()).then(|| s.url.clone())) else {
            return Some(None);
        };
        let committed = match main_url {
            Some(ref u) if matches!(classify_origin(u), OriginClass::Web(_)) => Some(u.as_str()),
            Some(ref u) if u.trim().to_ascii_lowercase().starts_with("chrome-error://") => {
                // Unreachable error page: inspect `snap_url` only to see if the failed target held a grant.
                snap_url.as_deref()
            }
            Some(ref u) => Some(u.as_str()),
            None => snap_url.as_deref(),
        };
        let Some(u) = committed else {
            return Some(None);
        };
        if self.has_inherited_default_grant(u) {
            Some(Some(u.to_owned()))
        } else {
            None
        }
    }

    /// Resets every remaining active `"Allow this time"` grant back to `DEFAULT` (or restores its
    /// `Default` persistent setting) and drops every held `RequestContext` BEFORE `cef::shutdown()`.
    pub fn shutdown(&self) {
        let actions = {
            let mut st = self.state.lock().unwrap();
            let drained: Vec<_> = st.visits.drain().collect();
            let mut all_actions = Vec::new();
            for ((scope, _), tab_map) in drained {
                for ((orig, kind), ctx) in tab_map {
                    let restore = if scope.is_none() {
                        st.persistent.get(&(orig.clone(), kind)).copied()
                    } else {
                        None
                    };
                    all_actions.push((ctx, orig, kind, restore));
                }
            }
            all_actions
        };
        apply_reset_actions(actions);
    }
}

fn compute_reset_actions(
    st: &TrackerState,
    ctx_scope: Option<&str>,
    dropped: Vec<(String, PermissionKind, RequestContext)>,
) -> Vec<(RequestContext, String, PermissionKind, Option<bool>)> {
    let mut actions = Vec::new();
    for (orig, kind, ctx) in dropped {
        let key = (orig.clone(), kind);
        let held_elsewhere = st
            .visits
            .iter()
            .any(|((scope, _), map)| scope.as_deref() == ctx_scope && map.contains_key(&key));
        if held_elsewhere {
            continue;
        }
        let restore = if ctx_scope.is_none() {
            st.persistent.get(&key).copied()
        } else {
            None
        };
        actions.push((ctx, orig, kind, restore));
    }
    actions
}

fn apply_reset_actions(actions: Vec<(RequestContext, String, PermissionKind, Option<bool>)>) {
    for (ctx, orig, kind, restore) in actions {
        match restore {
            Some(allow) => set_content_setting(&ctx, &orig, kind, allow),
            None => reset_content_setting(&ctx, &orig, kind),
        }
    }
}

wrap_navigation_entry_visitor! {
    struct HistoryEntryCollector {
        entries: Arc<Mutex<Vec<(i32, bool, String)>>>,
    }

    impl NavigationEntryVisitor {
        fn visit(
            &self,
            entry: Option<&mut NavigationEntry>,
            current: ::std::os::raw::c_int,
            index: ::std::os::raw::c_int,
            _total: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            if let Some(e) = entry {
                let url = CefString::from(&e.url()).to_string();
                self.entries.lock().unwrap().push((index, current != 0, url));
            }
            1
        }
    }
}

/// Synchronously reads the URL of the session history entry at `current_index + delta` (`-1` for
/// Back, `+1` for Forward) so controlled `Back`/`Forward` refuses granted origins before `go_back`
/// can trigger a BFCache restore.
pub fn history_entry_url(host: &BrowserHost, delta: i32) -> Option<String> {
    let entries = Arc::new(Mutex::new(Vec::new()));
    let mut visitor = HistoryEntryCollector::new(entries.clone());
    host.navigation_entries(Some(&mut visitor), 0);
    let list = entries.lock().unwrap();
    let cur_idx = list.iter().find(|(_, cur, _)| *cur)?.0;
    let target_idx = cur_idx.checked_add(delta)?;
    list.iter().find(|(idx, _, _)| *idx == target_idx).map(|(_, _, u)| u.clone())
}

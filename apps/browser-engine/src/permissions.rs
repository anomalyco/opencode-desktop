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
use std::sync::Mutex;

use cef::{
    CefString, CefStringList, ImplDictionaryValue, ImplPreferenceManager, ImplRequestContext,
    ImplValue, RequestContext, sys,
};

use crate::prompts::origin_of;
use crate::wire::{ContextKind, PermissionKind, SitePermissionEntry};

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

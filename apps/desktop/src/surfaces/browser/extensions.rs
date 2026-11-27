//! Desktop-side MV3 browser extension lifecycle, verified startup spec sync, and monotonic
//! [`InstallMachine`] state stored in an `opencode_kernel::Slot` (zero `struct App` fields).
//!
//! Security invariants (`notes/review-rd274.md` & `notes/review-rd280.md`):
//! - Always constructs [`ExtensionManager`] via [`ExtensionManager::in_user_data`] anchored at the
//!   per-user app data directory (`crate::dirs::data()`), never `ExtensionManager::new`.
//! - Never writes profile files during unattended runs without `RUNE_DIRS_ROOT`.
//! - Drives install/review/swap transitions strictly through [`InstallMachine::step`] with a
//!   monotonic [`Instant`]-derived millisecond clock (never wall-clock `SystemTime`).
//! - Persisted [`StoredExtension`] records live in `opencode-storage` (`plugin.browser/extensions`),
//!   updated ONLY after disk swap + verification succeed.

#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, UNIX_EPOCH};

use browser_extensions::manager::{ExtensionManager, StagedInstall};
use browser_extensions::manifest::read_manifest;
use browser_extensions::review::{InstallEffect, InstallEvent, InstallMachine};
use browser_extensions::sources::PlatformDirs;
use browser_extensions::{
    CompatibilityLevel, ExtensionCompatibility, ExtensionSource, InstallFailure, ProfileExtension,
    StoredExtension,
};
use browser_pane::StartupExtensionSpec;
use opencode_kernel::Slot;
use rune::time::Instant;

use crate::App;

/// Chrome version token advertised when requesting CRX3 updates from the Chrome Web Store / Edge Add-ons.
pub const CHROME_STORE_VERSION: &str = "156.0.0.0";

/// How long a worker-verified extension list snapshot stays fresh before the next Settings visit
/// triggers a non-blocking background directory-stamp check.
const EXT_VERIFY_FRESH: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredFingerprint {
    id: String,
    version: Option<String>,
    enabled: bool,
    directory: String,
    digest: Option<String>,
    source: ExtensionSource,
    name: Option<String>,
}

fn stored_fingerprint(stored: &[StoredExtension]) -> Vec<StoredFingerprint> {
    stored
        .iter()
        .map(|r| StoredFingerprint {
            id: r.id.clone(),
            version: r.version.clone(),
            enabled: r.enabled,
            directory: r.directory.clone(),
            digest: r.digest.clone(),
            source: r.source,
            name: r.name.clone(),
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DirStamp {
    dir_mtime_ns: Option<u128>,
    manifest_mtime_ns: Option<u128>,
    manifest_len: Option<u64>,
}

fn mtime_ns(path: &Path) -> Option<u128> {
    std::fs::symlink_metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
}

fn dir_stamp(dir: &Path) -> DirStamp {
    let manifest = dir.join("manifest.json");
    let manifest_meta = std::fs::symlink_metadata(&manifest).ok();
    DirStamp {
        dir_mtime_ns: mtime_ns(dir),
        manifest_mtime_ns: manifest_meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos()),
        manifest_len: manifest_meta.map(|m| m.len()),
    }
}

struct VerifyRequest {
    stored: Vec<StoredExtension>,
    engine_failed: HashSet<String>,
    force_rehash: bool,
    restart_if_live: bool,
}

struct PendingApply {
    specs: Vec<StartupExtensionSpec>,
    has_filters: bool,
    profiles_changed: bool,
    restart_if_live: bool,
}

#[derive(Default)]
struct ExtCacheInner {
    stored_fp: Vec<StoredFingerprint>,
    dir_stamps: Vec<(String, DirStamp)>,
    changed_ids: HashSet<String>,
    specs: Vec<StartupExtensionSpec>,
    has_filters: bool,
    profiles: Vec<ProfileExtension>,
    verified_at: Option<Instant>,
    swept_staging: bool,
    in_flight: bool,
    dirty: bool,
    dirty_gen: u64,
    queued: Option<VerifyRequest>,
    pending_apply: Option<PendingApply>,
}

/// Synchronized install/review state shared safely between the UI thread and blocking worker threads.
pub struct ExtInstallCore {
    started_at: Instant,
    pub machine: InstallMachine,
    pub staged: Option<StagedInstall>,
    next_gen: u64,
}

impl Default for ExtInstallCore {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            machine: InstallMachine::new(),
            staged: None,
            next_gen: 1,
        }
    }
}

impl ExtInstallCore {
    /// Monotonic milliseconds elapsed since this runtime started (safe against wall-clock rollback).
    pub fn now_ms(&self) -> u64 {
        u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    pub fn next_generation(&mut self) -> u64 {
        let generation = self.next_gen;
        self.next_gen = self.next_gen.saturating_add(1);
        generation
    }

    pub fn step(&mut self, event: InstallEvent) -> Vec<InstallEffect> {
        let now = self.now_ms();
        self.machine.step(event, now)
    }
}

/// Per-window runtime state for browser extension staging, review approval, and integrity alerts.
#[derive(Default)]
pub struct BrowserExtensionsRuntime {
    pub core: Arc<Mutex<ExtInstallCore>>,
    pub integrity_failed: HashSet<String>,
    cache: Arc<Mutex<ExtCacheInner>>,
}

pub static BROWSER_EXTENSIONS: Slot<BrowserExtensionsRuntime> = Slot::new();

/// Resolves the production [`ExtensionManager`] anchored inside the per-user app data directory.
/// Refuses unattended non-rooted runs so fixtures/tests never touch real user profile data.
pub fn manager() -> Result<ExtensionManager, String> {
    if !crate::attended() && !crate::dirs::rooted() {
        return Err("Browser extensions are unavailable in unattended fixture runs.".into());
    }
    let user_data = crate::dirs::data()
        .ok_or_else(|| "Application data directory is unavailable.".to_owned())?;
    if !user_data.exists() {
        std::fs::create_dir_all(&user_data)
            .map_err(|e| format!("Could not prepare application data directory: {e}"))?;
    }
    ExtensionManager::in_user_data(&user_data)
}

/// Platform directories for explicit user-triggered "From your browsers" Chromium extension discovery.
pub fn platform_dirs() -> PlatformDirs {
    #[allow(clippy::disallowed_methods)]
    let env_path = |key: &str| std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(root) = env_path("OPENCODE_BROWSER_IMPORT_ROOT") {
        return PlatformDirs {
            home: Some(root.clone()),
            local_app_data: Some(root.clone()),
            app_data: Some(root.clone()),
            xdg_config_home: Some(root),
        };
    }
    PlatformDirs {
        home: env_path("HOME").or_else(|| env_path("USERPROFILE")),
        local_app_data: env_path("LOCALAPPDATA"),
        app_data: env_path("APPDATA"),
        xdg_config_home: env_path("XDG_CONFIG_HOME"),
    }
}

fn fallback_profile(rec: &StoredExtension, changed: bool) -> ProfileExtension {
    ProfileExtension {
        id: rec.id.clone(),
        name: rec.name.clone().unwrap_or_else(|| rec.id.clone()),
        version: rec.version.clone().unwrap_or_default(),
        enabled: rec.enabled && !changed && rec.digest.is_some(),
        source: rec.source,
        icon: None,
        popup: None,
        options: None,
        error: None,
        changed: changed || rec.digest.is_none(),
        compatibility: ExtensionCompatibility {
            level: CompatibilityLevel::Partial,
            cause: None,
            missing: Vec::new(),
        },
        access: Vec::new(),
    }
}

/// Marks the in-memory extension UI cache dirty so the next read or sync triggers a fresh
/// background worker pass. Never touches disk on the UI thread.
pub fn invalidate_cache(app: &App) {
    let cache = &app.kernel.slots.get(&BROWSER_EXTENSIONS).cache;
    let mut inner = cache.lock().unwrap_or_else(PoisonError::into_inner);
    inner.dirty = true;
    inner.dirty_gen = inner.dirty_gen.wrapping_add(1);
}

/// Non-blocking UI-thread read for `Browser:ExtList` and `Browser:ExtActions` inside `render()`.
///
/// Security invariant: this cache is strictly for Settings/toolbar display and non-blocking spec
/// preparation. The real integrity gate always runs inside `opencode-browser-engine` before every
/// `Extensions.loadUnpacked` (both startup in `extensions/integrity.rs` and runtime reload in
/// `extensions/mod.rs::reload_extension`), which independently re-hashes every file against the
/// approved SHA-256 and reports `EngineMsg::ExtensionIntegrityFailed` on mismatch.
pub fn cached_ext_list(app: &App, stored: &[StoredExtension]) -> Vec<ProfileExtension> {
    let rt = app.kernel.slots.get(&BROWSER_EXTENSIONS);
    let fp = stored_fingerprint(stored);
    let engine_failed = rt.integrity_failed.clone();

    let (should_spawn, mut out) = {
        let inner = rt.cache.lock().unwrap_or_else(PoisonError::into_inner);
        let stale = inner.verified_at.is_none_or(|at| at.elapsed() >= EXT_VERIFY_FRESH);
        let need_worker = inner.dirty || inner.stored_fp != fp || stale;

        let mut list: Vec<ProfileExtension> = stored
            .iter()
            .map(|rec| {
                let eng_bad = engine_failed.contains(&rec.id);
                let cache_bad = inner.changed_ids.contains(&rec.id);
                let is_changed = eng_bad || cache_bad;
                let cached_match = inner
                    .stored_fp
                    .iter()
                    .find(|f| f.id == rec.id && f.directory == rec.directory && f.digest == rec.digest)
                    .and_then(|_| inner.profiles.iter().find(|p| p.id == rec.id));
                match cached_match {
                    Some(prof) => {
                        let mut p = prof.clone();
                        p.changed = is_changed || rec.digest.is_none();
                        p.enabled = rec.enabled && !p.changed && p.error.is_none();
                        p
                    }
                    None => fallback_profile(rec, is_changed),
                }
            })
            .collect();
        list.sort_by_key(|e| e.name.to_ascii_lowercase());
        (need_worker, list)
    };

    if should_spawn {
        schedule_startup_sync(app, stored.to_vec(), false, false);
    }
    // Keep stable sort even if fallback names matched IDs.
    out.sort_by_key(|e| e.name.to_ascii_lowercase());
    out
}

/// Schedules off-thread SHA-256 verification + manifest/icon description for `stored`.
/// Never performs filesystem hashing on the UI thread and serializes at most one worker thread per
/// window; the worker wakes the UI via `app.waker` only when verified specs or rows change.
pub fn schedule_startup_sync(
    app: &App,
    stored: Vec<StoredExtension>,
    force_rehash: bool,
    restart_if_live: bool,
) {
    let Ok(mgr) = manager() else {
        return;
    };
    let rt = app.kernel.slots.get(&BROWSER_EXTENSIONS);
    let cache = rt.cache.clone();
    let req = VerifyRequest {
        stored,
        engine_failed: rt.integrity_failed.clone(),
        force_rehash,
        restart_if_live,
    };
    {
        let mut inner = cache.lock().unwrap_or_else(PoisonError::into_inner);
        if inner.in_flight {
            let merged_force = force_rehash || inner.queued.as_ref().is_some_and(|q| q.force_rehash);
            let merged_restart = restart_if_live || inner.queued.as_ref().is_some_and(|q| q.restart_if_live);
            inner.queued = Some(VerifyRequest {
                stored: req.stored,
                engine_failed: req.engine_failed,
                force_rehash: merged_force,
                restart_if_live: merged_restart,
            });
            return;
        }
        inner.in_flight = true;
    }
    let waker = (*app.waker).clone();
    let worker_cache = cache.clone();
    let spawned = std::thread::Builder::new()
        .name("browser-ext-verify".into())
        .spawn(move || run_verify_worker(worker_cache, mgr, req, waker));
    if spawned.is_err() {
        let mut inner = cache.lock().unwrap_or_else(PoisonError::into_inner);
        inner.in_flight = false;
    }
}

fn run_verify_worker(
    cache: Arc<Mutex<ExtCacheInner>>,
    mgr: ExtensionManager,
    mut req: VerifyRequest,
    waker: Option<rune::Waker>,
) {
    loop {
        let (swept, prev_fp, prev_stamps, had_verified, started_gen) = {
            let inner = cache.lock().unwrap_or_else(PoisonError::into_inner);
            (
                inner.swept_staging,
                inner.stored_fp.clone(),
                inner.dir_stamps.clone(),
                inner.verified_at.is_some(),
                inner.dirty_gen,
            )
        };
        if !swept {
            mgr.sweep_staging();
        }
        let fp = stored_fingerprint(&req.stored);
        let stamps: Vec<(String, DirStamp)> = req
            .stored
            .iter()
            .map(|r| (r.id.clone(), dir_stamp(Path::new(&r.directory))))
            .collect();

        let stamp_unchanged =
            had_verified && !req.force_rehash && !req.restart_if_live && prev_fp == fp && prev_stamps == stamps;
        let should_wake = if stamp_unchanged {
            let mut inner = cache.lock().unwrap_or_else(PoisonError::into_inner);
            inner.swept_staging = true;
            inner.verified_at = Some(Instant::now());
            if inner.dirty_gen == started_gen {
                inner.dirty = false;
            }
            false
        } else {
            let verified = mgr.verify_enabled_for_startup(&req.stored);
            let mut changed_ids = req.engine_failed.clone();
            for id in verified.changed_ids {
                changed_ids.insert(id);
            }
            let mut specs: Vec<StartupExtensionSpec> = Vec::with_capacity(verified.specs.len());
            let mut has_filters = false;
            for spec in verified.specs {
                if changed_ids.contains(&spec.id) {
                    continue;
                }
                if !has_filters
                    && let Some(manifest) = read_manifest(Path::new(&spec.dir))
                    && manifest
                        .permissions
                        .iter()
                        .any(|p| p == "declarativeNetRequest" || p == "declarativeNetRequestWithHostAccess")
                {
                    has_filters = true;
                }
                specs.push(StartupExtensionSpec::from(spec));
            }
            let mut profiles: Vec<ProfileExtension> = req
                .stored
                .iter()
                .filter_map(|rec| {
                    let is_changed = changed_ids.contains(&rec.id);
                    mgr.describe(rec, is_changed, None)
                })
                .collect();
            profiles.sort_by_key(|e| e.name.to_ascii_lowercase());

            let mut inner = cache.lock().unwrap_or_else(PoisonError::into_inner);
            let profiles_changed = !had_verified || inner.profiles != profiles || inner.changed_ids != changed_ids;
            let specs_changed = !had_verified || inner.specs != specs || inner.has_filters != has_filters;
            inner.swept_staging = true;
            inner.stored_fp = fp;
            inner.dir_stamps = stamps;
            inner.changed_ids = changed_ids;
            inner.specs = specs.clone();
            inner.has_filters = has_filters;
            inner.profiles = profiles;
            inner.verified_at = Some(Instant::now());
            if inner.dirty_gen == started_gen {
                inner.dirty = false;
            }
            if profiles_changed || specs_changed || req.restart_if_live {
                let prev_restart = inner.pending_apply.as_ref().is_some_and(|p| p.restart_if_live);
                let prev_prof = inner.pending_apply.as_ref().is_some_and(|p| p.profiles_changed);
                inner.pending_apply = Some(PendingApply {
                    specs,
                    has_filters,
                    profiles_changed: profiles_changed || prev_prof,
                    restart_if_live: req.restart_if_live || prev_restart,
                });
                true
            } else {
                false
            }
        };

        if should_wake && let Some(w) = &waker {
            w();
        }

        let next = {
            let mut inner = cache.lock().unwrap_or_else(PoisonError::into_inner);
            match inner.queued.take() {
                Some(q) => Some(q),
                None => {
                    inner.in_flight = false;
                    None
                }
            }
        };
        let Some(next_req) = next else {
            break;
        };
        req = next_req;
    }
}

/// Maps crate [`InstallFailure`] values to stable i18n catalog codes (never raw Rust strings).
pub fn failure_code_str(reason: InstallFailure) -> &'static str {
    match reason {
        InstallFailure::Manifest => "manifest",
        InstallFailure::Download => "download",
        InstallFailure::Load => "load",
        InstallFailure::Collision => "collision",
        InstallFailure::Expired => "expired",
        InstallFailure::Downgrade => "downgrade",
    }
}

/// Executes non-swap cleanup effects returned by [`InstallMachine::step`].
pub fn run_cleanup_effects(mgr: &ExtensionManager, effects: &[InstallEffect]) {
    for eff in effects {
        if let InstallEffect::RemoveStagingDir(dir) = eff {
            mgr.remove_staging_dir(dir);
        }
    }
}

/// Drains worker verification updates and engine extension events (integrity failures,
/// extension-opened tabs, focus requests). Returns `true` only when visible state or startup specs
/// actually changed (no spurious `Dep::Browser` invalidation).
pub fn poll(app: &mut App) -> bool {
    let mut changed = false;
    let pending = {
        let cache = &app.kernel.slots.get(&BROWSER_EXTENSIONS).cache;
        let mut inner = cache.lock().unwrap_or_else(PoisonError::into_inner);
        inner.pending_apply.take()
    };
    if let Some(apply) = pending {
        let failed = app.kernel.slots.get(&BROWSER_EXTENSIONS).integrity_failed.clone();
        let specs: Vec<_> = apply
            .specs
            .into_iter()
            .filter(|s| !failed.contains(&s.id))
            .collect();
        let has_filters = apply.has_filters && !specs.is_empty();
        let specs_changed = app
            .workspace_panes
            .browser
            .host
            .set_startup_extensions(specs, has_filters);
        if apply.restart_if_live
            && app.workspace_panes.browser.host.has_live_sidecar()
            && app.workspace_panes.browser.host.engine_extensions()
        {
            app.workspace_panes.browser.host.restart_engine_for_extensions();
        }
        changed |= specs_changed || apply.profiles_changed;
    }

    // Every tick, not only when another extension event arrived: the engine starts with
    // notifications off until told, and the Settings and engine-down clears can't wait.
    poll_extension_notifications(app);
    if !app.workspace_panes.browser.host.has_extension_events() {
        return changed;
    }

    let integrity_ids: Vec<String> = app
        .workspace_panes
        .browser
        .host
        .drain_extension_integrity_failures()
        .collect();
    if !integrity_ids.is_empty() {
        let rt = app.kernel.slots.get_mut(&BROWSER_EXTENSIONS);
        for id in integrity_ids {
            rt.integrity_failed.insert(id);
        }
        let mut inner = rt.cache.lock().unwrap_or_else(PoisonError::into_inner);
        inner.dirty = true;
        inner.dirty_gen = inner.dirty_gen.wrapping_add(1);
        changed = true;
    }

    let open_reqs: Vec<_> = app
        .workspace_panes
        .browser
        .host
        .drain_extension_open_requests()
        .collect();
    for req in open_reqs {
        if let Some(sid) = app.active_session_id().map(str::to_owned) {
            let tab_id = app
                .workspace_panes
                .browser
                .host
                .answer_extension_open_tab(req.request_id, &req.url, Vec::new());
            app.workspace_panes
                .browser
                .session_tabs
                .entry(sid)
                .or_default()
                .push(tab_id);
            changed = true;
        } else {
            app.workspace_panes
                .browser
                .host
                .cancel_extension_open_tab(req.request_id);
        }
    }

    let focus_ids: Vec<u64> = app
        .workspace_panes
        .browser
        .host
        .drain_extension_focus_tabs()
        .collect();
    for tab_id in focus_ids {
        app.workspace_panes.browser.host.focus_tab(tab_id);
        changed = true;
    }

    let _ = app
        .workspace_panes
        .browser
        .host
        .drain_extension_lifecycle_events()
        .count();

    changed
}

/// Syncs app notification settings to the browser engine and delivers extension OS notifications
/// (`chrome.notifications`, `extension-api/notifications.ts:18-92`) via `opencode-feature-notifications`.
fn poll_extension_notifications(app: &mut App) {
    let allowed = crate::notifications::extension_notifications_allowed(app);
    app.workspace_panes
        .browser
        .host
        .set_extension_notifications_enabled(allowed);
    // Settings > Notifications off takes this window's extension toasts down; an engine that
    // idled out or crashed can't hear their clicks, so its toasts go too.
    let host = &app.workspace_panes.browser.host;
    let engine = host.engine_extensions() && matches!(host.status(), browser_pane::EngineStatus::Ready);
    if !allowed || !engine {
        let cleared = crate::notifications::clear_all_extensions(app);
        if engine {
            for (ext_id, notif_id) in cleared {
                app.workspace_panes.browser.host.send_extension_notification_event(ext_id, notif_id, false, false);
            }
        }
    }
    let ops: Vec<_> = app
        .workspace_panes
        .browser
        .host
        .drain_extension_notification_ops()
        .collect();
    for op in ops {
        match op {
            // A toast from an engine that is no longer up couldn't hear its click.
            browser_pane::extensions::ExtensionNotificationOp::Show { .. } if !engine => {}
            browser_pane::extensions::ExtensionNotificationOp::Show {
                ext_id,
                ext_name,
                notif_id,
                body,
            } => {
                let os_id = format!("ext-{ext_id}-{notif_id}");
                // A web page's own (`page:<tab>`, the engine's notification gate) reads as the
                // page wrote it, as official's Electron shows it; its site is the body's last line.
                let title = if ext_id.starts_with("page:") {
                    ext_name
                } else {
                    opencode_i18n::t!("rune.browser.extension.notification.title", name = ext_name).to_string()
                };
                if !crate::notifications::post_extension(app, &os_id, &title, &body, &ext_id, &notif_id) {
                    app.workspace_panes.browser.host.send_extension_notification_event(
                        ext_id, notif_id, false, false,
                    );
                }
            }
            browser_pane::extensions::ExtensionNotificationOp::Clear { ext_id, notif_id } => {
                crate::notifications::clear_extension(app, &format!("ext-{ext_id}-{notif_id}"));
            }
        }
    }
    for (ext_id, notif_id) in crate::notifications::take_extension_evicted(app) {
        app.workspace_panes.browser.host.send_extension_notification_event(ext_id, notif_id, false, false);
    }
}

/// Focuses a newly opened extension popup immediately, keeps popup focus state synced with Rune's
/// real focus target, and restores focus to the page on close.
pub fn poll_popup_focus(app: &mut App, tx: &mut rune::Tx) -> bool {
    if app.workspace_panes.browser.host.take_popup_needs_focus() {
        tx.focus("browser-ext-popup", false);
        app.workspace_panes.browser.host.set_popup_focused(true);
        return true;
    }
    if app.workspace_panes.browser.host.popup_tab_id().is_some() {
        app.workspace_panes
            .browser
            .host
            .set_popup_focused(tx.focused() == Some("browser-ext-popup"));
        return false;
    }
    if app.workspace_panes.browser.host.take_popup_refocus_page()
        && tx.focused() == Some("browser-ext-popup")
    {
        tx.focus("browser-surface", false);
        return true;
    }
    false
}

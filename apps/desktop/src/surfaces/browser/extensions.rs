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
use std::sync::{Arc, Mutex};

use browser_extensions::manager::{ExtensionManager, StagedInstall};
use browser_extensions::manifest::read_manifest;
use browser_extensions::review::{InstallEffect, InstallEvent, InstallMachine};
use browser_extensions::sources::PlatformDirs;
use browser_extensions::{InstallFailure, StoredExtension};
use browser_pane::StartupExtensionSpec;
use opencode_kernel::Slot;
use rune::time::Instant;

use crate::App;

/// Chrome version token advertised when requesting CRX3 updates from the Chrome Web Store / Edge Add-ons.
pub const CHROME_STORE_VERSION: &str = "156.0.0.0";

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
    swept_staging: bool,
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

/// Ensures leftover `.staging-*` directories from earlier crashes are swept once per window.
pub fn sweep_staging_once(app: &mut App) {
    let rt = app.kernel.slots.get_mut(&BROWSER_EXTENSIONS);
    if rt.swept_staging {
        return;
    }
    rt.swept_staging = true;
    if let Ok(mgr) = manager() {
        mgr.sweep_staging();
    }
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

/// Re-verifies every enabled [`StoredExtension`] on disk and pushes verified
/// `--extension <id>:<sha256>:<dir>` specs into [`browser_pane::BrowserHost`].
/// Returns IDs whose on-disk integrity failed verification (including any engine integrity alerts).
pub fn sync_startup_specs(app: &mut App, stored: &[StoredExtension]) -> HashSet<String> {
    sweep_staging_once(app);
    let mut changed: HashSet<String> = app
        .kernel
        .slots
        .get(&BROWSER_EXTENSIONS)
        .integrity_failed
        .clone();
    let Ok(mgr) = manager() else {
        app.workspace_panes.browser.host.set_startup_extensions(Vec::new(), false);
        return changed;
    };
    let verified = mgr.verify_enabled_for_startup(stored);
    for id in verified.changed_ids {
        changed.insert(id);
    }
    let mut specs: Vec<StartupExtensionSpec> = Vec::with_capacity(verified.specs.len());
    let mut has_filters = false;
    for spec in verified.specs {
        if changed.contains(&spec.id) {
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
    app.workspace_panes.browser.host.set_startup_extensions(specs, has_filters);
    changed
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

/// Drains engine extension events (integrity failures, extension-opened tabs, focus requests).
pub fn poll(app: &mut App) -> bool {
    // Every tick, not only when another extension event arrived: the engine starts with
    // notifications off until told, and the Settings and engine-down clears can't wait.
    poll_extension_notifications(app);
    if !app.workspace_panes.browser.host.has_extension_events() {
        return false;
    }
    let mut changed = false;

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
                let title = opencode_i18n::t!("rune.browser.extension.notification.title", name = ext_name).to_string();
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

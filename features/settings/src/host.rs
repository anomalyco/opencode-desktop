//! Object-safe host boundary, process-wide configuration, and lightweight DTOs for
//! `opencode-feature-settings`.
//!
//! Every state machine, search index, key handler, job dispatcher, and poll entrypoint in this
//! crate operates on `&dyn SettingsHost` / `&mut dyn SettingsHost`. Thin generic bounds
//! (`H: SettingsHost + 'static` or `H: SettingsHost + PopupHost + 'static`) appear strictly at
//! `rune::El` view/callback boundaries and delegate in 1 line to non-generic functions.

use std::sync::OnceLock;

use opencode_kit::integration::{Integration, McpServer};
use opencode_kit::model::{Project, SessionInfo};
use opencode_kit::servers::health::Health;
use opencode_kit::servers::settings::Availability;
use opencode_kit::servers::{ServerConn, ServerKey};
use opencode_kit::wire_error::WireError;
use opencode_ui::keymap::Section as KeymapSection;
use rune::{Chord, El, Icon, Tx, el, icon};
use rune_theme_opencode::icons as ic;
use session_view::palette::Pal;
use session_view::pickers::Model;

use crate::Settings;
use crate::jobs::SettingsJob;
use crate::pages;
use crate::types::{ReduceMotion, TabLayout, TextRenderer};

/// Process-wide configuration registered once at startup by the host binary.
#[derive(Clone, Copy)]
pub struct SettingsConfig {
    pub build_channel: &'static str,
    pub build_version: &'static str,
}

const DEFAULT_CONFIG: SettingsConfig = SettingsConfig {
    build_channel: "local",
    build_version: "0.0.0",
};

static CONFIG: OnceLock<SettingsConfig> = OnceLock::new();

/// Registers the host's build channel and version fallback.
/// Call once at startup before reading build metadata.
pub fn configure(config: SettingsConfig) {
    CONFIG.set(config).ok();
}

pub fn build_channel() -> &'static str {
    CONFIG.get().map_or(DEFAULT_CONFIG.build_channel, |c| c.build_channel)
}

pub fn build_version() -> &'static str {
    CONFIG.get().map_or(DEFAULT_CONFIG.build_version, |c| c.build_version)
}

/// One connected or available provider row in Settings › Providers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderRow {
    pub id: String,
    pub name: String,
    pub integration: Option<String>,
}

/// One host command entry listed in Settings › Shortcuts.
#[derive(Clone, Debug)]
pub struct CommandItem {
    pub id: &'static str,
    pub title: String,
    pub section: Option<KeymapSection>,
    pub chord: Option<Chord>,
    pub has_keys: bool,
}

/// One plugin-contributed search row indexed by Settings search.
#[derive(Clone, Debug)]
pub struct PluginSearchItem {
    pub owner: String,
    pub page: String,
    pub id: String,
    pub title: String,
    pub description: String,
    pub keywords: String,
}

/// Metadata for a plugin-contributed page shown in the Settings rail.
#[derive(Clone, Debug)]
pub struct PluginPageMeta {
    pub id: String,
    pub title: String,
    pub icon: &'static Icon,
    /// Placed in the final group before Experimental and About.
    pub in_about_group: bool,
    /// Placed at the end of the server group (e.g. Pairing).
    pub in_server_group: bool,
}

/// An HTTP server added by address (`servers.json` / global `server.list`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpServer {
    pub url: String,
    pub password: Option<String>,
    pub name: Option<String>,
}

impl HttpServer {
    /// `serverName`: its name, else its address without the scheme.
    pub fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| {
            self.url
                .trim_start_matches("http://")
                .trim_start_matches("https://")
                .trim_end_matches('/')
                .to_owned()
        })
    }
}

/// Descriptor for a plugin-contributed server in Settings › Servers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginServerInfo {
    pub key: String,
    pub name: String,
    pub label: String,
    pub listed: bool,
    pub state_name: &'static str,
}

/// Where a server in Settings' inventory comes from.
#[derive(Clone, Debug)]
pub enum ServerKind {
    /// The app's own server (`"sidecar"`).
    Own { address: String },
    /// Added by address.
    Http(HttpServer),
    /// Listed by a plugin's server source (`<plugin>:<id>`).
    Plugin(PluginServerInfo),
}

/// One server as Settings shows it (`SettingsServer`).
#[derive(Clone, Debug)]
pub struct ServerEntry {
    pub key: ServerKey,
    pub name: String,
    pub kind: ServerKind,
    /// Its listed connection; a plugin's has one only while its plugin reports it ready.
    pub connection: Option<ServerConn>,
    pub health: Option<Health>,
    pub availability: Availability,
}

impl ServerEntry {
    /// The address its row shows: the connection's, or what the app's own server answers on.
    pub fn address(&self) -> String {
        match (&self.kind, &self.connection) {
            (ServerKind::Own { address }, _) => address.clone(),
            (_, Some(c)) => c.http().map(|h| h.url.clone()).unwrap_or_default(),
            (ServerKind::Http(s), None) => s.url.clone(),
            (ServerKind::Plugin(_), None) => String::new(),
        }
    }
}

/// Version and contributor count shown on Settings › About.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AboutInfo {
    pub version: String,
    pub other_contributors: u32,
}

impl Default for AboutInfo {
    fn default() -> Self {
        Self {
            version: build_version().to_owned(),
            other_contributors: pages::about::FALLBACK_OTHERS,
        }
    }
}

/// Object-safe host boundary for the Settings screen, preferences, search, and all built-in pages.
pub trait SettingsHost {
    fn settings(&self) -> &Settings;
    fn settings_mut(&mut self) -> &mut Settings;
    fn settings_pages(&self) -> &pages::State;
    fn settings_pages_mut(&mut self) -> &mut pages::State;

    /// Whether this window persists preference and fold changes to storage.
    fn persists(&self) -> bool {
        false
    }
    /// Whether this window may issue mutating requests to the server.
    fn can_write(&self) -> bool {
        false
    }
    /// Monotonic frame clock in seconds.
    fn clock(&self) -> f64 {
        0.0
    }
    /// Wall clock in Unix milliseconds.
    fn now_ms(&self) -> i64 {
        0
    }
    fn reduced_motion(&self) -> bool {
        false
    }
    fn zoom(&self) -> f32 {
        1.0
    }
    fn set_zoom_level(&mut self, level: f32) {
        let _ = level;
    }
    fn tab_layout(&self) -> TabLayout {
        self.settings().tab_layout
    }
    fn set_tab_layout(&mut self, layout: TabLayout, tx: &mut Tx) {
        let _ = tx;
        let before = self.settings().to_json();
        self.settings_mut().tab_layout = layout;
        crate::persist_settings_diff(self.persists(), &before, self.settings());
    }
    fn narrow(&self) -> bool {
        false
    }
    /// Whether the viewport is at least 640 pt wide (`layout.narrow.sm()`).
    fn narrow_sm(&self) -> bool {
        true
    }
    /// Live server base URL when persisting per-server model provider folds.
    fn live_server_base(&self) -> Option<String> {
        None
    }

    // ── Appearance, language, sounds & preferences hooks ──────────────────────────────────────

    fn set_language(&mut self, id: &str) {
        let before = self.settings().to_json();
        self.settings_mut().language = Some(id.to_owned());
        crate::persist_settings_diff(self.persists(), &before, self.settings());
    }
    fn set_auto_accept(&mut self, on: bool) {
        let before = self.settings().to_json();
        self.settings_mut().auto_accept = on;
        crate::persist_settings_diff(self.persists(), &before, self.settings());
    }
    fn set_project_auto_accept(&mut self, project_id: &str, value: Option<bool>) {
        let before = self.settings().to_json();
        match value {
            Some(on) => {
                self.settings_mut().project_auto_accept.insert(project_id.to_owned(), on);
            }
            None => {
                self.settings_mut().project_auto_accept.remove(project_id);
            }
        }
        crate::persist_settings_diff(self.persists(), &before, self.settings());
    }
    fn set_pinch_zoom(&mut self, on: bool, tx: &mut Tx) {
        tx.instant();
        let before = self.settings().to_json();
        self.settings_mut().pinch_zoom = on;
        crate::persist_settings_diff(self.persists(), &before, self.settings());
    }
    fn set_theme(&mut self, id: &str, tx: &mut Tx) {
        tx.instant();
        let all = session_view::themes::all();
        let resolved = all.get(session_view::themes::index(id)).map_or_else(|| session_view::themes::DEFAULT.to_owned(), |t| t.id.clone());
        session_view::themes::set_active(&resolved);
        let before = self.settings().to_json();
        self.settings_mut().theme = resolved;
        crate::persist_settings_diff(self.persists(), &before, self.settings());
    }
    fn set_ui_font(&mut self, index: usize, custom_name: &str, tx: &mut Tx) {
        tx.instant();
        let before = self.settings().to_json();
        let s = self.settings_mut();
        s.ui_font = index.min(crate::types::UI_FONTS.len() - 1);
        s.ui_font_name = custom_name.to_owned();
        crate::persist_settings_diff(self.persists(), &before, self.settings());
    }
    fn set_code_font(&mut self, index: usize, custom_name: &str, tx: &mut Tx) {
        tx.instant();
        let before = self.settings().to_json();
        let s = self.settings_mut();
        s.code_font = index.min(crate::types::CODE_FONTS.len() - 1);
        s.code_font_name = custom_name.to_owned();
        crate::persist_settings_diff(self.persists(), &before, self.settings());
    }
    fn set_terminal_font(&mut self, index: usize, custom_name: &str, tx: &mut Tx) {
        tx.instant();
        let before = self.settings().to_json();
        let s = self.settings_mut();
        s.terminal_font = index.min(crate::types::TERMINAL_FONTS.len() - 1);
        s.terminal_font_name = custom_name.to_owned();
        crate::persist_settings_diff(self.persists(), &before, self.settings());
    }
    fn set_reduce_motion(&mut self, mode: ReduceMotion) {
        let before = self.settings().to_json();
        self.settings_mut().reduce_motion = mode;
        crate::persist_settings_diff(self.persists(), &before, self.settings());
    }
    fn set_text_renderer(&mut self, renderer: TextRenderer) {
        let before = self.settings().to_json();
        self.settings_mut().text_renderer = renderer;
        crate::persist_settings_diff(self.persists(), &before, self.settings());
    }
    fn text_rendering_fallback_reason(&self) -> Option<&'static str> {
        None
    }
    fn play_sound(&mut self, id: &str) {
        let _ = id;
    }
    fn developer_section(&self, p: &Pal) -> Option<El> {
        let _ = p;
        None
    }

    // ── About & copy feedback ─────────────────────────────────────────────────────────────────

    fn about_info(&self) -> AboutInfo {
        AboutInfo::default()
    }
    /// Optional host-provided random value in `[0, 1)` so About's wordmark can share a seeded
    /// proof-run RNG stream; `None` uses the crate's local xorshift64* stream.
    fn about_random_unit(&self, seed: u64) -> Option<f64> {
        let _ = seed;
        None
    }
    fn on_wordmark_tick(&mut self, now: f64, letters: &str) {
        let _ = (now, letters);
    }
    fn is_copied(&self, key: &str) -> bool {
        let _ = key;
        false
    }
    fn mark_copied(&mut self, key: &str, tx: &mut Tx) {
        let _ = (key, tx);
    }

    // ── Providers & Models ────────────────────────────────────────────────────────────────────

    fn sync_catalogs(&mut self, dir: Option<&str>) {
        let _ = dir;
    }
    fn integrations(&self, dir: Option<&str>) -> Option<&[Integration]> {
        let _ = dir;
        None
    }
    fn provider_rows(&self, dir: Option<&str>) -> Vec<ProviderRow> {
        let _ = dir;
        Vec::new()
    }
    fn open_connect(&mut self, provider: Option<&str>, from_settings: bool, tx: &mut Tx) {
        let _ = (provider, from_settings, tx);
    }
    fn models(&self) -> &[Model] {
        &[]
    }
    fn model_visible(&self, model: &Model) -> bool {
        !model.hidden
    }
    fn set_model_visibility(&mut self, key: &str, visible: bool) {
        let _ = (key, visible);
    }

    // ── MCP & Extensions ──────────────────────────────────────────────────────────────────────

    fn sync_mcp(&mut self, dir: &str) {
        let _ = dir;
    }
    fn mcp_servers(&self, dir: &str) -> Option<&[McpServer]> {
        let _ = dir;
        None
    }
    fn mcp_row_trailing(&self, p: &Pal, dir: &str, server: &McpServer) -> El {
        let _ = (p, dir, server);
        el()
    }

    // ── Plugins ───────────────────────────────────────────────────────────────────────────────

    fn wake_settings_plugins(&mut self) {}
    fn plugin_pages(&self) -> Vec<PluginPageMeta> {
        Vec::new()
    }
    fn plugin_page_view(&self, p: &Pal, id: &str) -> Option<(String, String, El, Option<El>)> {
        let _ = (p, id);
        None
    }
    fn plugin_sections_for_page(&self, p: &Pal, page_id: &str) -> Vec<El> {
        let _ = (p, page_id);
        Vec::new()
    }
    fn plugin_extensions_section(&self, p: &Pal) -> Option<El> {
        let _ = p;
        None
    }
    fn plugin_search_entries(&self) -> Vec<PluginSearchItem> {
        Vec::new()
    }
    fn resolve_plugin_page(&self, page_id: &str) -> (String, Option<String>) {
        (page_id.to_owned(), None)
    }

    // ── Shortcuts / commands ──────────────────────────────────────────────────────────────────

    fn commands_catalog(&self) -> Vec<CommandItem> {
        Vec::new()
    }
    fn has_command(&self, id: &str) -> bool {
        let _ = id;
        false
    }
    fn run_command(&mut self, id: &str, tx: &mut Tx) {
        let _ = (id, tx);
    }

    // ── Projects & Worktrees ──────────────────────────────────────────────────────────────────

    fn projects(&self) -> &[Project] {
        &[]
    }
    fn used_projects(&self) -> Vec<&Project> {
        self.projects().iter().collect()
    }
    fn other_projects(&self) -> Vec<&Project> {
        Vec::new()
    }
    fn project_at(&self, dir: &str) -> Option<&Project> {
        self.projects().iter().find(|p| p.directory == dir)
    }
    fn apply_project(&mut self, updated: Project) {
        let _ = updated;
    }
    fn touch_project(&mut self, id: &str) {
        let _ = id;
    }
    fn project_has_unread(&self, project: &Project) -> bool {
        let _ = project;
        false
    }
    fn clear_project_notifications(&mut self, project: &Project, tx: &mut Tx) {
        let _ = (project, tx);
    }
    fn open_project_dialog(&mut self, tx: &mut Tx) {
        let _ = tx;
    }
    fn project_badge(&self, p: &Pal, project: Option<&Project>, label: &str) -> El {
        let _ = (project, label);
        icon(&ic::FOLDER, 16.0).color(p.icon_muted)
    }
    fn project_avatar_image(&self, p: &Pal, project: &Project, size: f32) -> El {
        let _ = project;
        el().size(size, size).center().child(icon(&ic::FOLDER, 16.0).color(p.icon_muted))
    }
    fn reveal_label(&self) -> &'static str {
        if cfg!(target_os = "macos") {
            opencode_i18n::t!("session.header.reveal.finder")
        } else if cfg!(windows) {
            opencode_i18n::t!("session.header.reveal.fileExplorer")
        } else {
            opencode_i18n::t!("session.header.reveal.containingFolder")
        }
    }
    fn reveal_path(&self, dir: &str) -> Result<(), String> {
        let _ = dir;
        Err("reveal unavailable".into())
    }
    fn list_worktrees(&mut self, project_id: &str) {
        let _ = project_id;
    }
    fn project_worktrees(&self, project_id: &str, directory: &str) -> Vec<String> {
        let _ = (project_id, directory);
        Vec::new()
    }
    fn sessions(&self) -> &[SessionInfo] {
        &[]
    }
    fn active_session_directory(&self) -> Option<String> {
        None
    }

    // ── Servers ───────────────────────────────────────────────────────────────────────────────

    fn servers_inventory(&self) -> Vec<ServerEntry> {
        vec![pages::servers::inventory::fallback_own()]
    }
    fn server_health(&self, key: &ServerKey) -> Option<Health> {
        let _ = key;
        None
    }
    fn desired_server_key(&self) -> ServerKey {
        ServerKey::sidecar()
    }
    fn active_server_key(&self) -> ServerKey {
        ServerKey::sidecar()
    }
    fn is_server_listed(&self, key: &ServerKey) -> bool {
        key.is_sidecar()
    }
    fn switch_server(&mut self, key: &ServerKey) {
        let _ = key;
    }
    fn retry_server_health(&mut self, tx: &mut Tx) -> Option<u64> {
        let _ = tx;
        None
    }
    fn server_health_retry_resolved(&self, waiter: u64) -> bool {
        let _ = waiter;
        false
    }
    fn open_server_dialog(&mut self, edit_url: Option<&str>, tx: &mut Tx) {
        let _ = (edit_url, tx);
    }
    fn remove_http_server(&mut self, url: &str, tx: &mut Tx) {
        let _ = (url, tx);
    }
    fn server_add_menu_items(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    fn run_server_add_menu_item(&mut self, key: &str, tx: &mut Tx) {
        let _ = (key, tx);
    }
    fn plugin_server_row(&self, p: &Pal, key: &str) -> Option<El> {
        let _ = (p, key);
        None
    }
    fn can_page_sign_out(&self) -> bool {
        false
    }
    fn is_page_server(&self, url: &str) -> bool {
        let _ = url;
        false
    }
    fn page_sign_out(&mut self) {}

    // ── Shell navigation, focus, toasts, jobs & overlays ──────────────────────────────────────

    /// Prepares the host before Settings opens (closes pickers/switcher, blurs composer).
    fn before_settings_open(&mut self) {}
    /// Handles project-navigation back-stack before closing Settings; returns true if handled.
    fn try_navigate_back_from_settings(&mut self, tx: &mut Tx) -> bool {
        let _ = tx;
        false
    }
    /// Restores focus (e.g. composer or blur) after leaving Settings via Back/Escape.
    fn restore_focus_after_settings_close(&mut self, tx: &mut Tx) {
        tx.blur();
    }
    /// Invoked after `toggle_settings` opens Settings (e.g. focuses `"settings"` over a session).
    fn on_toggle_settings_opened(&mut self, tx: &mut Tx) {
        let _ = tx;
    }
    fn open_external(&mut self, url: &str, tx: &mut Tx) {
        drop(tx.open_url(url));
    }
    fn toast_say(&mut self, title: String, description: String) {
        let _ = (title, description);
    }
    fn toast_fail(&mut self, title: String, description: String) {
        let _ = (title, description);
    }
    fn toast_succeed(&mut self, title: String, description: String) {
        let _ = (title, description);
    }
    fn toast_failed_wire(&mut self, title: String, err: &WireError) {
        self.toast_fail(title, err.to_string());
    }
    /// Schedules an off-thread server request whose [`crate::jobs::SettingsReply`] comes back
    /// through [`crate::jobs::apply_reply`].
    fn spawn_job(&mut self, job: SettingsJob) {
        let _ = job;
    }
    /// Registers an opened settings overlay (`OVERLAY_ID`, etc.) with `OverlayStack`.
    fn on_overlay_opened(&mut self, id: &'static str, return_focus: Option<String>);
    /// Removes a closed settings overlay from `OverlayStack`.
    fn on_overlay_closed(&mut self, id: &'static str);
}

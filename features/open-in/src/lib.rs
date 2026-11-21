//! Open a file or folder in an external editor or terminal, reveal it in the file manager, or
//! open an external URL, as Desktop's file panel does (`gui-plugins/src/file/open-in-app.tsx`,
//! `desktop/src/main/files`).
//!
//! Each platform lists Desktop's apps for it (macOS's editors and terminals; on Windows VS Code,
//! Cursor, Zed, PowerShell, and beyond Desktop's Windows Terminal and PowerShell 7, then Sublime
//! Text), only those installed (checked once, off the UI thread), with the file manager first
//! (Finder, File Explorer). The choice is kept across launches and defaults to the file manager,
//! as Desktop's does. Nothing shows unless the server shares this machine's files.

#![cfg_attr(not(target_os = "windows"), forbid(unsafe_code))]

pub mod drag;
#[cfg(target_os = "windows")]
pub mod windows;

use std::cell::Cell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use rune::{DragItem, Latest, Tx, Waker};
use serde::Serialize;
use serde_json::{Value, json};

pub const FINDER: &str = "finder";
pub const STORAGE_NS: &str = "plugin.file";
pub const STORAGE_KEY: &str = "openInApp";

pub struct OpenApp {
    pub id: &'static str,
    pub label: opencode_i18n::Key,
    /// Desktop's mark for it (`AppIcon`'s id).
    pub icon: &'static str,
    /// What the platform finds and opens it by: a bundle ID on macOS, a command elsewhere.
    pub open_with: &'static str,
}

const fn app(id: &'static str, label: opencode_i18n::Key, icon: &'static str, open_with: &'static str) -> OpenApp {
    OpenApp { id, label, icon, open_with }
}

/// Desktop's macOS list, in its order.
#[cfg(target_os = "macos")]
pub const APPS: &[OpenApp] = &[
    app("vscode", opencode_i18n::key!("plugin.file.open.app.vscode"), "vscode", "com.microsoft.VSCode"),
    app("cursor", opencode_i18n::key!("plugin.file.open.app.cursor"), "cursor", "com.todesktop.230313mzl4w4u92"),
    app("zed", opencode_i18n::key!("plugin.file.open.app.zed"), "zed", "dev.zed.Zed"),
    app("textmate", opencode_i18n::key!("plugin.file.open.app.textmate"), "textmate", "com.macromates.TextMate"),
    app("antigravity", opencode_i18n::key!("plugin.file.open.app.antigravity"), "antigravity", "com.google.antigravity"),
    app("terminal", opencode_i18n::key!("plugin.file.open.app.terminal"), "terminal", "com.apple.Terminal"),
    app("iterm2", opencode_i18n::key!("plugin.file.open.app.iterm2"), "iterm2", "com.googlecode.iterm2"),
    app("ghostty", opencode_i18n::key!("plugin.file.open.app.ghostty"), "ghostty", "com.mitchellh.ghostty"),
    app("warp", opencode_i18n::key!("plugin.file.open.app.warp"), "warp", "dev.warp.Warp-Stable"),
    app("xcode", opencode_i18n::key!("plugin.file.open.app.xcode"), "xcode", "com.apple.dt.Xcode"),
    app("android-studio", opencode_i18n::key!("plugin.file.open.app.androidStudio"), "android-studio", "com.google.android.studio"),
    app("sublime-text", opencode_i18n::key!("plugin.file.open.app.sublimeText"), "sublime-text", "com.sublimetext.4"),
];

/// Desktop's Windows list (`WINDOWS_OPEN_APPS`), in its order, by the commands it opens them with.
#[cfg(target_os = "windows")]
pub const APPS: &[OpenApp] = &[
    app("vscode", opencode_i18n::key!("plugin.file.open.app.vscode"), "vscode", "code"),
    app("cursor", opencode_i18n::key!("plugin.file.open.app.cursor"), "cursor", "cursor"),
    app("zed", opencode_i18n::key!("plugin.file.open.app.zed"), "zed", "zed"),
    app("powershell", opencode_i18n::key!("plugin.file.open.app.powershell"), "powershell", "powershell"),
    // Not Desktop's: what Windows developers open now (Windows Terminal, PowerShell 7).
    app("windows-terminal", opencode_i18n::key!("rune.openIn.windowsTerminal"), "windows-terminal", "wt"),
    app("pwsh", opencode_i18n::key!("rune.openIn.powershell7"), "powershell", "pwsh"),
    app("sublime-text", opencode_i18n::key!("plugin.file.open.app.sublimeText"), "sublime-text", "Sublime Text"),
];

/// Desktop's Linux list (and wasm32 fallback).
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const APPS: &[OpenApp] = &[
    app("vscode", opencode_i18n::key!("plugin.file.open.app.vscode"), "vscode", "code"),
    app("cursor", opencode_i18n::key!("plugin.file.open.app.cursor"), "cursor", "cursor"),
    app("zed", opencode_i18n::key!("plugin.file.open.app.zed"), "zed", "zed"),
    app("sublime-text", opencode_i18n::key!("plugin.file.open.app.sublimeText"), "sublime-text", "Sublime Text"),
];

/// The file manager's name and mark, as the host's project menus name it (`fileManagerApp`).
#[cfg(target_os = "macos")]
const FILE_MANAGER: (opencode_i18n::Key, &str) = (opencode_i18n::key!("plugin.file.open.finder"), "finder");
#[cfg(target_os = "windows")]
const FILE_MANAGER: (opencode_i18n::Key, &str) = (opencode_i18n::key!("plugin.file.open.fileExplorer"), "file-explorer");
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const FILE_MANAGER: (opencode_i18n::Key, &str) = (opencode_i18n::key!("plugin.file.open.fileManager"), "finder");

/// Localized display label for app `id`, falling back to the platform file manager.
pub fn label_of(id: &str) -> &'static str {
    opencode_i18n::I18N.text(APPS.iter().find(|a| a.id == id).map_or(FILE_MANAGER.0, |a| a.label))
}

/// App icon identifier for app `id`, falling back to the platform file manager.
pub fn icon_of(id: &str) -> &'static str {
    APPS.iter().find(|a| a.id == id).map_or(FILE_MANAGER.1, |a| a.icon)
}

/// Localized display label for this platform's file manager.
pub fn file_manager_label() -> &'static str {
    opencode_i18n::I18N.text(FILE_MANAGER.0)
}

/// App icon identifier for this platform's file manager.
pub fn file_manager_icon() -> &'static str {
    FILE_MANAGER.1
}

/// What launches and detects apps. The real one is the platform's; fixtures swap in [`Fixed`].
pub trait Launcher: Send + Sync + 'static {
    /// The apps among `apps` (by `OpenApp::open_with`) that are installed.
    fn installed(&self, apps: &[&'static str]) -> Vec<&'static str>;
    /// Opens `path` in the app found by `app`, or in its default app (a folder in the file manager).
    fn open(&self, path: &str, app: Option<&str>) -> Result<(), String>;
    /// Shows `path` selected in the file manager.
    fn reveal(&self, path: &str) -> Result<(), String>;
}

/// macOS (and non-Windows fallback): apps by bundle ID through `NSWorkspace`, opened with `open`.
#[cfg(not(target_os = "windows"))]
#[derive(Default)]
pub struct Mac;

#[cfg(not(any(target_os = "windows", target_arch = "wasm32")))]
fn run(cmd: &mut std::process::Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

#[cfg(not(target_os = "windows"))]
impl Launcher for Mac {
    fn installed(&self, bundles: &[&'static str]) -> Vec<&'static str> {
        #[cfg(target_os = "macos")]
        {
            let list = bundles.iter().map(|b| format!("\"{b}\"")).collect::<Vec<_>>().join(",");
            let script = format!(
                "ObjC.import('AppKit');[{list}].filter(function(i){{return !$.NSWorkspace.sharedWorkspace.URLForApplicationWithBundleIdentifier(i).isNil()}}).join('\\n')"
            );
            let out = run(std::process::Command::new("/usr/bin/osascript").args(["-l", "JavaScript", "-e", &script])).unwrap_or_default();
            bundles.iter().copied().filter(|b| out.lines().any(|l| l == *b)).collect()
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = bundles;
            Vec::new()
        }
    }

    fn open(&self, path: &str, bundle: Option<&str>) -> Result<(), String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let verified = browser_policy::verify_local_path(path).map_err(|e| e.to_string())?;
            // Never pass a path to bare `/usr/bin/open -- <path>` on macOS (which launches `.app`
            // and other bundles/packages); Finder-only requests always use safe reveal (`-R --`).
            let Some(b) = bundle else {
                return self.reveal(&verified.path);
            };
            if !APPS.iter().any(|a| a.open_with == b) {
                return Err("Unknown external application".into());
            }
            if rune::is_headless() {
                return Ok(());
            }
            #[cfg(target_os = "macos")]
            {
                run(std::process::Command::new("/usr/bin/open").args(["-b", b, "--", &verified.path])).map(|_| ())
            }
            #[cfg(not(target_os = "macos"))]
            {
                run(std::process::Command::new(b).args(["--", &verified.path])).map(|_| ())
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (path, bundle);
            Err("Opening local applications is not supported in the browser".into())
        }
    }

    fn reveal(&self, path: &str) -> Result<(), String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let verified = browser_policy::verify_local_path(path).map_err(|e| e.to_string())?;
            if log_harness_reveal(&verified.path) {
                return Ok(());
            }
            if rune::is_headless() {
                return Ok(());
            }
            browser_policy::reveal_in_file_manager(&verified.path).map(|_| ()).map_err(|e| e.to_string())
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            Err("Revealing local files is not supported in the browser".into())
        }
    }
}

/// The platform's default launcher.
#[cfg(target_os = "windows")]
pub type SystemLauncher = windows::Win;
/// The platform's default launcher.
#[cfg(not(target_os = "windows"))]
pub type SystemLauncher = Mac;

/// A fixture's stand-in: a fixed set of app IDs, and opening/revealing does nothing.
pub struct Fixed(pub Vec<&'static str>);

impl Launcher for Fixed {
    fn installed(&self, apps: &[&'static str]) -> Vec<&'static str> {
        apps.iter()
            .copied()
            .filter(|b| self.0.iter().any(|a| APPS.iter().any(|x| x.id == *a && x.open_with == *b)))
            .collect()
    }

    fn open(&self, _: &str, _: Option<&str>) -> Result<(), String> {
        Ok(())
    }

    fn reveal(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

/// Explicit state of background installed-app detection.
enum Installed {
    /// Unknown until the check answers, which starts the first time `options` is asked.
    Unknown { wanted: Cell<bool> },
    /// Detection completed; holds installed app IDs in platform order.
    Known(Vec<&'static str>),
}

/// Explicit state of an in-flight external app launch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum LaunchState {
    #[default]
    Idle,
    Launching,
}

/// Pending background results collected without mutating `OpenIn`.
struct Arrived {
    detect: bool,
    found: Option<Vec<&'static str>>,
    done: Option<Result<(), String>>,
}

/// Per-window external app detector and launcher state.
pub struct OpenIn {
    launcher: Arc<dyn Launcher>,
    found: Latest<Vec<&'static str>>,
    done: Latest<Result<(), String>>,
    installed: Installed,
    chosen: Option<String>,
    launch: LaunchState,
    dragged: drag::DragTracker,
}

/// What the first check in this process found, for every window after it.
static FOUND: Mutex<Option<Vec<&'static str>>> = Mutex::new(None);

/// Hard ceiling on distinct entries in [`PINNED_ROOTS`]. Past this limit, uncached roots fail
/// closed (`None`, matching `#294`) rather than falling open to live alias re-resolution.
const MAX_PINNED_ROOTS: usize = 4096;

/// Process-wide immutable table of pinned local workspace roots shared across all windows:
/// - **Key:** normalized raw local workspace/project/draft directory (`pin_key`: trimmed, `\` -> `/`,
///   trailing `/` stripped, ASCII-lowercased on Windows and macOS), plus the resolved physical path's
///   own normalized key so lookups on already-pinned physical paths hit without syscalls.
/// - **Value:** `Some(physical_path)` (the kernel-verified non-alias directory returned by
///   `browser_policy::resolve_safe_local_workspace_root`) or `None` when refused/unsafe/missing.
/// - **Lifecycle:** entries are created lazily on first use of a root anywhere in the process
///   (routing into a session, picking a draft folder, or invoking any local file feature on that
///   root — zero syscalls at boot for inactive listed projects). Once inserted, an entry is **never
///   updated or re-resolved for the life of the process**; past `MAX_PINNED_ROOTS` uncached roots
///   fail closed (`None`).
/// - **Re-pinning:** if a user intentionally moves or re-points a workspace symlink/junction on
///   disk and wants OpenCode to follow the new target, restart the app.
static PINNED_ROOTS: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();

fn pin_key(raw: &str) -> String {
    let mut norm = raw.trim().replace('\\', "/");
    while norm.len() > 1 && norm.ends_with('/') && !(norm.len() == 3 && norm.as_bytes()[1] == b':') {
        norm.pop();
    }
    if cfg!(any(windows, target_os = "macos")) {
        norm.to_ascii_lowercase()
    } else {
        norm
    }
}

/// Resolves and pins a local workspace root once per normalized raw path across the whole process,
/// returning the verified physical directory (never the raw symlink/junction alias) or `None` if
/// refused or past [`MAX_PINNED_ROOTS`] (fail-closed).
pub fn pinned_local_root(raw_dir: &str, home: Option<&str>) -> Option<String> {
    let key = pin_key(raw_dir);
    if key.is_empty() {
        return None;
    }
    let lock = PINNED_ROOTS.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let map = lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cached) = map.get(&key) {
            return cached.clone();
        }
        if map.len() >= MAX_PINNED_ROOTS {
            return None;
        }
    }
    let resolved = browser_policy::resolve_safe_local_workspace_root(raw_dir, home);
    let mut map = lock.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(cached) = map.get(&key) {
        return cached.clone();
    }
    if map.len() >= MAX_PINNED_ROOTS {
        return None;
    }
    if let Some(ref physical) = resolved {
        let phys_key = pin_key(physical);
        if !phys_key.is_empty() && phys_key != key && map.len() + 1 < MAX_PINNED_ROOTS {
            map.entry(phys_key).or_insert_with(|| Some(physical.clone()));
        }
    }
    map.insert(key, resolved.clone());
    resolved
}

impl Default for OpenIn {
    fn default() -> Self {
        Self::with(Arc::new(SystemLauncher::default()))
    }
}

impl OpenIn {
    pub fn with(launcher: Arc<dyn Launcher>) -> Self {
        Self {
            launcher,
            found: Latest::default(),
            done: Latest::default(),
            installed: Installed::Unknown { wanted: Cell::new(false) },
            chosen: None,
            launch: LaunchState::Idle,
            dragged: drag::DragTracker::default(),
        }
    }

    pub fn take_dragged_rel(&self, path: &Path) -> Option<String> {
        self.dragged.take(path)
    }

/// Convenience constructor for a fixture launcher with fixed installed app IDs.
    pub fn fixed(apps: Vec<&'static str>) -> Self {
        Self::with(Arc::new(Fixed(apps)))
    }

    pub fn set_waker(&mut self, waker: Waker) {
        self.found.set_waker(waker.clone());
        self.done.set_waker(waker);
    }

    /// Looks for installed apps in the background, once per process (a later window takes the first
    /// answer); until it answers the remembered choice stands.
    pub fn detect(&mut self) {
        if let Some(found) = FOUND.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            self.installed = Installed::Known(APPS.iter().filter(|a| found.contains(&a.open_with)).map(|a| a.id).collect());
            return;
        }
        let launcher = self.launcher.clone();
        self.found.request((), move |()| {
            let apps: Vec<&'static str> = APPS.iter().map(|a| a.open_with).collect();
            launcher.installed(&apps)
        });
    }

    /// What came in since the last poll, read without writing anything: `None` when nothing did.
    fn arrived(&self) -> Option<Arrived> {
        let detect = match &self.installed {
            Installed::Unknown { wanted } => wanted.take(),
            Installed::Known(_) => false,
        };
        let (found, done) = (self.found.take(), self.done.take());
        (detect || found.is_some() || done.is_some()).then_some(Arrived { detect, found, done })
    }

    fn apply_arrived(&mut self, arrived: Arrived) -> (bool, Option<String>) {
        let mut changed = false;
        if arrived.detect {
            self.detect();
            changed |= matches!(self.installed, Installed::Known(_));
        }
        if let Some(found) = arrived.found {
            *FOUND.lock().unwrap_or_else(|e| e.into_inner()) = Some(found.clone());
            self.installed = Installed::Known(APPS.iter().filter(|a| found.contains(&a.open_with)).map(|a| a.id).collect());
            changed = true;
        }
        let mut err = None;
        if let Some(r) = arrived.done {
            self.launch = LaunchState::Idle;
            changed = true;
            err = r.err();
        }
        (changed, err)
    }

    /// The file manager first, then what's installed: asking starts the check the first time.
    pub fn options(&self) -> Vec<(&'static str, &'static str)> {
        if let Installed::Unknown { wanted } = &self.installed {
            wanted.set(true);
        }
        self.listed()
    }

    /// `options` without starting the check, for what only shows the current choice.
    pub fn listed(&self) -> Vec<(&'static str, &'static str)> {
        let ids: Vec<&str> = match &self.installed {
            Installed::Known(ids) => ids.clone(),
            // Before the check answers, the remembered choice stands: it was installed when chosen.
            Installed::Unknown { .. } => self
                .chosen
                .as_deref()
                .and_then(|c| APPS.iter().find(|a| a.id == c))
                .map(|a| a.id)
                .into_iter()
                .collect(),
        };
        std::iter::once((FINDER, file_manager_label()))
            .chain(ids.into_iter().map(|id| (id, label_of(id))))
            .collect()
    }

    pub fn current(&self) -> &str {
        self.chosen.as_deref().filter(|c| self.listed().iter().any(|(id, _)| id == c)).unwrap_or(FINDER)
    }

    pub fn current_label(&self) -> &'static str {
        label_of(self.current())
    }

    pub fn current_icon(&self) -> &'static str {
        icon_of(self.current())
    }

    /// The file manager's localized label (the first listed option), for "Reveal in …".
    pub fn finder_label(&self) -> &'static str {
        self.listed().first().map_or("", |o| o.1)
    }

    pub fn set_chosen(&mut self, id: Option<String>) {
        self.chosen = id;
    }

    pub fn busy(&self) -> bool {
        self.launch == LaunchState::Launching
    }

    /// Reveals an absolute local fixed-drive `path` in the system file manager (selecting the
    /// item; never executes or opens it with its default application, and refuses UNC/device paths
    /// before touching the filesystem).
    pub fn reveal_local(&mut self, path: String) {
        if browser_policy::safe_local_reveal_path(&path).is_none() {
            return;
        }
        self.launch("", path, true);
    }

    /// Launches `app` on `path` in the background (or reveals `path` in the file manager).
    pub fn launch(&mut self, app: &str, path: String, reveal: bool) {
        if browser_policy::is_unc_or_device_path(&path) {
            return;
        }
        if self.launch == LaunchState::Launching {
            return;
        }
        self.launch = LaunchState::Launching;
        let launcher = self.launcher.clone();
        let open_with = APPS.iter().find(|a| a.id == app).map(|a| a.open_with);
        self.done.request((), move |()| match open_with {
            None if reveal => launcher.reveal(&path),
            None => launcher.open(&path, None),
            Some(b) => launcher.open(&path, Some(b)),
        });
    }
}

/// Whether `HARNESS_OPEN_EXTERNAL_LOG` may intercept external opens and reveals: only in debug
/// builds or headless driver runs (`--snap`, `--script`, `--tree`, …), never in a normal windowed
/// release launch.
#[cfg(not(target_arch = "wasm32"))]
fn allow_harness_log() -> bool {
    cfg!(debug_assertions) || rune::is_headless()
}

/// Logs a safe reveal to `HARNESS_OPEN_EXTERNAL_LOG` during debug or headless proofs without
/// spawning a real OS file manager window.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn log_harness_reveal(path: &str) -> bool {
    if !allow_harness_log() {
        return false;
    }
    let Some(log) = std::env::var_os("HARNESS_OPEN_EXTERNAL_LOG").filter(|v| !v.is_empty()) else {
        return false;
    };
    use std::io::Write as _;
    let log_path = Path::new(&log);
    if let Some(parent) = log_path.parent() {
        drop(std::fs::create_dir_all(parent));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_path) {
        let entry = json!({
            "pid": std::process::id(),
            "kind": "reveal",
            "path": path,
        });
        drop(writeln!(f, "{entry}"));
    }
    true
}

/// One app choice for an "Open in" menu or picker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MenuOption {
    pub id: &'static str,
    /// Action identifier (`"app:<id>"`) understood by [`pick`].
    pub item_id: String,
    pub label: &'static str,
    /// App icon identifier for `rune_theme_opencode::app_icons::named`.
    pub icon: &'static str,
    pub checked: bool,
}

/// Object-safe host boundary for the Open in feature.
pub trait OpenInHost: opencode_kernel::AppEnv + opencode_kernel::AppFeedback {
    fn open_in(&self) -> &OpenIn;
    fn open_in_mut(&mut self) -> &mut OpenIn;
    /// Whether the active server shares this machine's filesystem (local live server or fixture).
    fn is_local_server(&self) -> bool;
    /// The active session's working directory, if a session is open.
    fn active_session_directory(&self) -> Option<&str>;
    /// The active session's pinned physical local directory (resolved once up front when the
    /// workspace was opened through a local symlink/junction alias), or `None` if refused.
    fn active_session_physical_directory(&self) -> Option<String> {
        self.active_session_directory().map(str::to_owned)
    }
    /// Reads a JSON preference from host storage (`STORAGE_NS`, `STORAGE_KEY`).
    fn read_json(&self, ns: &str, key: &str) -> Option<Value>;
    /// Writes a JSON preference to host storage.
    fn write_json(&self, ns: &str, key: &str, val: &Value);
}

/// Opens `url` in the system browser/mail client ONLY when it passes the explicit OS scheme
/// allow-list (`browser_policy::is_safe_os_external_url`: credential-free `http:`/`https:`, or
/// attachment-free `mailto:`). In debug builds and headless driver runs it appends to
/// `HARNESS_OPEN_EXTERNAL_LOG` instead when that is set.
pub fn open_external(tx: &mut Tx, url: &str) -> bool {
    let trimmed = url.trim();
    if !browser_policy::is_safe_os_external_url(trimmed) {
        return false;
    }
    #[cfg(not(target_arch = "wasm32"))]
    if allow_harness_log()
        && let Some(log) = std::env::var_os("HARNESS_OPEN_EXTERNAL_LOG").filter(|v| !v.is_empty())
    {
        use std::io::Write as _;
        let path = Path::new(&log);
        if let Some(parent) = path.parent() {
            drop(std::fs::create_dir_all(parent));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let entry = json!({
                "pid": std::process::id(),
                "kind": "openExternal",
                "url": trimmed,
            });
            drop(writeln!(f, "{entry}"));
        }
        return true;
    }
    tx.open_url(trimmed).is_ok()
}

/// Launches a user-confirmed app-scheme URL (`slack:`, `figma:`, `obsidian:`, `mailto:`, etc.)
/// strictly after the user pressed Open on the 500 ms guarded native review bar (128-bit CSPRNG
/// single-use review token with 60 s TTL and `fits()` full-visibility verified by the host). Only
/// strict RFC 3986 URIs in `browser_policy`'s `APPS` + page-only `PAGE_ONLY` table are accepted.
pub fn open_confirmed_app_url(tx: &mut Tx, url: &str, source: browser_policy::AppSchemeSource) -> bool {
    let trimmed = url.trim();
    if trimmed != url
        || !browser_policy::is_strict_rfc3986_uri(trimmed)
        || !browser_policy::is_confirmed_app_scheme_url(trimmed, source)
    {
        return false;
    }
    #[cfg(not(target_arch = "wasm32"))]
    if allow_harness_log() && let Some(log) = std::env::var_os("HARNESS_OPEN_EXTERNAL_LOG").filter(|v| !v.is_empty()) {
        use std::io::Write as _;
        let path = Path::new(&log);
        if let Some(parent) = path.parent() { drop(std::fs::create_dir_all(parent)); }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            drop(writeln!(f, "{}", json!({ "pid": std::process::id(), "kind": "openConfirmedApp", "url": trimmed })));
        }
        return true;
    }
    if trimmed.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("mailto:")) {
        return tx.open_url(trimmed).is_ok();
    }
    if rune::is_headless() {
        return true;
    }
    #[cfg(target_os = "windows")]
    {
        return windows::open_app_uri(trimmed);
    }
    #[cfg(target_os = "macos")]
    {
        return std::process::Command::new("/usr/bin/open")
            .args(["--", trimmed])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        return std::process::Command::new("xdg-open")
            .arg(trimmed)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok();
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = tx;
        false
    }
}

/// Strips a trailing ` (os error N)` suffix and normalizes known transport error wording.
pub fn plain_error(raw: &str) -> String {
    let e = raw.trim();
    let e = match e.rfind(" (os error ") {
        Some(i) if e.ends_with(')') => &e[..i],
        _ => e,
    };
    opencode_kit::plain_error::plain_error(e)
}

/// Opening in external editors/terminals works on macOS/Windows where the server shares this machine's files.
pub fn can_open(host: &dyn OpenInHost) -> bool {
    cfg!(any(target_os = "macos", target_os = "windows")) && host.is_local_server()
}

/// Revealing a local file in the OS file manager works on every native desktop OS (Windows, macOS, Linux).
pub fn can_reveal(host: &dyn OpenInHost) -> bool {
    cfg!(not(target_arch = "wasm32")) && host.is_local_server()
}

/// Loads the last picked app (`plugin.file` / `openInApp`) from host storage.
pub fn load(host: &mut dyn OpenInHost) {
    let saved = host.read_json(STORAGE_NS, STORAGE_KEY).and_then(|v| v.as_str().map(str::to_owned));
    host.open_in_mut().set_chosen(saved);
}

/// Starts background installed-app detection for `host`.
pub fn detect(host: &mut dyn OpenInHost) {
    host.open_in_mut().detect();
}

/// Resolves `rel` inside the active session's pinned physical working directory.
pub fn resolve_path(host: &dyn OpenInHost, rel: &str) -> Option<String> {
    let dir = host.active_session_physical_directory()?;
    Some(if dir.is_empty() {
        rel.to_owned()
    } else if rel.is_empty() {
        dir
    } else {
        format!("{}/{rel}", dir.trim_end_matches(['/', '\\']))
    })
}

/// Opens `rel` in `app` (the file manager reveals it) and optionally remembers the choice.
pub fn open_app(host: &mut dyn OpenInHost, app: &str, rel: &str, remember: bool) {
    open_app_as(host, app, rel, remember, true);
}

/// Like [`open_app`]; with `reveal` false the file manager opens a folder `rel` rather than
/// showing it selected in its parent, as toolbar split buttons and menus do.
pub fn open_app_as(host: &mut dyn OpenInHost, app: &str, rel: &str, remember: bool, reveal: bool) {
    let Some(path) = resolve_path(host, rel) else { return };
    if remember {
        host.open_in_mut().set_chosen(Some(app.to_owned()));
        if host.persists() {
            host.write_json(STORAGE_NS, STORAGE_KEY, &json!(app));
        }
    }
    let Ok(verified) = browser_policy::verify_local_path(&path) else {
        return;
    };
    // Desktop's toolbar opens its panel's folder in the file manager; a file (or macOS bundle)
    // shows selected in its folder instead of opening in its default app.
    let reveal = reveal || !verified.is_dir || browser_policy::is_macos_bundle(&verified.path);
    host.open_in_mut().launch(app, verified.path, reveal && app == FINDER);
}

/// Opens `rel` in the currently remembered app when no launch is already running.
pub fn open_current(host: &mut dyn OpenInHost, rel: &str, remember: bool, reveal: bool) {
    if host.open_in().busy() {
        return;
    }
    let app = host.open_in().current().to_owned();
    open_app_as(host, &app, rel, remember, reveal);
}

/// `Workspace:OpenIn`: opens `path` (absolute, or inside the open session's folder) in `app`,
/// where the server shares this machine's files; a file shows selected in the file manager.
pub fn open_at(host: &mut dyn OpenInHost, app: &str, path: &str) {
    if !can_open(host)
        || !host.open_in().options().iter().any(|(id, _)| *id == app)
        || browser_policy::is_unc_or_device_path(path)
    {
        return;
    }
    let Some(path) = (if opencode_kit::path::is_absolute(path) { Some(path.to_owned()) } else { resolve_path(host, path) }) else {
        return;
    };
    let Ok(verified) = browser_policy::verify_local_path(&path) else {
        return;
    };
    let reveal = !verified.is_dir || browser_policy::is_macos_bundle(&verified.path);
    host.open_in_mut().launch(app, verified.path, reveal && app == FINDER);
}

/// Polls finished detection and launch work, reporting any launch error through `host.toast_fail`.
pub fn poll(host: &mut dyn OpenInHost) -> bool {
    // Every poll comes here: the state is written only when something arrived.
    let Some(arrived) = host.open_in().arrived() else { return false };
    let (changed, err) = host.open_in_mut().apply_arrived(arrived);
    if let Some(e) = err {
        host.toast_fail(opencode_i18n::t!("rune.openIn.failed").to_owned(), plain_error(&e));
    }
    changed
}

/// Installed `(id, label)` pairs when opening is available on this host, else empty.
pub fn apps(host: &dyn OpenInHost) -> Vec<(&'static str, &'static str)> {
    if can_open(host) { host.open_in().options() } else { Vec::new() }
}

/// Menu choices (`app:<id>`) for the host's installed apps, marking the current app when `with_current`.
pub fn menu_options(host: &dyn OpenInHost, with_current: bool) -> Vec<MenuOption> {
    let cur = host.open_in().current().to_owned();
    host.open_in()
        .options()
        .into_iter()
        .map(|(id, label)| MenuOption {
            id,
            item_id: format!("app:{id}"),
            label,
            icon: icon_of(id),
            checked: with_current && id == cur,
        })
        .collect()
}

/// The file manager's name (the first app), for "Reveal in …".
pub fn finder_label(host: &dyn OpenInHost) -> &'static str {
    host.open_in().finder_label()
}

/// Handles a menu selection (`"copy"` or `"app:<id>"`) for `rel`.
pub fn pick(host: &mut dyn OpenInHost, rel: &str, id: &str, reveal: bool, tx: &mut Tx) {
    if id == "copy" {
        if let Some(p) = resolve_path(host, rel) {
            tx.copy(p.clone());
            host.toast_succeed(opencode_i18n::t!("common.copied").to_owned(), p);
        }
        return;
    }
    if let Some(app) = id.strip_prefix("app:") {
        open_app_as(host, app, rel, true, reveal);
    }
}

/// Builds a file drag payload for `rel` when the verified local file exists on disk.
pub fn drag_item(host: &dyn OpenInHost, rel: &str) -> Option<DragItem> {
    let raw = can_open(host).then(|| resolve_path(host, rel)).flatten()?;
    let verified = browser_policy::verify_local_path(&raw).ok()?;
    host.open_in().dragged.record(verified.path.clone(), rel);
    Some(DragItem::new().file(verified.path).within_app())
}

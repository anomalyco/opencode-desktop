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

#[cfg(target_os = "windows")]
pub mod windows;

use std::cell::Cell;
use std::path::Path;
use std::sync::{Arc, Mutex};

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
            let mut cmd = std::process::Command::new("/usr/bin/open");
            if let Some(b) = bundle {
                cmd.args(["-b", b]);
            }
            run(cmd.arg(path)).map(|_| ())
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
            run(std::process::Command::new("/usr/bin/open").args(["-R", path])).map(|_| ())
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
}

/// What the first check in this process found, for every window after it.
static FOUND: Mutex<Option<Vec<&'static str>>> = Mutex::new(None);

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
        }
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

    /// Opens absolute `path` in its default app.
    pub fn open_default(&mut self, path: String) {
        self.launch("", path, false);
    }

    /// Launches `app` on `path` in the background (or reveals `path` in the file manager).
    pub fn launch(&mut self, app: &str, path: String, reveal: bool) {
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
pub trait OpenInHost {
    fn open_in(&self) -> &OpenIn;
    fn open_in_mut(&mut self) -> &mut OpenIn;
    /// Whether the active server shares this machine's filesystem (local live server or fixture).
    fn is_local_server(&self) -> bool;
    /// Whether user preferences (the remembered app) persist across launches.
    fn persists(&self) -> bool;
    /// The active session's working directory, if a session is open.
    fn active_session_directory(&self) -> Option<&str>;
    /// Reads a JSON preference from host storage (`STORAGE_NS`, `STORAGE_KEY`).
    fn read_json(&self, ns: &str, key: &str) -> Option<Value>;
    /// Writes a JSON preference to host storage.
    fn write_json(&self, ns: &str, key: &str, val: &Value);
    /// Shows a failure toast when launching an app fails.
    fn toast_fail(&mut self, title: String, description: String);
    /// Shows a success toast (for example after copying a resolved path).
    fn toast_succeed(&mut self, title: String, description: String);
}

/// Opens `url` in the system browser, or appends it to `HARNESS_OPEN_EXTERNAL_LOG` when a
/// hidden-desktop capture is driving the app (matching `harness/electron-open-external-stub.cjs`).
pub fn open_external(tx: &mut Tx, url: &str) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(log) = std::env::var_os("HARNESS_OPEN_EXTERNAL_LOG").filter(|v| !v.is_empty()) {
        use std::io::Write as _;
        let path = Path::new(&log);
        if let Some(parent) = path.parent() {
            drop(std::fs::create_dir_all(parent));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let entry = json!({
                "pid": std::process::id(),
                "kind": "openExternal",
                "url": url,
            });
            drop(writeln!(f, "{entry}"));
        }
        return true;
    }
    tx.open_url(url).is_ok()
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

/// Opening only works on macOS/Windows where the server shares this machine's files.
pub fn can_open(host: &dyn OpenInHost) -> bool {
    cfg!(any(target_os = "macos", target_os = "windows")) && host.is_local_server()
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

/// Resolves `rel` inside the active session's working directory.
pub fn resolve_path(host: &dyn OpenInHost, rel: &str) -> Option<String> {
    let dir = host.active_session_directory()?;
    Some(if dir.is_empty() {
        rel.to_owned()
    } else if rel.is_empty() {
        dir.to_owned()
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
    // Desktop's toolbar opens its panel's folder in the file manager; a file shows selected in its
    // folder instead of opening in its default app.
    let reveal = reveal || !Path::new(&path).is_dir();
    host.open_in_mut().launch(app, path, reveal && app == FINDER);
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
    if !can_open(host) || !host.open_in().options().iter().any(|(id, _)| *id == app) {
        return;
    }
    let Some(path) = (if opencode_kit::path::is_absolute(path) { Some(path.to_owned()) } else { resolve_path(host, path) }) else {
        return;
    };
    let reveal = !Path::new(&path).is_dir();
    host.open_in_mut().launch(app, path, reveal && app == FINDER);
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

/// Builds a file drag payload for `rel` when the local file exists on disk.
pub fn drag_item(host: &dyn OpenInHost, rel: &str) -> Option<DragItem> {
    let path = can_open(host).then(|| resolve_path(host, rel)).flatten().filter(|p| Path::new(p).exists())?;
    Some(DragItem::new().file(path).within_app())
}

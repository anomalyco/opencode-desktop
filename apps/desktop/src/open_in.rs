//! Open a file in an editor or terminal, or show it in the file manager, as Desktop's file panel
//! does (`gui-plugins/src/file/open-in-app.tsx`, `desktop/src/main/files`).
//!
//! | Where | What |
//! | --- | --- |
//! | Files and Changes toolbars | A split button: the remembered app's mark opens the file; the chevron lists the apps under "Open in", then Copy path. Picking one remembers it and opens the file there. |
//! | Tree rows (right-click) | The same apps; picking one opens that row and remembers it. The file manager reveals it. |
//!
//! Each platform lists Desktop's apps for it (macOS's editors and terminals; on Windows VS Code,
//! Cursor, Zed, PowerShell and Sublime Text), only those installed (checked once, off the UI
//! thread), with the file manager first (Finder, File Explorer). The choice is kept across
//! launches and defaults to the file manager, as Desktop's does. Nothing shows unless the server
//! shares this machine's files.

use std::sync::Arc;

use rune::{Align, Anim, Cursor, El, PointerPress, Latest, Tx, Waker, el, icon, row};
use rune_kit::Loader;

use crate::popup::{self, Item, Menu};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::palette::{Pal, kit};

#[cfg(target_os = "windows")]
mod windows;

pub const FINDER: &str = "finder";

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
    app("sublime-text", opencode_i18n::key!("plugin.file.open.app.sublimeText"), "sublime-text", "Sublime Text"),
];

/// Desktop's Linux list.
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

fn label_of(id: &str) -> &'static str {
    opencode_i18n::I18N.text(APPS.iter().find(|a| a.id == id).map_or(FILE_MANAGER.0, |a| a.label))
}

fn icon_of(id: &str) -> &'static str {
    APPS.iter().find(|a| a.id == id).map_or(FILE_MANAGER.1, |a| a.icon)
}

/// What launches apps. The real one is the platform's; tests and fixtures swap in their own.
pub trait Launcher: Send + Sync + 'static {
    /// The apps among `apps` (by `OpenApp::open_with`) that are installed.
    fn installed(&self, apps: &[&'static str]) -> Vec<&'static str>;
    /// Opens `path` in the app found by `app`, or in its default app (a folder in the file manager).
    fn open(&self, path: &str, app: Option<&str>) -> Result<(), String>;
    /// Shows `path` selected in the file manager.
    fn reveal(&self, path: &str) -> Result<(), String>;
}

/// macOS: apps by bundle ID through `NSWorkspace`, opened with `open`.
#[cfg(not(target_os = "windows"))]
#[derive(Default)]
pub struct Mac;

#[cfg(not(target_os = "windows"))]
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
        if !cfg!(target_os = "macos") {
            return vec![];
        }
        let list = bundles.iter().map(|b| format!("\"{b}\"")).collect::<Vec<_>>().join(",");
        let script = format!(
            "ObjC.import('AppKit');[{list}].filter(function(i){{return !$.NSWorkspace.sharedWorkspace.URLForApplicationWithBundleIdentifier(i).isNil()}}).join('\\n')"
        );
        let out = run(std::process::Command::new("/usr/bin/osascript").args(["-l", "JavaScript", "-e", &script])).unwrap_or_default();
        bundles.iter().copied().filter(|b| out.lines().any(|l| l == *b)).collect()
    }

    fn open(&self, path: &str, bundle: Option<&str>) -> Result<(), String> {
        let mut cmd = std::process::Command::new("/usr/bin/open");
        if let Some(b) = bundle {
            cmd.args(["-b", b]);
        }
        run(cmd.arg(path)).map(|_| ())
    }

    fn reveal(&self, path: &str) -> Result<(), String> {
        run(std::process::Command::new("/usr/bin/open").args(["-R", path])).map(|_| ())
    }
}

/// The platform's launcher.
#[cfg(target_os = "windows")]
type System = windows::Win;
#[cfg(not(target_os = "windows"))]
type System = Mac;

/// A fixture's stand-in: a fixed set of apps, and opening does nothing.
pub struct Fixed(pub Vec<&'static str>);

impl Launcher for Fixed {
    fn installed(&self, apps: &[&'static str]) -> Vec<&'static str> {
        apps.iter().copied().filter(|b| self.0.iter().any(|a| APPS.iter().any(|x| x.id == *a && x.open_with == *b))).collect()
    }
    fn open(&self, _: &str, _: Option<&str>) -> Result<(), String> {
        Ok(())
    }
    fn reveal(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

/// Which Open in apps are installed.
enum Installed {
    Unknown,
    Known(Vec<&'static str>),
}

pub struct OpenIn {
    launcher: Arc<dyn Launcher>,
    found: Latest<Vec<&'static str>>,
    done: Latest<Result<(), String>>,
    /// Which apps are installed: unknown until the check answers, which starts the first time the
    /// list is asked for (`options`: the menu opening, a plugin asking), not at launch.
    installed: Installed,
    /// The list was asked for while unknown: the next `poll` starts the check.
    wanted: std::cell::Cell<bool>,
    chosen: Option<String>,
    busy: bool,
}

/// What the first check in this process found, for every window after it.
static FOUND: std::sync::Mutex<Option<Vec<&'static str>>> = std::sync::Mutex::new(None);

impl Default for OpenIn {
    fn default() -> Self {
        Self::with(Arc::new(System::default()))
    }
}

impl OpenIn {
    pub fn with(launcher: Arc<dyn Launcher>) -> Self {
        Self { launcher, found: Latest::default(), done: Latest::default(), installed: Installed::Unknown, wanted: std::cell::Cell::new(false), chosen: None, busy: false }
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

    /// Takes in finished work; the error of a launch that failed.
    /// What came in since the last poll, read without writing anything: `None` when nothing did.
    fn arrived(&self) -> Option<(bool, Option<Vec<&'static str>>, Option<Result<(), String>>)> {
        let detect = self.wanted.take() && matches!(self.installed, Installed::Unknown);
        let (found, done) = (self.found.take(), self.done.take());
        (detect || found.is_some() || done.is_some()).then_some((detect, found, done))
    }

    fn poll(&mut self, (detect, found, done): (bool, Option<Vec<&'static str>>, Option<Result<(), String>>)) -> (bool, Option<String>) {
        let mut changed = false;
        if detect {
            self.detect();
            changed |= matches!(self.installed, Installed::Known(_));
        }
        if let Some(found) = found {
            *FOUND.lock().unwrap_or_else(|e| e.into_inner()) = Some(found.clone());
            self.installed = Installed::Known(APPS.iter().filter(|a| found.contains(&a.open_with)).map(|a| a.id).collect());
            changed = true;
        }
        let mut err = None;
        if let Some(r) = done {
            self.busy = false;
            changed = true;
            err = r.err();
        }
        (changed, err)
    }

    /// The file manager first, then what's installed: asking starts the check the first time.
    pub fn options(&self) -> Vec<(&'static str, &'static str)> {
        if matches!(self.installed, Installed::Unknown) {
            self.wanted.set(true);
        }
        self.listed()
    }

    /// `options` without starting the check, for what only shows the current choice.
    fn listed(&self) -> Vec<(&'static str, &'static str)> {
        let ids: Vec<&str> = match &self.installed {
            Installed::Known(ids) => ids.clone(),
            // Before the check answers, the remembered choice stands: it was installed when chosen.
            Installed::Unknown => self.chosen.as_deref().and_then(|c| APPS.iter().find(|a| a.id == c)).map(|a| a.id).into_iter().collect(),
        };
        std::iter::once((FINDER, opencode_i18n::I18N.text(FILE_MANAGER.0))).chain(ids.into_iter().map(|id| (id, label_of(id)))).collect()
    }

    pub fn current(&self) -> &str {
        self.chosen.as_deref().filter(|c| self.listed().iter().any(|(id, _)| id == c)).unwrap_or(FINDER)
    }

    pub fn current_label(&self) -> &'static str {
        label_of(self.current())
    }

    pub fn set_chosen(&mut self, id: Option<String>) {
        self.chosen = id;
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    /// Opens absolute `path` in its default app.
    pub fn open_default(&mut self, path: String) {
        self.launch("", path, false);
    }

    fn launch(&mut self, app: &str, path: String, reveal: bool) {
        if self.busy {
            return;
        }
        self.busy = true;
        let launcher = self.launcher.clone();
        let open_with = APPS.iter().find(|a| a.id == app).map(|a| a.open_with);
        self.done.request((), move |()| match open_with {
            None if reveal => launcher.reveal(&path),
            None => launcher.open(&path, None),
            Some(b) => launcher.open(&path, Some(b)),
        });
    }
}


impl App {
    /// Opening only works where the server shares this machine's files.
    pub fn can_open_in(&self) -> bool {
        cfg!(any(target_os = "macos", target_os = "windows"))
            && match &*self.source {
                crate::Source::Live { server, .. } => server.is_local(),
                crate::Source::Fixture => true,
            }
    }

    /// The app last picked (Desktop's `plugin.file` `openInApp`).
    pub fn load_open_in(&mut self) {
        let saved = crate::storage::json("plugin.file", "openInApp").and_then(|v| v.as_str().map(str::to_owned));
        self.open_in.set_chosen(saved);
    }

    /// `rel` in the open session's directory.
    pub fn open_in_path(&self, rel: &str) -> Option<String> {
        let dir = &self.session(self.active_session_id()?)?.directory;
        Some(if dir.is_empty() { rel.to_owned() } else if rel.is_empty() { dir.clone() } else { format!("{}/{rel}", dir.trim_end_matches(['/', '\\'])) })
    }

    /// Opens `rel` in `app` (the file manager reveals it) and remembers the choice.
    pub fn open_in_app(&mut self, app: &str, rel: &str, remember: bool) {
        self.open_in_app_as(app, rel, remember, true);
    }

    /// [`App::open_in_app`]; with `reveal` false the file manager opens a folder `rel` rather than
    /// show it selected in its parent, as the toolbar's button and menu do.
    pub fn open_in_app_as(&mut self, app: &str, rel: &str, remember: bool, reveal: bool) {
        let Some(path) = self.open_in_path(rel) else { return };
        if remember {
            self.open_in.set_chosen(Some(app.to_owned()));
            if self.persists() {
                crate::storage::set_json("plugin.file", "openInApp", &serde_json::json!(app));
            }
        }
        // Desktop's toolbar opens its panel's folder in the file manager; a file, as these
        // toolbars hold, shows selected in its folder instead of opening in its default app.
        let reveal = reveal || !std::path::Path::new(&path).is_dir();
        self.open_in.launch(app, path, reveal && app == FINDER);
    }

    /// `Workspace:OpenIn`: opens `path` (absolute, or inside the open session's folder) in `app`,
    /// where the server the window works against shares this machine's files; a file shows
    /// selected in the file manager.
    pub fn open_in_at(&mut self, app: &str, path: &str) {
        if !self.can_open_in() || !self.open_in.options().iter().any(|(id, _)| *id == app) {
            return;
        }
        let Some(path) = (if opencode_kit::path::is_absolute(path) { Some(path.to_owned()) } else { self.open_in_path(path) }) else { return };
        let reveal = !std::path::Path::new(&path).is_dir();
        self.open_in.launch(app, path, reveal && app == FINDER);
    }

    pub fn poll_open_in(&mut self) -> bool {
        // Every poll comes here: the state is written only when something arrived.
        let Some(arrived) = self.open_in.arrived() else { return false };
        let (changed, err) = self.open_in.poll(arrived);
        if let Some(e) = err {
            crate::status_toast::fail(self, opencode_i18n::t!("rune.openIn.failed"), crate::status_toast::plain(&e.to_string()));
        }
        changed
    }
}

/// Desktop's `OpenInAppMenuItemsV2`: "Open in" over each app with its mark (the current one
/// checked, from the toolbar), then Copy path.
fn menu(s: &App, with_current: bool) -> Menu {
    let cur = s.open_in.current();
    let apps = s.open_in.options().into_iter().map(|(id, label)| Item::new(format!("app:{id}"), label).app_icon(icon_of(id)).checked(with_current && id == cur));
    // menu-v2 content is at least 160 pt wide (`menu.css`).
    Menu::new(opencode_i18n::t!("plugin.file.open.in")).min_width(160.0).group(opencode_i18n::t!("plugin.file.open.in")).items(apps).separator().item(Item::new("copy", opencode_i18n::t!("plugin.file.open.copyPath")).weak_icon(&ic::COPY))
}

/// The apps alone (`OpenInAppMenuItemsV2` with `copyPath={false}`), the current one checked:
/// an artifact toolbar's Open in submenu, its rows `app:<id>`.
pub fn apps_menu(s: &App) -> Menu {
    let cur = s.open_in.current();
    Menu::new(opencode_i18n::t!("plugin.file.open.in")).min_width(160.0).items(s.open_in.options().into_iter().map(|(id, label)| Item::new(format!("app:{id}"), label).app_icon(icon_of(id)).checked(id == cur)))
}

/// The file manager's name (the first app), for Reveal in.
pub fn finder_label(s: &App) -> &'static str {
    s.open_in.listed().first().map_or("", |o| o.1)
}

fn pick(s: &mut App, rel: &str, id: &str, reveal: bool, tx: &mut Tx) {
    if id == "copy" {
        if let Some(p) = s.open_in_path(rel) {
            tx.copy(p.clone());
            crate::status_toast::succeed(s, opencode_i18n::t!("common.copied"), &p);
        }
        return;
    }
    if let Some(app) = id.strip_prefix("app:") {
        s.open_in_app_as(app, rel, true, reveal);
    }
}

/// Right-click on a tree row: the apps (the file manager revealing the row), and Copy path. A
/// file of this computer's drags out of the row too, to Explorer, a browser or a chat app, or
/// back into the composer (`rune::DragItem::within_app`).
pub fn context(row: El, rel: String) -> El {
    let dragged = rel.clone();
    row.drag_out(move |s: &App| {
        let path = s.can_open_in().then(|| s.open_in_path(&dragged)).flatten().filter(|p| std::path::Path::new(p).exists())?;
        Some(rune::DragItem::new().file(path).within_app())
    })
    .on_secondary_press(move |s: &mut App, press: PointerPress, tx: &mut Tx| {
        if !s.can_open_in() {
            return;
        }
        let rel = rel.clone();
        tx.instant();
        popup::open(s, menu(s, false), press, tx, move |s: &mut App, picked, tx| {
            tx.instant();
            if let Some(id) = picked {
                pick(s, &rel, id, true, tx);
            }
        });
    })
}

/// The toolbar's split button for `rel` (`split-button-v2`): 28 pt tall with 6 pt corners and a
/// 1 pt `border-muted` ring; the remembered app's 16 pt mark opens it, the chevron lists the rest.
/// While a launch runs both halves fade to half and the mark turns to the loader. Nothing where
/// opening isn't possible.
pub fn button(s: &App, p: &Pal, key: &'static str, rel: &str) -> Option<El> {
    if !s.can_open_in() {
        return None;
    }
    let label = s.open_in.current_label();
    let (rel_main, rel_menu) = (rel.to_owned(), rel.to_owned());
    let busy = s.open_in.busy();
    let chevron_key = format!("{key}-menu");
    let anchor = chevron_key.clone();
    let mark = match rune_theme_opencode::app_icons::named(icon_of(s.open_in.current()), p.dark) {
        _ if busy => Loader::new().size(14.0).build(kit(p)),
        Some(glyph) => icon(glyph, 16.0),
        None => el().size(16.0, 16.0),
    };
    let main = el()
        .key(format!("{key}-run"))
        .size(28.0, 28.0)
        .center()
        .cursor(Cursor::Pointer)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(Anim::INSTANT, Anim::INSTANT)
        .role("button")
        .label(opencode_i18n::t!("plugin.file.open.ariaLabel", app = label))
        .help(opencode_i18n::t!("plugin.file.open.ariaLabel", app = label))
        .layer_opacity(if busy { 0.5 } else { 1.0 })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            if s.open_in.busy() {
                return;
            }
            let app = s.open_in.current().to_owned();
            s.open_in_app_as(&app, &rel_main, false, false);
        })
        .child(mark);
    let open = s.popup.open_at(&chevron_key);
    let chev = el()
        .key(chevron_key)
        .size(20.0, 28.0)
        .center()
        .cursor(Cursor::Pointer)
        .fill(if open { p.hover } else { rune::Color::TRANSPARENT })
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(Anim::INSTANT, Anim::INSTANT)
        .role("button")
        .label(opencode_i18n::t!("plugin.file.open.menu"))
        .help(opencode_i18n::t!("plugin.file.open.menu"))
        .layer_opacity(if busy { 0.5 } else { 1.0 })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            if s.open_in.busy() {
                return;
            }
            let rel = rel_menu.clone();
            tx.instant();
            popup::open(s, menu(s, true).align(Align::End), anchor.clone(), tx, move |s: &mut App, picked, tx| {
                tx.instant();
                if let Some(id) = picked {
                    pick(s, &rel, id, false, tx);
                }
            });
        })
        .child(icon(&ic::CHEVRON_DOWN, 14.0).color(p.icon));
    Some(row().key(key).h(28.0).radius(6.0).clip_rounded(6.0).box_shadow(rune::Shadow { spread: 1.0, color: p.border_muted, inset: true, ..rune::Shadow::default() }).center_y().child(main).child(chev))
}


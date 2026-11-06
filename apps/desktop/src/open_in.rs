//! Open a file in an editor or terminal, or reveal it in Finder, as Desktop's file panel does
//! (`gui-plugins/src/file/open-in-app.tsx`).
//!
//! | Where | What |
//! | --- | --- |
//! | Files and Changes toolbars | A split button: the remembered app opens the file; the chevron lists the others, Reveal in Finder and Copy path. |
//! | Tree rows (right-click) | The same apps; picking one opens that row and remembers it. Finder reveals. |
//!
//! Only apps that are installed are listed (found by bundle ID through `NSWorkspace`, once, off
//! the UI thread), Finder always. The choice is kept across launches and defaults to Finder, as
//! Desktop's does. Nothing shows unless the server shares this machine's files.

use std::sync::Arc;

use rune::{Anim, Cursor, El, PointerPress, PopupItem, PopupMenu, Latest, Tx, Waker, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::palette::Pal;
use crate::view::UI_SMALL;

pub const FINDER: &str = "finder";

pub struct OpenApp {
    pub id: &'static str,
    pub label: &'static str,
    pub bundle: &'static str,
}

/// Desktop's macOS list, in its order.
pub const APPS: &[OpenApp] = &[
    OpenApp { id: "vscode", label: "VS Code", bundle: "com.microsoft.VSCode" },
    OpenApp { id: "cursor", label: "Cursor", bundle: "com.todesktop.230313mzl4w4u92" },
    OpenApp { id: "zed", label: "Zed", bundle: "dev.zed.Zed" },
    OpenApp { id: "textmate", label: "TextMate", bundle: "com.macromates.TextMate" },
    OpenApp { id: "antigravity", label: "Antigravity", bundle: "com.google.antigravity" },
    OpenApp { id: "terminal", label: "Terminal", bundle: "com.apple.Terminal" },
    OpenApp { id: "iterm2", label: "iTerm2", bundle: "com.googlecode.iterm2" },
    OpenApp { id: "ghostty", label: "Ghostty", bundle: "com.mitchellh.ghostty" },
    OpenApp { id: "warp", label: "Warp", bundle: "dev.warp.Warp-Stable" },
    OpenApp { id: "xcode", label: "Xcode", bundle: "com.apple.dt.Xcode" },
    OpenApp { id: "android-studio", label: "Android Studio", bundle: "com.google.android.studio" },
    OpenApp { id: "sublime-text", label: "Sublime Text", bundle: "com.sublimetext.4" },
];

fn label_of(id: &str) -> &'static str {
    if id == FINDER {
        return "Finder";
    }
    APPS.iter().find(|a| a.id == id).map_or("Finder", |a| a.label)
}

/// What launches apps. The real one shells out; tests and fixtures swap in their own.
pub trait Launcher: Send + Sync + 'static {
    /// Bundle IDs among `bundles` that are installed.
    fn installed(&self, bundles: &[&'static str]) -> Vec<&'static str>;
    /// Opens `path` in the app with `bundle`, or in its default app.
    fn open(&self, path: &str, bundle: Option<&str>) -> Result<(), String>;
    fn reveal(&self, path: &str) -> Result<(), String>;
}

pub struct Mac;

fn run(cmd: &mut std::process::Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

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

/// A fixture's stand-in: a fixed set of apps, and opening does nothing.
pub struct Fixed(pub Vec<&'static str>);

impl Launcher for Fixed {
    fn installed(&self, bundles: &[&'static str]) -> Vec<&'static str> {
        bundles.iter().copied().filter(|b| self.0.iter().any(|a| APPS.iter().any(|x| x.id == *a && x.bundle == *b))).collect()
    }
    fn open(&self, _: &str, _: Option<&str>) -> Result<(), String> {
        Ok(())
    }
    fn reveal(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

pub struct OpenIn {
    launcher: Arc<dyn Launcher>,
    found: Latest<Vec<&'static str>>,
    done: Latest<Result<(), String>>,
    /// Installed app IDs, in `APPS` order; none until the check answers.
    installed: Vec<&'static str>,
    chosen: Option<String>,
    busy: bool,
}

impl Default for OpenIn {
    fn default() -> Self {
        Self::with(Arc::new(Mac))
    }
}

impl OpenIn {
    pub fn with(launcher: Arc<dyn Launcher>) -> Self {
        Self { launcher, found: Latest::default(), done: Latest::default(), installed: vec![], chosen: None, busy: false }
    }

    pub fn set_waker(&mut self, waker: Waker) {
        self.found.set_waker(waker.clone());
        self.done.set_waker(waker);
        self.detect();
    }

    /// Looks for installed apps in the background.
    pub fn detect(&mut self) {
        let launcher = self.launcher.clone();
        self.found.request((), move |()| {
            let bundles: Vec<&'static str> = APPS.iter().map(|a| a.bundle).collect();
            launcher.installed(&bundles)
        });
    }

    /// Takes in finished work; the error of a launch that failed.
    pub fn poll(&mut self) -> (bool, Option<String>) {
        let mut changed = false;
        if let Some(bundles) = self.found.take() {
            self.installed = APPS.iter().filter(|a| bundles.contains(&a.bundle)).map(|a| a.id).collect();
            changed = true;
        }
        let mut err = None;
        if let Some(r) = self.done.take() {
            self.busy = false;
            changed = true;
            err = r.err();
        }
        (changed, err)
    }

    /// Finder first, then what's installed.
    pub fn options(&self) -> Vec<(&'static str, &'static str)> {
        std::iter::once((FINDER, "Finder")).chain(self.installed.iter().map(|id| (*id, label_of(id)))).collect()
    }

    pub fn current(&self) -> &str {
        self.chosen.as_deref().filter(|c| self.options().iter().any(|(id, _)| id == c)).unwrap_or(FINDER)
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
        let bundle = APPS.iter().find(|a| a.id == app).map(|a| a.bundle);
        self.done.request((), move |()| match bundle {
            None if reveal => launcher.reveal(&path),
            None => launcher.open(&path, None),
            Some(b) => launcher.open(&path, Some(b)),
        });
    }
}

fn file() -> Option<std::path::PathBuf> {
    crate::dirs::data().map(|d| d.join("open-in.json"))
}

impl App {
    /// Opening only works where the server shares this machine's files.
    pub fn can_open_in(&self) -> bool {
        cfg!(target_os = "macos")
            && match &self.source {
                crate::Source::Live { server, .. } => server.is_local(),
                crate::Source::Fixture => true,
            }
    }

    pub fn load_open_in(&mut self) {
        let saved = file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v["app"].as_str().map(str::to_owned));
        self.open_in.set_chosen(saved);
    }

    /// `rel` in the open session's directory.
    pub fn open_in_path(&self, rel: &str) -> Option<String> {
        let dir = &self.session(self.active_session_id()?)?.directory;
        Some(if dir.is_empty() { rel.to_owned() } else if rel.is_empty() { dir.clone() } else { format!("{}/{rel}", dir.trim_end_matches('/')) })
    }

    /// Opens `rel` in `app` (Finder reveals it) and remembers the choice.
    pub fn open_in_app(&mut self, app: &str, rel: &str, remember: bool) {
        let Some(path) = self.open_in_path(rel) else { return };
        if remember {
            self.open_in.set_chosen(Some(app.to_owned()));
            if self.persists()
                && let Some(f) = file()
            {
                let body = serde_json::json!({ "app": app }).to_string();
                crate::disk::later("open-in", move || {
                    crate::disk::replace(&f, body.as_bytes());
                });
            }
        }
        self.open_in.launch(app, path, app == FINDER);
    }

    pub fn poll_open_in(&mut self) -> bool {
        let (changed, err) = self.open_in.poll();
        if let Some(e) = err {
            crate::status_toast::fail(self, "Couldn't open", crate::status_toast::plain(&e.to_string()));
        }
        changed
    }
}

fn app_items(s: &App, with_current: bool) -> Vec<PopupItem> {
    let cur = s.open_in.current();
    s.open_in.options().into_iter().map(|(id, label)| PopupItem::new(format!("app:{id}"), label).checked(with_current && id == cur)).collect()
}

fn pick(s: &mut App, rel: &str, id: &str, tx: &mut Tx) {
    match id {
        "reveal" => s.open_in_app(FINDER, rel, false),
        "copy" => {
            if let Some(p) = s.open_in_path(rel) {
                tx.copy(p);
            }
        }
        _ => {
            if let Some(app) = id.strip_prefix("app:") {
                s.open_in_app(app, rel, true);
            }
        }
    }
}

/// Right-click on a tree row: the apps, Finder revealing, and Copy path.
pub fn context(row: El, rel: String) -> El {
    row.on_secondary_press(move |s: &mut App, press: PointerPress, tx: &mut Tx| {
        if !s.can_open_in() {
            return;
        }
        let menu = PopupMenu::new().items(app_items(s, false)).separator().item(PopupItem::new("copy", "Copy path"));
        let rel = rel.clone();
        tx.instant();
        tx.popup_menu(menu, press, move |s: &mut App, picked, tx| {
            tx.instant();
            if let Some(id) = picked {
                pick(s, &rel, id, tx);
            }
        });
    })
}

/// The toolbar's split button for `rel`: the remembered app opens it, the chevron lists the rest.
/// Nothing where opening isn't possible.
pub fn button(s: &App, p: &Pal, key: &'static str, rel: &str) -> Option<El> {
    if !s.can_open_in() {
        return None;
    }
    let label = s.open_in.current_label();
    let (rel_main, rel_menu) = (rel.to_owned(), rel.to_owned());
    let busy = s.open_in.busy();
    let chevron_key = format!("{key}-menu");
    let anchor = chevron_key.clone();
    let main = row()
        .key(format!("{key}-run"))
        .h(25.0)
        .px(7.0)
        .center_y()
        .radius(5.0)
        .cursor(Cursor::Pointer)
        .hover(p.hover)
        .hover_motion(Anim::INSTANT, Anim::INSTANT)
        .role("button")
        .label(format!("Open in {label}"))
        .help(format!("Open in {label}"))
        .dim(if busy { 0.5 } else { 1.0 })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            let app = s.open_in.current().to_owned();
            s.open_in_app(&app, &rel_main, false);
        })
        .child(text(label).font(UI_SMALL).color(p.muted));
    let chev = el()
        .key(chevron_key)
        .size(20.0, 25.0)
        .radius(5.0)
        .center()
        .cursor(Cursor::Pointer)
        .hover(p.hover)
        .hover_motion(Anim::INSTANT, Anim::INSTANT)
        .role("button")
        .label("Open options")
        .help("Open options")
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            let menu = PopupMenu::new()
                .items(app_items(s, true))
                .separator()
                .item(PopupItem::new("reveal", "Reveal in Finder"))
                .item(PopupItem::new("copy", "Copy path"));
            let rel = rel_menu.clone();
            tx.instant();
            tx.popup_menu(menu, anchor.clone(), move |s: &mut App, picked, tx| {
                tx.instant();
                if let Some(id) = picked {
                    pick(s, &rel, id, tx);
                }
            });
        })
        .child(icon(&ic::CHEVRON_DOWN, 11.0).color(p.icon_muted));
    Some(row().key(key).gap(0.0).center_y().child(main).child(chev))
}

#[cfg(test)]
mod tests;


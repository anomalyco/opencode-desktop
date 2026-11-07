//! Desktop's app menu off macOS (`windows-menu.tsx`): ≡ at the titlebar row's start opens
//! "OpenCode" and its File, Edit, View, Go, Window and Help submenus (`desktop-menu.ts`, the
//! entries Windows shows), drawn in the window in menu-v2's standard look. An item shows its
//! command's keys or the menu's own accelerator, and runs what Desktop's does.

use rune::{El, Key, KeyEvent, Mods, Tx, icon};

use rune_theme_opencode::icons as ic;

use crate::App;
use crate::keymap::Bind;
use crate::palette::Pal;
use crate::popup::{self, Item, Menu};

/// The button's key, which the menu opens under.
const KEY: &str = "app-menu";
/// The menu's width (`.desktop-app-menu`).
const W: f32 = 160.0;

/// What an item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    NewSession,
    Settings,
    NewWindow,
    CloseWindow,
    /// An Edit item: the key it sends to the focused field.
    Edit(EditKey),
    ToggleSidebar,
    ToggleTerminal,
    Reload,
    DevTools,
    ActualSize,
    ZoomIn,
    ZoomOut,
    FullScreen,
    Back,
    Forward,
    PreviousSession,
    NextSession,
    Minimize,
    Maximize,
    ExportLogs,
    Open(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditKey {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Delete,
    SelectAll,
}

/// An item's keys: its command's, as the keymap binds it here, or the menu's accelerator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Keys {
    None,
    Command(Bind),
    Fixed(&'static str),
}

/// One item, or a separator (`None`).
type Row = Option<(&'static str, Act, Keys)>;

const fn it(title: &'static str, act: Act, keys: Keys) -> Row {
    Some((title, act, keys))
}

/// `DESKTOP_MENU`, as Windows shows it: the app menu is macOS's alone.
const MENUS: [(&str, &[Row]); 6] = [
    ("File", &[
        it("New Session", Act::NewSession, Keys::Command(Bind::TabNew)),
        it("Settings", Act::Settings, Keys::Command(Bind::Settings)),
        it("New Window", Act::NewWindow, Keys::Fixed("Ctrl+Shift+N")),
        None,
        it("Close Window", Act::CloseWindow, Keys::Fixed("Ctrl+Shift+W")),
    ]),
    ("Edit", &[
        it("Undo", Act::Edit(EditKey::Undo), Keys::Fixed("Ctrl+Z")),
        it("Redo", Act::Edit(EditKey::Redo), Keys::Fixed("Ctrl+Y")),
        None,
        it("Cut", Act::Edit(EditKey::Cut), Keys::Fixed("Ctrl+X")),
        it("Copy", Act::Edit(EditKey::Copy), Keys::Fixed("Ctrl+C")),
        it("Paste", Act::Edit(EditKey::Paste), Keys::Fixed("Ctrl+V")),
        it("Delete", Act::Edit(EditKey::Delete), Keys::None),
        it("Select All", Act::Edit(EditKey::SelectAll), Keys::Fixed("Ctrl+A")),
    ]),
    ("View", &[
        it("Toggle Sidebar", Act::ToggleSidebar, Keys::Command(Bind::Sidebar)),
        it("Toggle Terminal", Act::ToggleTerminal, Keys::Command(Bind::DockToggle)),
        None,
        it("Reload", Act::Reload, Keys::None),
        it("Toggle Developer Tools", Act::DevTools, Keys::None),
        None,
        it("Actual Size", Act::ActualSize, Keys::Fixed("Ctrl+0")),
        it("Zoom In", Act::ZoomIn, Keys::Fixed("Ctrl++")),
        it("Zoom Out", Act::ZoomOut, Keys::Fixed("Ctrl+-")),
        None,
        it("Toggle Full Screen", Act::FullScreen, Keys::None),
    ]),
    ("Go", &[
        it("Back", Act::Back, Keys::Command(Bind::GoBack)),
        it("Forward", Act::Forward, Keys::Command(Bind::GoForward)),
        None,
        it("Previous Session", Act::PreviousSession, Keys::Command(Bind::SessionPrev)),
        it("Next Session", Act::NextSession, Keys::Command(Bind::SessionNext)),
    ]),
    ("Window", &[
        it("Minimize", Act::Minimize, Keys::None),
        it("Maximize", Act::Maximize, Keys::None),
        None,
        it("Close Window", Act::CloseWindow, Keys::None),
    ]),
    ("Help", &[
        it("OpenCode Documentation", Act::Open("https://opencode.ai/v2/docs"), Keys::None),
        it("Support Forum", Act::Open("https://discord.com/invite/opencode"), Keys::None),
        it("Export Logs…", Act::ExportLogs, Keys::None),
        None,
        it("Share Feedback", Act::Open("https://github.com/anomalyco/opencode/issues/new?template=feature_request.yml"), Keys::None),
        it("Report a Bug", Act::Open("https://github.com/anomalyco/opencode/issues/new?template=bug_report.yml"), Keys::None),
    ]),
];

/// The button at the titlebar row's start (`IconButton` ghost-muted, large) in its 36pt slot
/// (`h-7 w-9 px-1`): lit as pressed while its menu is open.
pub fn button(s: &App, p: &Pal) -> El {
    rune::row().key("app-menu-slot").size(36.0, 28.0).center().child(glyph_button(s, p))
}

fn glyph_button(s: &App, p: &Pal) -> El {
    let open = s.popup.open_at(KEY);
    rune::el()
        .key(KEY)
        .role("button")
        .label("OpenCode menu")
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .fill(if open { p.pressed } else { rune::Color::TRANSPARENT })
        .hover(if open { rune::Color::TRANSPARENT } else { p.hover })
        .press(p.pressed)
        .hover_motion(crate::view::INSTANT, crate::view::INSTANT)
        // Desktop remembers the focused field and puts it back for the Edit items.
        .keeps_text_selection()
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            open_menu(s, tx);
        })
        .child(icon(&ic::MENU, 16.0).color(if open { p.icon } else { p.icon_muted }))
}

/// Opens the menu under its button.
fn open_menu(s: &mut App, tx: &mut Tx) {
    popup::open_drawn(s, menu(s), KEY, tx, |s: &mut App, picked, tx| {
        if let Some(key) = picked.and_then(|p| p.strip_prefix(PLUGIN)) {
            crate::plugin_host::run_menubar(s, key, tx);
            return;
        }
        if let Some(act) = picked.and_then(find) {
            run(s, act, tx);
        }
    });
}

/// The prefix of a plugin's item id (`MenubarItem`): `app-menu-plugin-<plugin>.<id>`.
const PLUGIN: &str = "app-menu-plugin-";

/// The menu as it stands: a command's item is disabled while the command can't run. Plugins'
/// items follow each submenu's own, after a separator.
fn menu(s: &App) -> Menu {
    let plugin_items = crate::plugin_host::menubar_items(s);
    let menus = MENUS.iter().enumerate().map(|(m, (title, rows))| {
        let sub = rows.iter().enumerate().fold(Menu::new(*title), |menu, (r, row)| match row {
            None => menu.separator(),
            Some((title, act, keys)) => menu.item(Item::new(id(m, r), *title).shortcut(label(*keys)).enabled(enabled(s, *act))),
        });
        let theirs: Vec<_> = plugin_items.iter().filter(|(menu, _, _)| menu.eq_ignore_ascii_case(title)).collect();
        let sub = if theirs.is_empty() { sub } else { sub.separator() };
        let sub = theirs.into_iter().fold(sub, |menu, (_, key, title)| menu.item(Item::new(format!("{PLUGIN}{key}"), title.clone())));
        (m, *title, sub)
    });
    menus.fold(Menu::new("OpenCode").standard(W).keeps_focus().group("OpenCode"), |menu, (m, title, sub)| {
        menu.submenu(format!("app-menu-{m}"), title, sub)
    })
}

fn id(m: usize, r: usize) -> String {
    format!("app-menu-{m}-{r}")
}

/// The item an id names.
fn find(id: &str) -> Option<Act> {
    let mut parts = id.strip_prefix("app-menu-")?.split('-').map(str::parse::<usize>);
    let (m, r) = (parts.next()?.ok()?, parts.next()?.ok()?);
    MENUS.get(m)?.1.get(r)?.map(|(_, act, _)| act)
}

fn label(keys: Keys) -> String {
    match keys {
        Keys::None => String::new(),
        Keys::Command(bind) => bind.label(),
        Keys::Fixed(keys) => keys.to_owned(),
    }
}

/// Toggle Terminal needs a session, as Desktop's `terminal.toggle` command does, and Toggle
/// Sidebar a sidebar, which top tabs don't have.
fn enabled(s: &App, act: Act) -> bool {
    match act {
        Act::ToggleTerminal => s.active_session_id().is_some(),
        Act::ToggleSidebar => s.tab_layout().has_sidebar(),
        _ => true,
    }
}

fn run(s: &mut App, act: Act, tx: &mut Tx) {
    match act {
        Act::NewSession => s.new_session(tx),
        Act::Settings => {
            tx.instant();
            s.open_settings();
        }
        Act::NewWindow => new_window(),
        Act::CloseWindow => tx.close_window(),
        Act::Edit(key) => tx.send_key(edit_key(key)),
        Act::ToggleSidebar => {
            s.toggle_sidebar(tx);
        }
        Act::ToggleTerminal => s.toggle_terminal(tx),
        Act::Reload => s.resync(),
        Act::DevTools => {
            tx.instant();
            s.knobs.toggle();
        }
        Act::ActualSize => s.zoom_by(0, tx),
        Act::ZoomIn => s.zoom_by(1, tx),
        Act::ZoomOut => s.zoom_by(-1, tx),
        Act::FullScreen => tx.toggle_full_screen(),
        Act::Back => {
            s.navigate(false, tx);
        }
        Act::Forward => {
            s.navigate(true, tx);
        }
        Act::PreviousSession => s.select_adjacent(-1, false, tx),
        Act::NextSession => s.select_adjacent(1, false, tx),
        Act::Minimize => tx.minimize(),
        Act::Maximize => tx.toggle_maximized(),
        Act::ExportLogs => s.export_logs(),
        Act::Open(url) => {
            let _ = tx.open_url(url);
        }
    }
}

/// The key an Edit item sends, as Electron's `webContents` edit commands act on the focused
/// field: Rune's text fields take the primary modifier's letters, and Delete.
fn edit_key(key: EditKey) -> KeyEvent {
    let primary = Mods { cmd: true, ctrl: !cfg!(target_os = "macos"), ..Mods::default() };
    let (key, mods) = match key {
        EditKey::Undo => (Key::Command('z'), primary),
        EditKey::Redo => (Key::Command('z'), Mods { shift: true, ..primary }),
        EditKey::Cut => (Key::Command('x'), primary),
        EditKey::Copy => (Key::Command('c'), primary),
        EditKey::Paste => (Key::Command('v'), primary),
        EditKey::Delete => (Key::Delete, Mods::default()),
        EditKey::SelectAll => (Key::Command('a'), primary),
    };
    KeyEvent { key, mods }
}

/// Ctrl+Shift+N and Ctrl+Shift+W, the menu's accelerators that no command binds; true when
/// `ev` was one.
pub fn accelerator(ev: &KeyEvent, tx: &mut Tx) -> bool {
    if Bind::WindowNew.matches_tx(ev, tx) {
        new_window();
        return true;
    }
    if Bind::WindowClose.matches_tx(ev, tx) {
        tx.close_window();
        return true;
    }
    false
}

/// Another window: this app again, on the same server.
fn new_window() {
    let Ok(exe) = std::env::current_exe() else { return };
    let _ = std::process::Command::new(exe).args(std::env::args_os().skip(1)).spawn();
}

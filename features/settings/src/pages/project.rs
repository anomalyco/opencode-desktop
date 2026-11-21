//! A project's own settings (`settings/shell.tsx`'s `ProjectSettings`, `workspaces/project.tsx`):
//! the rail's Back to projects over General, Worktrees and Extensions. General reads the
//! project's avatar and name over its menu, then Project name (saved when the field loses
//! focus), Icon (an image chosen or dropped, or the custom one taken off), Color (while no image
//! is set), Auto-accept permissions (the default, On or Off for this project) and the Worktree
//! startup script; a save that fails says so under its field with Retry.

use opencode_i18n::t;
use opencode_kit::model::{Project, project_meta};
use opencode_ui::focus::FocusExt;
use opencode_ui::popup::{self, Item, Menu, MenuButton as _, PopupHost};
use rune::{Color, Cursor, Drag, El, Icon, Phase, TextEdit, TextStyle, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;
use serde_json::{Value, json};
use opencode_ui::palette::{Elevate as _, Pal, kit};

use super::{Page, header, page, select};
use crate::host::SettingsHost;
use crate::jobs::{SettingsJob, SettingsRequest};
use crate::{input_lift, list, setting};

/// A project page's tab (`SettingsProjectTab`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tab {
    #[default]
    General,
    Worktrees,
    Extensions,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::General, Tab::Worktrees, Tab::Extensions];

    pub fn id(self) -> &'static str {
        match self {
            Tab::General => "project-general",
            Tab::Worktrees => "project-workspaces",
            Tab::Extensions => "project-extensions",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::General => t!("settings.general.section.general"),
            Tab::Worktrees => t!("settings.tab.workspaces"),
            Tab::Extensions => t!("settings.tab.extensions"),
        }
    }

    pub fn icon(self) -> &'static Icon {
        match self {
            Tab::General => &ic::FOLDER,
            Tab::Worktrees => &ic::OUTLINE_WORKTREE,
            Tab::Extensions => &ic::EXTENSIONS,
        }
    }
}

/// A field whose last save failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    Icon,
    Startup,
}

/// What the page holds for the project it shows.
#[derive(Default)]
pub struct State {
    /// The project the fields were filled for.
    pub dir: String,
    pub name: TextEdit,
    pub startup: TextEdit,
    /// The script field's text height once its resize grip was dragged (`resize: vertical`).
    pub startup_h: Option<f32>,
    pub failed: Vec<Field>,
    /// The name and script as last saved or read: leaving a field that differs saves it.
    pub sent: (String, String),
    /// A Projects row being renamed in place, by folder, and its field.
    pub renaming: Option<String>,
    pub rename: TextEdit,
    /// Whether the server reported `LocationNotFoundError` for the loaded project's folder
    /// (`workspaces/location.tsx:63-77`, works on both local and remote servers).
    pub missing: bool,
    pub extensions: super::project_extensions::State,
}

/// The project `dir` names.
pub fn project<'a>(s: &'a dyn SettingsHost, dir: &str) -> Option<&'a Project> {
    s.project_at(dir)
}

/// Fills the fields for `dir`'s project, as it opens.
pub fn load(s: &mut dyn SettingsHost, dir: &str, tab: Tab) {
    if s.settings_pages().project.dir != dir {
        let meta = project(s, dir).map(|p| project_meta(&p.id)).unwrap_or_default();
        let shown = if meta.name.is_empty() { super::projects::folder_name(dir) } else { meta.name.clone() };
        let st = &mut s.settings_pages_mut().project;
        st.dir = dir.to_owned();
        st.name = TextEdit::new(&shown);
        st.startup = TextEdit::new(&meta.start);
        st.startup_h = None;
        st.sent = (shown, meta.start.clone());
        st.failed.clear();
        st.missing = false;
    }
    if !dir.is_empty() {
        s.spawn_job(SettingsJob::ActiveServer(SettingsRequest::CheckProjectLocation { dir: dir.to_owned() }));
    }
    match tab {
        Tab::Worktrees => {
            if let Some(id) = project(s, dir).map(|p| p.id.clone()) {
                s.list_worktrees(&id);
            }
        }
        Tab::Extensions => super::project_extensions::load(s, dir),
        Tab::General => {}
    }
}

/// The project's tab `tab`.
pub fn view<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal, dir: &str, tab: Tab) -> El {
    let Some(x) = project(s, dir).cloned() else {
        return page(p, "settings-project-page", header(p, t!("rune.settings.project.menu"), ""), el());
    };
    match tab {
        Tab::General => general(s, p, &x),
        Tab::Worktrees => super::worktrees::view_for(s, p, Some(&x.id)),
        Tab::Extensions => super::project_extensions::view(s, p, &x),
    }
}

const STARTUP: TextStyle = TextStyle::new(13.0, 440.0).lh(17.55).tracking(-0.04).mono();
const COPY: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0);
const TITLE: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0);
const FAILED: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0);
const HINT_CODE: TextStyle = TextStyle::new(12.0, 440.0).lh(20.0).mono();
const HINT_PILL: TextStyle = TextStyle::new(12.0, 440.0).lh(15.0).mono();

/// Desktop's avatar colours, in its order (`PROJECT_AVATAR_VARIANTS`).
const COLORS: [&str; 9] = ["orange", "yellow", "cyan", "green", "red", "pink", "blue", "purple", "gray"];

fn color_name(name: &str) -> &'static str {
    match name {
        "orange" => t!("common.color.orange"),
        "yellow" => t!("common.color.yellow"),
        "cyan" => t!("common.color.cyan"),
        "green" => t!("common.color.green"),
        "red" => t!("common.color.red"),
        "pink" => t!("common.color.pink"),
        "blue" => t!("common.color.blue"),
        "purple" => t!("common.color.purple"),
        _ => t!("common.color.gray"),
    }
}

fn variant(color: &str) -> usize {
    let color = match color {
        "mint" => "cyan",
        "lime" => "green",
        c => c,
    };
    COLORS.iter().position(|c| *c == color).unwrap_or(8)
}

fn tile(p: &Pal, letter: &str, v: usize, size: f32, font: f32) -> El {
    let (bg, ring) = p.avatars[v];
    // `project-avatar.css:24-26`: top-to-bottom `alpha-light-16` sheen applies to all 9 colour
    // variants including `gray` (`v == 8`).
    let t = el()
        .size(size, size)
        .radius(6.0)
        .fill(bg)
        .gradient(Color::WHITE.alpha(0x29 as f32 / 255.0), Color::WHITE.alpha(0.0))
        .box_shadow(Pal::ring_in(ring))
        .center();
    if letter.is_empty() {
        return t;
    }
    t.child(text(letter.to_uppercase()).font(TextStyle::new(font, 530.0).lh(font).tracking(0.05)).color(p.avatar_fg))
}

fn avatar(s: &dyn SettingsHost, p: &Pal, x: &Project) -> El {
    if x.icon.source(&x.id).is_some() {
        return s.project_avatar_image(p, x, 32.0);
    }
    let name = super::projects::name(x);
    tile(p, name.chars().next().map(String::from).as_deref().unwrap_or(""), variant(&x.icon.color), 32.0, 16.0)
}

/// `ProjectFolderMissing` (`project-folder-missing.tsx`, `project.css:105-140`): shown at the
/// top of Project › General and Project › Extensions when the project's folder is gone on disk.
pub fn folder_missing(p: &Pal, dir: &str) -> El {
    let copy = col()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(2.0)
        .child(text(t!("project.settings.folderMissing.title")).font(TITLE).color(p.text).wrap())
        .child(text(dir.to_owned()).font(HINT_CODE).color(p.muted).wrap_anywhere())
        .child(text(t!("project.settings.folderMissing.description")).font(COPY).color(p.muted).wrap());
    row()
        .key("project-settings-folder-missing")
        .role("status")
        .w_full()
        .px(16.0)
        .py(12.0)
        .gap(8.0)
        .items_start()
        .radius(8.0)
        .fill(p.layer1)
        .box_shadow(Pal::ring_in(p.border_muted))
        .child(icon(&ic::WARNING, 16.0).color(p.warning))
        .child(copy)
}

fn general<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal, x: &Project) -> El {
    let st = &s.settings_pages().project;
    let name = super::projects::name(x);
    let folder = super::projects::folder_name(&x.directory);
    let failed = |f: Field| st.failed.contains(&f);
    let all_servers = super::servers::inventory::list(s);
    let server_link = (all_servers.len() > 1).then(|| {
        let server_name = super::servers::inventory::selected(s).name;
        col()
            .key("project-settings-server-link")
            .role("button")
            .label(server_name.clone())
            .self_start()
            .radius(4.0)
            .cursor(Cursor::Pointer)
            .focus_default(p)
            .hover_group_focus()
            .on_press_tx(|s: &mut H, tx: &mut Tx| super::open_page(s, Page::Servers, tx))
            .child(text(server_name).font(super::NAME_REGULAR).color(p.muted).group_color(p.text).truncate())
            .child(el().pin(Some(0.0), None, Some(0.0), Some(0.0)).h(1.0).fill(p.text).group_opacity(0.0, 1.0))
    });
    let title_block = col()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(4.0)
        .child(text(name).font(super::TITLE).color(p.text).truncate())
        .children(server_link);
    let head = row()
        .key("settings-tab-header")
        .w_full()
        .min_h(32.0)
        .gap(16.0)
        .center_y()
        .child(row().grow(1.0).shrink().min_w(0.0).gap(12.0).center_y().child(avatar(s, p, x)).child(title_block))
        .child(options(s, p, x, "settings-project-options", 28.0));
    let name_field = col()
        .items_end()
        .gap(6.0)
        .child(
            rune_kit::TextInput::new(&st.name)
                .placeholder(folder)
                .label(t!("project.settings.name.title"))
                // `.project-settings-name { width: min(280px, 100%); }` inside `width: auto`
                // `[data-slot="settings-row-control"]` resolves to the `<input>`'s intrinsic width (195 px at 1x).
                .width(if crate::rows_wrap() { None } else { Some(195.0) })
                .spellcheck(s.settings().spellcheck)
                .text_style(super::NAME_REGULAR)
                .height(28.0)
                .invalid(failed(Field::Name))
                .on_edit(|s: &mut H| &mut s.settings_pages_mut().project.name)
                .build(kit(p))
                .key("settings-project-name-input"),
        )
        .children(failed(Field::Name).then(|| save_failed::<H>(p, Field::Name)));
    let mut rows = vec![
        ("settings-project-name", setting(p, "settings-project-name", t!("project.settings.name.title"), t!("project.settings.name.description"), name_field)),
        ("settings-project-icon", setting(p, "settings-project-icon", t!("dialog.project.edit.icon"), t!("project.settings.icon.description"), icon_button(s, p, x))),
    ];
    if x.icon.override_url.is_empty() {
        rows.push(("settings-project-color", setting(p, "settings-project-color", t!("dialog.project.edit.color"), t!("project.settings.color.description"), colors::<H>(p, x))));
    }
    rows.push((
        "settings-project-auto-accept-permissions",
        setting(p, "settings-project-auto-accept-permissions", t!("command.permissions.autoaccept.enable"), t!("project.settings.autoAccept.description"), auto_accept(s, p, x)),
    ));
    rows.push(("settings-project-startup", startup(s, p)));
    let card = list(p, "settings-project-list", super::rows(s, p, 20.0, rows));
    let body = col()
        .w_full()
        .gap(16.0)
        .children(st.missing.then(|| folder_missing(p, &x.directory)))
        .child(card);
    page(p, "settings-project-page", head, body)
}

/// `ProjectOptions`: Add project when it isn't in use, Edit and Rename where `row` offers them,
/// Reveal in the file manager, and Clear notifications for one in use.
pub fn options<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal, x: &Project, key: &str, size: f32) -> El {
    options_with(s, p, x, key, size, false)
}

fn build_options_menu(s: &dyn SettingsHost, used: bool, unread: bool, row_menu: bool) -> Menu {
    Menu::new(t!("rune.settings.project.menu"))
        .align(rune::Align::End)
        .gap(4.0)
        .items((!used).then(|| Item::new("add", t!("home.project.add"))))
        .items(row_menu.then(|| Item::new("edit", t!("common.edit"))))
        .items(row_menu.then(|| Item::new("rename", t!("common.rename"))))
        .item(Item::new("reveal", s.reveal_label()))
        .items(used.then(|| Item::new("clear", t!("sidebar.project.clearNotifications")).enabled(unread)))
        .hug()
}

fn handle_option_pick(s: &mut dyn SettingsHost, picked: Option<&str>, dir: &str, id: &str, tx: &mut Tx) {
    match picked {
        Some("add") => {
            tx.instant();
            s.touch_project(id);
        }
        Some("edit") => super::open_page(s, Page::Project(dir.to_owned(), Tab::General), tx),
        Some("rename") => {
            tx.instant();
            let name = s.projects().iter().find(|p| p.id == id).map(super::projects::name).unwrap_or_default();
            let st = &mut s.settings_pages_mut().project;
            st.renaming = Some(dir.to_owned());
            st.rename = TextEdit::new(&name);
            let end = st.rename.text.len();
            st.rename.set_selection(0, end);
            st.rename.focused = true;
        }
        Some("reveal") => {
            if let Err(e) = s.reveal_path(dir) {
                if e == "Path doesn't exist" {
                    let name = s
                        .projects()
                        .iter()
                        .find(|p| p.id == id)
                        .map(|p| p.name.clone())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| opencode_kit::path::basename(dir).to_owned());
                    s.toast_fail(t!("home.project.missing.title").into(), t!("home.project.missing.description", name = name));
                } else {
                    s.toast_fail(t!("common.requestFailed").into(), e);
                }
            }
        }
        Some("clear") => {
            if let Some(x) = s.projects().iter().find(|p| p.directory == dir).cloned() {
                s.clear_project_notifications(&x, tx);
            }
        }
        _ => {}
    }
}

pub fn options_with<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal, x: &Project, key: &str, size: f32, row_menu: bool) -> El {
    let (dir, id) = (x.directory.clone(), x.id.clone());
    let used = s.used_projects().iter().any(|u| u.id == x.id);
    let unread = used && s.project_has_unread(x);
    let anchor = key.to_owned();
    let open = s.popup().open_at(&anchor);
    row()
        .key(key.to_owned())
        .role("button")
        .label_static(t!("common.moreOptions"))
        .expanded(open)
        .size(size, size)
        .center()
        .radius(if size > 24.0 { 6.0 } else { 4.0 })
        .cursor(Cursor::Pointer)
        .focus_default(p)
        .fill(if open { p.pressed } else { Color::TRANSPARENT })
        .hover(if open { p.pressed } else { p.hover })
        .press(p.pressed)
        .menu_button(anchor.clone(), move |s: &mut H, tx: &mut Tx| {
            let menu = build_options_menu(s, used, unread, row_menu);
            let (dir, id) = (dir.clone(), id.clone());
            popup::open(s, menu, anchor.clone(), tx, move |s: &mut H, picked, tx| handle_option_pick(s, picked, &dir, &id, tx));
        })
        .child(icon(&ic::OUTLINE_DOTS, 16.0).color(p.icon_muted))
}

fn drop_icon_files(s: &mut dyn SettingsHost, files: &[std::path::PathBuf], tx: &mut Tx) {
    let Some(path) = files.first() else { return };
    let dir = s.settings_pages().project.dir.clone();
    let Some(id) = project(s, &dir).map(|x| x.id.clone()) else { return };
    read_icon(s, &id, path, tx);
}

fn icon_button<H: SettingsHost + 'static>(s: &H, p: &Pal, x: &Project) -> El {
    let custom = !x.icon.override_url.is_empty();
    let veil = el()
        .key("project-settings-icon-overlay")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .radius(6.0)
        .fill(p.contrast.alpha(0.8))
        .center()
        .group_opacity(0.0, 1.0)
        .child(icon(if custom { &ic::CLOSE } else { &ic::SHARE }, 16.0).color(p.icon_contrast));
    let failed = s.settings_pages().project.failed.contains(&Field::Icon);
    let id = x.id.clone();
    let button = el()
        .key("settings-project-icon-button")
        .role("button")
        .label_static(t!("dialog.project.edit.icon.alt"))
        .size(32.0, 32.0)
        .radius(6.0)
        .clip_rounded(6.0)
        .cursor(Cursor::Pointer)
        .focus_default(p)
        .hover_group_focus()
        .hover_motion(opencode_ui::style::HOVER_FADE, opencode_ui::style::HOVER_FADE)
        .on_press_tx(move |s: &mut H, tx: &mut Tx| {
            tx.instant();
            if custom {
                set_icon(s, &id, None, tx);
            } else {
                choose_icon::<H>(&id, tx);
            }
        })
        .on_drop_files(|s: &mut H, files: &[std::path::PathBuf], tx: &mut Tx| drop_icon_files(s, files, tx))
        .child(avatar(s, p, x))
        .child(veil);
    let button = if failed { button.box_shadow(Pal::ring_out(p.danger)) } else { button };
    col().items_end().gap(6.0).child(button).children(failed.then(|| save_failed::<H>(p, Field::Icon)))
}

fn pick_color(s: &mut dyn SettingsHost, id: &str, color_idx: usize, selected: bool, tx: &mut Tx) {
    tx.instant();
    let Some(x) = s.projects().iter().find(|p| p.id == id).cloned() else { return };
    let color = if selected { String::new() } else { COLORS[color_idx].to_owned() };
    patch(s, &x.id, Field::Icon, json!({ "icon": { "color": color, "override": x.icon.override_url } }), tx);
}

fn colors<H: SettingsHost + 'static>(p: &Pal, x: &Project) -> El {
    let chosen = if !x.icon.color.is_empty() { variant(&x.icon.color) } else { 8 };
    COLORS.iter().enumerate().fold(row().key("settings-project-color-list").gap(8.0).justify_end().wrap(), |r, (i, name)| {
        let selected = i == chosen;
        let id = x.id.clone();
        let ring = |c: Color| [rune::Shadow { spread: 2.0, color: p.layer1, ..Default::default() }, rune::Shadow { spread: 4.0, color: c, ..Default::default() }];
        let swatch = el()
            .key(format!("settings-project-color-{name}"))
            .role("button")
            .label(t!("dialog.project.edit.color.select", color = color_name(name)))
            .expanded(selected)
            .size(20.0, 20.0)
            .radius(6.0)
            .cursor(Cursor::Pointer)
            .focus_default(p)
            .on_press_tx(move |s: &mut H, tx: &mut Tx| pick_color(s, &id, i, selected, tx));
        // Selection is a neutral ring and focus the shared outline, so the two stay apart (`project.css`, opencode#64909);
        // unselected swatches fade in the `border-strong` double ring on hover over 120 ms (`project.css:341-346`).
        let swatch = if selected {
            ring(p.v2.border_inverse).into_iter().rev().fold(swatch, El::box_shadow)
        } else {
            let hover_ring = ring(p.q.border_strong).into_iter().rev().fold(
                el().key("hover-ring").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).radius(6.0).group_opacity(0.0, 1.0),
                El::box_shadow,
            );
            swatch.hover_group().hover_motion(opencode_ui::style::HOVER_FADE, opencode_ui::style::HOVER_FADE).child(hover_ring)
        };
        let swatch = swatch
            .child(tile(p, "", i, 20.0, 11.0))
            .children(selected.then(|| row().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).center().child(icon(&ic::CHECK, 16.0).color(p.icon_contrast))));
        r.child(swatch)
    })
}

fn pick_project_auto_accept(s: &mut dyn SettingsHost, id: &str, _tx: &mut Tx) {
    let dir = s.settings_pages().project.dir.clone();
    let Some(project_id) = project(s, &dir).map(|x| x.id.clone()) else { return };
    let value = match id {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    };
    s.set_project_auto_accept(&project_id, value);
}

fn auto_accept<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal, x: &Project) -> El {
    let default = if s.settings().auto_accept { t!("project.settings.autoAccept.default.on") } else { t!("project.settings.autoAccept.default.off") };
    let options = vec![
        ("default".to_owned(), default.to_owned()),
        ("on".to_owned(), t!("project.settings.autoAccept.on").to_owned()),
        ("off".to_owned(), t!("project.settings.autoAccept.off").to_owned()),
    ];
    let current = match s.settings().project_auto_accept.get(&x.id) {
        Some(true) => "on",
        Some(false) => "off",
        None => "default",
    };
    select(s, p, "settings-project-auto-accept-permissions-select", t!("command.permissions.autoaccept.enable"), options, current, pick_project_auto_accept)
}

const STARTUP_MIN_H: f32 = 80.0 - 16.0;

fn drag_startup_resize(s: &mut dyn SettingsHost, d: &mut Drag, tx: &mut Tx) {
    tx.instant();
    let st = &mut s.settings_pages_mut().project;
    if matches!(d.phase, Phase::Down) {
        d.data = st.startup_h.unwrap_or(5.0 * 17.55);
    }
    st.startup_h = Some((d.data + d.dy).max(STARTUP_MIN_H));
}

fn resize_grip<H: SettingsHost + 'static>() -> El {
    const DARK: [(u8, u8); 13] = [(5, 0), (6, 0), (4, 1), (5, 1), (3, 2), (4, 2), (2, 3), (3, 3), (1, 4), (2, 4), (0, 5), (1, 5), (0, 6)];
    const DARK_SHORT: [(u8, u8); 5] = [(5, 4), (6, 4), (4, 5), (5, 5), (4, 6)];
    const LIGHT: [(u8, u8); 8] = [(6, 1), (5, 2), (4, 3), (3, 4), (2, 5), (1, 6), (6, 5), (5, 6)];
    let dot = |(x, y): (u8, u8), c: Color| el().absolute(f32::from(x), f32::from(y)).size(1.0, 1.0).fill(c);
    let black = Color::BLACK.alpha(0.6);
    let white = Color::WHITE.alpha(0.6);
    let glyph = el()
        .key("glyph")
        .absolute(4.0, 4.0)
        .size(7.0, 7.0)
        .children(LIGHT.iter().map(|&d| dot(d, white)))
        .children(DARK.iter().chain(DARK_SHORT.iter()).map(|&d| dot(d, black)));
    el().key("settings-project-startup-resize")
        .size(15.0, 15.0)
        .pin(None, None, Some(0.0), Some(0.0))
        .cursor(rune::Cursor::ResizeRow)
        .on_drag(|s: &mut H, d: &mut Drag, tx: &mut Tx| drag_startup_resize(s, d, tx))
        .child(glyph)
}

fn startup<H: SettingsHost + 'static>(s: &H, p: &Pal) -> El {
    let st = &s.settings_pages().project;
    let failed = st.failed.contains(&Field::Startup);
    let input = opencode_ui::style::field(&st.startup, STARTUP)
        .key("settings-project-startup-input")
        .w_full()
        .color(p.text)
        .placeholder(t!("dialog.project.edit.worktree.startup.placeholder"), p.faint)
        .caret(p.text, p.selection)
        .selection_style(p.field_selection)
        .label_static(t!("dialog.project.edit.worktree.startup"))
        .on_edit(|s: &mut H| &mut s.settings_pages_mut().project.startup);
    let input = match st.startup_h {
        Some(h) => input.h(h),
        None => input.min_h(5.0 * 17.55),
    };
    let field = col()
        .key("settings-project-startup-field")
        .w_full()
        .pt(8.0)
        .pl(8.0)
        .pr(10.0)
        .pb(10.0)
        .radius(6.0)
        .fill(p.bg_base)
        .elevate(input_lift(p))
        .child(input)
        .child(resize_grip::<H>());
    let field = if failed { field.box_shadow(Pal::ring_out(p.danger)) } else { field };
    let hint = |sentence: &'static str| {
        let at = sentence.find('$').unwrap_or(sentence.len());
        let end = sentence[at..].find(|c: char| !(c == '$' || c == '_' || c.is_ascii_alphanumeric())).map_or(sentence.len(), |n| at + n);
        let (before, var, after) = (&sentence[..at], &sentence[at..end], &sentence[end..]);
        row()
            .w_full()
            .min_h(21.0)
            .wrap()
            .center_y()
            .children((!before.is_empty()).then(|| text(before).font(COPY).color(p.muted)))
            .children((!var.is_empty()).then(|| row().h(15.0).px(4.0).center_y().radius(4.0).fill(p.layer2).child(text(var).font(HINT_PILL).color(p.text))))
            .children((!after.is_empty()).then(|| text(after).font(COPY).color(p.muted)))
    };
    col()
        .key("setting-settings-project-startup")
        .w_full()
        .py(20.0)
        .gap(12.0)
        .child(
            col()
                .w_full()
                .gap(8.0)
                .child(text(t!("dialog.project.edit.worktree.startup")).font(TITLE).color(p.text))
                .child(text(t!("project.settings.worktree.startup.description")).font(COPY).color(p.muted).wrap()),
        )
        .child(field)
        .children(failed.then(|| save_failed::<H>(p, Field::Startup)))
        .child(col().w_full().child(hint(t!("project.settings.worktree.startup.hint.base"))).child(hint(t!("project.settings.worktree.startup.hint.new"))))
}

fn save_failed<H: SettingsHost + 'static>(p: &Pal, field: Field) -> El {
    row()
        .key("project-settings-save-failed")
        .role("alert")
        .gap(8.0)
        .center_y()
        .justify_end()
        .child(text(t!("common.requestFailed")).font(FAILED).color(p.danger))
        .child(
            text(t!("common.retry"))
                .key("project-settings-save-retry")
                .role("button")
                .label_static(t!("common.retry"))
                .font(FAILED)
                .color(p.text)
                .radius(4.0)
                .cursor(Cursor::Pointer)
                .focus_default(p)
                .spans(vec![rune::Span::new(0, t!("common.retry").len()).underline(p.text)])
                .on_press_tx(move |s: &mut H, tx: &mut Tx| save(s, field, tx)),
        )
}

/// A field left with a change saves it (Desktop saves on `blur`).
pub fn poll(s: &mut dyn SettingsHost, tx: &mut Tx) -> bool {
    if !matches!(s.settings_pages().page, Page::Project(..)) || !s.settings_open() {
        return false;
    }
    let st = &s.settings_pages().project;
    let name = (!st.name.focused && st.name.text.trim() != st.sent.0).then(|| st.name.text.trim().to_owned());
    let start = (!st.startup.focused && st.startup.text != st.sent.1).then(|| st.startup.text.clone());
    if let Some(n) = name {
        s.settings_pages_mut().project.sent.0 = n;
        save(s, Field::Name, tx);
    }
    if let Some(t) = start {
        s.settings_pages_mut().project.sent.1 = t;
        save(s, Field::Startup, tx);
    }
    false
}

fn save(s: &mut dyn SettingsHost, field: Field, tx: &mut Tx) {
    let dir = s.settings_pages().project.dir.clone();
    let Some(x) = project(s, &dir).cloned() else { return };
    let st = &s.settings_pages().project;
    let body = match field {
        Field::Name => {
            let typed = st.name.text.trim();
            json!({ "name": if typed == super::projects::folder_name(&x.directory) { "" } else { typed } })
        }
        Field::Startup => json!({ "commands": { "start": st.startup.text } }),
        Field::Icon => json!({ "icon": { "color": x.icon.color, "override": x.icon.override_url } }),
    };
    patch(s, &x.id, field, body, tx);
}

fn patch(s: &mut dyn SettingsHost, id: &str, field: Field, body: Value, _tx: &mut Tx) {
    s.settings_pages_mut().project.failed.retain(|f| *f != field);
    s.spawn_job(SettingsJob::ActiveServer(SettingsRequest::PatchProject {
        id: id.to_owned(),
        field,
        body,
    }));
}

fn choose_icon<H: SettingsHost + 'static>(id: &str, tx: &mut Tx) {
    let id = id.to_owned();
    let options = rune::FileDialogOptions { title: t!("dialog.project.edit.icon.alt").into(), ..Default::default() }
        .filter(t!("rune.dialog.images"), &["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico"]);
    tx.open_file(options, move |s: &mut H, result, tx: &mut Tx| {
        if let Ok(Some(path)) = result {
            read_icon(s, &id, &path, tx);
        }
    });
}

fn read_icon(s: &mut dyn SettingsHost, id: &str, path: &std::path::Path, tx: &mut Tx) {
    let Ok(bytes) = std::fs::read(path) else {
        s.settings_pages_mut().project.failed.push(Field::Icon);
        return;
    };
    let mime = match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("bmp") => "image/bmp",
        Some("ico") => "image/x-icon",
        _ => {
            s.settings_pages_mut().project.failed.push(Field::Icon);
            return;
        }
    };
    let url = format!("data:{mime};base64,{}", encode_base64(&bytes));
    set_icon(s, id, Some(url), tx);
}

fn set_icon(s: &mut dyn SettingsHost, id: &str, url: Option<String>, tx: &mut Tx) {
    let color = s.projects().iter().find(|p| p.id == id).map(|p| p.icon.color.clone()).unwrap_or_default();
    patch(s, id, Field::Icon, json!({ "icon": { "color": color, "override": url.unwrap_or_default() } }), tx);
}

/// Opens Settings on project `dir`'s own General page (Edit project).
pub fn edit_project(s: &mut dyn SettingsHost, dir: &str, tx: &mut Tx) {
    tx.instant();
    crate::open_settings_tx(s, Page::Project(dir.to_owned(), Tab::General), tx);
}

/// Renames project `dir` as a Projects row's field has it (`createInlineRename`).
pub fn commit_project_rename(s: &mut dyn SettingsHost, _tx: &mut Tx) {
    let (dir, typed) = {
        let st = &mut s.settings_pages_mut().project;
        let Some(dir) = st.renaming.take() else { return };
        (dir, st.rename.text.trim().to_owned())
    };
    let Some(x) = project(s, &dir).cloned() else { return };
    if typed.is_empty() || typed == super::projects::name(&x) {
        return;
    }
    let name = if typed == super::projects::folder_name(&dir) { String::new() } else { typed };
    s.spawn_job(SettingsJob::ActiveServer(SettingsRequest::RenameProject { id: x.id, name }));
}

/// Standard RFC 4648 base64 encoder with `=` padding.
fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map_or(0, u32::from);
        let b2 = chunk.get(2).copied().map_or(0, u32::from);
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 { TABLE[((triple >> 6) & 0x3F) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[(triple & 0x3F) as usize] as char } else { '=' });
    }
    out
}

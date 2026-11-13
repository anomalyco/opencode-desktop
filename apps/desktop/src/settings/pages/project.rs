//! A project's own settings (`settings/shell.tsx`'s `ProjectSettings`, `workspaces/project.tsx`):
//! the rail's Back to projects over General, Worktrees and Extensions. General reads the
//! project's avatar and name over its menu, then Project name (saved when the field loses
//! focus), Icon (an image chosen or dropped, or the custom one taken off), Color (while no image
//! is set), Auto-accept permissions (the default, On or Off for this project) and the Worktree
//! startup script; a save that fails says so under its field with Retry. Worktrees lists this
//! project's, and Extensions what its folder adds to the server's (`project_extensions.rs`).

use base64::Engine as _;
use opencode_kit::model::Project;
use rune::{Color, El, Icon, TextEdit, TextStyle, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;
use serde_json::{Value, json};

use super::{Page, header, page, select};
use crate::App;
use crate::jobs::Apply;
use crate::palette::{Elevate as _, Pal};
use crate::popup::{self, Item, Menu};
use crate::settings::{self, setting};
use opencode_i18n::t;

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
    pub extensions: super::project_extensions::State,
}

/// The project `dir` names.
pub fn project<'a>(s: &'a App, dir: &str) -> Option<&'a Project> {
    s.projects.iter().find(|p| p.directory == dir)
}

/// Fills the fields for `dir`'s project, as it opens.
pub fn load(s: &mut App, dir: &str, tab: Tab) {
    if s.settings_pages.project.dir != dir {
        let meta = project(s, dir).map(|p| opencode_kit::model::project_meta(&p.id)).unwrap_or_default();
        // The field shows the name the project goes by, its folder's when it has none of its own.
        let shown = if meta.name.is_empty() { crate::projects::folder_name(dir) } else { meta.name.clone() };
        let st = &mut s.settings_pages.project;
        st.dir = dir.to_owned();
        st.name = TextEdit::new(&shown);
        st.startup = TextEdit::new(&meta.start);
        st.startup_h = None;
        st.sent = (shown, meta.start.clone());
        st.failed.clear();
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
pub fn view(s: &App, p: &Pal, dir: &str, tab: Tab) -> El {
    let Some(x) = project(s, dir) else {
        return page(p, "settings-project-page", header(p, t!("rune.settings.project.menu"), ""), el());
    };
    match tab {
        Tab::General => general(s, p, x),
        Tab::Worktrees => super::worktrees::view_for(s, p, Some(&x.id)),
        Tab::Extensions => super::project_extensions::view(s, p, x),
    }
}

/// `.settings-project-name`, the startup script's 13/17.55 and the save failure's line.
const STARTUP: TextStyle = TextStyle::new(13.0, 440.0).lh(17.55).tracking(-0.04).mono();
const COPY: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0);
const TITLE: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0);
const FAILED: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0);
const HINT_CODE: TextStyle = TextStyle::new(12.0, 440.0).lh(20.0).mono();

/// Desktop's avatar colours, in its order (`PROJECT_AVATAR_VARIANTS`).
const COLORS: [&str; 9] = ["orange", "yellow", "cyan", "green", "red", "pink", "blue", "purple", "gray"];

/// A colour's name, as its swatch's label says it (`common.color.*`).
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

/// The colour a project's saved `color` is (`getProjectAvatarVariant`): gray when unset.
fn variant(color: &str) -> usize {
    let color = match color {
        "mint" => "cyan",
        "lime" => "green",
        c => c,
    };
    COLORS.iter().position(|c| *c == color).unwrap_or(8)
}

/// A letter tile in colour `v` (`project-avatar-surface`): 6 pt corners, a 0.5 pt inset ring,
/// the letter in 530 at `font`.
fn tile(p: &Pal, letter: &str, v: usize, size: f32, font: f32) -> El {
    let (bg, ring) = p.avatars[v];
    let t = el().size(size, size).radius(6.0).fill(bg).box_shadow(Pal::ring_in(ring)).center();
    if letter.is_empty() {
        return t;
    }
    t.child(text(letter.to_uppercase()).font(TextStyle::new(font, 530.0).lh(font).tracking(0.05)).color(p.avatar_fg))
}

/// The project's 32 pt avatar: its image, else its letter in its colour.
fn avatar(s: &App, p: &Pal, x: &Project) -> El {
    if x.icon.source(&x.id).is_some() {
        return s.icons.view(&crate::icons::palette(p), x, 32.0);
    }
    let name = super::projects::name(x);
    tile(p, name.chars().next().map(String::from).as_deref().unwrap_or(""), variant(&x.icon.color), 32.0, 16.0)
}

fn general(s: &App, p: &Pal, x: &Project) -> El {
    let st = &s.settings_pages.project;
    let name = super::projects::name(x);
    let folder = crate::projects::folder_name(&x.directory);
    let failed = |f: Field| st.failed.contains(&f);
    let head = row().key("settings-tab-header").w_full().h(32.0).gap(16.0).center_y()
        .child(row().grow(1.0).shrink().min_w(0.0).gap(12.0).center_y()
            .child(avatar(s, p, x))
            .child(text(name.clone()).font(super::TITLE).color(p.text).truncate().shrink().min_w(0.0)))
        .child(options(s, p, x, "settings-project-options", 28.0));
    let name_field = col().items_end().gap(6.0)
        // `.project-settings-name`: the base text input, as wide as the row leaves it.
        .child(rune_kit::TextInput::new(&st.name).placeholder(folder.clone()).label(t!("project.settings.name.title")).width(Some(198.67)).spellcheck(s.settings.spellcheck).text_style(super::NAME_REGULAR).height(28.0)
            .invalid(failed(Field::Name)).on_edit(|s: &mut App| &mut s.settings_pages.project.name).build(crate::palette::kit(p)).key("settings-project-name-input"))
        .children(failed(Field::Name).then(|| save_failed(p, Field::Name)));
    let mut rows = vec![
        ("settings-project-name", setting(p, "settings-project-name", t!("project.settings.name.title"), t!("project.settings.name.description"), name_field)),
        ("settings-project-icon", setting(p, "settings-project-icon", t!("dialog.project.edit.icon"), t!("project.settings.icon.description"), icon_button(s, p, x))),
    ];
    if x.icon.override_url.is_empty() {
        rows.push(("settings-project-color", setting(p, "settings-project-color", t!("dialog.project.edit.color"), t!("project.settings.color.description"), colors(p, x))));
    }
    rows.push(("settings-project-auto-accept-permissions", setting(p, "settings-project-auto-accept-permissions", t!("command.permissions.autoaccept.enable"), t!("project.settings.autoAccept.description"), auto_accept(s, p, x))));
    rows.push(("settings-project-startup", startup(s, p)));
    let list = settings::list(p, "settings-project-list", super::rows(s, p, 20.0, rows));
    let band = col().key("settings-tab-header-band").w_full().pt(48.0).fill(p.bg_base).sticky_top(0.0).child(head);
    col().key("settings-project-page").w_full().px(4.0)
        .child(band)
        .child(col().key("settings-project-page-body").w_full().pt(24.0).pb(80.0).child(list))
}

/// `ProjectOptions`: Add project when it isn't in use, Edit and Rename where `row` offers them,
/// Reveal in the file manager, and Clear notifications for one in use.
pub fn options(s: &App, p: &Pal, x: &Project, key: &str, size: f32) -> El {
    options_with(s, p, x, key, size, false)
}

pub fn options_with(s: &App, p: &Pal, x: &Project, key: &str, size: f32, row_menu: bool) -> El {
    let (dir, id) = (x.directory.clone(), x.id.clone());
    let used = s.used_projects().iter().any(|u| u.id == x.id);
    let unread = used && s.project_has_unread(x);
    let anchor = key.to_owned();
    row().key(key.to_owned()).role("button").label_static(t!("common.moreOptions")).size(size, size).center().radius(if size > 24.0 { 6.0 } else { 4.0 })
        .hover(p.hover).press(p.pressed)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            let menu = Menu::new(t!("rune.settings.project.menu")).align(rune::Align::End)
                .items((!used).then(|| Item::new("add", t!("home.project.add"))))
                .items(row_menu.then(|| Item::new("edit", t!("common.edit"))))
                .items(row_menu.then(|| Item::new("rename", t!("common.rename"))))
                .item(Item::new("reveal", crate::view::location_card::reveal_label()))
                .items(used.then(|| Item::new("clear", t!("sidebar.project.clearNotifications")).enabled(unread)))
                .hug();
            let (dir, id) = (dir.clone(), id.clone());
            popup::open(s, menu, anchor.clone(), tx, move |s: &mut App, picked, tx| match picked {
                Some("add") => {
                    tx.instant();
                    s.touch_project(&id);
                }
                Some("edit") => s.open_settings_page(Page::Project(dir.clone(), Tab::General), tx),
                Some("rename") => {
                    tx.instant();
                    let name = s.projects.iter().find(|p| p.id == id).map(super::projects::name).unwrap_or_default();
                    let st = &mut s.settings_pages.project;
                    st.renaming = Some(dir.clone());
                    st.rename = TextEdit::new(&name);
                    let end = st.rename.text.len();
                    st.rename.set_selection(0, end);
                    st.rename.focused = true;
                }
                Some("reveal") => {
                    if let Err(e) = crate::view::location_card::reveal(&dir) {
                        crate::status_toast::say(s, t!("rune.status.openFolderFailed"), &e);
                    }
                }
                Some("clear") => {
                    if let Some(x) = s.projects.iter().find(|p| p.directory == dir).cloned() {
                        s.clear_project_notifications(&x, tx);
                    }
                }
                _ => {}
            });
        })
        .child(icon(&ic::OUTLINE_DOTS, 16.0).color(p.icon_muted))
}

/// `project-settings-icon`: the 32 pt avatar; under the pointer, the dark veil with Upload (or
/// Remove over a custom image). A press chooses an image, or takes the custom one off.
fn icon_button(s: &App, p: &Pal, x: &Project) -> El {
    let custom = !x.icon.override_url.is_empty();
    let veil = el().key("project-settings-icon-overlay").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).radius(6.0)
        .fill(p.contrast.alpha(0.8)).center().group_opacity(0.0, 1.0)
        .child(icon(if custom { &ic::XMARK_SMALL } else { &ic::SHARE }, 16.0).color(p.icon_contrast));
    let failed = s.settings_pages.project.failed.contains(&Field::Icon);
    let id = x.id.clone();
    let button = el().key("settings-project-icon-button").role("button").label_static(t!("dialog.project.edit.icon.alt")).size(32.0, 32.0).radius(6.0).clip_rounded(6.0)
        .hover_group().hover_motion(session_view::style::HOVER_FADE, session_view::style::HOVER_FADE)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            if custom {
                set_icon(s, &id, None, tx);
            } else {
                choose_icon(s, &id, tx);
            }
        })
        .on_drop_files(|s: &mut App, files: &[std::path::PathBuf], tx: &mut Tx| {
            let Some(path) = files.first() else { return };
            let Some(x) = project(s, &s.settings_pages.project.dir.clone()).map(|x| x.id.clone()) else { return };
            read_icon(s, &x, path, tx);
        })
        .child(avatar(s, p, x))
        .child(veil);
    let button = if failed { button.box_shadow(Pal::ring_out(p.danger)) } else { button };
    col().items_end().gap(6.0).child(button).children(failed.then(|| save_failed(p, Field::Icon)))
}

/// `project-settings-colors`: Desktop's nine avatar colours, 20 pt and 8 apart; the chosen one
/// ringed in `border-focus` past a 2 pt gap and ticked. Choosing it again takes it off.
fn colors(p: &Pal, x: &Project) -> El {
    let chosen = (!x.icon.color.is_empty()).then(|| variant(&x.icon.color)).unwrap_or(8);
    let strong = session_view::palette::token(p.dark, "border-border-strong").unwrap_or(p.border);
    COLORS.iter().enumerate().fold(row().key("settings-project-color-list").gap(8.0).justify_end(), |r, (i, name)| {
        let selected = i == chosen;
        let id = x.id.clone();
        let ring = |c: Color| [rune::Shadow { spread: 2.0, color: p.layer1, ..Default::default() }, rune::Shadow { spread: 4.0, color: c, ..Default::default() }];
        let swatch = el().key(format!("settings-project-color-{name}")).role("button").label(t!("dialog.project.edit.color.select", color = color_name(name))).expanded(selected)
            .size(20.0, 20.0).radius(6.0)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                let Some(x) = s.projects.iter().find(|p| p.id == id).cloned() else { return };
                let color = if selected { String::new() } else { COLORS[i].to_owned() };
                patch(s, &x.id, Field::Icon, json!({ "icon": { "color": color, "override": x.icon.override_url } }), tx);
            })
            .child(tile(p, "", i, 20.0, 11.0))
            .children(selected.then(|| row().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).center().child(icon(&ic::CHECK, 14.0).color(p.icon_contrast))));
        let _ = strong;
        let swatch = if selected { ring(p.v2.border_focus).into_iter().rev().fold(swatch, El::box_shadow) } else { swatch };
        r.child(swatch)
    })
}

/// The project's own Auto-accept permissions: the default (and what it is), On or Off.
fn auto_accept(s: &App, p: &Pal, x: &Project) -> El {
    let default = if s.settings.auto_accept { t!("project.settings.autoAccept.default.on") } else { t!("project.settings.autoAccept.default.off") };
    let options = vec![("default".to_owned(), default.to_owned()), ("on".to_owned(), t!("project.settings.autoAccept.on").to_owned()), ("off".to_owned(), t!("project.settings.autoAccept.off").to_owned())];
    let current = match s.settings.project_auto_accept.get(&x.id) {
        Some(true) => "on",
        Some(false) => "off",
        None => "default",
    };
    select(s, p, "settings-project-auto-accept-permissions-select", t!("command.permissions.autoaccept.enable"), options, current, |s, id, _| {
        let Some(x) = project(s, &s.settings_pages.project.dir.clone()).map(|x| x.id.clone()) else { return };
        s.set_project_auto_accept(&x, match id { "on" => Some(true), "off" => Some(false), _ => None });
    })
}

/// The script field's height can't drag below `textarea-v2`'s 80 px less its 8 px padding.
const STARTUP_MIN_H: f32 = 80.0 - 16.0;

/// The resize grip Chromium draws in a `resize: vertical` textarea's corner (`textarea-v2`):
/// two diagonal strokes of black at 60% with white at 60% a pixel to their right, so they read on
/// light and dark fields, 4 in from the field's bottom right in its 15 pt corner. Dragging it sets
/// the script's height; the row stays as wide as the page.
fn resize_grip() -> El {
    const DARK: [(u8, u8); 13] = [(5, 0), (6, 0), (4, 1), (5, 1), (3, 2), (4, 2), (2, 3), (3, 3), (1, 4), (2, 4), (0, 5), (1, 5), (0, 6)];
    const DARK_SHORT: [(u8, u8); 5] = [(5, 4), (6, 4), (4, 5), (5, 5), (4, 6)];
    const LIGHT: [(u8, u8); 8] = [(6, 1), (5, 2), (4, 3), (3, 4), (2, 5), (1, 6), (6, 5), (5, 6)];
    let dot = |(x, y): (u8, u8), c: Color| el().absolute(f32::from(x), f32::from(y)).size(1.0, 1.0).fill(c);
    let black = Color::BLACK.alpha(0.6);
    let white = Color::WHITE.alpha(0.6);
    let glyph = el().key("glyph").absolute(4.0, 4.0).size(7.0, 7.0)
        .children(LIGHT.iter().map(|&d| dot(d, white)))
        .children(DARK.iter().chain(DARK_SHORT.iter()).map(|&d| dot(d, black)));
    el().key("settings-project-startup-resize").size(15.0, 15.0).pin(None, None, Some(0.0), Some(0.0))
        .cursor(rune::Cursor::ResizeRow)
        .on_drag(|s: &mut App, d: &mut rune::Drag, tx: &mut Tx| {
            tx.instant();
            let st = &mut s.settings_pages.project;
            if matches!(d.phase, rune::Phase::Down) {
                d.data = st.startup_h.unwrap_or(5.0 * 17.55);
            }
            st.startup_h = Some((d.data + d.dy).max(STARTUP_MIN_H));
        })
        .child(glyph)
}

/// `project-settings-startup`: its title and description, the script's five mono lines, saved
/// when the field loses focus, and how to name the worktrees in it.
fn startup(s: &App, p: &Pal) -> El {
    let st = &s.settings_pages.project;
    // `textarea-v2`: five 17.55 pt mono lines 8 in, on the base fill under the neutral lift.
    let failed = st.failed.contains(&Field::Startup);
    let input = rune::text_input(&st.startup, STARTUP).key("settings-project-startup-input").w_full()
        .color(p.text).placeholder(t!("dialog.project.edit.worktree.startup.placeholder"), p.faint).caret(p.text, p.selection).selection_style(p.field_selection).label_static(t!("dialog.project.edit.worktree.startup"))
        .on_edit(|s: &mut App| &mut s.settings_pages.project.startup);
    let input = match st.startup_h {
        Some(h) => input.h(h),
        None => input.min_h(5.0 * 17.55),
    };
    // The textarea's 8 in, and the 2 more the wrapper keeps at its end and bottom for the grip.
    let field = col().key("settings-project-startup-field").w_full().pt(8.0).pl(8.0).pr(10.0).pb(10.0).radius(6.0).fill(p.bg_base)
        .elevate(crate::status_toast::neutral_lift(p)).child(input).child(resize_grip());
    let field = if failed { field.box_shadow(Pal::ring_out(p.danger)) } else { field };
    // `inlineVariables`: the `$NAME` in the sentence set as code.
    let hint = |sentence: &'static str| {
        let at = sentence.find('$').unwrap_or(sentence.len());
        let end = sentence[at..].find(|c: char| !(c == '$' || c == '_' || c.is_ascii_alphanumeric())).map_or(sentence.len(), |n| at + n);
        let (before, var, after) = (&sentence[..at], &sentence[at..end], &sentence[end..]);
        row().center_y()
            .children((!before.is_empty()).then(|| text(before).font(COPY).color(p.muted)))
            .children((!var.is_empty()).then(|| row().px(4.0).radius(4.0).fill(p.layer2).child(text(var).font(HINT_CODE).color(p.text))))
            .children((!after.is_empty()).then(|| text(after).font(COPY).color(p.muted)))
    };
    col().key("setting-settings-project-startup").w_full().py(20.0).gap(12.0)
        .child(col().w_full().gap(8.0)
            .child(text(t!("dialog.project.edit.worktree.startup")).font(TITLE).color(p.text))
            .child(text(t!("project.settings.worktree.startup.description")).font(COPY).color(p.muted)))
        .child(field)
        .children(st.failed.contains(&Field::Startup).then(|| save_failed(p, Field::Startup)))
        .child(col().w_full()
            .child(hint(t!("project.settings.worktree.startup.hint.base")))
            .child(hint(t!("project.settings.worktree.startup.hint.new"))))
}

/// `project-settings-save-failed`: Request failed, and Retry underlined in the base ink.
fn save_failed(p: &Pal, field: Field) -> El {
    row().key("project-settings-save-failed").role("alert").gap(8.0).center_y().justify_end()
        .child(text(t!("common.requestFailed")).font(FAILED).color(p.danger))
        .child(text(t!("common.retry")).key("project-settings-save-retry").role("button").label_static(t!("common.retry")).font(FAILED).color(p.text)
            .spans(vec![rune::Span::new(0, t!("common.retry").len()).underline(p.text)])
            .on_press_tx(move |s: &mut App, tx: &mut Tx| save(s, field, tx)))
}

/// A field left with a change saves it (Desktop saves on `blur`).
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    if !matches!(s.settings_pages.page, Page::Project(..)) || !s.settings.open {
        return false;
    }
    let st = &s.settings_pages.project;
    let name = (!st.name.focused && st.name.text.trim() != st.sent.0).then(|| st.name.text.trim().to_owned());
    let start = (!st.startup.focused && st.startup.text != st.sent.1).then(|| st.startup.text.clone());
    if let Some(n) = name {
        s.settings_pages.project.sent.0 = n;
        save(s, Field::Name, tx);
    }
    if let Some(t) = start {
        s.settings_pages.project.sent.1 = t;
        save(s, Field::Startup, tx);
    }
    false
}

/// Saves `field` as typed, when it changed.
fn save(s: &mut App, field: Field, tx: &mut Tx) {
    let Some(x) = project(s, &s.settings_pages.project.dir.clone()).cloned() else { return };
    let st = &s.settings_pages.project;
    let body = match field {
        Field::Name => {
            // The folder's own name saves as none (`project-row.tsx`).
            let typed = st.name.text.trim();
            json!({ "name": if typed == crate::projects::folder_name(&x.directory) { "" } else { typed } })
        }
        Field::Startup => {
            json!({ "commands": { "start": st.startup.text } })
        }
        Field::Icon => json!({ "icon": { "color": x.icon.color, "override": x.icon.override_url } }),
    };
    patch(s, &x.id, field, body, tx);
}

/// `PATCH /api/project/:id`, the project as the server returns it then applied; a failure marks
/// `field` and toasts.
fn patch(s: &mut App, id: &str, field: Field, body: Value, tx: &mut Tx) {
    let _ = tx;
    s.settings_pages.project.failed.retain(|f| *f != field);
    let path = format!("/api/project/{}", crate::switcher::encode(id));
    s.job(move |srv| {
        let answer = srv.write("PATCH", &path, Some(body));
        Box::new(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            match answer.ok().and_then(|v| opencode_kit::model::parse_project(v.get("data").unwrap_or(&v))) {
                Some(updated) => s.apply_project(updated),
                None => {
                    s.settings_pages.project.failed.push(field);
                    crate::status_toast::say(s, t!("common.requestFailed"), t!("rune.settings.project.saveFailed"));
                }
            }
        }) as Apply
    });
}

/// Opens the file dialog for an image, then saves it as the project's icon.
fn choose_icon(_: &mut App, id: &str, tx: &mut Tx) {
    let id = id.to_owned();
    let options = rune::FileDialogOptions { title: t!("dialog.project.edit.icon.alt").into(), ..Default::default() }.filter(t!("rune.dialog.images"), &["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico"]);
    tx.open_file(options, move |s: &mut App, result, tx: &mut Tx| {
        if let Ok(Some(path)) = result {
            read_icon(s, &id, &path, tx);
        }
    });
}

/// Reads image `path` as a data URL and saves it as project `id`'s icon.
fn read_icon(s: &mut App, id: &str, path: &std::path::Path, tx: &mut Tx) {
    let Ok(bytes) = std::fs::read(path) else {
        s.settings_pages.project.failed.push(Field::Icon);
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
            s.settings_pages.project.failed.push(Field::Icon);
            return;
        }
    };
    let url = format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes));
    set_icon(s, id, Some(url), tx);
}

fn set_icon(s: &mut App, id: &str, url: Option<String>, tx: &mut Tx) {
    let color = s.projects.iter().find(|p| p.id == id).map(|p| p.icon.color.clone()).unwrap_or_default();
    patch(s, id, Field::Icon, json!({ "icon": { "color": color, "override": url.unwrap_or_default() } }), tx);
}

impl App {
    /// Opens Settings on project `dir`'s own General page (Edit project).
    pub fn edit_project(&mut self, dir: &str, tx: &mut Tx) {
        tx.instant();
        self.open_settings_at(Page::Project(dir.to_owned(), Tab::General));
    }

    /// A project as the server returned it after a change.
    pub fn apply_project(&mut self, updated: Project) {
        match self.projects.iter_mut().find(|p| p.id == updated.id) {
            Some(p) => *p = updated,
            None => self.projects.push(updated),
        }
    }

    /// Renames project `dir` as a Projects row's field has it (`createInlineRename`): its
    /// folder's name sends none.
    pub fn commit_project_rename(&mut self, tx: &mut Tx) {
        let st = &mut self.settings_pages.project;
        let Some(dir) = st.renaming.take() else { return };
        let typed = st.rename.text.trim().to_owned();
        let Some(x) = project(self, &dir).cloned() else { return };
        if typed.is_empty() || typed == super::projects::name(&x) {
            return;
        }
        let value = if typed == crate::projects::folder_name(&dir) { String::new() } else { typed };
        let path = format!("/api/project/{}", crate::switcher::encode(&x.id));
        let _ = tx;
        self.job(move |srv| {
            let answer = srv.write("PATCH", &path, Some(json!({ "name": value })));
            Box::new(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                match answer {
                    Ok(v) => {
                        if let Some(updated) = opencode_kit::model::parse_project(v.get("data").unwrap_or(&v)) {
                            s.apply_project(updated);
                        }
                    }
                    Err(e) => crate::status_toast::failed_wire(s, t!("rune.settings.project.saveFailed"), &e),
                }
            }) as Apply
        });
    }
}


//! Settings › Projects (`settings/workspaces/projects.tsx`, `project-row.tsx`): the server's
//! projects, the ones in use first, the rest under a fold (Other projects, with their count);
//! Add project; a search once there are more than seven or any others; each row with its avatar,
//! name and ⋯ menu (Add project, Reveal in the file manager).

use opencode_i18n::t;
use opencode_kit::model::Project;
use opencode_kit::path;
use opencode_ui::focus::FocusExt;
use opencode_ui::popup::PopupHost;
use opencode_ui::widgets;
use rune::{Align, Color, Cursor, El, Key, KeyEvent, TextEdit, TextStyle, Tx, col, el, icon, row, text};
use rune_kit::ButtonVariant;
use rune_theme_opencode::icons as ic;
use opencode_ui::palette::{Pal, kit};

use super::{filter_field, ghost_button, header_with, page, tab_no_results};
use crate::host::SettingsHost;

/// A project's name: 13 on 20 at 530.
const NAME: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
/// The fold's title and count.
const FOLD: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
const COUNT: TextStyle = TextStyle::new(11.0, 530.0).lh(14.0);

#[derive(Default)]
pub struct State {
    pub filter: TextEdit,
    /// Other projects unfolded.
    pub others: bool,
}

/// The final path segment of `dir`, or `dir` itself when empty.
pub fn folder_name(dir: &str) -> String {
    Some(path::basename(dir)).filter(|n| !n.is_empty()).unwrap_or(dir).to_owned()
}

/// What a project goes by: its name, else its folder's (`displayName`).
pub fn name(p: &Project) -> String {
    if p.name.is_empty() { folder_name(&p.directory) } else { p.name.clone() }
}

fn open_add_project(s: &mut dyn SettingsHost, tx: &mut Tx) {
    s.open_project_dialog(tx);
}

fn toggle_others(s: &mut dyn SettingsHost, tx: &mut Tx) {
    tx.instant();
    s.settings_pages_mut().projects.others = !s.settings_pages().projects.others;
}

fn open_project_general(s: &mut dyn SettingsHost, dir: &str, tx: &mut Tx) {
    super::open_page(s, super::Page::Project(dir.to_owned(), super::project::Tab::General), tx);
}

fn rename_key(s: &mut dyn SettingsHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key != Key::Escape {
        return false;
    }
    tx.instant();
    s.settings_pages_mut().project.renaming = None;
    true
}

/// Whether the Projects filter search bar is visible (more than 7 projects or any "other" projects).
pub(crate) fn searchable(s: &dyn SettingsHost) -> bool {
    let used = s.used_projects();
    let others = s.other_projects();
    used.len() + others.len() > 7 || !others.is_empty()
}

pub fn view<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal) -> El {
    let used = s.used_projects();
    let others = s.other_projects();
    let searchable = used.len() + others.len() > 7 || !others.is_empty();
    let query = if searchable { s.settings_pages().projects.filter.text.trim().to_lowercase() } else { String::new() };
    let hit = |x: &&Project| query.is_empty() || name(x).to_lowercase().contains(&query) || x.directory.to_lowercase().contains(&query);
    let added: Vec<&Project> = used.iter().copied().filter(hit).collect();
    let rest: Vec<&Project> = others.iter().copied().filter(hit).collect();
    let add = (!(used.is_empty() && others.is_empty()))
        .then(|| ghost_button::<H>(p, "settings-add-project", t!("home.project.add"), Some(&ic::PLUS), true, true, open_add_project));
    let actions = match (add, super::servers::switcher(s, p)) {
        (Some(a), Some(sw)) => Some(row().gap(8.0).center_y().child(a).child(sw)),
        (a, sw) => a.or(sw),
    };
    let mut head = col().w_full().gap(24.0).child(header_with(p, t!("settings.projects.title"), t!("settings.projects.description"), actions));
    if searchable {
        head = head.child(filter_field::<H>(p, "settings-projects-search", t!("settings.projects.search.placeholder"), &s.settings_pages().projects.filter, |st| &mut st.projects.filter));
    }
    let body = if used.is_empty() && others.is_empty() {
        empty::<H>(p)
    } else if added.is_empty() && rest.is_empty() {
        tab_no_results(p, "settings-projects-empty", &query)
    } else {
        let mut list = col().key("settings-projects").w_full().gap(12.0);
        if !added.is_empty() {
            list = list.child(project_list(s, p, "settings-project-list", &added, false));
        }
        if !rest.is_empty() {
            if query.is_empty() {
                list = list.child(fold::<H>(p, others.len(), s.settings_pages().projects.others));
            } else {
                list = list.child(text(t!("settings.projects.other")).font(FOLD).color(p.muted));
            }
            if s.settings_pages().projects.others || !query.is_empty() {
                list = list.child(project_list(s, p, "settings-other-project-list", &rest, true));
            }
        }
        list
    };
    page(p, "settings-projects-page", head, body)
}

/// `settings-project-empty-card`: the folder, No projects yet, and Add project.
fn empty<H: SettingsHost + 'static>(p: &Pal) -> El {
    col()
        .key("settings-project-empty-card")
        .w_full()
        .px(24.0)
        .py(96.0)
        .gap(8.0)
        .items_center()
        .radius(8.0)
        .fill(p.layer1)
        .box_shadow(Pal::ring_in(p.border_muted))
        .child(icon(&ic::FOLDER, 24.0).color(p.icon_muted).mb(8.0))
        .child(text(t!("settings.projects.empty.title")).font(FOLD).color(p.text).wrap().align(Align::Center))
        .child(text(t!("settings.projects.empty.description")).font(super::NAME_REGULAR).color(p.muted).wrap().align(Align::Center))
        .child(el().mt(24.0).child(widgets::button(t!("home.project.add"), ButtonVariant::Neutral).icon(&ic::PLUS).on_press_tx(|s: &mut H, tx: &mut Tx| open_add_project(s, tx)).build(kit(p))))
}

/// Other projects' fold: the triangle, its title and the count badge.
fn fold<H: SettingsHost + 'static>(p: &Pal, count: usize, open: bool) -> El {
    row()
        .key("settings-projects-others")
        .role("button")
        .label_static(t!("settings.projects.other"))
        .expanded(open)
        .h(28.0)
        .pl(4.0)
        .pr(8.0)
        .gap(4.0)
        .center_y()
        .radius(4.0)
        .self_start()
        .cursor(Cursor::Pointer)
        .focus_default(p)
        .hover(p.layer2)
        .on_press_tx(|s: &mut H, tx: &mut Tx| toggle_others(s, tx))
        .child(
            el().size(20.0, 20.0)
                .center()
                .child(icon(&ic::FILL_TRIANGLE_DOWN, 16.0).color(p.icon_muted).rotate(if open { 0.0 } else { -std::f32::consts::FRAC_PI_2 }).mirror_in_rtl()),
        )
        .child(
            row()
                .gap(6.0)
                .center_y()
                .child(text(t!("settings.projects.other")).font(FOLD).color(p.text))
                .child(row().h(16.0).min_w(16.0).px(4.0).center().radius(3.0).fill(p.layer2).child(text(count.to_string()).font(COUNT).color(p.muted))),
        )
}

/// `.settings-project-list`: a card of 40 pt rows 4 in, split by inset rules; one row alone is
/// 48 pt, 16 in.
fn project_list<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal, key: &str, projects: &[&Project], others: bool) -> El {
    let alone = projects.len() == 1;
    let n = projects.len();
    let mut list = col()
        .key(key.to_owned())
        .role("list")
        .w_full()
        .radius(8.0)
        .fill(p.layer1)
        .box_shadow(Pal::ring_in(p.border_muted))
        .clip_rounded(8.0);
    for (i, x) in projects.iter().enumerate() {
        let target = format!("project-{}", x.directory);
        let card = card(s, p, x, others, alone);
        let shell = col()
            .key(format!("setting-{target}"))
            .w_full()
            .px(if alone { 0.0 } else { 4.0 })
            .py(if alone { 0.0 } else { 4.0 })
            .child(card)
            .children((i + 1 < n).then(|| el().pin(Some(12.0), None, Some(12.0), Some(0.0)).h(0.5).fill(p.border)));
        list = list.child(super::revealable(s, p, &target, 0.0, shell));
    }
    list
}

/// `SettingsProjectRow`: the project's avatar and name, which open its own pages, and its menu.
fn card<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal, x: &Project, _other: bool, alone: bool) -> El {
    let label = name(x);
    let menu_key = format!("settings-project-menu-{}", x.id);
    let menu_open = s.popup().open_at(&menu_key);
    let st = &s.settings_pages().project;
    let renaming = st.renaming.as_deref() == Some(x.directory.as_str());
    let card = row()
        .key(format!("settings-project-{}", x.id))
        .role("listitem")
        .label(label.clone())
        .w_full()
        .h(if alone { 48.0 } else { 40.0 })
        .px(if alone { 16.0 } else { 12.0 })
        .gap(8.0)
        .center_y()
        .radius(if alone { 8.0 } else { 4.0 })
        .fill(if menu_open || renaming { p.layer2 } else { Color::TRANSPARENT })
        .hover(p.layer2);
    if renaming {
        let field = opencode_ui::style::field(&st.rename, NAME)
            .key("settings-project-rename")
            .spellcheck(s.settings().spellcheck)
            .single_line()
            .dir(rune::Direction::Auto)
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .color(p.text)
            .caret(p.text, p.selection)
            .selection_style(p.field_selection)
            .label_static(t!("common.rename"))
            .on_edit(|s: &mut H| &mut s.settings_pages_mut().project.rename)
            .on_submit(|s: &mut H, tx: &mut Tx| super::project::commit_project_rename(s, tx))
            .on_key(|s: &mut H, ev: &KeyEvent, tx: &mut Tx| rename_key(s, ev, tx));
        return card.child(row().grow(1.0).shrink().min_w(0.0).gap(8.0).center_y().child(SettingsHost::project_badge(s, p, Some(x), &label)).child(field));
    }
    let dir = x.directory.clone();
    // The button opens the project and covers the whole row, so its inset ring is the row's, as
    // `.settings-project-card:has(> button:first-child:focus-visible)` draws it (opencode#64909);
    // the menu's button sits over its end with a ring of its own.
    let open = row()
        .key("open")
        .role("button")
        .label(label.clone())
        .help(x.directory.clone())
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .pl(if alone { 16.0 } else { 12.0 })
        .pr(if alone { 16.0 } else { 12.0 } + 20.0 + 8.0)
        .radius(if alone { 8.0 } else { 4.0 })
        .cursor(Cursor::Pointer)
        .focus_inset(p)
        .gap(8.0)
        .center_y()
        .on_press_tx(move |s: &mut H, tx: &mut Tx| open_project_general(s, &dir, tx))
        .child(SettingsHost::project_badge(s, p, Some(x), &label))
        .child(text(label).font(NAME).color(p.text).truncate());
    card.child(open).child(rune::spacer()).child(super::project::options_with(s, p, x, &menu_key, 20.0, true))
}

/// A row's field lost focus: the rename is dropped, as a click away drops Desktop's.
pub fn poll(s: &mut dyn SettingsHost) -> bool {
    let st = &s.settings_pages().project;
    if st.renaming.is_none() || st.rename.focused {
        return false;
    }
    s.settings_pages_mut().project.renaming = None;
    true
}

//! Settings › Projects (`settings/workspaces/projects.tsx`, `project-row.tsx`): the server's
//! projects, the ones in use first, the rest under a fold (Other projects, with their count);
//! Add project; a search once there are more than seven or any others; each row with its avatar,
//! name and ⋯ menu (Add project, Reveal in the file manager).

use opencode_kit::model::Project;
use rune::{El, TextEdit, TextStyle, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use super::{filter_field, ghost_button, header_with, no_results, page};
use crate::App;
use crate::palette::Pal;

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

/// What a project goes by: its name, else its folder's (`displayName`).
pub fn name(p: &Project) -> String {
    if p.name.is_empty() { crate::projects::folder_name(&p.directory) } else { p.name.clone() }
}

pub fn view(s: &App, p: &Pal) -> El {
    let used = s.used_projects();
    let others = s.other_projects();
    let searchable = used.len() + others.len() > 7 || !others.is_empty();
    let query = if searchable { s.settings_pages.projects.filter.text.trim().to_lowercase() } else { String::new() };
    let hit = |x: &&Project| query.is_empty() || name(x).to_lowercase().contains(&query) || x.directory.to_lowercase().contains(&query);
    let added: Vec<&Project> = used.iter().copied().filter(hit).collect();
    let rest: Vec<&Project> = others.iter().copied().filter(hit).collect();
    let add = (!(used.is_empty() && others.is_empty())).then(|| ghost_button(p, "settings-add-project", "Add project", Some(&ic::PLUS), true, true, |s, tx| s.open_project_dialog(tx)));
    // Add project, then the server switcher (`settings-tab-header-actions`).
    let actions = match (add, super::servers::switcher(s, p)) {
        (Some(a), Some(sw)) => Some(row().gap(8.0).center_y().child(a).child(sw)),
        (a, sw) => a.or(sw),
    };
    let mut head = col().w_full().gap(24.0).child(header_with(p, "Projects", "Manage projects and their settings", actions));
    if searchable {
        head = head.child(filter_field(s, p, "settings-projects-search", "Search projects", &s.settings_pages.projects.filter, |s| &mut s.settings_pages.projects.filter));
    }
    let body = if used.is_empty() && others.is_empty() {
        empty(p)
    } else if added.is_empty() && rest.is_empty() {
        no_results(p, "settings-projects-empty", &query)
    } else {
        let mut list = col().key("settings-projects").w_full().gap(12.0);
        if !added.is_empty() {
            list = list.child(project_list(s, p, "settings-project-list", &added, false));
        }
        if !rest.is_empty() {
            if query.is_empty() {
                list = list.child(fold(p, others.len(), s.settings_pages.projects.others));
            } else {
                list = list.child(text("Other projects").font(FOLD).color(p.muted));
            }
            if s.settings_pages.projects.others || !query.is_empty() {
                list = list.child(project_list(s, p, "settings-other-project-list", &rest, true));
            }
        }
        list
    };
    page(p, "settings-projects-page", head, body)
}

/// `settings-project-empty-card`: the folder, No projects yet, and Add project.
fn empty(p: &Pal) -> El {
    col().key("settings-project-empty-card").w_full().py(96.0).gap(8.0).items_center().radius(8.0).fill(p.layer1).box_shadow(Pal::ring_in(p.border_muted))
        .child(icon(&ic::FOLDER, 24.0).color(p.icon_muted).mb(8.0))
        .child(text("No projects yet").font(FOLD).color(p.text))
        .child(text("Add a project to get started").font(super::NAME_REGULAR).color(p.muted))
        .child(el().mt(24.0).child(crate::widgets::button("Add project", rune_kit::ButtonVariant::Neutral).icon(&ic::PLUS).on_press_tx(|s: &mut App, tx: &mut Tx| s.open_project_dialog(tx)).build(crate::palette::kit(p))))
}

/// Other projects' fold: the triangle, its title and the count badge.
fn fold(p: &Pal, count: usize, open: bool) -> El {
    row().key("settings-projects-others").role("button").label("Other projects").h(28.0).pl(4.0).pr(8.0).gap(4.0).center_y().radius(4.0).self_start()
        .hover(p.layer2)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.settings_pages.projects.others = !s.settings_pages.projects.others;
        })
        .child(el().size(20.0, 20.0).center().child(icon(&ic::FILL_TRIANGLE_DOWN, 16.0).color(p.icon_muted).rotate(if open { 0.0 } else { -std::f32::consts::FRAC_PI_2 })))
        .child(row().gap(6.0).center_y()
            .child(text("Other projects").font(FOLD).color(p.text))
            .child(row().h(16.0).min_w(16.0).px(4.0).center().radius(4.0).fill(p.layer3).child(text(count.to_string()).font(COUNT).color(p.muted))))
}

/// `.settings-project-list`: a card of 40 pt rows 4 in, split by inset rules; one row alone is
/// 48 pt, 16 in.
fn project_list(s: &App, p: &Pal, key: &str, projects: &[&Project], others: bool) -> El {
    let alone = projects.len() == 1;
    let n = projects.len();
    let mut list = col().key(key.to_owned()).role("list").w_full().radius(8.0).fill(p.layer1).box_shadow(Pal::ring_in(p.border_muted)).clip_rounded(8.0);
    for (i, x) in projects.iter().enumerate() {
        let target = format!("project-{}", x.directory);
        let card = card(s, p, x, others, alone);
        let shell = col().key(format!("setting-{target}")).w_full().px(if alone { 0.0 } else { 4.0 }).py(if alone { 0.0 } else { 4.0 }).child(card)
            .children((i + 1 < n).then(|| el().pin(Some(12.0), None, Some(12.0), Some(0.0)).h(0.5).fill(p.border)));
        list = list.child(super::revealable(s, p, &target, 0.0, shell));
    }
    list
}

/// `SettingsProjectRow`: the project's avatar and name, which open its own pages, and its menu
/// (Add project, Edit, Rename, Reveal, Clear notifications); renaming, the name is a field in
/// place that Enter saves and Escape or a click away drops.
fn card(s: &App, p: &Pal, x: &Project, _other: bool, alone: bool) -> El {
    let label = name(x);
    let card = row().key(format!("settings-project-{}", x.id)).role("listitem").label(label.clone())
        .w_full().h(if alone { 48.0 } else { 40.0 }).px(if alone { 16.0 } else { 12.0 }).gap(8.0).center_y().radius(if alone { 8.0 } else { 4.0 })
        .hover(p.layer2);
    let st = &s.settings_pages.project;
    if st.renaming.as_deref() == Some(x.directory.as_str()) {
        let field = rune::text_input(&st.rename, NAME).key("settings-project-rename").spellcheck(s.settings.spellcheck).single_line().grow(1.0).shrink().min_w(0.0)
            .color(p.text).caret(p.text, p.selection).label("Rename")
            .on_edit(|s: &mut App| &mut s.settings_pages.project.rename)
            .on_submit(|s: &mut App, tx: &mut Tx| s.commit_project_rename(tx))
            .on_key(|s: &mut App, ev: &rune::KeyEvent, tx: &mut Tx| {
                if ev.key != rune::Key::Escape {
                    return false;
                }
                tx.instant();
                s.settings_pages.project.renaming = None;
                true
            });
        return card.child(row().grow(1.0).shrink().min_w(0.0).gap(8.0).center_y().child(crate::icons::badge(s, p, Some(x), &label)).child(field));
    }
    let dir = x.directory.clone();
    let open = row().key("open").role("button").label(label.clone()).help(x.directory.clone()).grow(1.0).shrink().min_w(0.0).h_full().gap(8.0).center_y()
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.open_settings_page(super::Page::Project(dir.clone(), super::project::Tab::General), tx))
        .child(crate::icons::badge(s, p, Some(x), &label))
        .child(text(label).font(NAME).color(p.text).truncate());
    card.child(open).child(super::project::options_with(s, p, x, &format!("settings-project-menu-{}", x.id), 24.0, true))
}

/// A row's field lost focus: the rename is dropped, as a click away drops Desktop's.
pub fn poll(s: &mut App) -> bool {
    let st = &mut s.settings_pages.project;
    if st.renaming.is_some() && !st.rename.focused {
        st.renaming = None;
        return true;
    }
    false
}

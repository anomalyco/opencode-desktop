//! The remote directory picker dialog view (`workspaces/selection/dialog.tsx:574-736`,
//! `dialog.css`): `<Dialog size="large" fit>` (`DialogSize::Large`, `Fit::Content`), its title and
//! the server's name, a hairline, then the path row, the recent projects, breadcrumbs and the
//! folder tree, and the selection beside Cancel and Select folder.

use std::collections::{BTreeMap, BTreeSet};

use opencode_i18n::t;
use opencode_ui::widgets::{DialogSize, Fit, button, dialog_window, sized_modal};
use rune::{Color, Cx, Direction, El, TextStyle, Tx, col, el, icon, row, text};
use rune_kit::{ButtonVariant, TextInput};
use rune_theme_opencode::icons as ic;
use opencode_ui::palette::{Elevate as _, Pal, kit};
use opencode_ui::style::{UI, UI_MED};

use super::{Dir, Node, Open, Suggest, Tree};
use crate::host::ProjectsHost;

/// One tree row (`--trees-item-height`) and the indent each level adds.
const ROW_H: f32 = 24.0;
const INDENT: f32 = 16.0;

/// Renders the remote directory picker modal dialog while open; returns an empty `Vec` when closed.
pub fn view<H: ProjectsHost + 'static>(s: &H, p: &Pal, cx: &Cx<H>) -> Vec<El> {
    let host: &dyn ProjectsHost = s;
    let Some(o) = host.remote_picker().open() else { return Vec::new() };
    let k = kit(p);
    let titles = header_titles(o, p);
    let close = close_button_shell(p).on_press_tx(|s: &mut H, tx: &mut Tx| super::close(s, tx));
    let header = row().key("directory-picker-header").w_full().padding(16.0).items_start().child(titles).child(close);

    let input = TextInput::new(&o.input)
        .placeholder(t!("dialog.directory.search.placeholder"))
        .label(t!("dialog.directory.search.placeholder"))
        .width(None)
        .spellcheck(false);
    // `showClearButton`: only while something is typed.
    let input = if o.input.text.is_empty() { input } else { input.clear_button() };
    let input = input
        .on_edit(|s: &mut H| super::input_edit(s))
        .on_change(|s: &mut H, tx: &mut Tx| super::typed(s, tx))
        .on_key(|s: &mut H, ev, tx| super::key(s, ev, tx))
        .build(k)
        .key("directory-picker-input")
        .grow(1.0)
        .shrink()
        .min_w(0.0);
    let mut path = row().key("directory-picker-path").w_full().gap(8.0).center_y().child(input);
    if let Suggest::Shown { items, active } = &o.suggest {
        let mut list = suggestions_shell(p);
        for (i, item) in items.iter().enumerate() {
            let to = item.clone();
            let lit = Some(i) == *active;
            list = list.child(
                suggestion_row_shell(p, i, item, &o.home, lit)
                    .on_press_tx(move |s: &mut H, tx: &mut Tx| super::choose_input(s, to.clone(), tx)),
            );
        }
        path = path.child(list.raise());
    }

    let mut body = col().key("directory-picker-body").w_full().grow(1.0).min_h(0.0).gap(12.0).pt(14.0).px(16.0).child(path);
    if !o.recent.is_empty() {
        let mut chips = row().key("directory-picker-recent-list").w_full().gap(8.0).wrap();
        for proj in o.recent.iter().take(3) {
            let to = proj.directory.clone();
            let name = proj.name.clone();
            let full = super::display(&proj.directory, &o.home);
            let avatar = host.project_badge(p, Some(proj), &proj.name);
            let chip = button(name.clone(), ButtonVariant::Neutral)
                .label_with(move |font, color| {
                    row()
                        .gap(6.0)
                        .center_y()
                        .max_w(214.0)
                        .child(avatar)
                        .child(text(name).font(font).color(color).truncate().shrink().min_w(0.0))
                })
                .help(full)
                .on_press_tx(move |s: &mut H, tx: &mut Tx| super::navigate(s, to.clone(), tx))
                .build(k)
                .key(format!("recent-{}", proj.id));
            chips = chips.child(chip);
        }
        if o.recent.len() > 3 {
            chips = chips.child(
                button("…", ButtonVariant::Ghost)
                    .help(t!("dialog.directory.moreProjects"))
                    .on_press_tx(|s: &mut H, tx: &mut Tx| super::toggle_more_projects(s, tx))
                    .build(k),
            );
        }
        body = body.child(chips);
        if o.suggest == Suggest::MoreProjects {
            let mut menu = col().role("menu").padding(4.0).radius(8.0).fill(p.bg_base).elevate(p.overlay);
            for project in o.recent.iter().skip(3) {
                let dir = project.directory.clone();
                menu = menu.child(
                    button(project.name.clone(), ButtonVariant::Ghost)
                        .help(dir.clone())
                        .on_press_tx(move |s: &mut H, tx: &mut Tx| super::navigate(s, dir.clone(), tx))
                        .build(k),
                );
            }
            body = body.child(menu);
        }
    }

    if let Tree::Browsing { root, .. } = &o.tree {
        let crumbs = breadcrumbs(root, &o.home);
        let mut nav = row().role("navigation").label_static(t!("dialog.directory.breadcrumbs")).center_y().gap(2.0);
        for (i, (label, path)) in crumbs.into_iter().enumerate() {
            if i > 0 {
                nav = nav.child(icon(&ic::CHEVRON_RIGHT, 14.0).color(p.faint));
            }
            nav = nav.child(
                button(label, ButtonVariant::Ghost)
                    .help(path.clone())
                    .on_press_tx(move |s: &mut H, tx: &mut Tx| super::navigate(s, path.clone(), tx))
                    .build(k),
            );
        }
        body = body.child(nav);
    }

    body = body.child(browser::<H>(o, p));
    let picked = o.tree.result().is_some() || !o.input.text.trim().is_empty();
    let cancel = button(t!("common.cancel"), ButtonVariant::Neutral)
        .on_press_tx(|s: &mut H, tx: &mut Tx| super::close(s, tx))
        .build(k)
        .key("directory-picker-cancel");
    let select = button(t!("dialog.directory.action.selectFolder"), ButtonVariant::Contrast)
        .disabled(!picked)
        .on_press_tx(|s: &mut H, tx: &mut Tx| super::resolve_pick(s, tx))
        .build(k)
        .key("directory-picker-select");
    let footer = row()
        .key("directory-picker-footer")
        .w_full()
        .padding(16.0)
        .gap(8.0)
        .center_y()
        .child(selection_label(o, p))
        .child(cancel)
        .child(select);

    // `<Dialog size="large" fit>` (`workspaces/selection/dialog.tsx`): large, on bg-base with
    // 12 pt corners, as tall as its content up to the large height.
    let card = col()
        .w_full()
        .radius(12.0)
        .fill(p.bg_base)
        .elevate(p.overlay)
        // The clip on a child: on the card it would clip the card's own shadow too.
        .child(
            col()
                .w_full()
                .clip_rounded(12.0)
                .child(header)
                // `divider-v2`: a 1 point rule drawn at half its height (`scaleY(0.5)`), centred in it.
                .child(col().w_full().h(1.0).py(0.25).child(el().w_full().h(0.5).fill(p.q.border_strong)))
                .child(body)
                .child(footer),
        );
    let card = col()
        .key("directory-picker")
        .role("dialog")
        .label(o.title.clone())
        .modal_tx(true, |s: &mut H, tx: &mut Tx| super::close(s, tx))
        .child(card);
    sized_modal("directory-picker-modal", p, DialogSize::Large, Fit::Content, dialog_window(cx, host.zoom()), card)
}

fn header_titles(o: &Open, p: &Pal) -> El {
    col()
        .key("directory-picker-titles")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(8.0)
        .child(text(o.title.clone()).font(TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04)).color(p.text))
        .child(
            text(o.subtitle.clone())
                .font(TextStyle::new(11.0, 440.0).lh(12.0).tracking(0.05))
                .color(p.muted)
                .truncate()
                .dir(Direction::Ltr),
        )
}

fn close_button_shell(p: &Pal) -> El {
    row()
        .key("directory-picker-close")
        .role("button")
        .label_static(t!("common.close"))
        .size(20.0, 20.0)
        .mt(-2.0)
        .mr(-2.0)
        .radius(4.0)
        .center()
        .hover(p.hover)
        .child(icon(&ic::CLOSE, 20.0).color(p.icon_muted))
}

fn suggestions_shell(p: &Pal) -> El {
    col()
        .key("directory-picker-suggestions")
        .role("listbox")
        .pin(Some(0.0), Some(34.0), Some(0.0), None)
        .padding(4.0)
        .radius(6.0)
        .fill(p.layer2)
        .box_shadow(Pal::ring_in(p.border))
        .elevate(p.overlay)
}

fn suggestion_row_shell(p: &Pal, i: usize, item: &str, home: &str, lit: bool) -> El {
    row()
        .key(format!("suggestion-{i}"))
        .role("option")
        .w_full()
        .px(8.0)
        .py(6.0)
        .radius(4.0)
        .fill(if lit { p.hover } else { Color::TRANSPARENT })
        .hover(p.hover)
        .child(
            text(format!("{}/", super::display(item, home)))
                .font(UI)
                .color(if lit { p.text } else { p.muted })
                .truncate()
                .dir(Direction::Ltr),
        )
}

fn breadcrumbs(root: &str, home: &str) -> Vec<(String, String)> {
    let mut crumbs = Vec::new();
    let mut path = root.to_owned();
    loop {
        let parent = super::parent_of(&path);
        let label = if path == home {
            "~"
        } else if parent == path {
            &path
        } else {
            opencode_kit::path::basename(&path)
        };
        crumbs.push((label.to_owned(), path.clone()));
        if parent == path {
            break;
        }
        path = if path == home { super::root_of(&path) } else { parent };
    }
    crumbs.reverse();
    let lead = if crumbs.get(1).is_some_and(|(label, _)| label == "~") { 2 } else { 1 };
    if crumbs.len() > lead + 3 {
        crumbs.drain(lead..crumbs.len() - 3);
        if let Some(slot) = crumbs.get_mut(lead) {
            slot.0 = "…".into();
        }
    }
    crumbs
}

fn selection_label(o: &Open, p: &Pal) -> El {
    let selection = o.tree.result().map(|r| super::shorten(r, &o.home)).unwrap_or_default();
    let full = o.tree.result().map(|r| super::display(r, &o.home)).unwrap_or_default();
    let mut chosen = text(selection.clone())
        .key("directory-picker-selection")
        .font(UI)
        .color(p.faint)
        .truncate()
        .dir(Direction::Ltr)
        .grow(1.0)
        .shrink()
        .min_w(0.0);
    if full != selection {
        chosen = chosen.help(full);
    }
    chosen
}

/// `directory-picker-browser`: 280 tall, 6 in, a 0.5 pt border (a whole device pixel that
/// takes its room, as a CSS border does), 6 pt corners on bg-base; the root's folders as a
/// tree, or Loading, or why it can't be read.
fn browser<H: ProjectsHost + 'static>(o: &Open, p: &Pal) -> El {
    let frame = col()
        .key("directory-picker-browser")
        .w_full()
        .h(280.0)
        .shrink()
        .min_h(0.0)
        .padding(6.0)
        .radius(6.0)
        .fill(p.bg_base)
        .border(0.5, p.border)
        .border_layout();
    let state = |words: &str| frame.clone().center().child(text(words.to_owned()).font(UI).color(p.muted));
    let Tree::Browsing { root, nodes, expanded, selected } = &o.tree else {
        return if o.tree == Tree::Failed { state(t!("dialog.directory.readError")) } else { state(t!("common.loading")) };
    };
    match nodes.get(root) {
        Some(Node::Loading { .. }) | None => return state(t!("common.loading")),
        Some(Node::Failed) => return state(t!("dialog.directory.readError")),
        Some(Node::Loaded(_)) => {}
    }
    let mut out: Vec<(Dir, usize)> = Vec::new();
    flatten(root, 0, nodes, expanded, &mut out);
    let rows: Vec<El> = out
        .into_iter()
        .map(|(d, depth)| {
            let open = expanded.contains(&d.absolute);
            let lit = selected.as_deref() == Some(d.absolute.as_str());
            let failed = matches!(nodes.get(&d.absolute), Some(Node::Failed));
            let is_git = o.git(&d.absolute);
            let to = d.absolute.clone();
            tree_row_shell(p, &d, depth, open, lit, failed, is_git)
                .on_press_tx(move |s: &mut H, tx: &mut Tx| super::toggle(s, &to, tx))
        })
        .collect();
    // Desktop's Pierre tree scrolls itself with its own bar (`workspaces/selection/dialog.tsx:533`),
    // its `--trees-fg` left at the tree's default, text-base on bg-base.
    let bar = opencode_ui::scroller::Bar::Tree { fg: p.text, bg: p.bg_base };
    frame.child(opencode_ui::scroller::scroller("directory-picker-tree").bar(bar).build::<H>(
        p,
        col().role("tree").w_full().grow(1.0).min_h(0.0),
        col().w_full().children(rows),
    ))
}

fn tree_row_shell(p: &Pal, d: &Dir, depth: usize, open: bool, lit: bool, failed: bool, is_git: bool) -> El {
    let mut r = row()
        .key(format!("dir-{}", d.absolute))
        .role("treeitem")
        .label(d.name.clone())
        .expanded(open)
        .w_full()
        .h(ROW_H)
        .pl(8.0 + depth as f32 * INDENT)
        .pr(8.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .child(
            icon(&ic::CHEVRON_DOWN, 14.0)
                .color(p.icon_muted)
                .rotate(if open { 0.0 } else { -std::f32::consts::FRAC_PI_2 })
                .mirror_in_rtl(),
        )
        .child(icon(&ic::FOLDER, 14.0).color(p.icon_muted))
        .child(
            text(d.name.clone())
                .font(if lit { UI_MED } else { UI })
                .color(if lit { p.text } else if failed { p.faint } else { p.muted })
                .truncate()
                .shrink()
                .min_w(0.0),
        );
    if is_git {
        r = r.child(
            row()
                .key("git")
                .h(16.0)
                .px(4.0)
                .radius(2.0)
                .center_y()
                .fill(p.layer3)
                .child(text(t!("dialog.directory.gitRepo")).font(TextStyle::new(11.0, 530.0).lh(12.0).tracking(0.05)).color(p.muted)),
        );
    }
    if lit {
        r = r.fill(p.pressed).box_shadow(Pal::ring_in(p.border));
    }
    r
}

/// The open folders' rows, each folder's children right under it.
fn flatten(
    dir: &str,
    depth: usize,
    nodes: &BTreeMap<String, Node>,
    expanded: &BTreeSet<String>,
    out: &mut Vec<(Dir, usize)>,
) {
    let Some(Node::Loaded(children)) = nodes.get(dir) else { return };
    for c in children {
        out.push((c.clone(), depth));
        if expanded.contains(&c.absolute) {
            flatten(&c.absolute, depth + 1, nodes, expanded, out);
        }
    }
}

//! What the card draws (`location-missing.tsx`, `location-missing.css`): a raised body with the
//! warning, the folder that is gone and what to do, over a footer tray with Choose worktree (a
//! Git project's) and Choose directory; and Choose worktree's menu, a popover over its trigger
//! with New worktree, the project's other worktrees, and a search field from ten of them.

use opencode_i18n::t;
use opencode_ui::palette::{Elevate as _, Pal, kit};
use opencode_ui::style::{UI, dock_w};
use rune::{Align, Color, El, PointerEvent, Side, TextStyle, Transition, Tx, col, el, icon, row, text};
use rune_kit::Loader;
use rune_theme_opencode::icons as ic;
use session_view::dock::{Variant, button};

use super::card::{Card, Source};
use super::{CHOOSE, LIST, LOCATION_MISSING, SCROLL, Subject, TRIGGER};
use crate::App;

/// `location-title`: 13 on the base line height at 530.
const TITLE: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0);
/// `location-path`: the code face at 12.
const PATH: TextStyle = TextStyle::new(12.0, 400.0).mono().lh(20.0);
/// `location-description`: 13 at 440.
const COPY: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0);
/// `button-v2`'s label.
const BUTTON: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04).features("\"tnum\"");
/// How far the tray tucks up under the body (`margin-block-start: -9px`).
const TUCK: f32 = 9.0;
/// The menu's width (`w-[200px]`).
const MENU_W: f32 = 200.0;

/// `SessionLocationMissing`, in the composer's place.
pub(crate) fn panel(s: &App, p: &Pal) -> El {
    let st = s.kernel.slots.get(&LOCATION_MISSING);
    // A frame before the poll that starts its steps draws it at rest.
    let Some((subject, card)) = st.card.clone().or_else(|| super::subject(s).map(|subject| (subject, Card::default()))) else { return el() };
    // `light-dark(yellow-800, yellow-700)`.
    let yellow = opencode_ui::palette::token(p.dark, if p.dark { "yellow-700" } else { "yellow-800" }).unwrap_or(p.warning);
    let description = t!("session.location.description");
    let status = row()
        .key("location-status")
        .w_full()
        .items_start()
        .gap(8.0)
        .padding(4.0)
        .child(icon(&ic::WARNING, 16.0).color(yellow))
        .child(
            col()
                .key("location-details")
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .gap(4.0)
                .child(text(t!("session.location.unavailable")).key("location-title").role("status").font(TITLE).color(p.text).wrap())
                .child(text(subject.directory.clone()).key("location-path").font(PATH).color(p.faint).wrap_anywhere().selectable(true).selection_style(p.field_selection))
                .child(text(description).key("location-description").font(COPY).color(p.muted).wrap()),
        );
    let body = col().key("location-body").w_full().padding(8.0).radius(10.0).fill(p.bg_base).elevate(p.raised).child(status);

    let git = subject.project.as_deref().is_some_and(opencode_kit::model::project_git);
    let live = !card.disabled();
    let worktree = git.then(|| match card.pending() {
        Some(Source::Worktree) => moving(p, TRIGGER),
        _ => {
            let b = button(p, TRIGGER, t!("session.location.worktree"), Variant::Neutral, None, live).expanded(card.menu_open()).child(icon(&ic::CHEVRON_DOWN, 16.0).color(p.text));
            if live { b.on_press_tx(super::toggle_menu) } else { b }
        }
    });
    let choose = match card.pending() {
        Some(Source::Directory) => moving(p, CHOOSE),
        _ => {
            let b = button(p, CHOOSE, t!("session.location.choose"), Variant::Submit, None, live);
            if live { b.on_press_tx(super::choose_directory) } else { b }
        }
    };
    // The tray (`location-footer`): `layer-01` with a 0.5 border, under the body's bottom edge so
    // the body's shadow falls on it (the Classic question's tray, `dock/question/classic.rs`).
    let footer = row()
        .key("location-footer")
        .w_full()
        .mt(-TUCK)
        .pt(16.0)
        .pb(8.0)
        .px(12.0)
        .gap(8.0)
        .flex_wrap()
        .center_y()
        .justify_end()
        .corners([0.0, 0.0, 10.0, 10.0])
        .fill(p.layer1)
        .border_sides([0.5, 0.5, 0.5, 0.5], p.border)
        .border_layout()
        .paint_order(-1)
        .children(worktree)
        .child(choose);
    col()
        .key("session-location-missing")
        .role("group")
        .labelled_by("location-title")
        .description(format!("{} {description}", subject.directory))
        .w_full()
        .max_w(dock_w())
        .child(body)
        .child(footer)
}

/// A button while its move runs (`variant="loading"`): the loader, then "Moving session…".
fn moving(p: &Pal, key: &str) -> El {
    let label = t!("session.location.moving");
    row()
        .key(key.to_owned())
        .role("button")
        .label_static(label)
        .h(32.0)
        .px(15.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .fill(p.layer2)
        .box_shadow(Pal::ring_in(p.border_muted))
        .disabled(true)
        .focusable(false)
        .child(Loader::new().size(16.0).build(kit(p)))
        .child(text(label).font(BUTTON).color(p.text))
}

pub(super) fn row_key(directory: &str) -> String {
    format!("location-worktree-{directory}")
}

/// A row (`palette-menu-row` at `data-size="compact"`): 28 tall, 8 in, 6 pt corners, the hover
/// overlay while lit. The pointer lights it without taking focus from the search field.
fn menu_row(p: &Pal, directory: &str, active: &str, live: bool) -> El {
    let (glyph, label) = if directory.is_empty() { (&ic::PLUS, t!("workspace.new")) } else { (&ic::OUTLINE_WORKTREE, super::name(directory)) };
    let (point, pick) = (directory.to_owned(), directory.to_owned());
    let line = row()
        .key(row_key(directory))
        .role("option")
        .label(label.to_owned())
        .w_full()
        .h(28.0)
        .px(8.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .transition(Transition::none())
        .motion(rune::Spring::NONE)
        .fill(if active == directory { p.hover } else { Color::TRANSPARENT })
        .child(icon(glyph, 16.0).color(p.text))
        .child(text(label.to_owned()).font(UI).color(p.text).grow(1.0).shrink().min_w(0.0).truncate());
    let line = if directory.is_empty() { line } else { line.help(directory.to_owned()) };
    if !live {
        return line.disabled(true);
    }
    line.focusable(false)
        .on_pointer_move_local(move |s: &mut App, ev: PointerEvent, tx: &mut Tx| super::pointed(s, &point, ev, tx))
        .on_press_tx(move |s: &mut App, tx: &mut Tx| super::choose(s, &pick, tx))
}

/// Choose worktree's menu over its trigger (`Popover placement="top-end" gutter={4}`), and the
/// layer that closes it.
pub(super) fn menu(s: &App, p: &Pal) -> Option<[El; 2]> {
    let st = s.kernel.slots.get(&LOCATION_MISSING);
    let (subject, card): &(Subject, Card) = st.card.as_ref()?;
    if !card.menu_open() {
        return None;
    }
    let others = super::others(s, subject);
    let field = super::searchable(&others);
    let query = st.menu.search.text.trim().to_owned();
    let rows = super::visible(&others, &query);
    let active = super::active(s);
    let live = !card.disabled();
    // The first list is still on its way and there's nothing to show meanwhile.
    let loading = matches!(card.state, super::card::State::MenuLoading { .. }) && others.is_empty();
    let search = field.then(|| {
        row()
            .key("location-worktrees-search")
            .h(32.0)
            .pl(12.0)
            .pr(8.0)
            .gap(8.0)
            .center_y()
            .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
            .child(
                opencode_ui::style::field(&st.menu.search, UI)
                    .key("location-worktrees-input")
                    .single_line()
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .color(p.text)
                    .placeholder(t!("session.new.workspace.search.placeholder"), p.faint)
                    .caret(p.text, p.selection)
                    .selection_style(p.field_selection)
                    .on_edit(|s: &mut App| &mut s.kernel.slots.get_mut(&LOCATION_MISSING).menu.search)
                    .on_input_change(super::searched)
                    .on_input_key(super::menu_key),
            )
    });
    let mut list = col().key(LIST).role("listbox").label_static(t!("session.location.worktree")).w_full().child(menu_row(p, "", &active, live));
    // With a search field, the field holds focus and the keys; without one the list does.
    if !field {
        list = list.focusable(true).no_focus_ring().on_key(super::menu_key);
    }
    if loading || !others.is_empty() {
        list = list.child(el().key("location-worktrees-separator").h(0.5).mx(4.0).my(2.0).fill(p.border));
    }
    for directory in &rows {
        list = list.child(menu_row(p, directory, &active, live));
    }
    let mut content = col().w_full().padding(2.0).child(list);
    if loading {
        content = content.child(row().key("location-worktrees-loading").h(28.0).px(8.0).center_y().child(text(t!("common.loading")).font(UI).color(p.faint)));
    }
    if !query.is_empty() && rows.is_empty() {
        content = content.child(row().key("location-worktrees-empty").w_full().px(12.0).py(16.0).center().child(text(t!("session.new.workspace.search.empty")).font(UI).color(p.muted)));
    }
    // `w-[200px] rounded-md bg-layer-01`, `--v2-elevation-floating`; as tall as two thirds of the
    // window at most, and no taller than the room over the trigger (`anchored_fit`).
    let max_h = rune::memo::layout_size("zoom").map(|[_, h]| h * 2.0 / 3.0);
    let mut surface = col()
        .key("location-worktrees")
        .role("dialog")
        .label_static(t!("session.location.worktree"))
        .w(MENU_W)
        .radius(6.0)
        .fill(p.layer1)
        .elevate(p.floating)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(rune::Spring::NONE)
        .on_press(|_: &mut App| {})
        .children(search)
        .children(field.then(|| el().key("location-worktrees-divider").h(0.5).w_full().fill(p.border)))
        .child(opencode_ui::scroller::scroller(SCROLL).bar(opencode_ui::scroller::Bar::MenuScroll).build::<App>(p, col().w_full().shrink().min_h(0.0), content));
    if let Some(h) = max_h {
        surface = surface.max_h(h);
    }
    let scrim = el()
        .key("location-worktrees-scrim")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .transition(Transition::none())
        .on_background_press_tx(|s: &mut App, tx: &mut Tx| super::close_menu(s, false, tx));
    Some([scrim, surface.anchored_fit(TRIGGER.to_owned(), Side::Top, Align::End, 4.0)])
}

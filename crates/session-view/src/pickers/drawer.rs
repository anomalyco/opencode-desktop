//! The pickers in a narrow window: drawers from the window's foot ([`crate::sheet`]) in place of
//! the model dialog and the agent and effort menus.
//!
//! - **Models** (`ModelSelectorDrawer`, `select-dialog.tsx:309-471`, the narrow `ModelList`):
//!   Search models on the base surface, each provider's models as Settings › Models draws a
//!   provider (a header with its chevron, mark and name over a card of 48 pt rows, the picked
//!   one checked at its end), then Manage models and Connect an integration as 40 pt buttons.
//! - **Agent and effort** (`editor.tsx:1114-1180`, `mobileDrawer`): their choices as rows of a
//!   raised card, the picked one checked.

use rune::{El, TextStyle, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use super::{FAVORITES, Item, Kind, ModelAction, PickerHost, RECENT};
use crate::palette::{Elevate as _, Pal};
use crate::popover as pop;
use crate::style::INSTANT;

/// A row's name (`settings-row-title`): 13 / 16 at 440.
const ROW: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0);
/// A provider's name (`settings-models-group-title`): 13 / 16 at 530.
const GROUP: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0);
/// `button-v2` normal: 13 / 20 at 530.
const BUTTON: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0);
/// A menu drawer's rows (`text-[13px] font-[440] leading-[var(--line-height-base)]`).
const MENU: TextStyle = TextStyle::new(13.0, 440.0).lh(24.0);

pub const SHEET: &str = "picker-drawer";

/// The open picker's drawer, in a window `win_h` tall.
pub fn layer<S: PickerHost>(s: &S, p: &Pal, win_h: f32) -> Vec<El> {
    let Some(kind) = s.pickers().open else { return Vec::new() };
    let content = if kind == Kind::Model { models(s, p) } else { menu(s, p, kind) };
    crate::sheet::sheet(p, SHEET, false, win_h, win_h, content, |s: &mut S, tx: &mut Tx| {
        tx.instant();
        s.close_picker(true);
    })
}

fn models<S: PickerHost>(s: &S, p: &Pal) -> El {
    let pk = s.pickers();
    let field = pop::search_field(p, "picker-search", &pk.search, opencode_i18n::t!("dialog.model.search.placeholder"))
        .on_edit(|s: &mut S| &mut s.pickers_mut().search)
        .on_input_key(|s: &mut S, ev, tx| s.picker_key(ev, tx) || s.picker_search_key(ev, tx));
    // `text-input-v2` on the base surface: 28 tall, 6 pt corners, the query 8 in.
    let search = row()
        .key("picker-drawer-search")
        .w_full()
        .h(28.0)
        .pl(8.0)
        .pr(8.0)
        .center_y()
        .radius(6.0)
        .fill(p.bg_base)
        .elevate(p.raised)
        .child(field);
    let selected = s.picker_current(Kind::Model);
    let items: Vec<Item> = s.picker_items().into_iter().filter(|i| i.group != FAVORITES && i.group != RECENT).collect();
    let mut groups = col().key("picker-drawer-groups").w_full().gap(12.0).pt(4.0).pb(16.0);
    let mut at = 0;
    while at < items.len() {
        let group = items[at].group.clone();
        let end = items[at..].iter().position(|i| i.group != group).map_or(items.len(), |n| at + n);
        groups = groups.child(provider(s, p, &items[at..end], &selected));
        at = end;
    }
    if items.is_empty() {
        groups = groups.child(el().key("picker-empty").w_full().h(120.0).center().child(text(opencode_i18n::t!("dialog.model.empty")).font(crate::style::UI).color(p.muted)));
    }
    let actions = s.picker_model_actions().into_iter().rev().fold(col().key("picker-drawer-actions").w_full().gap(8.0), |c, a| c.child(action::<S>(p, a)));
    col()
        .key("picker-drawer-models")
        .w_full()
        .min_h(0.0)
        .gap(8.0)
        .child(
            col()
                .key("picker-drawer-list")
                .w_full()
                .shrink()
                .min_h(0.0)
                .child(col().w_full().pt(1.0).pb(12.0).child(search))
                .child(col().key("picker-drawer-scroll").w_full().shrink().min_h(0.0).scroll(rune::ScrollSpec { stick_bottom: false }).child(groups)),
        )
        .child(actions)
}

/// A provider's section (`settings-models-provider`): its header over a `catalog` card.
fn provider<S: PickerHost>(s: &S, p: &Pal, items: &[Item], selected: &str) -> El {
    let first = &items[0];
    let name = if first.heading.is_empty() { first.group.clone() } else { first.heading.clone() };
    let header = row()
        .key("header")
        .h(28.0)
        .py(4.0)
        .pl(4.0)
        .pr(8.0)
        .gap(4.0)
        .center_y()
        .child(row().size(20.0, 20.0).center().child(icon(&ic::CHEVRON_DOWN, 14.0).color(p.icon_muted)))
        .child(row().gap(8.0).center_y().children(s.picker_provider_mark(p, &first.group)).child(text(name).font(GROUP).color(p.text)));
    let n = items.len();
    let card = items.iter().enumerate().fold(
        col().key("card").w_full().radius(8.0).fill(p.layer1).box_shadow(Pal::ring_in(p.border_muted)),
        |card, (i, item)| {
            let pick = item.key.clone();
            let corners = [if i == 0 { 8.0 } else { 0.0 }, if i == 0 { 8.0 } else { 0.0 }, if i + 1 == n { 8.0 } else { 0.0 }, if i + 1 == n { 8.0 } else { 0.0 }];
            card.child(
                row()
                    .key(format!("option-{}", item.key))
                    .role("option")
                    .label(item.label.clone())
                    .w_full()
                    .h(48.0)
                    .px(12.0)
                    .gap(16.0)
                    .center_y()
                    .corners(corners)
                    .hover(p.hover)
                    .hover_motion(INSTANT, INSTANT)
                    .on_press_tx(move |s: &mut S, tx: &mut Tx| s.choose_picker_tx(&pick, tx))
                    .child(text(item.label.clone()).font(ROW).color(p.text).truncate().grow(1.0).shrink().min_w(0.0))
                    .children((item.key == selected).then(|| icon(&ic::CHECK, 14.0).color(p.icon))),
            )
            .children((i + 1 < n).then(|| el().w_full().h(0.5).mx(12.0).fill(p.border)))
        },
    );
    col().key(format!("provider-{}", first.group)).w_full().gap(12.0).pb(12.0).child(header).child(card)
}

/// Manage models or Connect an integration: `Button` ghost, 40 tall, its icon then its words.
fn action<S: PickerHost>(p: &Pal, a: ModelAction) -> El {
    let glyph = match a {
        ModelAction::Connect => &ic::PLUS,
        ModelAction::Manage => &ic::OUTLINE_SLIDERS,
    };
    row()
        .key(match a {
            ModelAction::Connect => "picker-drawer-connect",
            ModelAction::Manage => "picker-drawer-manage",
        })
        .role("button")
        .label(a.label())
        .w_full()
        .h(40.0)
        .px(11.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            tx.instant();
            s.close_picker(false);
            match a {
                ModelAction::Manage => s.pickers_mut().open_manage(),
                ModelAction::Connect => s.picker_model_action(a, tx),
            }
        })
        .child(icon(glyph, 16.0).color(p.icon))
        .child(text(a.label()).font(BUTTON).color(p.text))
}

/// The agent's or effort's choices as a drawer's card (`session-mobile-view-options`' look).
fn menu<S: PickerHost>(s: &S, p: &Pal, kind: Kind) -> El {
    let selected = s.picker_current(kind);
    s.picker_items().into_iter().fold(
        col().key("picker-drawer-menu").w_full().gap(2.0).padding(2.0).radius(6.0).fill(if p.dark { p.layer1 } else { p.bg_base }).elevate(p.raised),
        |card, item| {
            let pick = item.key.clone();
            let on = item.key == selected;
            card.child(
                row()
                    .key(format!("option-{}", item.key))
                    .role("option")
                    .label(item.label.clone())
                    .w_full()
                    .min_h(44.0)
                    .py(4.0)
                    .pl(12.0)
                    .pr(8.0)
                    .gap(8.0)
                    .center_y()
                    .radius(4.0)
                    .fill(if on { p.pressed } else { rune::Color::TRANSPARENT })
                    .hover(p.hover)
                    .hover_motion(INSTANT, INSTANT)
                    .on_press_tx(move |s: &mut S, tx: &mut Tx| s.choose_picker_tx(&pick, tx))
                    .child(text(item.label.clone()).font(MENU).color(p.text).truncate().grow(1.0).shrink().min_w(0.0))
                    .children(on.then(|| icon(&ic::CHECK, 16.0).color(p.icon_muted).ml(4.0))),
            )
        },
    )
}

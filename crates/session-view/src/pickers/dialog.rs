//! The model picker: Desktop's `DialogSelectModel` (`providers/models/select-dialog.tsx:629-1124`,
//! `select-dialog.css`), a dialog centred over the scrim rather than a popover on the composer.
//!
//! - **The dialog** (`select-dialog.css:40-45`, `shell/commands/dialog.css:6-12`): 480 wide
//!   and 480 tall, less 24 and 96 in a small window, 12 pt corners on `bg-base`, floating.
//! - **The search** (`select-dialog.css:49-96`): Search models, with Connect an integration and
//!   Manage models at its end, and a hairline under it.
//! - **The list** (`select-dialog.css:98-187`): Favorites, Recent, then each provider, under
//!   headers that stick for the whole list; 28 pt rows with the check before the name, and on
//!   the lit row its effort, "Default ›", whose flyout ([`super::flyout`]) opens beside it.

use opencode_kit::model::capitalize;
use rune::{El, Span, Spring, Transition, Tx, col, el, icon, rich, row, text};
use rune_theme_opencode::icons as ic;

use super::hover::ModelHover as _;
use super::{Item, Kind, ModelAction, PickerHost, flyout, hover};
use crate::palette::{Elevate as _, Pal};
use crate::popover::{self as pop, LIST};
use crate::style::UI;

/// `select-dialog.css:40-45`: at most 480 wide, 24 short of a narrower window.
const W: f32 = 480.0;
/// `dialog.css:6-12`: at most 480 tall, 96 short of a shorter window.
const H: f32 = 480.0;
/// `search-list-empty`: "No model results" in the middle of 120 pt.
const EMPTY_H: f32 = 120.0;
/// `model-dialog-variant`: the lit row's effort is at most 120 wide.
const VARIANT_W: f32 = 120.0;
/// Desktop's `Icon size="small"`: 14 pt (`icon.tsx`).
const SMALL: f32 = 14.0;

/// The scrim, the dialog and the lit model's flyout, while the model picker is open.
pub fn layer<S: PickerHost>(s: &S, p: &Pal) -> Vec<El> {
    // A press on the scrim closes it; nothing gets focus back, so the search field can unmount
    // in this frame.
    let scrim = col().key("picker-model-scrim").absolute(0.0, 0.0).w_full().h_full().px(12.0).py(48.0).center()
        .fill(p.scrim)
        .transition(Transition::none())
        .on_press_tx(|s: &mut S, tx: &mut Tx| {
            tx.instant();
            s.close_picker(false);
        })
        .child(dialog(s, p));
    let mut out = vec![hover::watch::<S>(scrim)];
    out.extend(flyout::view(s, p));
    out
}

fn dialog<S: PickerHost>(s: &S, p: &Pal) -> El {
    let pk = s.pickers();
    let field = pop::search_field(p, "picker-search", &pk.search, opencode_i18n::t!("dialog.model.search.placeholder"))
        .on_edit(|s: &mut S| &mut s.pickers_mut().search)
        .on_input_key(|s: &mut S, ev, tx| s.picker_key(ev, tx) || s.picker_search_key(ev, tx));
    let trailing: Vec<El> = s.picker_model_actions().into_iter().map(|a| model_action::<S>(p, a)).collect();
    col()
        .key("picker-model")
        .role("dialog")
        .label(opencode_i18n::t!("dialog.model.select.title"))
        .w_full()
        .max_w(W)
        .h(H)
        .shrink()
        .min_h(0.0)
        .radius(LIST.radius())
        .fill(p.bg_base)
        .elevate(p.floating)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .instant_changes()
        // Presses on its own space stay in it, off the scrim.
        .on_press(|_: &mut S| {})
        .child(pop::search_row(p, "picker-search-row", field, trailing))
        .child(list(s, p))
}

/// The results, filling the dialog under its search.
fn list<S: PickerHost>(s: &S, p: &Pal) -> El {
    let pk = s.pickers();
    let items = s.picker_items();
    let active = s.picker_active();
    let selected = s.picker_current(Kind::Model);
    let mut list = pop::List::new("picker-options", LIST).stuck_headers(p);
    let mut group = "";
    for item in &items {
        if item.group != group {
            let heading = if item.heading.is_empty() { item.group.as_str() } else { item.heading.as_str() };
            list.header(p, format!("group-{}", item.group), heading);
            group = &item.group;
        }
        let lit = active.as_deref() == Some(item.key.as_str());
        list.option(model_row(s, p, item, lit, item.key == selected, &pk.search.text), lit);
    }
    if items.is_empty() {
        list.other(el().key("picker-empty").w_full().h(EMPTY_H).center().transition(Transition::none())
            .child(text(opencode_i18n::t!("dialog.model.empty")).font(UI).color(p.muted)), EMPTY_H);
    }
    list.slide(s.knobs(), p, &pk.search.text);
    hover::scroll::<S>(list.build("picker-scroll", f32::MAX).grow(1.0).shrink().min_h(0.0))
}

/// A model's row (`select-dialog.tsx:911-979`, `select-dialog.css:177-218`): its check slot, its
/// name with the query marked, its provider after it in Favorites and Recent, Free and Latest,
/// and on the lit row the effort it would run with and a chevron. A press picks it.
fn model_row<S: PickerHost>(s: &S, p: &Pal, item: &Item, lit: bool, selected: bool, query: &str) -> El {
    let key = item.key.clone();
    let (enter, pick) = (key.clone(), key.clone());
    let name = match search_match(&item.label, query) {
        Some(r) => rich(item.label.clone(), vec![Span::new(r.start, r.end).color(p.accent)]),
        None => text(item.label.clone()),
    };
    let words = row().key("name").grow(1.0).shrink().min_w(0.0).gap(6.0).center_y()
        .child(name.key("label").font(UI).color(p.text).shrink().truncate())
        .children((!item.note.is_empty()).then(|| text(item.note.clone()).key("note").font(UI).color(p.faint).shrink().truncate()))
        .children(item.free.then(|| pop::badge(p, opencode_i18n::t!("model.tag.free"))))
        .children(item.latest.then(|| pop::badge(p, opencode_i18n::t!("model.tag.latest"))));
    let variant = lit.then(|| {
        let effort = s.model_effort(&key).unwrap_or_default();
        let label = if effort.is_empty() { opencode_i18n::t!("common.default").to_owned() } else { capitalize(&effort) };
        row().key("variant").shrink().min_w(0.0).max_w(VARIANT_W).center_y()
            .child(text(label).font(UI).color(p.muted).shrink().truncate())
            .child(icon(&ic::CHEVRON_RIGHT, SMALL).color(p.icon_muted).offset(0.0, 0.5))
    });
    let r = row()
        .key(format!("option-{key}"))
        .role("option")
        .label(item.label.clone())
        .transition(Transition::none())
        .h(LIST.row_h)
        .w_full()
        .pl(LIST.pad_l)
        .pr(LIST.pad_r)
        .gap(LIST.gap)
        .center_y()
        .radius(LIST.row_radius)
        // Not `on_hover`: its commit would animate the highlight from row to row.
        .on_hover_change(move |s: &mut S, inside, tx: &mut Tx| {
            if inside {
                tx.instant();
                pop::jump_highlight();
                s.model_row_entered(&enter);
            }
        })
        .on_press_tx(move |s: &mut S, tx: &mut Tx| s.choose_picker_tx(&pick, tx))
        .child(row().key("check").size(pop::SLOT, pop::SLOT).center().children(selected.then(|| icon(&ic::CHECK, SMALL).color(p.icon))))
        .child(words)
        .children(variant);
    hover::row::<S>(r, &key).children(flyout::anchor(s, &key))
}

/// Where the query first shows in a model's name, ignoring case: the one stretch Desktop's
/// `SearchHighlight` marks (`search-list.tsx:385-411`). None when it isn't there as typed, or
/// when lowercasing moves the name's offsets.
fn search_match(name: &str, query: &str) -> Option<std::ops::Range<usize>> {
    let needle = query.trim().to_lowercase();
    let hay = name.to_lowercase();
    if needle.is_empty() || hay.len() != name.len() {
        return None;
    }
    hay.find(&needle).map(|at| at..at + needle.len())
}

/// A `ghost-muted` button at the search's end (`select-dialog.tsx:1010-1034`): it closes the
/// dialog and opens Manage models, or hands Connect to the host.
fn model_action<S: PickerHost>(p: &Pal, action: ModelAction) -> El {
    let (key, glyph) = match action {
        ModelAction::Connect => ("picker-connect", &ic::PLUS),
        ModelAction::Manage => ("picker-manage", &ic::OUTLINE_SLIDERS),
    };
    pop::search_button::<S>(p, key, glyph, action.label(), move |s: &mut S, tx: &mut Tx| {
        tx.instant();
        s.close_picker(false);
        match action {
            ModelAction::Manage => s.pickers_mut().open_manage(),
            ModelAction::Connect => s.picker_model_action(action, tx),
        }
    })
}

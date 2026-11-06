//! Side region `"New tab"` launcher cards, `Recently closed` list, and tab/`+` context menus.

use rune::{Align, Anim, El, Icon, ScrollSpec, Side, Tx, col, el, icon, row, text};

use super::state::ResolvedRegion;
use super::{SessionPane, icon_by_name};
use crate::App;
use crate::keymap::{self, Bind};
use crate::palette::Pal;
use crate::popover::{self, MENU, Opt};
use crate::view::{UI, UI_MED, UI_SMALL};

const INSTANT: Anim = Anim::INSTANT;

pub(super) fn new_tab_view(s: &App, p: &Pal, reg: &ResolvedRegion, pane: SessionPane) -> El {
    let mut list = col().key("side-launchers").w_full().max_w(520.0).pt(28.0).pb(24.0).gap(8.0);

    for &(plugin, ref desc, open_fn) in &reg.launchers {
        if desc.opened {
            continue;
        }
        let mut left = row().gap(8.0).center_y().grow(1.0).min_w(0.0);
        left = left
            .child(icon(desc.icon, 15.0).color(p.icon_muted))
            .child(text(desc.title.clone()).font(UI_MED).color(p.text));
        if let Some(detail) = &desc.description {
            left = left.child(
                text(detail.clone())
                    .font(UI_SMALL)
                    .color(p.muted)
                    .truncate_fade()
                    .shrink(),
            );
        }

        let mut card = row()
            .key(format!("side-launch-{plugin}-{}", desc.id))
            .role("button")
            .label(desc.title.clone())
            .w_full()
            .h(41.0)
            .px(14.0)
            .gap(12.0)
            .center_y()
            .radius(8.0)
            .fill(p.layer1)
            .border(0.5, p.border_muted)
            .hover(p.layer2)
            .press(p.pressed)
            .hover_motion(INSTANT, INSTANT)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                open_fn(s, pane, tx);
            })
            .child(left);
        let keys = keymap::first_label(desc.shortcut);
        if !keys.is_empty() {
            card = card.child(text(keys).font(UI_SMALL).color(p.faint));
        }
        list = list.child(card);
    }

    let open_keys: std::collections::HashSet<&str> =
        reg.strip.iter().map(|e| e.key.as_str()).collect();
    let closed_items: Vec<&super::tabs::ClosedSideTab> = s
        .side
        .sessions
        .get(&reg.session_id)
        .map(|st| {
            st.closed
                .iter()
                .filter(|c| !open_keys.contains(c.key.as_str()))
                .take(6)
                .collect()
        })
        .unwrap_or_default();

    if !closed_items.is_empty() {
        let mut recent = col().key("side-recently-closed").w_full().pt(16.0).gap(2.0).child(
            el().px(8.0)
                .pb(4.0)
                .child(text("Recently closed").font(UI_SMALL).color(p.muted)),
        );
        for (i, item) in closed_items.into_iter().enumerate() {
            let reopen_key = item.key.clone();
            let glyph = icon_by_name(item.icon_name.as_deref());
            let mut label_row = row()
                .gap(6.0)
                .center_y()
                .grow(1.0)
                .min_w(0.0)
                .child(icon(glyph, 14.0).color(p.icon_muted))
                .child(text(item.title.clone()).font(UI).color(p.text).truncate_fade());
            if let Some(det) = item.detail.as_deref().or(item.file.as_deref()) {
                label_row = label_row.child(
                    text(det.to_owned())
                        .font(UI_SMALL)
                        .color(p.faint)
                        .truncate_fade()
                        .shrink(),
                );
            }
            let mut r = row()
                .key(format!("side-closed-{i}"))
                .role("button")
                .label(format!("Reopen {}", item.title))
                .w_full()
                .h(30.0)
                .px(8.0)
                .gap(8.0)
                .center_y()
                .radius(6.0)
                .hover(p.hover)
                .press(p.pressed)
                .hover_motion(INSTANT, INSTANT)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    let _ = s.reopen_closed_side_tab(Some(&reopen_key), tx);
                })
                .child(label_row);
            if i == 0 {
                r = r.child(text(Bind::TabReopen.label()).font(UI_SMALL).color(p.faint));
            }
            recent = recent.child(r);
        }
        list = list.child(recent);
    }

    col()
        .key("side-new-tab-view")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .px(20.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(row().w_full().center().child(list))
}

/// The side panel's tab and `+` menus, drawn on every platform with the one popover every menu
/// is built from (`popover::MENU`), at the right-click.
pub fn context_menus(s: &App, p: &Pal) -> Option<El> {
    if let Some((ref target_key, x, y)) = s.side.tab_menu {
        let tk = target_key.clone();
        let tk_split = target_key.clone();
        let tk_side = target_key.clone();
        let tk_bottom = target_key.clone();
        let reg = s.resolve_side_region();
        let entry = reg.as_ref().and_then(|r| r.entry(target_key));
        let can_close = entry.is_some_and(|e| e.tab.can_close());
        let pane = entry.map_or(SessionPane::Side, |e| e.pane);
        let others: Vec<String> = reg
            .as_ref()
            .map(|r| {
                r.pane_entries(pane)
                    .into_iter()
                    .filter(|e| e.key != *target_key && e.tab.can_close())
                    .map(|e| e.key.clone())
                    .collect()
            })
            .unwrap_or_default();

        let mut rows = col().key("side-tab-menu-rows").w_full().padding(MENU.inset);
        if can_close {
            rows = rows.child(menu_row(p, "side-tab-menu-close", None, "Close tab", &Bind::TabClose.label(), move |s: &mut App, tx: &mut Tx| {
                s.side.tab_menu = None;
                s.close_side_tab(&tk, tx);
            }));
        }
        let has_others = !others.is_empty();
        if has_others {
            rows = rows.child(menu_row(p, "side-tab-menu-close-others", None, "Close other tabs", "", move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.side.tab_menu = None;
                for k in &others {
                    s.close_side_tab(k, tx);
                }
            }));
        }
        if can_close || has_others {
            rows = rows.child(separator(p));
        }
        rows = rows
            .child(menu_row(p, "side-tab-menu-split", None, "Move to split below", "", move |s: &mut App, tx: &mut Tx| {
                s.side.tab_menu = None;
                s.move_side_tab(&tk_split, SessionPane::Split, None, tx);
            }))
            .child(menu_row(p, "side-tab-menu-top", None, "Move to top pane", "", move |s: &mut App, tx: &mut Tx| {
                s.side.tab_menu = None;
                s.move_side_tab(&tk_side, SessionPane::Side, None, tx);
            }))
            .child(menu_row(p, "side-tab-menu-bottom", None, "Move to bottom dock", &Bind::BottomToggle.label(), move |s: &mut App, tx: &mut Tx| {
                s.side.tab_menu = None;
                s.move_side_tab(&tk_bottom, SessionPane::Bottom, None, tx);
            }));
        let menu = popover::surface::<App>(p, MENU, "side-tab-menu", "Tab").w(196.0).child(rows);
        return Some(layer(s, (x, y), menu, |s| s.side.tab_menu = None));
    }

    if let Some((pane, x, y)) = s.side.add_menu {
        let reg = s.resolve_side_region()?;
        let mut rows = col().key("side-add-menu-rows").w_full().padding(MENU.inset);
        for &(_pl, ref desc, open_fn) in &reg.launchers {
            let key = format!("side-add-menu-{}", desc.id);
            rows = rows.child(menu_row(p, &key, Some(desc.icon), &desc.title, &keymap::first_label(desc.shortcut), move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.side.add_menu = None;
                open_fn(s, pane, tx);
            }));
        }
        let menu = popover::surface::<App>(p, MENU, "side-add-menu", "New tab").w(220.0).child(rows);
        return Some(layer(s, (x, y), menu, |s| s.side.add_menu = None));
    }

    None
}

/// `menu` at the point (`x`, `y`), kept inside the window, over a layer whose press closes it.
fn layer(s: &App, (x, y): (f32, f32), menu: El, close: fn(&mut App)) -> El {
    el().key("side-menu-backdrop")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            close(s);
        })
        .on_secondary_press(move |s: &mut App, _: rune::PointerPress, tx: &mut Tx| {
            tx.instant();
            close(s);
        })
        .child(el().key("side-menu-point").absolute(x, y).size(0.0, 0.0))
        .child(popover::place(&s.knobs, menu, "side-menu-point", Side::Bottom, Align::Start, 2.0))
}

/// A menu row as the shared popover draws one: lit while the pointer is on it.
fn menu_row(p: &Pal, key: &str, glyph: Option<&'static Icon>, label: &str, hint: &str, pick: impl Fn(&mut App, &mut Tx) + 'static) -> El {
    let mut o = Opt::new(key.to_owned(), label.to_owned()).role("menuitem");
    if let Some(glyph) = glyph {
        o = o.lead(icon(glyph, popover::SLOT).color(p.icon_muted));
    }
    if !hint.is_empty() {
        o = o.trailing(popover::hint(p, hint));
    }
    popover::option::<App>(p, MENU, o, |_| {}, pick).hover(p.row_active).hover_motion(INSTANT, INSTANT)
}

fn separator(p: &Pal) -> El {
    col().key("side-menu-separator").w_full().py(2.0).child(popover::divider(p, "line"))
}

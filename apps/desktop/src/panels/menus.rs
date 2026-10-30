//! Side region `"New tab"` launcher cards, `Recently closed` list, and tab/`+` context menus.

use rune::{Anim, El, Icon, ScrollSpec, Tx, col, el, icon, row, text};

use super::state::ResolvedRegion;
use super::{SessionPane, icon_by_name};
use crate::App;
use crate::palette::Pal;
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
        if !desc.shortcut_hint.is_empty() {
            card = card.child(text(desc.shortcut_hint).font(UI_SMALL).color(p.faint));
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
                r = r.child(text("⇧⌘T").font(UI_SMALL).color(p.faint));
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

pub fn context_menus(s: &App, p: &Pal) -> Option<El> {
    if let Some((ref target_key, x, y)) = s.side.tab_menu {
        let tk = target_key.clone();
        let tk_others = target_key.clone();
        let tk_split = target_key.clone();
        let tk_side = target_key.clone();
        let tk_bottom = target_key.clone();

        let menu = col()
            .key("side-tab-menu")
            .absolute(x.max(8.0), (y + 4.0).max(8.0))
            .w(176.0)
            .padding(4.0)
            .gap(2.0)
            .radius(8.0)
            .fill(p.layer2)
            .border(0.5, p.ring)
            .shadow(4.0, 12.0, p.shadow)
            .child(menu_row(p, "Close tab", "⌘W").on_press_tx(move |s: &mut App, tx: &mut Tx| {
                s.side.tab_menu = None;
                s.close_side_tab(&tk, tx);
            }))
            .child(menu_row(p, "Close other tabs", "").on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.side.tab_menu = None;
                if let Some(reg) = s.resolve_side_region() {
                    let pane = reg.entry(&tk_others).map_or(SessionPane::Side, |e| e.pane);
                    let others: Vec<String> = reg
                        .pane_entries(pane)
                        .into_iter()
                        .filter(|e| e.key != tk_others && !e.tab.pinned)
                        .map(|e| e.key.clone())
                        .collect();
                    for k in others {
                        s.close_side_tab(&k, tx);
                    }
                }
            }))
            .child(el().w_full().h(0.5).my(2.0).fill(p.border_muted))
            .child(menu_row(p, "Move to split below", "").on_press_tx(move |s: &mut App, tx: &mut Tx| {
                s.side.tab_menu = None;
                s.move_side_tab(&tk_split, SessionPane::Split, None, tx);
            }))
            .child(menu_row(p, "Move to top pane", "").on_press_tx(move |s: &mut App, tx: &mut Tx| {
                s.side.tab_menu = None;
                s.move_side_tab(&tk_side, SessionPane::Side, None, tx);
            }))
            .child(menu_row(p, "Move to bottom dock", "⌘J").on_press_tx(move |s: &mut App, tx: &mut Tx| {
                s.side.tab_menu = None;
                s.move_side_tab(&tk_bottom, SessionPane::Bottom, None, tx);
            }));

        return Some(
            el().key("side-menu-backdrop")
                .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .on_press_tx(|s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.side.tab_menu = None;
                })
                .child(menu),
        );
    }

    if let Some((pane, x, y)) = s.side.add_menu {
        let reg = s.resolve_side_region()?;
        let mut menu = col()
            .key("side-add-menu")
            .absolute(x.max(8.0), (y + 4.0).max(8.0))
            .w(196.0)
            .padding(4.0)
            .gap(2.0)
            .radius(8.0)
            .fill(p.layer2)
            .border(0.5, p.ring)
            .shadow(4.0, 12.0, p.shadow);
        for &(_pl, ref desc, open_fn) in &reg.launchers {
            menu = menu.child(
                menu_row_icon(p, desc.icon, &desc.title, desc.shortcut_hint).on_press_tx(
                    move |s: &mut App, tx: &mut Tx| {
                        tx.instant();
                        s.side.add_menu = None;
                        open_fn(s, pane, tx);
                    },
                ),
            );
        }
        return Some(
            el().key("side-menu-backdrop")
                .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .on_press_tx(|s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.side.add_menu = None;
                })
                .child(menu),
        );
    }

    None
}

fn menu_row(p: &Pal, label: &str, hint: &str) -> El {
    let mut r = row()
        .role("menuitem")
        .label(label)
        .w_full()
        .h(28.0)
        .px(8.0)
        .gap(8.0)
        .center_y()
        .radius(5.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .child(text(label.to_owned()).font(UI).color(p.text).truncate_fade().grow(1.0));
    if !hint.is_empty() {
        r = r.child(text(hint.to_owned()).font(UI_SMALL).color(p.faint));
    }
    r
}

fn menu_row_icon(p: &Pal, glyph: &'static Icon, label: &str, hint: &str) -> El {
    let mut r = row()
        .role("menuitem")
        .label(label)
        .w_full()
        .h(28.0)
        .px(8.0)
        .gap(8.0)
        .center_y()
        .radius(5.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .child(icon(glyph, 14.0).color(p.icon_muted))
        .child(text(label.to_owned()).font(UI).color(p.text).truncate_fade().grow(1.0));
    if !hint.is_empty() {
        r = r.child(text(hint.to_owned()).font(UI_SMALL).color(p.faint));
    }
    r
}

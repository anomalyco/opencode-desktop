//! Side region `"New tab"` launcher cards, `Recently closed` list, and tab/`+` context menus.

use rune::{Align, Anim, El, Icon, ScrollSpec, Side, Tx, col, el, icon, row, text};

use super::state::ResolvedRegion;
use super::{SessionPane, icon_by_name};
use crate::App;
use crate::keymap::{self, Bind};
use crate::palette::Pal;
use crate::popover::{self, MENU, Opt};
use crate::view::{UI, UI_SMALL};

const INSTANT: Anim = Anim::INSTANT;

pub(super) fn new_tab_view(s: &App, p: &Pal, reg: &ResolvedRegion, pane: SessionPane) -> El {
    // `Launchers`: rows 12 apart in a 560 column, 48 from the strip.
    let mut list = col().key("side-launchers").w_full().max_w(560.0).pt(48.0).pb(40.0).gap(12.0);

    for &(plugin, ref desc, launch) in &reg.launchers {
        if desc.opened {
            continue;
        }
        let id = desc.id;
        let mut left = row().gap(8.0).center_y().grow(1.0).min_w(0.0);
        left = left
            .child(icon(desc.icon, 16.0).color(p.icon_muted))
            .child(text(desc.title.clone()).font(UI).color(p.text));
        if let Some(detail) = &desc.description {
            left = left.child(text(detail.clone()).font(UI).color(p.muted).truncate_fade().shrink());
        }

        // `LauncherRow`: 41 tall, 8 corners, a 0.5 `border-base` edge on `layer-01`.
        let mut card = row()
            .key(format!("side-launch-{plugin}-{}", desc.id))
            .role("button")
            .label(desc.title.clone())
            .w_full()
            .h(41.0)
            .px(14.0)
            .gap(16.0)
            .center_y()
            .radius(8.0)
            .fill(p.layer1)
            .border(0.5, p.border)
            .hover(p.layer2)
            .hover_motion(INSTANT, INSTANT)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                launch.run(s, id, pane, tx);
            })
            .child(left);
        // Its shortcut as Desktop's neutral `Keybind` chips.
        let caps = desc.keycaps(s);
        if !caps.is_empty() {
            card = card.child(keymap::chips(p, caps));
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
                .child(text(opencode_i18n::t!("home.recentlyClosed")).font(UI_SMALL).color(p.muted)),
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
                .label(opencode_i18n::t!("rune.tabs.reopen", title = item.title))
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
        .px(24.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(row().w_full().center().child(list))
}

/// The side panel's tab and `+` menus, drawn on every platform with the one popover every menu
/// is built from (`popover::MENU`), at the right-click.
pub fn context_menus(s: &App, p: &Pal) -> Option<El> {
    // Desktop's strip menu (`side-region.tsx`): Close tab and Close other tabs, each disabled
    // when there's nothing it closes; tabs move between panes by dragging.
    if let Some((ref target_key, x, y)) = s.side.tab_menu {
        let tk = target_key.clone();
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
        let has_others = !others.is_empty();

        let mut rows = col().key("side-tab-menu-rows").w_full().padding(MENU.inset);
        // A browser tab's page: open it in the default browser, or copy its address.
        if let Some((plugin, tid)) = super::split_panel_key(target_key)
            && plugin == "browser"
            && let Ok(id) = tid.parse::<u64>()
        {
            let url = s.browser_tab_external_url(id);
            let copy = url.clone();
            rows = rows
                .child(menu_row(p, "side-tab-menu-open-out", None, opencode_i18n::t!("plugin.browser.external.menu"), "", url.is_some(), move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.side.tab_menu = None;
                    s.open_browser_tab_externally(id, tx);
                }))
                .child(menu_row(p, "side-tab-menu-copy-url", None, opencode_i18n::t!("plugin.browser.link.copy"), "", copy.is_some(), move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.side.tab_menu = None;
                    if let Some(url) = &copy {
                        tx.copy(url.clone());
                    }
                }))
                .child(col().w_full().py(4.0).child(popover::divider(p, "side-tab-menu-divider")));
        }
        let rows = rows
            .child(menu_row(p, "side-tab-menu-close", None, opencode_i18n::t!("common.closeTab"), "", can_close, move |s: &mut App, tx: &mut Tx| {
                s.side.tab_menu = None;
                s.close_side_tab(&tk, tx);
            }))
            .child(menu_row(p, "side-tab-menu-close-others", None, opencode_i18n::t!("common.closeOtherTabs"), "", has_others, move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.side.tab_menu = None;
                for k in &others {
                    s.close_side_tab(k, tx);
                }
            }));
        let menu = popover::surface::<App>(p, MENU, "side-tab-menu", opencode_i18n::t!("common.key.tab")).min_w(160.0).child(rows);
        return Some(layer(s, (x, y), menu, |s| s.side.tab_menu = None));
    }

    if let Some((pane, x, y)) = s.side.add_menu {
        let reg = s.resolve_side_region()?;
        let mut rows = col().key("side-add-menu-rows").w_full().padding(MENU.inset);
        for &(_pl, ref desc, launch) in &reg.launchers {
            let key = format!("side-add-menu-{}", desc.id);
            let id = desc.id;
            let hint = match keymap::first_label(desc.shortcut) {
                l if l.is_empty() => desc.command.and_then(|c| keymap::command_chord(s, c)).map(keymap::label).unwrap_or_default(),
                l => l,
            };
            rows = rows.child(menu_row(p, &key, Some(desc.icon), &desc.title, &hint, true, move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.side.add_menu = None;
                launch.run(s, id, pane, tx);
            }));
        }
        let menu = popover::surface::<App>(p, MENU, "side-add-menu", opencode_i18n::t!("session.tab.new")).w(220.0).child(rows);
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
#[allow(clippy::too_many_arguments)]
fn menu_row(p: &Pal, key: &str, glyph: Option<&'static Icon>, label: &str, hint: &str, enabled: bool, pick: impl Fn(&mut App, &mut Tx) + 'static) -> El {
    let mut o = Opt::new(key.to_owned(), label.to_owned()).role("menuitem").disabled(!enabled);
    if let Some(glyph) = glyph {
        o = o.lead(icon(glyph, popover::SLOT).color(p.icon_muted));
    }
    if !hint.is_empty() {
        o = o.trailing(popover::hint(p, hint));
    }
    popover::option::<App>(p, MENU, o, |_| {}, pick).hover(p.row_active).hover_motion(INSTANT, INSTANT)
}

//! The Recent sidebar (Settings › Experimental › Organize vertical sidebar, Recent; local and dev
//! builds only; `recent-sessions.tsx`): in place of the side tabs, the pinned sessions in their
//! saved order, then the root sessions active in the last 7 days by recency, at most 30 until
//! Show more reveals 30 more of any age (`windowRecentSessions`, `recent-sessions-model.ts`).
//! Rows look like side tabs, without close buttons; a drag reorders the pins.
//!
//! The sessions are those of the server the window works against, as the Projects list's are:
//! the window already holds its list, so no page of its own loads.

use rune_kit::shell::{TabItem, TabItemStatus, TabOrientation, TabStrip};

use super::rows::section_title;
use super::state::key;
use super::*;
use crate::popup::{self, Item, Menu};
use crate::session_menu::{Spot, Target};
use opencode_i18n::t;

/// `RECENT_WINDOW_MS` and `RECENT_VISIBLE_LIMIT`.
const WINDOW_MS: i64 = 7 * 24 * 60 * 60 * 1000;
pub(crate) const VISIBLE: usize = 30;

/// The pinned and recent sessions (`partitionRecentSessions`, `windowRecentSessions`), and
/// whether Show more has more to show.
fn partition(s: &App) -> (Vec<&SessionInfo>, Vec<&SessionInfo>, bool) {
    let here = crate::servers::runtime::active(s);
    let st = &s.project_state.recent;
    let pinned: Vec<&SessionInfo> = st.pins.iter().filter_map(|k| super::state::split(k)).filter(|(server, _)| *server == here.as_str()).filter_map(|(_, id)| s.session(id)).filter(|i| i.archived.is_none()).collect();
    let mut recent: Vec<&SessionInfo> = s.sessions.iter().filter(|i| i.parent.is_none() && i.archived.is_none() && !crate::view::is_automation(&i.title) && !pinned.iter().any(|p| p.id == i.id)).collect();
    recent.sort_by(|a, b| b.active_ms().cmp(&a.active_ms()).then_with(|| a.id.cmp(&b.id)));
    let cutoff = *s.now_ms - WINDOW_MS;
    let in_window = recent.iter().position(|i| i.active_ms() < cutoff).unwrap_or(recent.len());
    let limit = in_window.min(VISIBLE) + st.revealed;
    let more = recent.len() > limit;
    recent.truncate(limit);
    (pinned, recent, more)
}

/// The Recent list in the sidebar's list slot.
pub(crate) fn recent(s: &App, p: &Pal) -> El {
    let (pinned, recent, more) = partition(s);
    let mut list = col().key("recent-content").w_full().pb(4.0);
    if !pinned.is_empty() {
        let ids: Vec<String> = pinned.iter().map(|i| i.id.clone()).collect();
        list = list.child(
            col().key("recent-pinned").role("group").label(t!("sidebar.recent.pinned")).w_full().gap(8.0).pb(10.0)
                .child(section_title(p, "recent-pinned-title", t!("sidebar.recent.pinned")))
                .child(strip(s, p, "recent-pinned-tabs", &pinned, Some(ids))),
        );
    }
    if !recent.is_empty() || more {
        let mut group = col().key("recent-list").role("group").label(t!("sidebar.recent.section.recent")).w_full().gap(8.0).pb(10.0)
            .child(section_title(p, "recent-list-title", t!("sidebar.recent.section.recent")))
            .child(strip(s, p, "recent-tabs", &recent, None));
        if more {
            group = group.child(super::rows::ghost_button(p, "recent-more", t!("sidebar.inbox.showMore"), false).on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.project_state.recent.revealed += VISIBLE;
            }));
        }
        list = list.child(group);
    }
    if pinned.is_empty() && recent.is_empty() && !more {
        list = list.child(text(t!("sidebar.recent.empty")).key("recent-empty").font(UI).color(p.faint).px(6.0).py(8.0));
    }
    let viewport = col().grow(1.0).shrink().min_h(0.0);
    let scroll = session_view::scroller::scroller("recent-scroll").build::<App>(p, viewport, list);
    col().key("sidebar-recent").w_full().grow(1.0).shrink().min_h(0.0).child(scroll)
}

/// One section's rows, drawn as side tabs (`TabNavItem`, vertical): `pins` makes them the pinned
/// list, which a drag reorders.
fn strip(s: &App, p: &Pal, strip_key: &str, sessions: &[&SessionInfo], pins: Option<Vec<String>>) -> El {
    let here = crate::servers::runtime::active(s).clone();
    let selected = match &*s.route {
        crate::Route::Session { id, server } if *server == here && !s.settings.open => Some(s.root_of(id).to_owned()),
        _ => None,
    };
    let pinned = pins.is_some();
    let items = sessions.iter().enumerate().map(|(index, info)| {
        let target = Target::on(&here, &info.id, Spot::Row);
        let mut item = TabItem::new(info.id.as_str(), crate::tabs::tab_title(&info.title))
            .status(status(s, &info.id))
            .closable(false)
            .marked(s.menus.on.as_ref() == Some(&target))
            .avatar(crate::icons::badge(s, p, s.project_for_session(info), &info.title));
        if let Some(field) = crate::session_menu_view::field(s, p, &target, UI_MED) {
            item = item.editor(field);
        }
        let (id, server, count) = (info.id.clone(), here.clone(), sessions.len());
        item.decorate(move |slot, _| slot.on_secondary_press(move |s: &mut App, press: rune::PointerPress, tx: &mut Tx| open_menu(s, &server, &id, pinned.then_some((index, count)), press, tx)))
    });
    let mut strip = TabStrip::new(strip_key, TabOrientation::Vertical)
        .tabs(items)
        .active(selected.as_deref())
        .fill(p.bg_deep)
        .close_icon(&ic::XMARK_SMALL)
        .busy_indicator(|k| session_view::style::tab_busy_glyph(k, 14.0))
        .label(t!("sidebar.recent.section.recent"))
        .on_select(|s: &mut App, id, tx| s.open(id, tx))
        .on_hover(|s: &mut App, id, inside, _| {
            if inside {
                s.prefetch_session(id);
            }
        });
    if let Some(visible) = pins {
        let server = here.as_str().to_owned();
        strip = strip.drag_state(&s.tab_drag, |s: &mut App| &mut s.tab_drag).on_reorder(move |s: &mut App, r, _| {
            let visible: Vec<String> = visible.iter().map(|id| key(&server, id)).collect();
            s.project_state.recent.reorder(&visible, r.from, r.to);
        });
    }
    let h = rune::memo::layout_with(&format!("{strip_key}-list"), |r| r[3]).unwrap_or(0.0);
    col().key(format!("{strip_key}-box")).w_full().h(h).child(strip.build(crate::palette::kit(p)))
}

/// A row's mark, as a side tab's (`useSessionTabAvatarState`).
fn status(s: &App, id: &str) -> Option<TabItemStatus> {
    Some(match s.tab_status(id)? {
        crate::TabStatus::Busy => TabItemStatus::Busy,
        crate::TabStatus::Attention => TabItemStatus::Attention,
        crate::TabStatus::Unread => TabItemStatus::Unread,
        crate::TabStatus::Failed => TabItemStatus::Failed,
    })
}

/// A row's menu (`RecentSessionTab`'s `menu`): Pin, or Unpin, Move up and Move down for a pinned
/// one; then Rename and Archive; then Delete….
fn open_menu(s: &mut App, server: &opencode_kit::servers::ServerKey, id: &str, pinned: Option<(usize, usize)>, press: rune::PointerPress, tx: &mut Tx) {
    let target = Target::on(server, id, Spot::Row);
    let mut menu = Menu::new(t!("command.category.session"));
    menu = match pinned {
        None => menu.item(Item::new("pin", t!("sidebar.recent.pin"))),
        Some((at, count)) => menu
            .item(Item::new("unpin", t!("sidebar.recent.unpin")))
            .item(Item::new("up", t!("sidebar.recent.moveUp")).enabled(at > 0))
            .item(Item::new("down", t!("sidebar.recent.moveDown")).enabled(at + 1 < count)),
    };
    menu = menu
        .separator()
        .item(Item::new("rename", t!("common.rename")))
        .item(Item::new("archive", t!("common.archive")))
        .separator()
        .item(Item::new("delete", format!("{}\u{2026}", t!("common.delete"))).danger(true));
    s.finish_rename(true);
    s.menus.on = Some(target.clone());
    tx.instant();
    let pin = key(server.as_str(), id);
    popup::open(s, menu, press, tx, move |s: &mut App, picked, tx| {
        s.menus.on = None;
        tx.instant();
        let visible = || pinned_keys(s);
        match picked {
            Some("pin") => s.project_state.recent.pin(pin),
            Some("unpin") => s.project_state.recent.unpin(&pin),
            Some(dir @ ("up" | "down")) => {
                let visible = visible();
                s.project_state.recent.shift(&visible, &pin, dir == "up");
            }
            Some("archive") => {
                let id = target.id.clone();
                s.archive_session(&id, tx);
            }
            Some(other) => crate::session_menu::run(s, &target, other, tx),
            None => {}
        }
    });
}

/// The pins the list shows, in order: the server's, unarchived.
fn pinned_keys(s: &App) -> Vec<String> {
    let here = crate::servers::runtime::active(s).as_str().to_owned();
    partition(s).0.iter().map(|i| key(&here, &i.id)).collect()
}

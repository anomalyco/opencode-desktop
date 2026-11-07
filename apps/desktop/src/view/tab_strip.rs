//! The session tabs, as Desktop's `TitlebarTabStrip` draws them (`tab-strip.tsx`,
//! `tab-nav.tsx`): across the titlebar for top tabs, down the sidebar for side tabs. One strip
//! over one tab store, so a tab is the same element in either place and flies between them
//! when the layout changes (`El::matched`, `shell-tab-{id}`).

use rune_kit::shell::{TabItem, TabItemStatus, TabOrientation, TabStrip};

use super::*;
use crate::session_menu::{Spot, Target};

/// The matched-flight prefix every tab shares across layouts.
pub(crate) const MATCHED: &str = "shell-tab";

/// A draft tab's pencil, drawn inline in `DraftTabItem` (not the catalog's `edit`).
static DRAFT_PENCIL: Icon = Icon {
    name: "draft-pencil",
    view: 16.0,
    body: r#"<path d="M9.00002 13.5H14M2.60419 10.9167V13.3958H5.08335L13.3959 5.08333L10.9167 2.60416L2.60419 10.9167Z" stroke="currentColor"/>"#,
};

/// The strip keyed `key`, running `orientation`. A horizontal one shares `viewport` points
/// (its width at the last layout) and keeps `trailing` (the `+` button) after its last tab.
pub(crate) fn tab_strip(s: &App, p: &Pal, key: &str, orientation: TabOrientation, viewport: Option<f32>, trailing: Option<El>) -> El {
    let current = match &s.route {
        Route::Session(id) if !s.settings.open => Some(s.root_of(id)),
        Route::Draft(id) if !s.settings.open => Some(id.as_str()),
        _ => None,
    };
    let vertical = orientation == TabOrientation::Vertical;
    let nav_ids: std::rc::Rc<Vec<String>> = std::rc::Rc::new(s.tabs.iter().map(|t| t.id.clone()).collect());
    let items = s.tabs.iter().enumerate().map(|(index, t)| item(s, p, t, index, nav_ids.clone()));
    let mut strip = TabStrip::new(key, orientation)
        .tabs(items)
        .active(current)
        .viewport_width(viewport)
        .zoom(s.zoom)
        .fill(p.bg_deep)
        .close_icon(&ic::XMARK_SMALL)
        .busy_indicator(busy_ring)
        .matched_prefix(Some(MATCHED))
        .label(if vertical { "Side tabs" } else { "Tabs" })
        .drag_state(&s.tab_drag, |s: &mut App| &mut s.tab_drag)
        .on_select(|s: &mut App, id, tx| s.select_tab(id, tx))
        .on_close(|s: &mut App, id, tx| s.close_tab(id, tx))
        .on_double_click(|s: &mut App, id, tx| {
            tx.instant();
            s.begin_rename(&Target::new(id, Spot::Tab));
        })
        .on_hover(|s: &mut App, id, inside, tx| {
            tx.live_only();
            if inside {
                s.prefetch_session(id);
            }
        })
        .on_reorder(|s: &mut App, r, _| s.move_tab(r.from, r.to));
    if let Some(t) = trailing {
        strip = strip.trailing_action(t);
    }
    strip.build(crate::palette::kit(p))
}

fn item<'a>(s: &'a App, p: &Pal, t: &'a crate::tabs::Tab, index: usize, nav_ids: std::rc::Rc<Vec<String>>) -> TabItem<'a, App> {
    let draft = t.is_draft();
    let info = s.session(&t.id);
    let title = match info {
        _ if draft => crate::tabs::tab_title(""),
        Some(i) => crate::tabs::tab_title(&i.title),
        None => crate::tabs::tab_title(&t.title),
    };
    let target = Target::new(&t.id, Spot::Tab);
    let marked = s.menus.on.as_ref() == Some(&target);
    let mut item = TabItem::new(t.id.as_str(), title).status(status(s, &t.id)).marked(marked);
    item = if draft {
        item.icon(&DRAFT_PENCIL)
    } else {
        let dir = info.map_or(t.directory.as_str(), |i| i.directory.as_str());
        let project = info.and_then(|i| s.project_for_session(i)).or_else(|| s.project_at(dir));
        item.avatar(crate::icons::badge(s, p, project, &project_or_dir(project, dir)))
    };
    if let Some(field) = crate::session_menu_view::field(s, p, &target, UI_MED) {
        item = item.editor(field);
    }
    let switching = s.switching_layout();
    item.decorate(move |slot, _| {
        // The arrows open the tab they reach as they pass it; its menu on a right-click.
        let slot = slot.on_key(move |s: &mut App, ev, _: &mut Tx| s.prefetch_navigation(&nav_ids, index, ev));
        // A layout switch flies the tab between the titlebar and the sidebar on GLIDE.
        let slot = if switching { slot.motion(GLIDE) } else { slot };
        crate::session_menu::on(slot, target)
    })
}

/// Desktop's tab mark (`tab-nav.tsx`'s `status`): busy while the family runs, then nothing on
/// the open tab, then attention, then the unread dot, red when the run failed.
fn status(s: &App, id: &str) -> Option<TabItemStatus> {
    Some(match s.tab_status(id)? {
        TabStatus::Busy => TabItemStatus::Busy,
        TabStatus::Attention => TabItemStatus::Attention,
        TabStatus::Unread => TabItemStatus::Unread,
        TabStatus::Failed => TabItemStatus::Failed,
    })
}

/// Desktop's "Soft Radar" activity ring at 14pt, in the muted icon colour; over a dark tab the
/// base one, where the muted spinner's dim dots vanish (`tab-nav.css`).
fn busy_ring(k: &rune_kit::Kit) -> El {
    let p = crate::palette::pal(k.dark);
    if !k.dark {
        return activity_ring(p);
    }
    let mut lit = p.clone();
    lit.icon_muted = p.icon;
    activity_ring(&lit)
}

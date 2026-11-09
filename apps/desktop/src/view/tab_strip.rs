//! The session tabs, as Desktop's `TitlebarTabStrip` draws them (`tab-strip.tsx`,
//! `tab-nav.tsx`): across the titlebar for top tabs, down the sidebar for side tabs. One strip
//! over one tab store, so a tab is the same element in either place and flies between them
//! when the layout changes (`El::matched`, `shell-tab-{id}`).

use rune_kit::shell::{TabItem, TabItemStatus, TabOrientation, TabStrip, TearOutSpec};

use super::*;
use crate::session_menu::{Spot, Target};

/// The matched-flight prefix every tab shares across layouts.
pub(crate) const MATCHED: &str = "shell-tab";
/// The active session's tab or Inbox card, the same element in every layout.
pub(crate) const ACTIVE: &str = "shell-active";

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
        // Close tab is a Tab stop after its tab, as Desktop's button is.
        .close_tab_stop(true)
        .busy_indicator(|k| session_view::style::busy_glyph(k, 14.0))
        .matched_prefix(Some(MATCHED))
        .label(if vertical { opencode_i18n::t!("settings.general.row.interfaceMode.side") } else { opencode_i18n::t!("titlebar.tabs") })
        .drag_state(&s.tab_drag, |s: &mut App| &mut s.tab_drag)
        .on_select(|s: &mut App, id, tx| s.select_tab(id, tx))
        .on_close(|s: &mut App, id, tx| s.close_tab(id, tx))
        .on_double_click(|s: &mut App, id, tx| {
            tx.instant();
            s.begin_rename(&Target::new(id, Spot::Tab));
        })
        .on_hover(|s: &mut App, id, inside, tx| {
            if inside {
                s.prefetch_session(id);
            }
            // The hover preview (`tab_preview`).
            s.preview_hover(id, inside, tx);
        })
        .on_reorder(|s: &mut App, r, _| s.move_tab(r.from, r.to));
    // A tab pulled off the strip opens in a window of its own (`windows::tear`): 36 across a
    // horizontal strip, 48 out of a vertical one.
    if s.persists() && s.tabs.len() > 1 {
        let detach_distance = if vertical { 48.0 } else { 36.0 };
        strip = strip
            .tear_out(Some(TearOutSpec { detach_distance, ..TearOutSpec::default() }))
            .on_tear_move(|s: &mut App, m, tx| s.tear_move(m, tx))
            .on_tear_out(|s: &mut App, t, tx| s.tear_release(t, tx));
    }
    if let Some(t) = trailing {
        strip = strip.trailing_action(t);
    }
    strip.build(crate::palette::kit(p)).on_layout_tx(|s: &mut App, r: [f32; 4], tx: &mut Tx| {
        tx.live_only();
        s.strip_laid_out(r);
    })
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
    let spring = s.switch_spring();
    let vertical = s.tab_layout().has_sidebar();
    // The routed session's, Settings open over it or not: Interface layout switches there.
    let active = matches!(&s.route, Route::Session(r) if s.root_of(r) == t.id);
    let id = t.id.clone();
    item.decorate(move |slot, _| {
        // The arrows open the tab they reach as they pass it; its menu on a right-click.
        let slot = slot.on_key(move |s: &mut App, ev, _: &mut Tx| s.prefetch_navigation(&nav_ids, index, ev));
        // Down the side tabs, the preview card follows the pointer's height.
        let slot = if vertical {
            let id = id.clone();
            slot.on_pointer_move_local(move |s: &mut App, p: rune::PointerEvent, tx: &mut Tx| s.preview_pointer(&id, p.y, p.window_y, tx))
        } else {
            slot
        };
        // A layout switch flies the tab between the titlebar and the sidebar on GLIDE.
        let slot = if switching { slot.motion(GLIDE) } else { slot };
        // The active tab is the session's place in every layout: it flies into its Inbox card,
        // and back, as one element (`shell-active-{id}`), on SETTLE between the sidebar's two.
        let slot = match (active, spring) {
            (true, Some(spring)) => slot.matched(format!("{ACTIVE}-{id}")).motion(spring),
            (true, None) => slot.matched(format!("{ACTIVE}-{id}")),
            (false, _) => slot,
        };
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

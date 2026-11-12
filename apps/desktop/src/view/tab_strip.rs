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
    // Every server's tabs mix in one strip, each named by its `Tab::uid`.
    let current = if s.settings.open { None } else { s.current_tab() };
    let vertical = orientation == TabOrientation::Vertical;
    let uids: Vec<String> = s.tabs.iter().map(crate::tabs::Tab::uid).collect();
    let nav_ids: std::rc::Rc<Vec<String>> = std::rc::Rc::new(uids.clone());
    let items = s.tabs.iter().zip(&uids).enumerate().map(|(index, (t, uid))| item(s, p, t, uid, index, nav_ids.clone()));
    let mut strip = TabStrip::new(key, orientation)
        .tabs(items)
        .active(current.as_deref())
        .viewport_width(viewport)
        .fill(p.bg_deep)
        .close_icon(&ic::XMARK_SMALL)
        // Close tab is a Tab stop after its tab, as Desktop's button is.
        .close_tab_stop(true)
        .busy_indicator(|k| session_view::style::tab_busy_glyph(k, 14.0))
        .matched_prefix(Some(MATCHED))
        .label(if vertical { opencode_i18n::t!("settings.general.row.interfaceMode.side") } else { opencode_i18n::t!("titlebar.tabs") })
        .drag_state(&s.tab_drag, |s: &mut App| &mut s.tab_drag)
        .on_select(|s: &mut App, id, tx| s.select_tab(id, tx))
        .on_close(|s: &mut App, id, tx| s.close_tab(id, tx))
        .on_double_click(|s: &mut App, id, tx| {
            tx.instant();
            if let Some(target) = s.tab(id).map(|t| target(s, t)) {
                s.begin_rename(&target);
            }
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
    // horizontal strip, 48 out of a vertical one. The only tab goes too while another window can
    // take it.
    if s.can_tear() {
        let detach_distance = if vertical { 48.0 } else { 36.0 };
        strip = strip
            .tear_out(Some(TearOutSpec { detach_distance, lone: true, ..TearOutSpec::default() }))
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

fn item<'a>(s: &'a App, p: &Pal, t: &'a crate::tabs::Tab, uid: &'a str, index: usize, nav_ids: std::rc::Rc<Vec<String>>) -> TabItem<'a, App> {
    let draft = t.is_draft();
    let face = face(s, t);
    let target = target(s, t);
    let marked = s.menus.on.as_ref() == Some(&target);
    // No server badge or dot on the tab itself: only its preview names the server
    // (`tab-nav.tsx:158-164`).
    let mut item = TabItem::new(uid, face.title).status(status(s, t)).marked(marked);
    item = if draft { item.icon(&DRAFT_PENCIL) } else { item.avatar(crate::icons::badge(s, p, face.project, &project_or_dir(face.project, face.dir))) };
    if let Some(field) = crate::session_menu_view::field(s, p, &target, UI_MED) {
        item = item.editor(field);
    }
    let switching = s.switching_layout();
    let spring = s.switch_spring();
    let vertical = s.tab_layout().has_sidebar();
    // The routed session's, Settings open over it or not: Interface layout switches there.
    let active = !draft && matches!(&*s.route, Route::Session { id: r, server } if *server == t.server && s.root_of(r) == t.id);
    let id = uid.to_owned();
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

/// What a tab's menu and rename act on: its session (or draft) on its own server, whichever one
/// the window works against (`tab-nav.tsx:103-104`).
pub(crate) fn target(_: &App, t: &crate::tabs::Tab) -> Target {
    Target::on(&t.server, &t.id, Spot::Tab)
}

/// What a tab shows of its session, read on the tab's own server: live on the server the window
/// works against, as of its last checkout on another (`servers::runtime::Checkout`).
pub(crate) struct Face<'a> {
    /// The strip's label (`sessionTabTitle`).
    pub title: String,
    /// The session's own title, when its server has it (the preview's `title`).
    pub session_title: Option<&'a str>,
    pub dir: &'a str,
    pub project: Option<&'a model::Project>,
}

pub(crate) fn face<'a>(s: &'a App, t: &'a crate::tabs::Tab) -> Face<'a> {
    if t.is_draft() {
        return Face { title: crate::tabs::tab_title(""), session_title: None, dir: &t.directory, project: None };
    }
    if t.server != *crate::servers::runtime::active(s) {
        let known = s.checkout.known(&t.server, &t.id);
        return Face { title: crate::tabs::tab_title(&t.title), session_title: known.then_some(t.title.as_str()), dir: &t.directory, project: s.checkout.project(&t.server, &t.id) };
    }
    let info = s.session(&t.id);
    let dir = info.map_or(t.directory.as_str(), |i| i.directory.as_str());
    let project = info.and_then(|i| s.project_for_session(i)).or_else(|| s.project_at(dir));
    Face { title: crate::tabs::tab_title(info.map_or(t.title.as_str(), |i| i.title.as_str())), session_title: info.map(|i| i.title.as_str()), dir, project }
}

/// Desktop's tab mark (`tab-nav.tsx`'s `status`): busy while the family runs, then nothing on
/// the open tab, then attention, then the unread dot, red when the run failed.
fn status(s: &App, t: &crate::tabs::Tab) -> Option<TabItemStatus> {
    let mark = if t.is_draft() { s.tab_status(&t.id) } else { s.tab_status_of(t) };
    Some(match mark? {
        TabStatus::Busy => TabItemStatus::Busy,
        TabStatus::Attention => TabItemStatus::Attention,
        TabStatus::Unread => TabItemStatus::Unread,
        TabStatus::Failed => TabItemStatus::Failed,
    })
}

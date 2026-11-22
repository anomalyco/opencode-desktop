//! The session tabs, as Desktop's `TitlebarTabStrip` draws them (`tab-strip.tsx`,
//! `tab-nav.tsx`): across the titlebar for top tabs, down the sidebar for side tabs. One strip
//! over one tab store, so a tab is the same element in either place and flies between them
//! when the layout changes (`El::matched`, `shell-tab-{id}`).

use rune_kit::shell::{TabItem, TabItemStatus, TabOrientation, TabSlot, TabStrip, TearOutSpec};

use super::*;
use crate::session_menu::{Spot, Target};

/// The matched-flight prefix every tab shares across layouts.
pub(crate) const MATCHED: &str = "shell-tab";
/// The active session's tab or Inbox card, the same element in every layout.
pub(crate) const ACTIVE: &str = "shell-active";

/// A draft tab's pencil, drawn inline in `DraftTabItem` (not the catalog's `edit`).
pub(crate) static DRAFT_PENCIL: Icon = Icon {
    name: "draft-pencil",
    view: 16.0,
    body: r#"<path d="M9.00002 13.5H14M2.60419 10.9167V13.3958H5.08335L13.3959 5.08333L10.9167 2.60416L2.60419 10.9167Z" stroke="currentColor"/>"#,
};

/// The strip keyed `key`, running `orientation`. A horizontal one shares `viewport` points
/// (its width at the last layout) and keeps `trailing` (the `+` button) after its last tab.
pub(crate) fn tab_strip(s: &App, p: &Pal, key: &str, orientation: TabOrientation, viewport: Option<f32>, trailing: Option<El>) -> El {
    let all: Vec<usize> = (0..s.tabs.len()).collect();
    tab_strip_filtered(s, p, key, orientation, viewport, trailing, &all)
}

/// [`tab_strip`] over a subset of tab indices (e.g. unpinned top tabs under Consolidated), keeping
/// `strip_laid_out` tracking intact for tear-out positioning and cross-window drops.
pub(crate) fn tab_strip_filtered(
    s: &App,
    p: &Pal,
    key: &str,
    orientation: TabOrientation,
    viewport: Option<f32>,
    trailing: Option<El>,
    indices: &[usize],
) -> El {
    strip(s, p, key, orientation, viewport, trailing, indices, false).on_layout_tx(|s: &mut App, r: [f32; 4], tx: &mut Tx| {
        tx.live_only();
        s.strip_laid_out(r);
    })
}

/// A side strip of only the tabs at `indices` (in `s.tabs`), as the Customize sidebar's sections
/// draw theirs (`verticalTabStrip(…, grouped)`, `sidebar-customized.tsx`). A drag reorders them
/// among themselves; `hide_avatars` drops their project avatars under a project's header.
pub(crate) fn section_strip(s: &App, p: &Pal, key: &str, indices: &[usize], hide_avatars: bool) -> El {
    strip(s, p, key, TabOrientation::Vertical, None, None, indices, hide_avatars)
}

#[allow(clippy::too_many_arguments)]
fn strip(s: &App, p: &Pal, key: &str, orientation: TabOrientation, viewport: Option<f32>, trailing: Option<El>, indices: &[usize], hide_avatars: bool) -> El {
    // Every server's tabs mix in one strip, each named by its `Tab::uid`.
    let current = if s.settings_open() { None } else { s.current_tab() };
    let vertical = orientation == TabOrientation::Vertical;
    let tabs: Vec<&crate::tabs::Tab> = indices.iter().filter_map(|&i| s.tabs.get(i)).collect();
    let uids: Vec<String> = tabs.iter().map(|t| t.uid()).collect();
    let nav_ids: std::rc::Rc<Vec<String>> = std::rc::Rc::new(uids.clone());
    // Each tab is read and built only when the strip shows it (`TabStrip::lazy_tabs`); its menu
    // or rename keeps it built wherever the strip scrolled.
    let slots = tabs
        .iter()
        .zip(&uids)
        .map(|(t, uid)| {
            let target = target(s, t);
            let menus = s.kernel.slots.get(&crate::session_menu::MENUS);
            let keep = menus.on.as_ref() == Some(&target) || menus.renames(&target) || s.previewed_tab() == Some(uid.as_str());
            TabSlot { id: uid.as_str().into(), pinned: false, two_line: false, keep }
        })
        .collect();
    let (built_tabs, built_uids) = (tabs.clone(), uids.clone());
    let show_hints = crate::tab_hints::shown(s);
    let nav_order = s.nav_order();
    let shortcuts: Vec<Option<usize>> = built_uids
        .iter()
        .map(|uid| nav_order.iter().position(|u| u == uid).filter(|&i| i < 9).map(|i| i + 1))
        .collect();
    let order: Vec<usize> = indices.to_vec();
    let mut strip = TabStrip::new(key, orientation)
        .words(tab_words())
        .lazy_tabs(slots, move |index| {
            item(s, p, built_tabs[index], built_uids[index].clone(), index, nav_ids.clone(), hide_avatars, shortcuts[index], show_hints)
        })
        .active(current.as_deref())
        .viewport_width(viewport)
        .fill(p.bg_deep)
        .close_icon(&ic::XMARK_SMALL)
        // Close tab is a Tab stop after its tab, as Desktop's button is.
        .close_tab_stop(true)
        .busy_indicator(|k| session_view::style::tab_busy_glyph(k, 14.0))
        .matched_prefix(Some(MATCHED))
        .label(if vertical { opencode_i18n::t!("settings.general.row.interfaceMode.side") } else { opencode_i18n::t!("titlebar.tabs") })
        .drag_state(&s.gestures.tab_drag, |s: &mut App| &mut s.gestures.tab_drag)
        // A mouse selects a tab as its button goes down, top or side, as Desktop's do
        // (`tab-nav.tsx`'s `onMouseDown`); a finger only on a tap, so a long press opens the
        // tab's menu and selects nothing.
        .select_on_drag_start(!session_view::style::touch_controls())
        // `tab-nav.css`'s `(hover: none)`: every tab shows its close button, its mark beside it.
        .hover_none(session_view::style::hover_none())
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
        .on_reorder(move |s: &mut App, r, _| {
            if let (Some(&from), Some(&to)) = (order.get(r.from), order.get(r.to)) {
                s.move_tab(from, to);
            }
        });
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
    strip.build(crate::palette::kit(p))
}

#[allow(clippy::too_many_arguments)]
fn item<'a>(
    s: &'a App,
    p: &Pal,
    t: &'a crate::tabs::Tab,
    uid: String,
    index: usize,
    nav_ids: std::rc::Rc<Vec<String>>,
    hide_avatar: bool,
    shortcut: Option<usize>,
    show_hints: bool,
) -> TabItem<'a, App> {
    let draft = t.is_draft();
    let face = face(s, t);
    let target = target(s, t);
    let marked = s.kernel.slots.get(&crate::session_menu::MENUS).on.as_ref() == Some(&target);
    // No server badge or dot on the tab itself: only its preview names the server
    // (`tab-nav.tsx:158-164`).
    let id = uid.clone();
    let mark = status(s, t);
    let muted = !draft && s.project_navigation() && crate::session_menu::preferences::muted(&t.server, &t.id) && mark != Some(TabItemStatus::Busy);
    let mut item = TabItem::new(uid, face.title).status(if muted { None } else { mark }).marked(marked);
    let active_tab = s.current_tab().as_deref() == Some(id.as_str()) && !s.settings_open();
    item = match (draft, hide_avatar) {
        (true, _) => match shortcut {
            Some(n) => item.avatar(draft_avatar(p, active_tab, n, show_hints)),
            None => item.icon(&DRAFT_PENCIL),
        },
        // Its slot stays, empty (`[data-avatar="hidden"]`'s `visibility: hidden`, `tab-nav.css:466`).
        (false, true) => item.avatar(el().size(16.0, 16.0)),
        (false, false) => item.avatar(crate::icons::tab_badge(s, p, face.project, &project_or_dir(face.project, face.dir), shortcut, show_hints)),
    };
    if let Some(field) = crate::session_menu_view::field(s, p, &target, UI_MED) {
        item = item.editor(field);
    }
    let switching = s.switching_layout();
    let spring = s.switch_spring();
    // The routed session's, Settings open over it or not: Interface layout switches there.
    let active = !draft && matches!(&*s.route, Route::Session { id: r, server } if *server == t.server && s.root_of(r) == t.id);
    let bell = muted.then(|| super::sidebar::pins::muted_mark(p));
    item.decorate(move |slot, _| {
        let slot = if let Some(bell_el) = bell { super::sidebar::pins::marked_slot(slot, bell_el, mark) } else { slot };
        // The arrows open the tab they reach as they pass it; its menu on a right-click.
        let slot = slot.on_key(move |s: &mut App, ev, _: &mut Tx| s.prefetch_navigation(&nav_ids, index, ev));
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

/// A draft tab's 16×16 leading face (`DraftTabItem` in `tab-nav.tsx`, #63089): the pencil icon
/// slides up while the outline shortcut badge rises in when Mod is held.
pub(crate) fn draft_avatar(p: &Pal, active: bool, shortcut: usize, show_hints: bool) -> El {
    let anim = rune::Anim::new(Curve::tween(0.16, Easing::EASE_OUT));
    let font = TextStyle::new(11.0, 530.0).lh(11.0).tracking(0.05).features("\"tnum\"");
    let pencil = icon(&DRAFT_PENCIL, 16.0)
        .key("tab-icon-face")
        .color(if active { p.icon } else { p.icon_muted })
        .opacity(if show_hints { 0.0 } else { 1.0 })
        .offset(0.0, if show_hints { -6.0 } else { 0.0 })
        .opacity_motion(anim)
        .offset_motion(anim);
    let badge = el()
        .key("tab-shortcut-face")
        .absolute(0.0, 0.0)
        .size(16.0, 16.0)
        .radius(4.0)
        .fill(p.bg_base)
        .box_shadow(Pal::ring_in(p.q.border_strong))
        .center()
        .opacity(if show_hints { 1.0 } else { 0.0 })
        .opacity_motion(anim)
        .a11y_hidden()
        .child(
            text(shortcut.to_string())
                .font(font)
                .color(p.muted)
                .offset(0.0, if show_hints { 0.0 } else { 6.0 })
                .offset_motion(anim),
        );
    el().key("draft-avatar").size(16.0, 16.0).clip_soft(0.0).center().child(pencil).child(badge)
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

/// What a reader hears of a tab beyond its title, in the app's language: its close button and
/// the status its mark shows (the Inbox's words for the same states).
pub(crate) fn tab_words() -> rune_kit::shell::TabWords {
    use opencode_i18n::t;
    rune_kit::shell::TabWords {
        close: t!("common.closeTab"),
        busy: t!("sidebar.inbox.status.working"),
        attention: t!("sidebar.inbox.section.needsAttention"),
        unread: t!("sidebar.inbox.section.unread"),
        failed: t!("sidebar.inbox.status.failed"),
    }
}

/// Desktop's tab mark (`tab-nav.tsx`'s `status`): busy while the family runs, then nothing on
/// the open tab, then attention, then the unread dot, red when the run failed.
pub(crate) fn status(s: &App, t: &crate::tabs::Tab) -> Option<TabItemStatus> {
    let mark = if t.is_draft() { s.tab_status(&t.id) } else { s.tab_status_of(t) };
    Some(match mark? {
        TabStatus::Busy => TabItemStatus::Busy,
        TabStatus::Attention => TabItemStatus::Attention,
        TabStatus::Unread => TabItemStatus::Unread,
        TabStatus::Failed => TabItemStatus::Failed,
    })
}

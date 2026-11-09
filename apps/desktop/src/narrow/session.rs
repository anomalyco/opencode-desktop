//! A session in a narrow window (`session/screen.tsx` below 768 px, `runtime/plugin/mobile.tsx`):
//! the Session / Changes / More switcher across the panel's top, the session's location on its
//! own bar under it (the timeline's header hides), then the conversation, or the view a tab or
//! the More drawer picked.

use rune::{El, TextStyle, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use super::machine::{CONVERSATION, MoreDrawer, MoreEvent};
use crate::palette::{Elevate as _, Pal};
use crate::panels::{PanelTabDesc, PanelViewCtx, SessionPane};
use crate::App;
use opencode_i18n::t;

/// `tabs-v2-trigger`: 13 / 16 at 440, -0.04 tracking.
const TAB: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
/// The More menu's rows (`text-[13px] font-[440] leading-[var(--line-height-base)]`).
const MENU: TextStyle = TextStyle::new(13.0, 440.0).lh(24.0);

pub const MORE_SHEET: &str = "narrow-more-sheet";

/// A narrow-screen view of a panel (`Panel.mobile`): its plugin, title, order, kind and icon.
pub struct MobileView {
    pub plugin: &'static str,
    pub title: fn() -> &'static str,
    pub order: i32,
    pub kind: Kind,
    pub icon: &'static rune::Icon,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A tab of its own beside Session.
    Tab,
    /// A row of the More drawer that switches the view.
    Menu,
    /// A row of the More drawer that opens inside it.
    Drawer,
}

/// The app's panels' narrow views (`review`, `file` and `terminal` renderers' `mobile`). Plugins'
/// panels declare theirs in the plugin API, which doesn't carry it yet.
pub const VIEWS: &[MobileView] = &[
    MobileView { plugin: "review", title: || t!("plugin.review.mobile.title.other"), order: 10, kind: Kind::Tab, icon: &ic::REVIEW },
    MobileView { plugin: "file", title: || t!("plugin.file.mobile.title"), order: 20, kind: Kind::Menu, icon: &ic::FOLDER },
    MobileView { plugin: "terminal", title: || t!("plugin.terminal.tab.title"), order: 30, kind: Kind::Menu, icon: &crate::terminal::grid::TERMINAL_ICON },
];

fn find(view: &str) -> Option<&'static MobileView> {
    VIEWS.iter().find(|v| v.plugin == view)
}

/// The page: the switcher, then the conversation (`conversation`, built by the caller) under
/// the location bar, or the picked view.
pub fn page(s: &App, p: &Pal, id: &str, conversation: impl FnOnce() -> El) -> El {
    let shown = s.narrow_view();
    let page = col().key("session").grow(1.0).w_full().min_h(0.0).child(tabs(s, p, id, shown));
    if shown == CONVERSATION {
        return page.child(location_bar(s, p, id)).child(conversation());
    }
    page.child(view(s, p, id, shown))
}

/// `session-mobile-view-navigation`: 36 tall, a 1 px `border-base` rule along its foot, each
/// view an equal share (`flex-1`); the picked one in `text-base` over a 1 px `text-faint` rule,
/// the others `text-muted` until hovered.
fn tabs(s: &App, p: &Pal, id: &str, shown: &str) -> El {
    let more_open = s.layout.narrow.more.is_open();
    let picked = if shown == CONVERSATION || VIEWS.iter().any(|v| v.kind == Kind::Tab && v.plugin == shown) { shown } else { "more" };
    let mut bar = row()
        .key("narrow-view-tabs")
        .role("tablist")
        .focus_group()
        .label(t!("session.view.select"))
        .w_full()
        .h(36.0)
        .min_h(36.0)
        .child(el().key("rule").pin(Some(0.0), None, Some(0.0), Some(0.0)).h(1.0).fill(p.border))
        .child(tab(p, "narrow-view-session", t!("session.tab.session"), picked == CONVERSATION, {
            let id = id.to_owned();
            move |s: &mut App, tx: &mut Tx| s.select_narrow_view(&id, CONVERSATION, tx)
        }));
    for v in VIEWS.iter().filter(|v| v.kind == Kind::Tab) {
        let id = id.to_owned();
        bar = bar.child(tab(p, &format!("narrow-view-{}", v.plugin), (v.title)(), picked == v.plugin, move |s: &mut App, tx: &mut Tx| enter(s, &id, v.plugin, tx)));
    }
    if VIEWS.iter().any(|v| v.kind != Kind::Tab) {
        bar = bar.child(
            tab(p, "narrow-view-more", t!("session.tab.more"), picked == "more", |s: &mut App, tx: &mut Tx| s.more_drawer(MoreEvent::Open, tx))
                .expanded(more_open),
        );
    }
    bar
}

fn tab(p: &Pal, key: &str, label: &str, selected: bool, press: impl Fn(&mut App, &mut Tx) + 'static) -> El {
    row()
        .key(key.to_owned())
        .role("tab")
        .label(label.to_owned())
        .focusable(true)
        .focus_selected(selected)
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .h_full()
        .padding(6.0)
        .center()
        .hover_group()
        .on_press_tx(press)
        .child(text(label.to_owned()).font(TAB).color(if selected { p.text } else { p.muted }).group_color(p.text).truncate())
        .child(el().key("rule").pin(Some(0.0), None, Some(0.0), Some(0.0)).h(1.0).fill(if selected { p.faint } else { rune::Color::TRANSPARENT }))
}

/// Shows `view` for session `id`, as its tab or More row does, readying what it reads.
fn enter(s: &mut App, id: &str, view: &str, tx: &mut Tx) {
    if view == "review" {
        s.ensure_session_review(id);
    }
    s.select_narrow_view(id, view, tx);
}

/// `session-location-bar`: 36 tall, `px-2`, a 1 px `border-base` rule along its foot, the
/// session's location (compact for a subagent's).
fn location_bar(s: &App, p: &Pal, id: &str) -> El {
    let info = s.session(id);
    let project = info.and_then(|i| s.project(&i.project));
    let name = info.map(|i| crate::view::project_or_dir(project, &i.directory)).unwrap_or_default();
    let worktree = info.filter(|i| i.parent.is_none()).and_then(|i| crate::view::location::worktree_name(project, &i.directory));
    let collapse = if info.is_some_and(|i| i.parent.is_some()) { 2 } else { 0 };
    row()
        .key("narrow-location")
        .w_full()
        .h(36.0)
        .min_h(36.0)
        .px(8.0)
        .center_y()
        .child(el().key("rule").pin(Some(0.0), None, Some(0.0), Some(0.0)).h(1.0).fill(p.border))
        .children(info.map(|_| crate::view::location::trigger(s, p, project, &name, worktree.as_deref(), collapse)))
}

/// The picked panel's narrow view, filling the rest of the panel.
fn view(s: &App, p: &Pal, id: &str, shown: &str) -> El {
    let body = match shown {
        "review" => crate::review::mobile::view(s, p, id),
        plugin => panel(s, p, id, plugin),
    };
    col().key(format!("narrow-view-{shown}-body")).w_full().grow(1.0).min_h(0.0).child(body)
}

/// A panel's own render for the tab it shows: its selected one, else its first.
fn panel(s: &App, p: &Pal, id: &str, plugin: &str) -> El {
    let Some(reg) = s.resolve_side_region() else { return el() };
    let stored = s.side.sessions.get(id).map(|st| &st.tab_state.tabs);
    let active = stored.and_then(|t| t.active.clone()).filter(|k| k.starts_with(&format!("{plugin}:")));
    let entry = active.as_deref().and_then(|k| reg.entry(k)).or_else(|| reg.entries.iter().find(|e| e.plugin == plugin && !e.tab.hidden));
    let fallback;
    let (provider, tab) = match entry {
        Some(e) => (e.provider, &e.tab),
        None => {
            let Some(provider) = crate::panels::find_panel(plugin) else { return el() };
            fallback = PanelTabDesc::new(if plugin == "file" { crate::files::data::OPEN_TAB_ID } else { "" }, "");
            (*provider, &fallback)
        }
    };
    let w = s.layout.narrow.width() - 2.0;
    let ctx = PanelViewCtx {
        session_id: &reg.session_id,
        directory: &reg.directory,
        project_id: &reg.project_id,
        pane: SessionPane::Side,
        pane_size: (w, 600.0),
        env_scale: 1.0,
        tab,
        visible: true,
        focused: s.side.focused,
    };
    (provider.render)(s, p, &ctx)
}

/// The More drawer's contents (`session-mobile-view-options`): a raised card of rows, 44 tall,
/// `ps-3 pe-2 gap-2`, each view's icon in icon-muted, a check by the one showing; a drawer
/// kind's row ends in a chevron and opens inside.
pub fn more(s: &App, p: &Pal) -> El {
    // Shut, it keeps its menu while it slides away.
    if let MoreDrawer::Panel { .. } = s.layout.narrow.more {
        return el();
    }
    let shown = s.narrow_view().to_owned();
    let mut card = col()
        .key("narrow-more-options")
        .w_full()
        .gap(2.0)
        .padding(2.0)
        .radius(6.0)
        .fill(if p.dark { p.layer1 } else { p.bg_base })
        .elevate(p.raised);
    let mut rows: Vec<&MobileView> = VIEWS.iter().filter(|v| v.kind != Kind::Tab).collect();
    rows.sort_by_key(|v| (v.kind == Kind::Drawer, v.order));
    for v in rows {
        let current = shown == v.plugin;
        let plugin = v.plugin;
        let mut r = row()
            .key(format!("narrow-more-{plugin}"))
            .role("button")
            .label((v.title)())
            .w_full()
            .min_h(44.0)
            .py(4.0)
            .pl(12.0)
            .pr(8.0)
            .gap(8.0)
            .center_y()
            .radius(4.0)
            .hover(p.hover)
            .hover_motion(crate::view::INSTANT, crate::view::INSTANT)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                let Some(id) = s.active_session_id().map(str::to_owned) else { return };
                if find(plugin).is_some_and(|v| v.kind == Kind::Drawer) {
                    s.more_drawer(MoreEvent::Drill { view: plugin.to_owned() }, tx);
                    return;
                }
                enter(s, &id, plugin, tx);
                s.more_drawer(MoreEvent::Close, tx);
            })
            .child(icon(v.icon, 16.0).color(p.icon_muted))
            .child(text((v.title)()).font(MENU).color(p.text).truncate().grow(1.0).shrink().min_w(0.0));
        if current {
            r = r.fill(p.pressed).child(icon(&ic::CHECK, 16.0).color(p.icon_muted).ml(4.0));
        }
        if v.kind == Kind::Drawer {
            r = r.child(icon(&ic::CHEVRON_RIGHT, 16.0).color(p.icon_muted).ml(4.0));
        }
        card = card.child(r);
    }
    card
}

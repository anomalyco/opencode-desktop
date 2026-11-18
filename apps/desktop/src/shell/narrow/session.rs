//! A session in a narrow window (`session/screen.tsx` below 768 px, `runtime/plugin/mobile.tsx`):
//! the Session / Changes / More switcher across the panel's top, the session's location on its
//! own bar under it (the timeline's header hides), then the conversation, or the view a tab or
//! the More drawer picked.

use rune::{El, TextStyle, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use super::machine::{CONVERSATION, MoreEvent};
use crate::palette::{Elevate as _, Pal};
use crate::panels::{PanelTabDesc, PanelViewCtx, SessionPane};
use crate::App;
use opencode_i18n::t;

/// `tabs-v2-trigger`: 13 / 16 at 440, -0.04 tracking.
const TAB: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
/// The More menu's rows (`text-[13px] font-[440] leading-[var(--line-height-base)]`).
const MENU: TextStyle = TextStyle::new(13.0, 440.0).lh(24.0);

pub const MORE_SHEET: &str = "narrow-more-sheet";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A tab of its own beside Session.
    Tab,
    /// A row of the More drawer that switches the view.
    Menu,
    /// A row of the More drawer that opens inside it.
    Drawer,
}

/// A view the narrow switcher offers: a plugin panel's (`PanelSpec.mobile`), `id` its tab's
/// key (`review:changes`, `terminal:main`), as Desktop keys them by panel (`createMobileViews`).
pub struct NarrowView {
    pub id: String,
    pub title: String,
    pub order: f64,
    pub kind: Kind,
    pub icon: &'static rune::Icon,
}

/// Every narrow view the window offers, in ascending order.
pub fn views(s: &App) -> Vec<NarrowView> {
    use opencode_plugin_spec::PanelMobileKind;
    let mut out: Vec<NarrowView> = crate::plugin_host::panels::narrow_views(s).into_iter().map(|(id, v)| NarrowView {
        id,
        title: v.title,
        order: v.order,
        kind: match v.kind {
            PanelMobileKind::Tab => Kind::Tab,
            PanelMobileKind::Drawer => Kind::Drawer,
            _ => Kind::Menu,
        },
        icon: crate::plugin_host::icons::resolve("", v.icon.as_deref().unwrap_or("window-analytics")),
    }).collect();
    out.sort_by(|a, b| a.order.total_cmp(&b.order));
    out
}

/// The narrow view tab `key` (`<plugin>:<tab>`) opens: its plugin panel's, else its app panel's.
pub fn view_for_tab(s: &App, key: &str) -> Option<String> {
    let plugin = crate::panels::split_panel_key(key).map(|(plugin, _)| plugin)?;
    let views = views(s);
    // A Luau panel's own tab shows its view; an instance tab (`<plugin>:<instance>`) its plugin's.
    let own = views.iter().find(|v| v.id == key);
    let same = || views.iter().find(|v| v.id == plugin || crate::panels::split_panel_key(&v.id).is_some_and(|(p, _)| p == plugin));
    own.or_else(same).map(|v| v.id.clone())
}

/// The page: the switcher, then the conversation (`conversation`, built by the caller) under
/// the location bar, or the picked view.
pub fn page(s: &App, p: &Pal, id: &str, conversation: impl FnOnce() -> El) -> El {
    let shown = s.narrow_view();
    // Settings › Bottom navigation puts the switcher under everything, its rule along its top.
    let bottom = crate::view::chrome().bottom;
    let switcher = tabs(s, p, id, shown, bottom);
    let (above, below) = if bottom { (None, Some(switcher)) } else { (Some(switcher), None) };
    let page = col().key("session").grow(1.0).w_full().min_h(0.0).children(above);
    let page = if shown == CONVERSATION { page.child(location_bar(s, p, id)).child(conversation()) } else { page.child(view(s, p, id, shown)) };
    page.children(below)
}

/// `session-mobile-view-navigation`: 36 tall, a 1 px `border-base` rule along its foot, each
/// view an equal share (`flex-1`); the picked one in `text-base` over a 1 px `text-faint` rule,
/// the others `text-muted` until hovered.
fn tabs(s: &App, p: &Pal, id: &str, shown: &str, bottom: bool) -> El {
    let more_open = s.layout.narrow.more.is_open();
    let views = views(s);
    let picked = if shown == CONVERSATION || views.iter().any(|v| v.kind == Kind::Tab && v.id == shown) { shown } else { "more" };
    let mut bar = row()
        .key("narrow-view-tabs")
        .role("tablist")
        .focus_group()
        .label_static(t!("session.view.select"))
        .w_full()
        .h(36.0)
        .min_h(36.0)
        .child(el().key("rule").pin(Some(0.0), if bottom { Some(0.0) } else { None }, Some(0.0), if bottom { None } else { Some(0.0) }).h(1.0).fill(p.border))
        .child(tab(p, "narrow-view-session", t!("session.tab.session"), picked == CONVERSATION, {
            let id = id.to_owned();
            move |s: &mut App, tx: &mut Tx| s.select_narrow_view(&id, CONVERSATION, tx)
        }));
    for v in views.iter().filter(|v| v.kind == Kind::Tab) {
        let (id, view) = (id.to_owned(), v.id.clone());
        bar = bar.child(tab(p, &format!("narrow-view-{}", v.id), &v.title, picked == v.id, move |s: &mut App, tx: &mut Tx| enter(s, &id, &view, tx)));
    }
    if views.iter().any(|v| v.kind != Kind::Tab) {
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
        // `border-bottom: 1px` (`text-faint` when picked) takes its device pixel of the height.
        .pb(6.0 + crate::view::chrome().hairline)
        .center()
        .hover_group()
        .on_press_tx(press)
        .border_sides([0.0, 0.0, 1.0, 0.0], if selected { p.faint } else { rune::Color::TRANSPARENT })
        .child(text(label.to_owned()).font(TAB).color(if selected { p.text } else { p.muted }).group_color(p.text).truncate())
}

/// Shows `view` for session `id`, as its tab or More row does, readying what it reads.
fn enter(s: &mut App, id: &str, view: &str, tx: &mut Tx) {
    s.select_narrow_view(id, view, tx);
    match view {
        // The narrow Terminal view starts the first terminal of a session that has none.
        "terminal:main" => {
            let wk = s.terminal_workspace_key_for_session(id);
            s.ensure_terminal_workspace_hydrated(&wk);
            let empty = s.terminal.workspaces.get(&wk).is_none_or(|w| w.tabs.is_empty() && w.failure.is_none());
            if empty && s.live_terminals().is_some_and(|(_, observer)| !observer) {
                s.create_terminal_tab(None, tx);
            }
        }
        _ => {}
    }
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
        .pb(crate::view::chrome().hairline)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], p.border)
        .children(info.map(|_| crate::view::location::trigger(s, p, project, &name, worktree.as_deref(), collapse)))
}


/// The picked panel's narrow view, filling the rest of the panel.
fn view(s: &App, p: &Pal, id: &str, shown: &str) -> El {
    let body = match shown {
        plugin => panel(s, p, id, plugin),
    };
    col().key(format!("narrow-view-{shown}-body")).w_full().grow(1.0).min_h(0.0).child(body)
}

/// A panel's own render as a narrow view: a plugin panel's view (`plugin:<plugin>.<panel>`)
/// draws its panel's tab; an app panel's, its selected tab, else its first.
fn panel(s: &App, p: &Pal, id: &str, view: &str) -> El {
    let Some(reg) = s.resolve_side_region() else { return el() };
    let fallback;
    let (provider, tab) = match plugin_tab(s, view) {
        Some((provider, tab)) => {
            fallback = tab;
            (provider, &fallback)
        }
        None => {
            let stored = s.side.sessions.get(id).map(|st| &st.tab_state.tabs);
            let active = stored.and_then(|t| t.active.clone()).filter(|k| k.starts_with(&format!("{view}:")));
            match active.as_deref().and_then(|k| reg.entry(k)).or_else(|| reg.entries.iter().find(|e| e.plugin == view && !e.tab.hidden)) {
                Some(e) => (e.provider, &e.tab),
                None => {
                    let Some(provider) = s.find_panel(view) else { return el() };
                    fallback = PanelTabDesc::new("", "");
                    (provider, &fallback)
                }
            }
        }
    };
    let w = s.layout.narrow.width() - 2.0;
    let ctx = PanelViewCtx {
        plugin: provider.plugin,
        session_id: &reg.session_id,
        directory: &reg.directory,
        project_id: &reg.project_id,
        pane: SessionPane::Side,
        pane_size: (w, 600.0),
        env_scale: 1.0,
        tab,
        visible: true,
        focused: s.side.focused,
        mobile: true,
    };
    provider.render(s, p, &ctx)
}

/// A Luau plugin panel's narrow view (`<plugin>:<panel>`): its plugin's provider, and the tab,
/// titled as the view (Desktop hands the render a tab with the panel's id and the view's title).
fn plugin_tab(s: &App, view: &str) -> Option<(crate::panels::PanelRegistration, PanelTabDesc)> {
    let (plugin, tab) = crate::panels::split_panel_key(view)?;
    let v = views(s).into_iter().find(|v| v.id == view)?;
    let provider = s.find_panel(plugin)?;
    Some((provider, PanelTabDesc::new(tab, v.title)))
}

/// The More drawer's contents (`session-mobile-view-options`): a raised card of rows, 44 tall,
/// `ps-3 pe-2 gap-2`, each view's icon in icon-muted, a check by the one showing; a drawer
/// kind's row ends in a chevron and opens inside, where its panel draws instead of the card.
pub fn more(s: &App, p: &Pal) -> El {
    let all = views(s);
    if let Some(view) = s.layout.narrow.more.drawn_view() {
        let Some(id) = s.active_session_id() else { return el() };
        // A nested view the panel opened (`mobile-panel-drawer.tsx`): a ghost Back button named
        // after the panel's view (`mb-2 !justify-start`), then what the panel draws for it.
        let back = s.layout.narrow.more.nested().map(|_| {
            let title = all.iter().find(|v| v.id == view).map(|v| v.title.clone()).unwrap_or_default();
            let button = crate::widgets::button(title, rune_kit::ButtonVariant::Ghost).icon(crate::plugin_host::icons::resolve("", "arrow-left")).build(crate::palette::kit(p));
            row().key("narrow-more-back").w_full().mb(8.0).child(button.key("narrow-more-back-button").label_static(t!("common.goBack")).on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.more_drawer(MoreEvent::Back, tx);
            }))
        });
        let body = col().key(format!("narrow-more-{view}-body")).w_full().min_h(0.0);
        // The drawer's hidden heading names the nested view (`MobileDrawerLabel`).
        let body = match s.layout.narrow.more.nested() {
            Some(n) => body.role("group").label(n.title.clone()),
            None => body,
        };
        return body.children(back).child(panel(s, p, id, view));
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
    // The menu kind's rows, then the drawer kind's, each in order (`MobileViewTabs`).
    let rows = all.iter().filter(|v| v.kind == Kind::Menu).chain(all.iter().filter(|v| v.kind == Kind::Drawer));
    for v in rows {
        let current = shown == v.id;
        let (view, drawer) = (v.id.clone(), v.kind == Kind::Drawer);
        let mut r = row()
            .key(format!("narrow-more-{}", v.id))
            .role("button")
            .label(v.title.clone())
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
                if drawer {
                    s.more_drawer(MoreEvent::Drill { view: view.clone() }, tx);
                    return;
                }
                enter(s, &id, &view, tx);
                s.more_drawer(MoreEvent::Close, tx);
            })
            .child(icon(v.icon, 16.0).color(p.icon_muted))
            .child(text(v.title.clone()).font(MENU).color(p.text).truncate().grow(1.0).shrink().min_w(0.0));
        if current && !drawer {
            r = r.fill(p.pressed).child(icon(&ic::CHECK, 16.0).color(p.icon_muted).ml(4.0));
        }
        if drawer {
            r = r.child(icon(&ic::CHEVRON_RIGHT, 16.0).color(p.icon_muted).ml(4.0));
        }
        card = card.child(r);
    }
    card
}
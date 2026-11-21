//! A session in a narrow window (`session/screen.tsx` below 768 px, `runtime/plugin/mobile.tsx`):
//! the Session / Changes / More switcher across the panel's top, the session's location on its
//! own bar under it (the timeline's header hides), then the conversation, or the view a tab or
//! the More drawer picked.

use opencode_i18n::t;
use rune::{El, TextStyle, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;
use opencode_ui::palette::{Elevate as _, Pal, kit};
use opencode_ui::style::INSTANT;

use crate::host::{NarrowChrome, NarrowHost};
use crate::machine::{CONVERSATION, MoreEvent};

/// `tabs-v2-trigger`: 13 / 16 at 440, -0.04 tracking.
const TAB: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
/// The More menu's rows (`text-[13px] font-[440] leading-[var(--line-height-base)]`).
const MENU: TextStyle = TextStyle::new(13.0, 440.0).lh(24.0);

pub const MORE_SHEET: &str = "narrow-more-sheet";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug)]
pub struct NarrowView {
    pub id: String,
    pub title: String,
    pub order: f64,
    pub kind: Kind,
    pub icon: &'static rune::Icon,
}

/// Every narrow view the window offers, in ascending order.
pub fn views(host: &dyn NarrowHost) -> Vec<NarrowView> {
    let mut out = host.narrow_views();
    out.sort_by(|a, b| a.order.total_cmp(&b.order));
    out
}

/// The narrow view tab `key` (`<plugin>:<tab>`) opens: its plugin panel's, else its app panel's.
pub fn view_for_tab(host: &dyn NarrowHost, key: &str) -> Option<String> {
    let (plugin, _) = key.split_once(':')?;
    let views = views(host);
    // A Luau panel's own tab shows its view; an instance tab (`<plugin>:<instance>`) its plugin's.
    let own = views.iter().find(|v| v.id == key);
    let same = || views.iter().find(|v| v.id == plugin || v.id.split_once(':').is_some_and(|(p, _)| p == plugin));
    own.or_else(same).map(|v| v.id.clone())
}

/// Shows `view` for session `id`, as its tab or More row does, readying what it reads.
pub fn enter(host: &mut dyn NarrowHost, id: &str, view: &str, tx: &mut Tx) {
    crate::select_narrow_view(host, id, view, tx);
    host.on_enter_narrow_view(id, view, tx);
}

fn more_back(host: &mut dyn NarrowHost, tx: &mut Tx) {
    tx.instant();
    crate::more_drawer(host, MoreEvent::Back, tx);
}

fn pick_more_row(host: &mut dyn NarrowHost, view: &str, drawer: bool, tx: &mut Tx) {
    let Some(id) = host.active_session_id().map(str::to_owned) else {
        return;
    };
    if drawer {
        crate::more_drawer(host, MoreEvent::Drill { view: view.to_owned() }, tx);
        return;
    }
    enter(host, &id, view, tx);
    crate::more_drawer(host, MoreEvent::Close, tx);
}

/// The page: the switcher, then the conversation (`conversation`, built by the caller) under
/// the location bar, or the picked view.
pub fn page<H: NarrowHost + 'static>(s: &H, p: &Pal, id: &str, conversation: impl FnOnce() -> El) -> El {
    let shown = crate::narrow_view(s);
    // Settings › Bottom navigation puts the switcher under everything, its rule along its top.
    let chrome = s.chrome();
    let all = views(s);
    let switcher = tabs(s, p, id, shown, chrome, &all);
    let (above, below) = if chrome.bottom { (None, Some(switcher)) } else { (Some(switcher), None) };
    let page = col().key("session").grow(1.0).w_full().min_h(0.0).children(above);
    let page = if shown == CONVERSATION {
        page.child(location_bar(s, p, id, chrome.hairline)).child(conversation())
    } else {
        page.child(view(s, p, id, shown, &all))
    };
    page.children(below)
}

/// `session-mobile-view-navigation`: 36 tall, a 1 px `border-base` rule along its foot, each
/// view an equal share (`flex-1`); the picked one in `text-base` over a 1 px `text-faint` rule,
/// the others `text-muted` until hovered.
fn tabs<H: NarrowHost + 'static>(s: &H, p: &Pal, id: &str, shown: &str, chrome: NarrowChrome, all: &[NarrowView]) -> El {
    let more_open = s.narrow_state().more.is_open();
    let picked = if shown == CONVERSATION || all.iter().any(|v| v.kind == Kind::Tab && v.id == shown) { shown } else { "more" };
    let mut bar = row()
        .key("narrow-view-tabs")
        .role("tablist")
        .focus_group()
        .label_static(t!("session.view.select"))
        .w_full()
        .h(36.0)
        .min_h(36.0)
        .child(
            el().key("rule")
                .pin(Some(0.0), if chrome.bottom { Some(0.0) } else { None }, Some(0.0), if chrome.bottom { None } else { Some(0.0) })
                .h(1.0)
                .fill(p.border),
        )
        .child(tab::<H>(p, "narrow-view-session", t!("session.tab.session"), picked == CONVERSATION, chrome.hairline, {
            let id = id.to_owned();
            move |s: &mut H, tx: &mut Tx| crate::select_narrow_view(s, &id, CONVERSATION, tx)
        }));
    for v in all.iter().filter(|v| v.kind == Kind::Tab) {
        let (id, view) = (id.to_owned(), v.id.clone());
        bar = bar.child(tab::<H>(p, &format!("narrow-view-{}", v.id), &v.title, picked == v.id, chrome.hairline, move |s: &mut H, tx: &mut Tx| {
            enter(s, &id, &view, tx)
        }));
    }
    if all.iter().any(|v| v.kind != Kind::Tab) {
        bar = bar.child(
            tab::<H>(p, "narrow-view-more", t!("session.tab.more"), picked == "more", chrome.hairline, |s: &mut H, tx: &mut Tx| {
                crate::more_drawer(s, MoreEvent::Open, tx)
            })
            .expanded(more_open),
        );
    }
    bar
}

fn tab<H: NarrowHost + 'static>(p: &Pal, key: &str, label: &str, selected: bool, hairline: f32, press: impl Fn(&mut H, &mut Tx) + 'static) -> El {
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
        .pb(6.0 + hairline)
        .center()
        .hover_group()
        .on_press_tx(press)
        .border_sides([0.0, 0.0, 1.0, 0.0], if selected { p.faint } else { rune::Color::TRANSPARENT })
        .child(text(label.to_owned()).font(TAB).color(if selected { p.text } else { p.muted }).group_color(p.text).truncate())
}

/// `session-location-bar`: 36 tall, `px-2`, a 1 px `border-base` rule along its foot, the
/// session's location (compact for a subagent's).
fn location_bar(host: &dyn NarrowHost, p: &Pal, id: &str, hairline: f32) -> El {
    row()
        .key("narrow-location")
        .w_full()
        .h(36.0)
        .min_h(36.0)
        .px(8.0)
        .pb(hairline)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], p.border)
        .children(host.location_trigger(p, id))
}

/// The picked panel's narrow view, filling the rest of the panel.
fn view(host: &dyn NarrowHost, p: &Pal, id: &str, shown: &str, all: &[NarrowView]) -> El {
    col().key(format!("narrow-view-{shown}-body")).w_full().grow(1.0).min_h(0.0).child(panel(host, p, id, shown, all))
}

/// A panel's own render as a narrow view: delegates to [`NarrowHost::render_narrow_panel`]
/// with the matched narrow view's title and shell width.
fn panel(host: &dyn NarrowHost, p: &Pal, id: &str, view: &str, all: &[NarrowView]) -> El {
    let title = all.iter().find(|v| v.id == view).map(|v| v.title.as_str());
    let width = host.narrow_state().width() - 2.0;
    host.render_narrow_panel(p, id, view, title, width)
}

/// The More drawer's contents (`session-mobile-view-options`): a raised card of rows, 44 tall,
/// `ps-3 pe-2 gap-2`, each view's icon in icon-muted, a check by the one showing; a drawer
/// kind's row ends in a chevron and opens inside, where its panel draws instead of the card.
pub fn more<H: NarrowHost + 'static>(s: &H, p: &Pal) -> El {
    let all = views(s);
    if let Some(view) = s.narrow_state().more.drawn_view() {
        let Some(id) = s.active_session_id() else {
            return el();
        };
        // A nested view the panel opened (`mobile-panel-drawer.tsx`): a ghost Back button named
        // after the panel's view (`mb-2 !justify-start`), then what the panel draws for it.
        let back = s.narrow_state().more.nested().map(|_| {
            let title = all.iter().find(|v| v.id == view).map(|v| v.title.clone()).unwrap_or_default();
            let button = opencode_ui::widgets::button(title, rune_kit::ButtonVariant::Ghost).icon(&ic::ARROW_LEFT).build(kit(p));
            row().key("narrow-more-back").w_full().mb(8.0).child(
                button
                    .key("narrow-more-back-button")
                    .label_static(t!("common.goBack"))
                    .on_press_tx(|s: &mut H, tx: &mut Tx| more_back(s, tx)),
            )
        });
        let body = col().key(format!("narrow-more-{view}-body")).w_full().min_h(0.0);
        // The drawer's hidden heading names the nested view (`MobileDrawerLabel`).
        let body = match s.narrow_state().more.nested() {
            Some(n) => body.role("group").label(n.title.clone()),
            None => body,
        };
        return body.children(back).child(panel(s, p, id, view, &all));
    }
    let shown = crate::narrow_view(s).to_owned();
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
            .cursor(rune::Cursor::Pointer)
            .hover(if current && !drawer { rune::Color::TRANSPARENT } else { p.hover })
            .hover_motion(INSTANT, INSTANT)
            .on_press_tx(move |s: &mut H, tx: &mut Tx| pick_more_row(s, &view, drawer, tx))
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

//! The titlebar row (`titlebar.tsx`): Desktop's `titlebar-v2` header across the window's top.
//! Top tabs put the app menu (off macOS), Home, Search sessions, the tab strip and `+` in it;
//! side tabs and the inbox, on Windows, only the app menu and the sidebar toggle. Its gaps, the
//! bands above and below the controls and the stretch after `+` move the window, double-click
//! to maximize it and, on Windows, right-click for the window menu.

use rune_kit::shell::{TabOrientation, WindowTitlebar};

use super::*;
use crate::layout_state::TabLayout;
use opencode_i18n::t;

/// The tab strip's key, whose width at the last layout the tabs share.
const STRIP: &str = "tab-strip";

/// The width Desktop keeps for the traffic lights before top tabs (`macTrafficLightsBaseWidth`).
const LIGHTS_W: f32 = 74.0;

/// Matched-flight ids for the controls that move between the titlebar and the sidebar.
pub(crate) const NAV_HOME: &str = "shell-nav-home";
pub(crate) const NAV_SEARCH: &str = "shell-nav-search";
pub(crate) const NAV_NEW: &str = "shell-nav-new";

/// The row, where this layout and platform keep one.
pub(super) fn titlebar(s: &App, p: &Pal, cx: &mut Cx<App>) -> Option<El> {
    let chrome = chrome();
    if chrome.row_h == 0.0 {
        return None;
    }
    let insets = cx.chrome_insets();
    let top = chrome.layout == TabLayout::Horizontal;
    let mut bar = WindowTitlebar::from_insets(insets, s.zoom)
        .key("titlebar")
        .height(chrome.row_h)
        .top_padding(chrome.row_pad)
        .leading_padding(leading_pad(top, insets.leading, s.zoom))
        .fill(p.bg_deep);
    if !cfg!(target_os = "macos") {
        bar = bar.leading(crate::app_menu::button(s, p).motion(GLIDE));
    }
    // A narrow window's row holds one trigger for the tabs' drawer, `px-2` (`titlebar.tsx:581`),
    // and the titlebar items' row 6 after it, empty or not.
    if chrome.narrow {
        let built = bar.trailing_padding(8.0 + 6.0).tabs(crate::narrow::tabs::trigger(s, p));
        // Along the foot, its controls sit in the row's top 36, 8 clear of the window's edge.
        if chrome.bottom {
            let row = built.height(chrome.row_h - 8.0).top_padding(0.0).build(crate::palette::kit(p));
            return Some(col().key("titlebar-foot").w_full().h(chrome.row_h).child(row));
        }
        return Some(built.build(crate::palette::kit(p)));
    }
    if !top {
        return Some(bar.leading(toggle_button(s, p, s.sidebar_open)).build(crate::palette::kit(p)));
    }
    let nav = row().key("titlebar-nav").gap(2.0).center_y().child(home_button(s, p)).child(search_button(p));
    let viewport = cx.layout_size(STRIP).map(|[w, _]| w);
    let strip = tab_strip(s, p, STRIP, TabOrientation::Horizontal, viewport, Some(new_button(p)));
    Some(bar.leading(nav).tabs(strip).build(crate::palette::kit(p)))
}

/// Points before the row's first control: `ps-3.5` on Windows, `ps-[sidebar-inset - 2]` beside
/// side tabs (so the menu's glyph lines up with the sidebar's icons), past the traffic lights'
/// 74pt and `pl-2.5` on macOS, and `px-2` elsewhere and in full screen.
fn leading_pad(top: bool, lights: f32, zoom: f32) -> f32 {
    match () {
        _ if cfg!(target_os = "windows") && top => 14.0,
        _ if cfg!(target_os = "windows") => INSET_START - 2.0,
        // `WindowTitlebar` already keeps the lights' own width clear.
        _ if lights > 0.0 => ((LIGHTS_W - lights) / zoom).max(0.0) + 10.0,
        _ => 8.0,
    }
}

/// A titlebar icon button (`IconButton` ghost-muted large, `!w-9`): 36 × 28, the hover wash,
/// the pressed wash and base ink while `pressed`.
fn nav_button(p: &Pal, key: &str, label: &str, glyph: &'static Icon, pressed: bool) -> El {
    let b = row()
        .key(key.to_owned())
        .role("button")
        .label(label.to_owned())
        .size(36.0, 28.0)
        .radius(6.0)
        .center()
        .hover(if pressed { p.pressed } else { p.hover })
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .child(icon(glyph, 16.0).color(if pressed { p.icon } else { p.icon_muted }));
    if pressed { b.fill(p.pressed) } else { b }
}

fn home_button(s: &App, p: &Pal) -> El {
    let home = s.route == Route::Home && !s.settings.open;
    nav_button(p, "home", t!("home.title"), &ic::GRID_PLUS, home)
        .help(t!("home.title")).help_keys(crate::keymap::Bind::Home.keycaps())
        .matched(NAV_HOME).matched_crop().motion(GLIDE)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            s.toggle_home(tx);
        })
}

fn search_button(p: &Pal) -> El {
    nav_button(p, "search", t!("command.session.search"), &ic::MAGNIFYING_GLASS, false)
        .help(t!("command.session.search")).help_keys(crate::keymap::Bind::SessionSearch.keycaps())
        .matched(NAV_SEARCH).matched_crop().motion(GLIDE)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            s.close_settings();
            s.open_session_search(tx)
        })
}

/// `+` after the last tab (`RecentlyClosedTabsMenu`'s trigger, 28 × 28): a new session tab, and
/// on a right-click the Recently closed menu.
fn new_button(p: &Pal) -> El {
    row()
        .key("new")
        .role("button")
        .label(t!("command.session.new"))
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .help(t!("command.session.new")).help_keys(crate::keymap::Bind::TabNew.keycaps())
        .matched(NAV_NEW).matched_crop().motion(GLIDE)
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.new_session(tx))
        .on_secondary_press(|s: &mut App, press: rune::PointerPress, tx: &mut Tx| crate::recently_closed::open(s, press, tx))
        .child(icon(&ic::PLUS, 16.0).color(p.icon_muted))
}

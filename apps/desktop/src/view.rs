//! Views: the shell (sidebar + panel), the home page and the session page. Values follow
//! OpenCode Desktop's v2 styles (`packages/app`, `packages/session-ui`, `packages/ui` tokens).

use crate::pickers::PickerHost as _;

use rune::{
    AnimExt, Color, Curve, Cursor, Cx, Drag, Easing, El, Icon, Key, Phase, ScrollSpec,
    Align, Side, Spring, TextStyle, Tx, col, el, icon, row, spacer, text, text_input,
};

use rune_theme_opencode::icons as ic;
use opencode_kit::model::{self, Message, SessionInfo, Transcript};
use crate::palette::{Elevate as _, Pal, pal};
use crate::{App, Route, TabStatus};
use crate::odometer::odometer_end;
use crate::pickers::Kind;

mod chrome;
mod composer;
mod draft;
pub(crate) mod draft_location;
mod header;
mod header_fit;
mod home;
pub(crate) mod location;
pub(crate) mod location_card;
pub(crate) mod move_recovery;
mod inbox;
mod narrow;
mod narrow_story;
mod session;
mod sidebar;
pub(crate) mod sidebar_organized;
mod tab_strip;
pub(crate) mod tab_preview;
mod titlebar;

pub(crate) use chrome::*;
pub(crate) use tab_strip::tab_strip;
pub use composer::*;
pub(crate) use draft::*;
pub(crate) use header::*;
pub(crate) use home::{home, home_modifiers, session_row};
pub(crate) use home::fold::add_flow;
pub(crate) use home::list::{Hold as HomeHold, is_automation};
pub(crate) use session::*;
pub use sidebar::*;

pub(crate) use session_view::composer::PROMPT;
pub(crate) use session_view::style::{BUBBLE, CODE, DOCK_SIDE, H2, INSTANT, UI, UI_MED, UI_SMALL};
#[cfg(test)]
pub(crate) use session_view::style::CONTENT_W;

/// What a layout switch flies its tabs and controls on (`layouts-design.md`'s GLIDE).
pub(crate) const GLIDE: Spring = rune::tok::motion::GLIDE;

/// The window, drawn at the app's zoom: the shell is laid out at 1/zoom of the window and
/// scaled up about the centre, so text re-rasterizes crisp and hits follow what's drawn. The
/// outer, unscaled element is always mounted (it reports the window's size and keeps every
/// path below it stable across zoom changes).
pub fn app(s: &App, cx: &mut Cx<App>) -> El {
    crate::themes::set_active(&s.settings.theme);
    s.motion.note_system(cx.system_reduced_motion());
    let p = pal(s.dark(cx));
    // Desktop's `(max-width: 767px)`: the shell's width, read once it crosses or while narrow.
    let zoom = s.zoom;
    let width = cx.layout_with("zoom", move |r| (r[2] / zoom).min(crate::narrow::machine::BREAKPOINT));
    let narrow = width.is_some_and(|w| s.layout.narrow.measure(w)) && s.stage.is_none();
    // A narrow window keeps top tabs' titlebar row in every layout, its tabs in a drawer.
    let layout = if narrow { crate::layout_state::TabLayout::Horizontal } else { s.tab_layout() };
    set_chrome(cx.chrome_insets(), s.zoom, cx.env().scale, layout, narrow, s.settings.mobile_titlebar == crate::settings::TitlebarPosition::Bottom);
    session_view::style::set_narrow(narrow);
    // Timeline width sets the session column for everything this frame builds.
    session_view::style::set_content_w(session_view::TimelineHost::content_width(s));
    // The caption buttons line up with the titlebar row at every zoom; on macOS the traffic
    // lights move to the layout's titlebar.
    if cfg!(target_os = "macos") {
        cx.set_titlebar(mac_titlebar(layout, s.zoom, chrome().bottom));
    } else {
        cx.titlebar_height(titlebar_height(s.zoom));
    }
    cx.caption_color(caption_color(p.bg_deep));
    // Text and fields left uncoloured take the theme's text, and focusable controls without a
    // ring of their own draw button.css's: 2px `border-focus`, 2.5px out, keyboard focus only.
    cx.foreground(p.text);
    cx.focus_ring(2.0, 2.5, crate::palette::kit(p).c.border_focus);
    // Every button, link and tab is a Tab stop in document order, and dialogs open focused, as
    // in Desktop's DOM.
    cx.web_focus();
    // The debug bar's FOCUS: every control shows its focus ring.
    if s.plugins.luau.force_focus {
        cx.force_focus_rings();
    }
    // Tooltips are Desktop's compact `Tooltip` (`tooltip.css`), placed and timed as its Kobalte.
    cx.help_look(rune_kit::TooltipStyle::from_kit(crate::palette::kit(p), rune_kit::TooltipAppearance::Compact).help_look());
    // A quote's source stays lit in the timeline while its comment is written.
    cx.mark_texts(crate::quote::marks(s));
    crate::icons::prepare(s, cx);
    crate::diagram::prepare(s);
    crate::files::link_tip::prepare(s);
    let mut inner = match s.stage {
        Some(st) => {
            cx.reduced_motion(s.reduced_motion);
            crate::stories::page(s, p, st)
        }
        None => shell(s, cx),
    };
    inner = inner.children(crate::narrow::sheet::layer(s, p, cx));
    inner = inner.children(session_view::pickers::view::layer(s, p));
    if let Route::Session { id, .. } = &s.route
        && let Some(panel) = crate::subagents::panel(s, p, id)
    {
        inner = inner.child(el().key("activity-dismiss").absolute(0.0, 0.0).w_full().h_full().transition(rune::Transition::none())
            .on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                crate::subagents::shells::set_open(s, false);
            }))
            .child(panel);
    }
    if let Route::Session { id, .. } = &s.route
        && let Some(panel) = crate::todos::panel(s, p, id)
    {
        inner = inner.child(el().key("todo-dismiss").absolute(0.0, 0.0).w_full().h_full().transition(rune::Transition::none())
            .on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.todo_open = false;
            }))
            .child(panel);
    }
    inner = inner.children(crate::mentions::view(s, p, cx));
    inner = inner.children(crate::slash::commands_view(s, p, cx));
    inner = inner.children(crate::suggestions::history::view(s, p, cx));
    inner = inner.children(crate::add_menu::menu(s, p));
    inner = inner.children(crate::new_worktree::picker(s, p).into_iter().flatten());
    if let Some(pop) = home::home_projects_popover(s, p) {
        inner = inner.children(pop);
    }
    if s.switcher.open {
        inner = inner.children(crate::switcher::view(s, p, cx));
    }
    if s.project_switcher.open {
        inner = inner.children(crate::project_switcher::view(s, p, cx));
    }
    inner = inner.children(crate::attachments::viewer(s, p, cx));
    inner = inner.children(crate::diagram::view(s, p, cx));
    inner = inner.children(session_view::tools::details_overlay(s, p, cx));
    inner = inner.children(crate::session_menu_view::confirm(s, p));
    inner = inner.children(crate::settings::pages::worktrees::dialog(s, p));
    inner = inner.children(crate::quote::editor::view(s, p, cx));
    inner = inner.children(crate::quote::toolbar(s, p, cx));
    inner = inner.children(crate::panels::view::context_menus(s, p));
    inner = inner.children(crate::panels::drag::layer(s, p));
    if s.fork.open {
        inner = inner.children(crate::fork::view(s, p, cx));
    }
    inner = inner.children(crate::connect::view::layer(s, p, cx));
    inner = inner.children(crate::connect::welcome::layer(s, p, cx));
    inner = inner.children(crate::mcp::view::layer(s, p, cx));
    inner = inner.children(crate::mcp::sign_in::layer(s, p));
    inner = inner.children(crate::usage::layer(s, p, cx));
    inner = inner.children(move_recovery::view(s, p));
    inner = inner.children(crate::plugin_host::dialog_view(s, p));
    inner = inner.children(crate::servers::dialog::view(s, p));
    inner = inner.children(tab_preview::view(s, &p));
    inner = inner.children(crate::popup::view(s, p));
    inner = inner.children(crate::plugin_host::popover_view(s, p));
    inner = inner.children(crate::plugin_host::trust_dialog(s, p));
    inner = inner.children(crate::status_toast::view(s, p));
    let sized = if (s.zoom - 1.0).abs() > 1e-4 {
        let [w, h] = cx.layout_size("zoom").unwrap_or([0.0, 0.0]);
        if w > 0.0 && h > 0.0 { el().w(w / s.zoom).h(h / s.zoom) } else { el().w_full().h_full() }
    } else {
        el().w_full().h_full()
    };
    let root = el().key("zoom");
    // Arabic, Hebrew, Persian, Urdu… mirror the whole window, as Desktop's `dir` on the
    // document root does (`language.tsx:397`); the caption buttons Rune draws stay put.
    let root = root.dir(ui_direction());
    root.on_key_up(crate::dictation::key_up)
        .on_window_focus(|s: &mut App, focused, tx| {
            s.plugins.luau.window_blurred.set(!focused);
            if focused { crate::launch::network_available(s, tx); }
        })
        // A resize across the breakpoint reaches the drawers in the frame it lands.
        .on_layout_tx(|s: &mut App, _, tx: &mut Tx| { if !crate::narrow::settle(s, tx) { tx.live_only(); } })
        .on_magnify(|s: &mut App, ev, tx| s.pinch(ev, tx))
        // A text field's menu shows and flips Settings › General › Spellcheck, as Desktop's
        // context menu does (`spellcheck.ts`).
        .on_spellcheck(s.settings.spellcheck, |s: &mut App, on, _| s.change_settings(|x| x.spellcheck = on))
        // Off macOS it also picks the dictionaries (Spellcheck Languages).
        .on_spellcheck_languages(|s: &mut App, tags, _| s.change_settings(|x| x.spellcheck_languages = tags))
        .w_full()
        .h_full()
        .center()
        // Hovers land and leave at once, like Desktop.
        .hover_motion_within(INSTANT, INSTANT)
        .child(crate::glitch::wrap(s, sized.key("zoomed").scale(s.zoom).child(inner)))
        .child(crate::knobs::layer(s))
}

fn shell(s: &App, cx: &mut Cx<App>) -> El {
    let p = pal(s.dark(cx));
    cx.time_scale(crate::knobs::time_scale(s));
    cx.reduced_motion(s.reduced_motion);
    cx.time_scale(crate::knobs::time_scale(s));
    let w = s.sidebar_w;
    let side_tabs = s.shows_sidebar();
    // A narrow window's panel runs edge to edge, 1 pt clear for its outline (`--shell-inline-inset`).
    let inset = if s.narrow() { 1.0 } else { 8.0 };
    let open = s.sidebar_open && side_tabs;
    // The side region keeps its open width as the far end of a close's travel, so the row's width
    // is read whenever a session shows.
    let win_w = if matches!(s.route, Route::Session { .. }) {
        cx.layout_size("zoom").map_or(0.0, |[w, _]| w / s.zoom)
    } else {
        0.0
    };
    let need_panel_sz = (!s.settings.open && matches!(s.route, Route::Draft(_) | Route::Home)) || s.attach.drop.present();
    let panel_sz = if need_panel_sz {
        cx.layout_size("panel").map_or((0.0, 0.0), |[w, h]| (w, h))
    } else {
        (0.0, 0.0)
    };
    let settings_sz = if s.settings.open { cx.layout_size("panel").map_or((0.0, 0.0), |[w, h]| (w, h)) } else { (0.0, 0.0) };
    let bottom_reserve = crate::panels::view::bottom_reserve_height(s, cx);
    // A layout switch moves the panel, the sidebar and every tab on GLIDE, with the panel on top:
    // the tabs slide under its edges between the titlebar and the sidebar rather than over the
    // page. The titlebar row comes and goes at once, and below the panel while it does.
    let switching = s.switching_layout();
    let band = titlebar::titlebar(s, p, cx)
        .map(|bar| {
            // Settings › Bottom navigation moves a narrow window's row to its foot (`order-last`).
            let band = el().key("titlebar-band").h(chrome().row_h).transition(rune::Transition::none()).child(bar);
            if chrome().bottom { band.pin(Some(0.0), None, Some(0.0), Some(0.0)) } else { band.pin(Some(0.0), Some(0.0), Some(0.0), None) }
        });
    let (band_below, band) = if switching { (band, None) } else { (None, band) };
    let mut panel = col()
        .key("panel")
        .grow(1.0)
        .shrink()
        .ml(inset)
        .mt(chrome().top())
        .mr(inset)
        .mb(8.0 + bottom_reserve + chrome().foot())
        .radius(10.0)
        .fill(p.bg_base)
        .elevate(p.raised)
        .clip_soft(0.0);
    panel = if switching { panel.motion(GLIDE).raise() } else { panel.motion(crate::panels::view::SIDE_SPRING) };
    let mut root = row()
        .key("window")
        .role("window")
        .fill(p.bg_deep)
        .w_full()
        .h_full()
        .child(sidebar(s, p))
        .children(band_below)
        // The empty inset above the panels moves the window, as the titlebar does; on Windows
        // it's the titlebar row itself.
        .child(el().key("drag-inset").pin(Some(0.0), Some(0.0), Some(chrome().caption_w), None).h(chrome().top()).window_drag().unmirrored())
        // The titlebar row before the page, as in Desktop's DOM, so Tab reaches it first.
        .children(band)
        // Toggle review comes next, before the page (`screen.tsx`: its `z-30` row ahead of the
        // chat panel), and paints over the page and the panels after it.
        .children(crate::panels::view::side_toggle(s, p).map(|toggle| toggle.paint_order(1)))
        .child(
            // `session/screen.tsx`'s frame: bg-base, 10 pt corners, `--v2-elevation-raised`.
            panel
                .focusable(false)
                .on_press(|s: &mut App| {
                    s.side.focused = false;
                    s.side.tab_menu = None;
                    s.side.add_menu = None;
                })
                // Files dropped anywhere on the page attach, as in Desktop; a timeline image
                // dragged out of the page attaches only on the composer.
                .on_drop_files(crate::dropzone::drop)
                .accepts_files(|path| !session_view::images::drag::is_own(path))
                .on_drop_hover(crate::dropzone::hover)
                .child(if s.settings.open { crate::settings::page(s, p, settings_sz) } else { match &s.route {
                    Route::Home => home(s, p, [panel_sz.0, panel_sz.1], cx.layout_size("zoom").map_or([0.0, 0.0], |[w, h]| [w / s.zoom, h / s.zoom])),
                    Route::Session { id, .. } => session_page(s, p, id),
                    Route::Draft(_) => draft_page(s, p, panel_sz.0),
                } })
                .child(crate::dropzone::overlay(s, p, panel_sz)),
        )
        .child(crate::panels::view::side_region(s, p, win_w, bottom_reserve, cx));
    if let Some(dock) = crate::panels::view::bottom_dock(s, p, cx) {
        root = root.child(
            dock.pin(Some(s.sidebar_shown_w()), None, Some(0.0), Some(0.0))
                .motion(crate::panels::view::SIDE_SPRING),
        );
    }
    if open {
        root = root.child(resize_handle(s, p, w));
    }
    if let Some(handle) = crate::panels::view::resize_handle(s, p, win_w) {
        root = root.child(handle);
    }
    root = root.children(sidebar::peek(s, p));
    // Without a titlebar row, the toggle rides the sidebar header's line (`sidebar-toggle.tsx`).
    if side_tabs && chrome().sidebar_header() {
        root = root.child(sidebar_toggle(s, p, s.sidebar_open, w));
    }
    root = root.children(crate::plugin_host::titlebar_items(s, p));
    // Plugins' `window.bottom` slot (the debug bar) runs across the window's foot, under the
    // panels, taking its own room (`shell.tsx`'s last `PluginSlot`).
    let sid = match &s.route {
        Route::Session { id, .. } => id.as_str(),
        _ => "",
    };
    let bottom = crate::plugin_host::slot(s, p, "window.bottom", sid);
    if bottom.is_empty() {
        return root;
    }
    col()
        .key("window-column")
        .w_full()
        .h_full()
        .fill(p.bg_deep)
        .child(el().key("window-main").w_full().grow(1.0).min_h(0.0).child(root))
        .child(col().key("plugin-window-bottom").w_full().children(bottom))
}

// ---------------------------------------------------------------------------------------------
// Pieces

/// CSS `text-left` in a right-to-left window: the left edge, which a right-to-left text's
/// `End` is. `Start` left to right.
pub(crate) fn text_left() -> Align {
    match opencode_i18n::direction() {
        opencode_i18n::Direction::Ltr => Align::Start,
        opencode_i18n::Direction::Rtl => Align::End,
    }
}

/// The window's layout direction: the language's (`language.direction()`).
pub(crate) fn ui_direction() -> rune::Direction {
    match opencode_i18n::direction() {
        opencode_i18n::Direction::Ltr => rune::Direction::Ltr,
        opencode_i18n::Direction::Rtl => rune::Direction::Rtl,
    }
}

pub(crate) fn title_of(s: &SessionInfo) -> String {
    if s.title.is_empty() { opencode_i18n::t!("rune.session.untitled").into() } else { s.title.clone() }
}

pub(crate) fn project_or_dir(project: Option<&model::Project>, dir: &str) -> String {
    project.map(|p| p.name.clone()).unwrap_or_else(|| opencode_kit::path::basename(dir).to_owned())
}

#[cfg(test)]
mod composer_height_tests;
#[cfg(test)]
mod composer_tests;
#[cfg(test)]
mod diagram_width_tests;
#[cfg(test)]
mod narrow_tests;
#[cfg(test)]
pub(crate) mod selection_tests;
#[cfg(test)]
mod inline_code_selection_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod icon_centre_tests;

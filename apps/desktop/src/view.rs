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
mod inbox;
mod narrow;
mod narrow_story;
mod session;
mod sidebar;
mod tab_strip;
mod titlebar;

pub(crate) use chrome::*;
pub(crate) use tab_strip::tab_strip;
pub use composer::*;
pub(crate) use draft::*;
pub(crate) use header::*;
pub(crate) use narrow::{Agent as ToolbarAgent, toolbar_shown};
pub(crate) use home::{home, home_modifiers, session_row};
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
    // Timeline width sets the session column for everything this frame builds.
    session_view::style::set_content_w(session_view::TimelineHost::content_width(s));
    s.motion.note_system(cx.system_reduced_motion());
    let p = pal(s.dark(cx));
    let layout = s.tab_layout();
    set_chrome(cx.chrome_insets(), s.zoom, cx.env().scale, layout);
    // The caption buttons line up with the titlebar row at every zoom; on macOS the traffic
    // lights move to the layout's titlebar.
    if cfg!(target_os = "macos") {
        cx.set_titlebar(mac_titlebar(layout, s.zoom));
    } else {
        cx.titlebar_height(titlebar_height(s.zoom));
    }
    cx.caption_color(caption_color(p.bg_deep));
    // Text and fields left uncoloured take the theme's text, and focusable controls without a
    // ring of their own draw button.css's: 2px `border-focus`, 2.5px out, keyboard focus only.
    cx.foreground(p.text);
    cx.focus_ring(2.0, 2.5, crate::palette::kit(p).c.border_focus);
    // Tooltips are Desktop's compact `Tooltip` (`tooltip.css`), placed and timed as its Kobalte.
    cx.help_look(rune_kit::TooltipStyle::from_kit(crate::palette::kit(p), rune_kit::TooltipAppearance::Compact).help_look());
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
    inner = inner.children(session_view::pickers::view::layer(s, p));
    if let Route::Session(id) = &s.route
        && let Some(panel) = crate::subagents::panel(s, p, id)
    {
        inner = inner.child(el().key("activity-dismiss").absolute(0.0, 0.0).w_full().h_full().transition(rune::Transition::none())
            .on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.activity_open = false;
            }))
            .child(panel);
    }
    if let Route::Session(id) = &s.route
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
    inner = inner.children(crate::quote_cards::editor(s, p, cx));
    inner = inner.children(crate::quote::toolbar(s, p, cx));
    inner = inner.children(crate::panels::view::context_menus(s, p));
    inner = inner.children(crate::panels::drag::layer(s, p));
    if s.fork.open {
        inner = inner.children(crate::fork::view(s, p, cx));
    }
    inner = inner.children(crate::connect::view::layer(s, p, cx));
    inner = inner.children(crate::mcp::view::layer(s, p, cx));
    inner = inner.children(crate::settings::pages::pairing::layer(s, p));
    inner = inner.children(crate::plugin_host::dialog_view(s, p));
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
    root.on_key_up(crate::dictation::key_up)
        .on_magnify(|s: &mut App, ev, tx| s.pinch(ev, tx))
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
    cx.reduced_motion(s.reduced_motion);
    let w = s.sidebar_w;
    let side_tabs = s.tab_layout().has_sidebar();
    let open = s.sidebar_open && side_tabs;
    // The side region keeps its open width as the far end of a close's travel, so the row's width
    // is read whenever a session shows.
    let win_w = if matches!(s.route, Route::Session(_)) {
        cx.layout_size("zoom").map_or(0.0, |[w, _]| w / s.zoom)
    } else {
        0.0
    };
    let need_panel_sz = (!s.settings.open && matches!(s.route, Route::Draft(_) | Route::Home)) || s.attach.drop.over;
    let panel_sz = if need_panel_sz {
        cx.layout_size("panel").map_or((0.0, 0.0), |[w, h]| (w, h))
    } else {
        (0.0, 0.0)
    };
    let settings_sz = if s.settings.open { cx.layout_size("panel").map_or((0.0, 0.0), |[w, h]| (w, h)) } else { (0.0, 0.0) };
    let bottom_reserve = crate::panels::view::bottom_reserve_height(s, cx);
    let mut root = row()
        .key("window")
        .role("window")
        .fill(p.bg_deep)
        .w_full()
        .h_full()
        .child(sidebar(s, p))
        .child(
            // `session/screen.tsx`'s frame: bg-base, 10 pt corners, `--v2-elevation-raised`.
            col()
                .key("panel")
                .grow(1.0)
                .shrink()
                .ml(8.0)
                .mt(chrome().top())
                .mr(8.0)
                .mb(8.0 + bottom_reserve)
                .radius(10.0)
                .fill(p.bg_base)
                .elevate(p.raised)
                .clip_soft(0.0)
                .motion(crate::panels::view::SIDE_SPRING)
                .on_press(|s: &mut App| {
                    s.side.focused = false;
                    s.side.dock_focused = false;
                    s.side.tab_menu = None;
                    s.side.add_menu = None;
                })
                // Files dropped anywhere on the page attach, as in Desktop.
                .on_drop_files(crate::dropzone::drop)
                .on_drop_hover(crate::dropzone::hover)
                .child(if s.settings.open { crate::settings::page(s, p, settings_sz) } else { match &s.route {
                    Route::Home => home(s, p, panel_sz.0, cx.layout_size("zoom").map_or([0.0, 0.0], |[w, h]| [w / s.zoom, h / s.zoom])),
                    Route::Session(id) => session_page(s, p, id),
                    Route::Draft(_) => draft_page(s, p, panel_sz.0),
                } })
                .child(crate::dropzone::overlay(s, p, panel_sz)),
        )
        .child(crate::panels::view::side_region(s, p, win_w, bottom_reserve, cx))
        // The empty inset above the panels moves the window, as the titlebar does; on Windows
        // it's the titlebar row itself.
        .child(el().key("drag-inset").pin(Some(0.0), Some(0.0), Some(chrome().caption_w), None).h(chrome().top()).window_drag());
    if let Some(bar) = titlebar::titlebar(s, p, cx) {
        root = root.child(el().key("titlebar-band").pin(Some(0.0), Some(0.0), Some(0.0), None).h(chrome().row_h).child(bar));
    }
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
    if let Some(toggle) = crate::panels::view::side_toggle(s, p) {
        root = root.child(toggle);
    }
    root = root.children(sidebar::peek(s, p));
    // Without a titlebar row, the toggle rides the sidebar header's line (`sidebar-toggle.tsx`).
    if side_tabs && chrome().sidebar_header() {
        root = root.child(sidebar_toggle(s, p, s.sidebar_open, w));
    }
    root = root.children(crate::plugin_host::titlebar_items(s, p));
    // Plugins' `window.bottom` slot (a debug bar) runs across the window's foot.
    let sid = match &s.route {
        Route::Session(id) => id.as_str(),
        _ => "",
    };
    let bottom = crate::plugin_host::slot(s, p, "window.bottom", sid);
    if !bottom.is_empty() {
        root = root.child(row().key("plugin-window-bottom").pin(Some(0.0), None, Some(0.0), Some(0.0)).children(bottom));
    }
    root
}

// ---------------------------------------------------------------------------------------------
// Pieces

pub(crate) fn title_of(s: &SessionInfo) -> String {
    if s.title.is_empty() { "Untitled session".into() } else { s.title.clone() }
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

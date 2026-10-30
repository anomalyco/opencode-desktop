//! Views: the shell (sidebar + panel), the home page and the session page. Values follow
//! OpenCode Desktop's v2 styles (`packages/app`, `packages/session-ui`, `packages/ui` tokens).

use std::sync::OnceLock;

use rune::{
    Anim, AnimExt, Color, Curve, Cursor, Cx, Cycle, Drag, Easing, El, GrowthSpec, Icon, Key, Phase, ScrollSpec,
    Align, Side, Spring, TextStyle, Tx, col, edge, el, icon, row, spacer, text, text_input,
};

use rune_kit::{Markdown, MarkdownStyle};

use rune_theme_opencode::icons as ic;
use opencode_kit::model::{self, Assistant, InboxItem, Message, Part, Pending, SessionInfo, Transcript};
use crate::palette::{Pal, pal};
use crate::{App, Route, TabStatus};
use crate::odometer::odometer_end;
use crate::pickers::Kind;
use crate::turns::{self, Block, Step};

mod composer;
mod home;
mod sidebar;
mod timeline;

pub use composer::*;
pub(crate) use home::{home, session_row};
pub use sidebar::*;
pub(crate) use timeline::*;

pub(crate) const INSTANT: Anim = Anim::INSTANT;

// Type: Inter variable (weights 440/530/670 are real instances) and IBM Plex Mono.
pub(crate) const UI: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
pub(crate) const UI_MED: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
pub(crate) const UI_SMALL: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);
const MD: TextStyle = TextStyle::new(14.0, 400.0).lh(22.4);
pub(crate) const BUBBLE: TextStyle = TextStyle::new(14.0, 400.0).lh(21.0);
const H1: TextStyle = TextStyle::new(17.0, 600.0).lh(24.0);
const H2: TextStyle = TextStyle::new(15.0, 600.0).lh(24.0);
const H3: TextStyle = TextStyle::new(13.0, 600.0).lh(20.0);
const CODE: TextStyle = TextStyle::new(13.0, 400.0).mono().lh(20.0);
const PROMPT: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0);
const SHELL_PROMPT: TextStyle = TextStyle::new(13.0, 400.0).mono().lh(20.0);

/// `--container-session`: 744 including 24 px padding on each side.
pub(crate) const CONTENT_W: f32 = 696.0;

/// The window, drawn at the app's zoom: the shell is laid out at 1/zoom of the window and
/// scaled up about the centre, so text re-rasterizes crisp and hits follow what's drawn. The
/// outer, unscaled element is always mounted (it reports the window's size and keeps every
/// path below it stable across zoom changes).
pub fn app(s: &App, cx: &mut Cx<App>) -> El {
    let p = pal(s.dark(cx));
    crate::icons::prepare(s, cx);
    let mut inner = match s.stage {
        Some(st) => {
            cx.reduced_motion(s.reduced_motion);
            crate::stories::page(s, p, st)
        }
        None => shell(s, cx),
    };
    if s.pickers.open.is_some() {
        inner = inner.child(el().key("picker-dismiss").absolute(0.0, 0.0).w_full().h_full()
            .on_press_tx(|s: &mut App, tx: &mut Tx| {
                // Nothing gets focus back, so the search field can unmount in this frame.
                tx.instant();
                s.close_picker(false);
            }))
            .child(picker(s, p));
    }
    if let Route::Session(id) = &s.route
        && let Some(panel) = crate::subagents::panel(s, p, id)
    {
        inner = inner.child(el().key("activity-dismiss").absolute(0.0, 0.0).w_full().h_full()
            .on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.activity_open = false;
            }))
            .child(panel);
    }
    inner = inner.children(crate::mentions::view(s, p, cx));
    inner = inner.children(crate::slash::commands_view(s, p, cx));
    inner = inner.children(crate::add_menu::menu(s, p));
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
    inner = inner.children(crate::session_menu_view::confirm(s, p));
    inner = inner.children(crate::quote_cards::editor(s, p, cx));
    inner = inner.children(crate::quote::toolbar(s, p, cx));
    let sized = if (s.zoom - 1.0).abs() > 1e-4 {
        let [w, h] = cx.layout_size("zoom").unwrap_or([0.0, 0.0]);
        if w > 0.0 && h > 0.0 { el().w(w / s.zoom).h(h / s.zoom) } else { el().w_full().h_full() }
    } else {
        el().w_full().h_full()
    };
    let root = el().key("zoom");
    root.on_key_up(crate::dictation::key_up)
        .w_full()
        .h_full()
        .center()
        // Hovers land and leave at once, like Desktop.
        .hover_motion_within(INSTANT, INSTANT)
        .child(sized.key("zoomed").scale(s.zoom).child(inner))
        .child(crate::knobs::layer(s))
}

/// An anchored overlay escapes the window's zoom, so it scales itself: it grows about the corner that touches its anchor (`above` is the
/// bottom-left, else the top-left), landing at once.
pub(crate) fn follow_zoom(overlay: El, zoom: f32, above: bool) -> El {
    overlay.scale(zoom).scale_motion(Spring::NONE).origin(0.0, if above { 1.0 } else { 0.0 })
}

fn shell(s: &App, cx: &mut Cx<App>) -> El {
    let p = pal(s.dark(cx));
    cx.reduced_motion(s.reduced_motion);
    let w = s.sidebar_w;
    let open = s.sidebar_open;
    let need_panel_sz = (!s.settings.open && matches!(s.route, Route::Draft | Route::Home)) || s.attach.drop.over;
    let panel_sz = if need_panel_sz {
        cx.layout_size("panel").map_or((0.0, 0.0), |[w, h]| (w / s.zoom, h / s.zoom))
    } else {
        (0.0, 0.0)
    };
    let mut root = row()
        .key("window")
        .role("window")
        .fill(p.bg_deep)
        .w_full()
        .h_full()
        .child(sidebar(s, p))
        .child(
            col()
                .key("panel")
                .grow(1.0)
                .shrink()
                .ml(8.0)
                .mt(8.0)
                .mr(8.0)
                .mb(8.0)
                .radius(10.0)
                .fill(p.bg_base)
                .border(0.5, p.ring)
                .shadow(2.0, 4.0, p.shadow)
                .shadow(1.0, 2.0, p.shadow)
                .clip_soft(0.0)
                // Files dropped anywhere on the page attach, as in Desktop.
                .on_drop_files(crate::dropzone::drop)
                .on_drop_hover(crate::dropzone::hover)
                .child(if s.settings.open { crate::settings::page(s, p) } else { match &s.route {
                    Route::Home => home(s, p, panel_sz.0),
                    Route::Session(id) => session_page(s, p, id),
                    Route::Draft => draft_page(s, p, panel_sz.0),
                } })
                .child(crate::dropzone::overlay(s, p, panel_sz)),
        )
        .children(crate::browser::view::aside(s, p))
        // The empty inset above the panels moves the window, as the titlebar does.
        .child(el().key("drag-inset").absolute(0.0, 0.0).w_full().h(8.0).window_drag());
    if open {
        root = root.child(resize_handle(s, p, w));
    }
    if s.browser.open {
        let win_w = cx.layout_size("zoomed").map_or(0.0, |[w, _]| w);
        if let Some(handle) = crate::browser::view::resize_handle(s, p, win_w) {
            root = root.child(handle);
        }
    }
    root = root.child(sidebar_toggle(p, open, w));
    root
}

// ---------------------------------------------------------------------------------------------
// Pieces

fn roots(s: &App) -> impl Iterator<Item = &SessionInfo> {
    s.sessions.iter().filter(|x| x.parent.is_none())
}

pub(crate) fn title_of(s: &SessionInfo) -> String {
    if s.title.is_empty() { "Untitled session".into() } else { s.title.clone() }
}

pub(crate) fn project_or_dir(project: Option<&model::Project>, dir: &str) -> String {
    project.map(|p| p.name.clone()).unwrap_or_else(|| dir.rsplit('/').find(|x| !x.is_empty()).unwrap_or("").to_owned())
}

fn home_dir() -> String {
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| std::env::var("HOME").unwrap_or_else(|_| "\u{0}".into())).clone()
}

#[cfg(test)]
mod composer_tests;
#[cfg(test)]
pub(crate) mod selection_tests;
#[cfg(test)]
mod tests;

//! Views: the shell (sidebar + panel), the home page and the session page. Values follow
//! OpenCode Desktop's v2 styles (`packages/app`, `packages/session-ui`, `packages/ui` tokens).

use std::sync::OnceLock;

use rune::{
    Anim, AnimExt, Color, Curve, Cursor, Cx, Cycle, Drag, Easing, El, GrowthSpec, Icon, Key, Phase, ScrollSpec,
    Align, Shadow, Side, Spring, TextStyle, Tx, col, edge, el, icon, row, spacer, text, text_input,
};

use rune_kit::{Kit, Markdown, MarkdownStyle};

use rune_theme_opencode::icons as ic;
use opencode_kit::model::{self, Assistant, InboxItem, Message, Part, Pending, SessionInfo, Transcript};
use crate::palette::{Pal, pal};
use crate::{App, Route, TabStatus};
use crate::pickers::Kind;
use crate::turns::{self, Block, Step};

mod composer;
mod home;
mod sidebar;
mod timeline;

pub use composer::*;
use home::*;
pub use sidebar::*;
pub(crate) use timeline::*;

// Type: Inter variable (weights 440/530/670 are real instances) and IBM Plex Mono.
pub(crate) const UI: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
pub(crate) const UI_MED: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
pub(crate) const UI_SMALL: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);
const AVATAR: TextStyle = TextStyle::new(11.0, 530.0).lh(16.0);
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
    let mut inner = shell(s, cx);
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
    let composer_w = cx.layout("composer").map_or(0.0, |r| r[2]);
    inner = inner.children(crate::mentions::view(s, p, composer_w));
    inner = inner.children(crate::add_menu::commands_view(s, p, composer_w));
    inner = inner.children(crate::add_menu::menu(s, p));
    if s.switcher.open {
        inner = inner.children(crate::switcher::view(s, p, cx));
    }
    inner = inner.children(crate::attachments::viewer(s, p, cx));
    let (w, h) = cx.layout("zoom").map_or((0.0, 0.0), |r| (r[2], r[3]));
    let sized = if w > 0.0 && h > 0.0 { el().w(w / s.zoom).h(h / s.zoom) } else { el().w_full().h_full() };
    el().key("zoom")
        .w_full()
        .h_full()
        .center()
        // Hovers land and leave at once, like Desktop.
        .hover_motion_within(INSTANT, INSTANT)
        .child(sized.key("zoomed").scale(s.zoom).child(inner))
        .child(crate::knobs::layer(s))
}

fn shell(s: &App, cx: &mut Cx<App>) -> El {
    if !s.playground.is_empty() {
        return crate::icons::playground(s);
    }
    let p = pal(s.dark(cx));
    cx.reduced_motion(s.reduced_motion);
    let w = s.sidebar_w;
    let open = s.sidebar_open;
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
                .on_drop_files(crate::attachments::drop_files)
                .on_drop_hover(crate::attachments::drop_hover)
                .child(if s.story.is_some() { crate::stories::page(s, p) } else if s.settings.open { crate::settings::page(s, p) } else { match &s.route {
                    Route::Home => home(s, p),
                    Route::Session(id) => session_page(s, p, id),
                    Route::Draft => draft_page(s, p, cx.layout("panel").map_or(0.0, |r| r[2] / s.zoom)),
                } })
                .children(crate::attachments::drop_overlay(s, p, cx.layout("panel").map_or((0.0, 0.0), |r| (r[2] / s.zoom, r[3] / s.zoom)))),
        )
        // The empty inset above the panels moves the window, as the titlebar does.
        .child(el().key("drag-inset").absolute(0.0, 0.0).w_full().h(8.0).window_drag());
    if open {
        root = root.child(resize_handle(s, p, w));
    }
    root = root.child(sidebar_toggle(p, open, w));
    root
}

// ---------------------------------------------------------------------------------------------
// Pieces

pub(crate) fn avatar(p: &Pal, name: &str) -> El {
    let k = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) as usize % p.avatars.len();
    let (bg, border) = p.avatars[k];
    let letter = name.chars().find(|c| c.is_alphanumeric()).map_or("?".into(), |c| c.to_uppercase().to_string());
    el().size(16.0, 16.0)
        .radius(4.0)
        .fill(bg)
        .border(0.5, border)
        .center()
        .child(text(letter).font(AVATAR).color(p.avatar_fg))
}

fn roots(s: &App) -> impl Iterator<Item = &SessionInfo> {
    s.sessions.iter().filter(|x| x.parent.is_none())
}

pub(crate) fn title_of(s: &SessionInfo) -> String {
    if s.title.is_empty() { "Untitled session".into() } else { s.title.clone() }
}

fn home_dir() -> String {
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| std::env::var("HOME").unwrap_or_else(|_| "\u{0}".into())).clone()
}

#[cfg(test)]
mod tests;

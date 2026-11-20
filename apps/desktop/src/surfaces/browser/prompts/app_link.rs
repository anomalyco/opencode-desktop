//! Native Rune UI for the 500 ms guarded app-scheme link review bar matching official
//! `origin/dev` `b9a60d9d` (`packages/gui-plugins/src/browser/{refusal-bar.tsx,notice-actions.ts}`):
//!
//! - Drawn natively by the host above the built-in browser panel (`plugin_host/ui/panels.rs`),
//!   with `.on_press_tx` wired directly to `App::confirm_app_link_review` (never hover, never
//!   Luau, never reachable from plugins or MCP tools).
//! - Live `fits()` check (`refusal-bar.tsx:18-61`): compares the wrapped target text rect against
//!   the `max_h(80.0)` target viewport box (`on_drawn_layout_tx`); whenever any part of the URL is
//!   clipped (or the 60 s token TTL expires), the action downgrades to Copy-only.
//! - 500 ms activation delay (`refusal-bar.tsx:63`) starts ONLY once the bar is visible and
//!   stationary in the focused window (and restarts if `fits()` transitions).

use browser_policy::{AppSchemeAction, AppSchemeSource};
use opencode_i18n::t;
use rune::time::Instant;
use rune::{Cursor, Direction, El, Family, TextStyle, Tx, col, icon, row, text};
use rune_kit::{ButtonVariant, ControlSize};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::palette::{Pal, kit};
use crate::view::UI_SMALL;

const MONO_SMALL: TextStyle = TextStyle::new(12.0, 400.0).family(Family::Mono).lh(16.0);
/// Matches official `refusal-bar.tsx:92` `max-h-20` (80 px = 5 lines at 16 px line height).
const TARGET_MAX_H: f32 = 80.0;

/// Renders the active app-link review bar for browser tab `tab_id` when the window is focused and
/// the tab is owned by the active session.
pub fn app_link_review_bar(s: &App, p: &Pal, tab_id: u64) -> Option<El> {
    if s.plugins.window_blurred.get() || s.browser_tab_session(tab_id) != s.active_session_id() {
        return None;
    }
    let review = s.workspace_panes.browser.active_app_reviews.borrow().get(&tab_id)?.clone();
    let k = kit(p);
    let origin = match review.source {
        AppSchemeSource::Conversation => "From this conversation",
        AppSchemeSource::Page => "From this page",
    };
    let effective_open = matches!(review.action, AppSchemeAction::Open | AppSchemeAction::Mail)
        && review.target_fits
        && Instant::now() < review.expires_at;
    let action_label = if effective_open {
        t!("common.open")
    } else {
        t!("rune.browser.menu.copyLink")
    };

    let confirm_token = review.token.clone();
    let action_btn = crate::widgets::button(action_label, ButtonVariant::Neutral)
        .control_size(ControlSize::Regular)
        .on_press_tx(move |app: &mut App, tx: &mut Tx| {
            app.confirm_app_link_review(&confirm_token, tx);
        })
        .build(k)
        .key("browser-refusal-action");

    let dismiss_token = review.token.clone();
    let dismiss_btn = row()
        .key("browser-refusal-dismiss")
        .role("button")
        .label(t!("common.close"))
        .focusable(true)
        .w(24.0)
        .h(24.0)
        .center()
        .radius(4.0)
        .cursor(Cursor::Pointer)
        .hover(p.hover)
        .on_press_tx(move |app: &mut App, tx: &mut Tx| {
            app.dismiss_app_link_review(&dismiss_token, tx);
        })
        .child(icon(&ic::CLOSE, 14.0).color(p.icon_muted));

    let header = col()
        .role("status")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(2.0)
        .child(text(origin).font(UI_SMALL).color(p.text).wrap())
        .child(text("This link opens another app:").font(UI_SMALL).color(p.text).wrap());

    let top = row()
        .w_full()
        .gap(12.0)
        .center_y()
        .child(header)
        .child(action_btn)
        .child(dismiss_btn);

    let text_token = review.token.clone();
    let target_text = text(review.url.clone())
        .key("browser-refusal-target-text")
        .dir(Direction::Ltr)
        .font(MONO_SMALL)
        .color(p.muted)
        .wrap()
        .on_drawn_layout_tx(move |app: &mut App, rect, tx| {
            app.observe_app_link_target_text(&text_token, rect, tx);
        });

    let box_token = review.token.clone();
    let target_box = col()
        .key("browser-refusal-target")
        .w_full()
        .max_h(TARGET_MAX_H)
        .on_drawn_layout_tx(move |app: &mut App, rect, tx| {
            app.observe_app_link_target_box(&box_token, rect, tx);
        })
        .child(session_view::scroller::scroller("browser-refusal-target-scroll").build::<App>(
            p,
            col().w_full().max_h(TARGET_MAX_H),
            target_text,
        ));

    let content = col()
        .w_full()
        .px(12.0)
        .py(8.0)
        .gap(4.0)
        .fill(p.layer1)
        .child(top)
        .child(target_box);

    let layout_token = review.token;
    Some(
        col()
            .key("browser-refusal")
            .w_full()
            .on_drawn_layout_tx(move |app: &mut App, rect, tx| {
                app.observe_app_link_review_layout(&layout_token, rect, tx);
            })
            .child(content)
            .child(col().w_full().h(1.0).fill(p.border_muted)),
    )
}

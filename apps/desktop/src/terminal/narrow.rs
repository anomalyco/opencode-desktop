//! The narrow window's Terminal view (`gui-plugins/src/terminal/panel.tsx` at the `mobile`
//! placement): a 49 pt bar of the session's terminals as panel tabs with `+` after them, over the
//! selected terminal. Shown on a session with none, it starts the first.

use rune::{Color, El, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use super::grid;
use crate::App;
use crate::palette::Pal;
use crate::view::{INSTANT, UI};
use opencode_i18n::t;

pub fn view(s: &App, p: &Pal, sid: &str) -> El {
    s.terminal.painted_dark.set(Some(p.dark));
    let wk = s.terminal_workspace_key_for_session(sid);
    let ws = s.terminal.workspaces.get(&wk);
    let active = ws.and_then(|w| w.active.clone().or_else(|| w.tabs.first().map(|t| t.id.clone())));
    let tabs = ws.map_or(&[][..], |w| w.tabs.as_slice()).iter().fold(row().key("narrow-terminal-tabs").gap(8.0).center_y().min_w(0.0).scroll_x(), |strip, t| {
        let on = active.as_deref() == Some(t.id.as_str());
        let (pick, close, wk_pick, wk_close) = (t.id.clone(), t.id.clone(), wk.clone(), wk.clone());
        strip.child(
            row()
                .key(format!("narrow-terminal-tab-{}", t.id))
                .role("tab")
                .label(t.display_title())
                .focus_selected(on)
                .h(28.0)
                .pl(10.0)
                .pr(4.0)
                .gap(6.0)
                .center_y()
                .radius(6.0)
                .fill(if on { p.layer2 } else { Color::TRANSPARENT })
                .hover(if on { p.layer2 } else { p.hover })
                .hover_motion(INSTANT, INSTANT)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.select_terminal_tab(&wk_pick, &pick);
                    s.focus_terminal_surface();
                })
                .child(text(t.display_title()).font(UI).color(if on { p.text } else { p.muted }).truncate().max_w(160.0))
                .child(
                    row()
                        .key("close")
                        .role("button")
                        .label(t!("common.closeTab"))
                        .size(20.0, 20.0)
                        .center()
                        .radius(4.0)
                        .hover(p.hover)
                        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.close_terminal_tab(&wk_close, &close, tx))
                        .child(icon(&ic::XMARK_SMALL, 14.0).color(p.faint)),
                ),
        )
    });
    let add = row()
        .key("narrow-terminal-new")
        .role("button")
        .label(t!("plugin.terminal.command.new"))
        .help(t!("plugin.terminal.command.new"))
        .help_keys(crate::keymap::TERMINAL_NEW.keycaps())
        .size(28.0, 28.0)
        .center()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.create_terminal_tab(None, tx))
        .child(icon(&ic::PLUS_SMALL, 20.0).color(p.icon));
    let bar = row()
        .key("narrow-terminal-bar")
        .w_full()
        .h(49.0)
        .min_h(49.0)
        .px(12.0)
        .gap(8.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], p.border)
        .child(tabs)
        .child(add);
    let size = rune::memo::layout_with("narrow-terminal-body", |r| (r[2], r[3])).unwrap_or((s.layout.narrow.width() - 2.0, 600.0));
    let body = match (ws.and_then(|w| w.failure.clone()), active.as_deref()) {
        (Some(why), _) => col().key("narrow-terminal-failed").grow(1.0).w_full().center().px(16.0).child(text(why).font(UI).color(p.muted)),
        (None, Some(id)) => grid::render_terminal_pane(s, p, &wk, Some(id), size, 1.0, !s.composer.focused, true),
        (None, None) => col().key("narrow-terminal-loading").grow(1.0).w_full().center().child(text(t!("plugin.terminal.loading")).font(UI).color(p.muted)),
    };
    col()
        .key("narrow-terminal")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(p.bg_base)
        .child(bar)
        .child(el().key("narrow-terminal-body").w_full().grow(1.0).min_h(0.0).child(body))
}

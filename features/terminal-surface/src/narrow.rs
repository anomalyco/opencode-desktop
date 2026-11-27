//! The narrow window's Terminal view (`gui-plugins/src/terminal/panel.tsx` at the `mobile`
//! placement): a 49 pt bar of the session's terminals as panel tabs with `+` after them, over the
//! selected terminal. Shown on a session with none, it starts the first.

use opencode_i18n::t;
use opencode_ui::keymap::TERMINAL_NEW;
use rune::{Color, El, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;
use opencode_ui::palette::Pal;
use opencode_ui::style::{INSTANT, UI};
use opencode_ui::tab::TabExt;

use crate::{TerminalSurfaceHost, grid, ops, side_ops};

fn pick_tab(host: &mut dyn TerminalSurfaceHost, wk: &str, id: &str, tx: &mut Tx) {
    tx.instant();
    ops::select_terminal_tab(host, wk, id);
    ops::focus_terminal_surface(host);
}

fn close_tab(host: &mut dyn TerminalSurfaceHost, wk: &str, id: &str, tx: &mut Tx) {
    ops::close_terminal_tab(host, wk, id, tx);
}

fn new_tab(host: &mut dyn TerminalSurfaceHost, tx: &mut Tx) {
    side_ops::create_terminal_tab(host, None, tx);
}

pub fn view<H: TerminalSurfaceHost + 'static>(s: &H, p: &Pal, sid: &str) -> El {
    s.terminal().painted_dark.set(Some(p.dark));
    let wk = ops::terminal_workspace_key_for_session(s, sid);
    let ws = s.terminal().workspaces.get(&wk);
    let active = ws.and_then(|w| w.active.clone().or_else(|| w.tabs.first().map(|t| t.id.clone())));
    let tabs = ws.map_or(&[][..], |w| w.tabs.as_slice()).iter().fold(
        row().key("narrow-terminal-tabs").gap(8.0).center_y().min_w(0.0).scroll_x(),
        |strip, t| {
            let on = active.as_deref() == Some(t.id.as_str());
            let (pick, close, mid, wk_pick, wk_close, wk_mid) = (t.id.clone(), t.id.clone(), t.id.clone(), wk.clone(), wk.clone(), wk.clone());
            let close_btn = close_button_shell(p)
                .on_press_tx(move |s: &mut H, tx: &mut Tx| close_tab(s, &wk_close, &close, tx));
            let tab = tab_shell(p, &t.id, t.display_title(), on, close_btn)
                .closable_tab(move |s: &mut H, tx: &mut Tx| close_tab(s, &wk_mid, &mid, tx))
                .on_press_tx(move |s: &mut H, tx: &mut Tx| pick_tab(s, &wk_pick, &pick, tx));
            strip.child(tab)
        },
    );
    let add = add_button_shell(p).on_press_tx(|s: &mut H, tx: &mut Tx| new_tab(s, tx));
    let size = rune::memo::layout_with("narrow-terminal-body", |r| (r[2], r[3]))
        .unwrap_or((s.narrow_width() - 2.0, 600.0));
    let body = match (ws.and_then(|w| w.failure.clone()), active.as_deref()) {
        (Some(why), _) => col()
            .key("narrow-terminal-failed")
            .grow(1.0)
            .w_full()
            .center()
            .px(16.0)
            .child(text(why).font(UI).color(p.muted)),
        (None, Some(id)) => grid::render_terminal_pane(s, p, &wk, Some(id), size, 1.0, !s.is_composer_focused(), true),
        (None, None) => col()
            .key("narrow-terminal-loading")
            .grow(1.0)
            .w_full()
            .center()
            .child(text(t!("plugin.terminal.loading")).font(UI).color(p.muted)),
    };
    frame_shell(p, bar_shell(p, tabs, add), body)
}

fn tab_shell(p: &Pal, id: &str, title: String, on: bool, close_btn: El) -> El {
    row()
        .key(format!("narrow-terminal-tab-{id}"))
        .role("tab")
        .label(title.clone())
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
        .child(text(title).font(UI).color(if on { p.text } else { p.muted }).truncate().max_w(160.0))
        .child(close_btn)
}

fn close_button_shell(p: &Pal) -> El {
    row()
        .key("close")
        .role("button")
        .label_static(t!("common.closeTab"))
        .size(20.0, 20.0)
        .center()
        .radius(4.0)
        .hover(p.hover)
        .child(icon(&ic::XMARK_SMALL, 14.0).color(p.faint))
}

fn add_button_shell(p: &Pal) -> El {
    row()
        .key("narrow-terminal-new")
        .role("button")
        .label_static(t!("plugin.terminal.command.new"))
        .help(t!("plugin.terminal.command.new"))
        .help_keys(TERMINAL_NEW.keycaps())
        .size(28.0, 28.0)
        .center()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .child(icon(&ic::PLUS_SMALL, 20.0).color(p.icon))
}

fn bar_shell(p: &Pal, tabs: El, add: El) -> El {
    row()
        .key("narrow-terminal-bar")
        .w_full()
        .h(49.0)
        .min_h(49.0)
        .px(12.0)
        .gap(8.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], p.border)
        .child(tabs)
        .child(add)
}

fn frame_shell(p: &Pal, bar: El, body: El) -> El {
    col()
        .key("narrow-terminal")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(p.bg_base)
        .child(bar)
        .child(el().key("narrow-terminal-body").w_full().grow(1.0).min_h(0.0).child(body))
}

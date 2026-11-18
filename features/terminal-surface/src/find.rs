//! Finding text in a terminal's screen and scrollback: Ctrl+F (⌘F), or Ctrl+Shift+F while a
//! program runs full screen (it keeps Ctrl+F: vim and less page with it). The bar looks like the
//! session's Find bar and sits at the pane's top right; the worker runs libghostty-vt's
//! own search (`crates/terminal/src/find.rs`), so it follows output as it arrives. Enter goes to
//! the next older match, Shift+Enter to the newer one, Escape closes. Desktop's terminal has none.

use rune::{El, Key, KeyEvent, TextEdit, TextStyle, Transition, Tx, icon, row, text};
use rune_theme_opencode::icons as ic;
use session_view::palette::Pal;
use session_view::style::{INSTANT, UI};
use terminal::TerminalSnapshot;

use crate::{TerminalSurfaceHost, TerminalTab, ops};

const COUNT: TextStyle = TextStyle::new(11.0, 440.0).lh(16.0).features("tnum");

/// Opens terminal `tid`'s find bar with the keyboard in it, holding the selection as its query
/// when there's a short one.
pub fn open(host: &mut dyn TerminalSurfaceHost, wk: &str, tid: &str) {
    let Some(t) = ops::terminal_tab_mut(host, wk, tid) else { return };
    let mut field = t.find.take().unwrap_or_default();
    if let Some(sel) = t.session.selected_text().filter(|s| !s.contains('\n') && s.len() <= 200) {
        field = TextEdit::new(sel);
        t.session.find(Some(field.text.clone()));
    }
    field.focused = true;
    t.find = Some(field);
}

fn close(host: &mut dyn TerminalSurfaceHost, wk: &str, tid: &str, tx: &mut Tx) {
    tx.instant();
    if let Some(t) = ops::terminal_tab_mut(host, wk, tid) {
        t.find = None;
        t.session.find(None);
    }
    ops::focus_terminal_surface(host);
}

fn on_change(host: &mut dyn TerminalSurfaceHost, wk: &str, tid: &str, tx: &mut Tx) {
    tx.instant();
    if let Some(t) = ops::terminal_tab_mut(host, wk, tid) {
        let needle = t.find.as_ref().map(|f| f.text.clone());
        t.session.find(needle);
    }
}

fn key(host: &mut dyn TerminalSurfaceHost, wk: &str, tid: &str, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(t) = ops::terminal_tab_mut(host, wk, tid) else { return false };
    if t.find.as_ref().is_some_and(TextEdit::is_composing) {
        return false;
    }
    let m = ev.mods;
    match ev.key {
        Key::Escape => {
            close(host, wk, tid, tx);
            true
        }
        Key::Enter | Key::Down | Key::Up if !m.alt && !m.cmd && !m.ctrl => {
            tx.instant();
            // Older output is up the screen: Enter and Down go there, as from a prompt.
            t.session.find_step(!(m.shift || ev.key == Key::Up));
            true
        }
        _ => false,
    }
}

/// The bar over terminal `tab`, while it's open.
pub fn bar<H: TerminalSurfaceHost + 'static>(
    p: &Pal,
    wk: &str,
    tab: &TerminalTab,
    snap: &TerminalSnapshot,
) -> Option<El> {
    let field = tab.find.as_ref()?;
    let view = snap.find.as_ref();
    let count = (!field.text.is_empty()).then(|| {
        let total = view.map_or(0, |v| v.total);
        let shown = view.and_then(|v| v.selected).map_or(0, |i| i + 1);
        text(format!("{shown}/{total}")).key("terminal-find-count").font(COUNT).color(p.muted)
    });
    let (wk_edit, tid_edit) = (wk.to_owned(), tab.id.clone());
    let (wk_change, tid_change) = (wk.to_owned(), tab.id.clone());
    let (wk_key, tid_key) = (wk.to_owned(), tab.id.clone());
    let (wk_close, tid_close) = (wk.to_owned(), tab.id.clone());
    let input = field_shell(p, field)
        .on_edit(move |s: &mut H| s.terminal_mut().find_field_mut(&wk_edit, &tid_edit))
        .on_input_change(move |s: &mut H, tx: &mut Tx| on_change(s, &wk_change, &tid_change, tx))
        .on_input_key(move |s: &mut H, ev: &KeyEvent, tx: &mut Tx| key(s, &wk_key, &tid_key, ev, tx));
    let close_btn = close_button_shell(p)
        .on_press_tx(move |s: &mut H, tx: &mut Tx| close(s, &wk_close, &tid_close, tx));
    Some(bar_shell(p, field.focused, input, count, close_btn))
}

fn field_shell(p: &Pal, field: &TextEdit) -> El {
    session_view::style::field(field, UI)
        .key("terminal-find-input")
        .single_line()
        .truncate_fade()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .h(20.0)
        .color(p.text)
        .placeholder(opencode_i18n::t!("rune.terminal.find"), p.faint)
        .caret(p.text, p.selection)
        .selection_style(p.field_selection)
}

fn close_button_shell(p: &Pal) -> El {
    row()
        .key("terminal-find-close")
        .role("button")
        .label_static(opencode_i18n::t!("common.close"))
        .size(20.0, 20.0)
        .center()
        .radius(2.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .child(icon(&ic::XMARK_SMALL, 16.0).color(p.icon_muted))
}

fn bar_shell(p: &Pal, focused: bool, input: El, count: Option<El>, close_btn: El) -> El {
    let bar = row().key("terminal-find");
    let bar = if focused { bar.border(1.0, p.v2.border_focus) } else { bar.box_shadow(Pal::ring_out(p.border)) };
    bar.transition(Transition::none())
        .pin(None, Some(8.0), Some(8.0), None)
        .h(28.0)
        .w(220.0)
        .pl(8.0)
        .pr(4.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .fill(p.bg_base)
        .child(
            row()
                .key("terminal-find-value")
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .gap(6.0)
                .center_y()
                .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
                .child(input),
        )
        .children(count)
        .child(close_btn)
}

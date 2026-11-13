//! Finding text in a terminal's screen and scrollback: Ctrl+F (⌘F), or Ctrl+Shift+F while a
//! program runs full screen (it keeps Ctrl+F: vim and less page with it). The bar looks like the
//! session's Find bar (`find.rs`) and sits at the pane's top right; the worker runs libghostty-vt's
//! own search (`crates/terminal/src/find.rs`), so it follows output as it arrives. Enter goes to
//! the next older match, Shift+Enter to the newer one, Escape closes. Desktop's terminal has none.

use rune::{El, Key, KeyEvent, TextEdit, TextStyle, Transition, Tx, icon, row, text};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::palette::Pal;
use crate::view::{INSTANT, UI};

const COUNT: TextStyle = TextStyle::new(11.0, 440.0).lh(16.0).features("tnum");

/// Opens terminal `tid`'s find bar with the keyboard in it, holding the selection as its query
/// when there's a short one.
pub(super) fn open(app: &mut App, wk: &str, tid: &str) {
    let Some(t) = app.terminal_tab_mut(wk, tid) else { return };
    let mut field = t.find.take().unwrap_or_default();
    if let Some(sel) = t.session.selected_text().filter(|s| !s.contains('\n') && s.len() <= 200) {
        field = TextEdit::new(sel);
        t.session.find(Some(field.text.clone()));
    }
    field.focused = true;
    t.find = Some(field);
}

fn close(app: &mut App, wk: &str, tid: &str, tx: &mut Tx) {
    tx.instant();
    if let Some(t) = app.terminal_tab_mut(wk, tid) {
        t.find = None;
        t.session.find(None);
    }
    app.focus_terminal_surface();
}

/// The bar over terminal `tid`, while it's open.
pub(super) fn bar(p: &Pal, wk: &str, tab: &super::TerminalTab, snap: &terminal::TerminalSnapshot) -> Option<El> {
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
    let bar = row().key("terminal-find");
    let bar = if field.focused { bar.border(1.0, p.v2.border_focus) } else { bar.box_shadow(Pal::ring_out(p.border)) };
    Some(
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
                row().key("terminal-find-value").grow(1.0).shrink().min_w(0.0).gap(6.0).center_y().child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted)).child(
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
                        .on_edit(move |s: &mut App| {
                            let super::TerminalPane { workspaces, spare_field, .. } = &mut *s.terminal;
                            match workspaces.get_mut(&wk_edit).and_then(|ws| ws.tabs.iter_mut().find(|t| t.id == tid_edit)).and_then(|t| t.find.as_mut()) {
                                Some(f) => f,
                                None => spare_field,
                            }
                        })
                        .on_input_change(move |s: &mut App, tx: &mut Tx| {
                            tx.instant();
                            if let Some(t) = s.terminal_tab_mut(&wk_change, &tid_change) {
                                let needle = t.find.as_ref().map(|f| f.text.clone());
                                t.session.find(needle);
                            }
                        })
                        .on_input_key(move |s: &mut App, ev: &KeyEvent, tx: &mut Tx| key(s, &wk_key, &tid_key, ev, tx)),
                ),
            )
            .children(count)
            .child(
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
                    .on_press_tx(move |s: &mut App, tx: &mut Tx| close(s, &wk_close, &tid_close, tx))
                    .child(icon(&ic::XMARK_SMALL, 16.0).color(p.icon_muted)),
            ),
    )
}

fn key(s: &mut App, wk: &str, tid: &str, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(t) = s.terminal_tab_mut(wk, tid) else { return false };
    if t.find.as_ref().is_some_and(TextEdit::is_composing) {
        return false;
    }
    let m = ev.mods;
    match ev.key {
        Key::Escape => {
            close(s, wk, tid, tx);
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

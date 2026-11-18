//! The terminal's context menu (right-click on its output, when the program doesn't read the
//! mouse): copy, paste, select all, find, quote into the composer, clear the scrollback. Desktop's
//! terminal has none; every native terminal does.

use opencode_i18n::t;
use rune::{PointerPress, Tx};
use terminal::SelectionOp;

use crate::{ContextMenuEntry, ContextMenuSpec, TerminalSurfaceHost, find, input, ops};

pub fn open(host: &mut dyn TerminalSurfaceHost, wk: &str, tid: &str, press: PointerPress, tx: &mut Tx) {
    tx.instant();
    let Some(selected) = ops::terminal_tab_mut(host, wk, tid).map(|t| t.session.selected_text()) else {
        return;
    };
    let quotable = match selected.as_deref() {
        Some(s) => host.can_quote_terminal_selection(s),
        None => false,
    };
    let spec = ContextMenuSpec {
        label: t!("plugin.terminal.tab.title"),
        entries: vec![
            ContextMenuEntry::Item { id: "copy", label: t!("desktop.menu.copy"), enabled: selected.is_some() },
            ContextMenuEntry::Item { id: "paste", label: t!("desktop.menu.paste"), enabled: true },
            ContextMenuEntry::Item { id: "select-all", label: t!("desktop.menu.selectAll"), enabled: true },
            ContextMenuEntry::Separator,
            ContextMenuEntry::Item { id: "find", label: t!("rune.terminal.find"), enabled: true },
            ContextMenuEntry::Item { id: "quote", label: t!("plugin.terminal.quote.add"), enabled: quotable },
            ContextMenuEntry::Separator,
            ContextMenuEntry::Item { id: "clear", label: t!("common.clear"), enabled: true },
        ],
    };
    let (wk, tid) = (wk.to_owned(), tid.to_owned());
    host.open_context_menu(
        spec,
        press,
        tx,
        Box::new(move |host, pick, tx| match pick {
            Some("copy") => {
                if let Some(t) = ops::terminal_tab_mut(host, &wk, &tid) {
                    input::copy_selection(&t.session, tx);
                }
            }
            Some("paste") => host.read_clipboard_for_paste(wk, tid, tx),
            Some("select-all") => {
                if let Some(t) = ops::terminal_tab_mut(host, &wk, &tid) {
                    t.session.select(SelectionOp::All);
                }
            }
            Some("find") => find::open(host, &wk, &tid),
            Some("quote") => {
                let text = ops::terminal_tab_mut(host, &wk, &tid).and_then(|t| {
                    let text = t.session.selected_text()?;
                    t.session.select(SelectionOp::Clear);
                    Some(text)
                });
                if let Some(text) = text {
                    host.quote_terminal_selection(&text, tx);
                }
            }
            // The screen stays; what scrolled off it goes (ED 3), as Windows Terminal's
            // "Clear buffer" and macOS Terminal's "Clear Scrollback" do.
            Some("clear") => {
                if let Some(t) = ops::terminal_tab_mut(host, &wk, &tid) {
                    t.session.clear_scrollback();
                }
            }
            _ => {}
        }),
    );
}

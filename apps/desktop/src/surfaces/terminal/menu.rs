//! The terminal's context menu (right-click on its output, when the program doesn't read the
//! mouse): copy, paste, select all, find, quote into the composer, clear the scrollback. Desktop's
//! terminal has none; every native terminal does.

use opencode_i18n::t;
use rune::{PointerPress, Tx};
use terminal::SelectionOp;

use crate::App;
use crate::popup::{self, Item, Menu};

pub(super) fn open(app: &mut App, wk: &str, tid: &str, press: PointerPress, tx: &mut Tx) {
    tx.instant();
    let Some(t) = app.terminal_tab_mut(wk, tid) else { return };
    let selected = t.session.selected_text();
    let quotable = selected.as_deref().is_some_and(|s| crate::quote::can_quote(app, s));
    let menu = Menu::new(t!("plugin.terminal.tab.title"))
        .item(Item::new("copy", t!("desktop.menu.copy")).enabled(selected.is_some()))
        .item(Item::new("paste", t!("desktop.menu.paste")))
        .item(Item::new("select-all", t!("desktop.menu.selectAll")))
        .separator()
        .item(Item::new("find", t!("rune.terminal.find")))
        .item(Item::new("quote", t!("plugin.terminal.quote.add")).enabled(quotable))
        .separator()
        .item(Item::new("clear", t!("common.clear")));
    let (wk, tid) = (wk.to_owned(), tid.to_owned());
    popup::open(app, menu, press, tx, move |app, pick, tx| {
        let Some(t) = app.terminal_tab_mut(&wk, &tid) else { return };
        match pick {
            Some("copy") => super::input::copy_selection(&t.session, tx),
            Some("paste") => super::input::paste_into(Some(wk.clone()), Some(tid.clone()), tx),
            Some("select-all") => t.session.select(SelectionOp::All),
            Some("find") => super::find::open(app, &wk, &tid),
            Some("quote") => {
                if let Some(text) = t.session.selected_text() {
                    t.session.select(SelectionOp::Clear);
                    crate::quote::quote_selected(app, &text, tx);
                }
            }
            // The screen stays; what scrolled off it goes (ED 3), as Windows Terminal's
            // "Clear buffer" and macOS Terminal's "Clear Scrollback" do.
            Some("clear") => t.session.clear_scrollback(),
            _ => {}
        }
    });
}

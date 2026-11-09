//! A terminal tab's inline rename (`TerminalTabLabel`, `inline-rename.ts`): a double-click on
//! its title or the label's context menu turns the title into a field over it, Enter or a click
//! away saves it on the server, Escape drops it. An emptied field gives back the shell's title.

use opencode_i18n::t;
use rune::{El, Key, KeyEvent, PointerPress, TextEdit, Tx, text_input};

use crate::App;
use crate::jobs::Apply;
use crate::palette::Pal;
use crate::panels::panel_key;
use crate::popup::{self, Item, Menu};

/// The tab being renamed and its field.
pub struct Renaming {
    ws: String,
    id: String,
    field: TextEdit,
}

/// Turns tab `id`'s title into a field holding it, all of it selected.
pub(super) fn start(s: &mut App, id: &str, tx: &mut Tx) {
    let Some(sid) = s.active_session_id().map(str::to_owned) else { return };
    let ws = s.terminal_workspace_key_for_session(&sid);
    let Some(tab) = s.terminal.workspaces.get(&ws).and_then(|w| w.tabs.iter().find(|t| t.id == id)) else { return };
    if s.terminal.renaming.is_some() {
        return;
    }
    tx.instant();
    let mut field = TextEdit::new(&tab.title);
    field.set_selection(0, field.text.len());
    field.focused = true;
    s.terminal.renaming = Some(Renaming { ws, id: id.to_owned(), field });
}

/// The label's context menu (`Menu.Context`): Rename, and Close.
pub(super) fn menu(s: &mut App, id: &str, press: PointerPress, tx: &mut Tx) {
    tx.instant();
    let menu = Menu::new(t!("plugin.terminal.tab.title")).item(Item::new("rename", t!("common.rename"))).item(Item::new("close", t!("common.close")));
    let id = id.to_owned();
    popup::open(s, menu, press, tx, move |s, pick, tx| match pick {
        Some("rename") => start(s, &id, tx),
        Some("close") => s.close_side_tab(&panel_key(super::side_ops::PLUGIN, &id), tx),
        _ => {}
    });
}

/// The field over tab `id`'s title while it's renamed: from past the glyph to the label's end.
pub(super) fn field(s: &App, p: &Pal, id: &str) -> Option<El> {
    let r = s.terminal.renaming.as_ref().filter(|r| r.id == id)?;
    Some(
        text_input(&r.field, crate::view::UI)
            .key("terminal-rename")
            .label(t!("common.rename"))
            .spellcheck(s.settings.spellcheck)
            .single_line()
            .pin(Some(20.0), Some(0.0), Some(0.0), Some(0.0))
            .px(4.0)
            .radius(4.0)
            .fill(p.layer1)
            .color(p.text)
            .caret(p.text, p.selection).selection_style(p.field_selection)
            .on_edit(|s: &mut App| &mut s.terminal.renaming.as_mut().expect("renaming").field)
            .on_submit(save)
            .on_key(|s: &mut App, ev: &KeyEvent, tx: &mut Tx| {
                if ev.key != Key::Escape {
                    return false;
                }
                tx.instant();
                s.terminal.renaming = None;
                true
            }),
    )
}

/// A click away saves, as the field's blur does.
pub(super) fn poll(s: &mut App, tx: &mut Tx) -> bool {
    if s.terminal.renaming.as_ref().is_some_and(|r| !r.field.focused) {
        save(s, tx);
        return true;
    }
    false
}

/// Sets the terminal's title on the server, showing it at once; a field emptied clears it so
/// the shell's title shows again. The title already shown, or clearing one never set, sends
/// nothing; a failure puts the old title back and says why.
fn save(s: &mut App, tx: &mut Tx) {
    let Some(r) = s.terminal.renaming.take() else { return };
    tx.instant();
    let typed = r.field.text.trim().to_owned();
    let Some(tab) = s.terminal.workspaces.get_mut(&r.ws).and_then(|w| w.tabs.iter_mut().find(|t| t.id == r.id)) else { return };
    let title = (!typed.is_empty()).then_some(typed);
    if title.as_deref() == Some(tab.title.as_str()) || (title.is_none() && !tab.custom_title) {
        return;
    }
    let before = (tab.title.clone(), tab.custom_title);
    if let Some(t) = &title {
        tab.title = t.clone();
        tab.custom_title = true;
    }
    let Some((srv, _)) = s.live_terminals() else { return };
    let (ws, id) = (r.ws, r.id);
    s.job(move |_| {
        let answer = srv.terminal_rename(&id, title.as_deref());
        Box::new(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            let Some(tab) = s.terminal.workspaces.get_mut(&ws).and_then(|w| w.tabs.iter_mut().find(|t| t.id == id)) else { return };
            match answer {
                Ok(info) => {
                    tab.title = info.title;
                    tab.custom_title = info.user_title.is_some();
                }
                Err(e) => {
                    (tab.title, tab.custom_title) = before;
                    crate::status_toast::fail(s, t!("plugin.terminal.rename.failed"), e);
                }
            }
            s.queue_terminal_save();
        }) as Apply
    });
}

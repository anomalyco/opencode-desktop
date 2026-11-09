//! Plugin dialogs (`contrib::DIALOG`, Desktop's `dialogs.open`): one open at a time, centred
//! over the scrim every app dialog uses. The plugin draws the card; Esc or a press outside it
//! closes it and tells the plugin (`EVENT_DISMISS`), and so does the plugin unloading.

use opencode_plugin::abi::{EVENT_DISMISS, SURFACE_DIALOG, contrib as kinds};
use rune::{El, Key, KeyEvent, Tx};

use super::contrib::active;
use super::render::{self, Props};
use crate::App;
use crate::palette::Pal;

/// The open dialog: its plugin's slot and its id.
pub struct Open {
    pub slot: usize,
    pub id: String,
}

/// Opens slot `i`'s dialog `id`, in place of any other.
pub fn open(app: &mut App, i: usize, id: &str, tx: &mut Tx) {
    if !active(&app.plugins, kinds::DIALOG).any(|(s, c)| s == i && c.id == id) {
        return;
    }
    tx.instant();
    crate::popup::dismiss(app, tx);
    super::popover::close(app, tx);
    app.plugins.dialog = Some(Open { slot: i, id: id.to_owned() });
}

/// Closes slot `i`'s dialog, if it's the one open.
pub fn close(app: &mut App, i: usize, tx: &mut Tx) {
    if app.plugins.dialog.as_ref().is_some_and(|d| d.slot == i) {
        tx.instant();
        app.plugins.dialog = None;
    }
}

/// Closes the open dialog as Esc or a press outside it does, telling its plugin.
fn dismiss(app: &mut App, tx: &mut Tx) {
    let Some(open) = app.plugins.dialog.take() else { return };
    tx.instant();
    let found = active(&app.plugins, kinds::DIALOG).find(|(s, c)| *s == open.slot && c.id == open.id).and_then(|(_, c)| Some((c.scope, c.run.as_ref()?.cb)));
    if let Some((scope, cb)) = found {
        super::run::action(app, tx, open.slot, scope, cb, EVENT_DISMISS, &open.id, "");
    }
}

/// Esc closes the open dialog.
pub fn key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key == Key::Escape && app.plugins.luau.dialog.is_some() {
        super::luau::requests::dismiss_dialog(app, tx);
        return true;
    }
    if app.plugins.dialog.is_none() || ev.key != Key::Escape {
        return false;
    }
    dismiss(app, tx);
    true
}

/// The open dialog over its scrim, for the window's top layer.
pub fn view(app: &App, p: &Pal) -> Vec<El> {
    let Some(open) = &app.plugins.dialog else { return super::luau::surfaces::dialog_view(app, p) };
    let Some((i, c)) = active(&app.plugins, kinds::DIALOG).find(|(s, c)| *s == open.slot && c.id == open.id) else { return vec![] };
    let Some(render) = c.render.as_ref() else { return vec![] };
    let Ok(Some(card)) = render::surface(app, i, render, &Props::new(SURFACE_DIALOG, &c.id), p) else { return vec![] };
    let card = rune::col().key(format!("plugin-dialog-{}", c.key)).role("dialog").label(c.title.clone()).modal(true, |s: &mut App| dismiss(s, &mut Tx::default())).child(card);
    crate::widgets::modal("plugin-dialog", p, card, None)
}

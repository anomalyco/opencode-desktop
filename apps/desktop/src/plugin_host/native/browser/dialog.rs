//! `BrowserTab:Dialog` and `BrowserTab:AnswerDialog`: the page's dialog the engine holds
//! (`browser_pane::dialog`), for the pane to show and the agent's `browser_dialog`. Desktop's
//! dialog gate (`packages/gui-plugins/src/browser/dialog-machine.ts`) stops the agent's commands
//! a dialog interrupts; `poll` does so here.
use super::{App, Dep, Lua, Table, cx, id_of, later, with_app};
use browser_pane::dialog::{DialogGate, Input};
use rune::Tx;
use rune::time::Instant;
use rune_luau::Data;

use super::map;

pub(super) fn methods(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.raw_set(
        "Dialog",
        lua.create_function(|lua, this: Table| {
            cx::read(Dep::Browser);
            let id = id_of(&this)?;
            let dialog = with_app("BrowserTab:Dialog", |app| Ok(app.workspace_panes.browser.host.tab(id).map(|t| t.dialog.clone())))?;
            let Some(DialogGate::Open { dialog, repeat, input }) = dialog else { return Ok(mlua::Value::Nil) };
            mlua::IntoLua::into_lua(
                map([
                    ("id", Data::String(dialog.id.to_string())),
                    ("kind", Data::String(dialog.kind.as_str().into())),
                    ("origin", Data::String(dialog.origin)),
                    ("message", Data::String(dialog.message)),
                    ("defaultText", Data::String(dialog.default_text)),
                    ("repeated", Data::Bool(repeat)),
                    ("armed", Data::Bool(input == Input::Live)),
                ]),
                lua,
            )
        })?,
    )?;
    m.raw_set(
        "AnswerDialog",
        lua.create_function(|_, (this, dialog, accept, text, stop): (Table, String, bool, Option<String>, Option<bool>)| {
            let id = id_of(&this)?;
            let Ok(dialog) = dialog.parse::<u64>() else { return Ok(false) };
            // The first answer wins: one for a dialog that already went finds it closed.
            let open = with_app("BrowserTab:AnswerDialog", |app| Ok(app.workspace_panes.browser.host.tab(id).and_then(|t| t.dialog.open()).is_some_and(|d| d.id == dialog)))?;
            if open {
                later(&this, "BrowserTab:AnswerDialog", move |s, id, _| {
                    s.workspace_panes.browser.host.answer_dialog(id, dialog, accept, text.unwrap_or_default(), stop == Some(true));
                })?;
            }
            Ok(open)
        })?,
    )
}

/// Arms the buttons of dialogs that have shown long enough, and fails the DevTools calls of a page
/// a dialog blocks, as Desktop's gate interrupts a running command (`chromium.ts` `runStopError`).
pub(super) fn poll(app: &mut App, tx: &mut Tx, now: Instant) -> bool {
    let (mut changed, wake) = app.workspace_panes.browser.host.tick_dialogs(now);
    if let Some(at) = wake {
        tx.wake_after(at.saturating_duration_since(now));
    }
    let blocked: Vec<u64> = app
        .workspace_panes.browser
        .devtools
        .iter()
        .filter(|(_, (_, _, tab))| app.workspace_panes.browser.host.tab(*tab).is_some_and(|t| t.dialog.open().is_some()))
        .map(|(&request, _)| request)
        .collect();
    for request in blocked {
        if let Some((done, _, tab)) = app.workspace_panes.browser.devtools.remove(&request) {
            done.complete(map([(
                "error",
                Data::String(format!(
                    "[browser.dialog_interrupted] A JavaScript dialog opened while the action was running. Inspect it with browser_dialog({{\"tabID\":{tab},\"action\":\"get\"}}) and accept or dismiss it. Do not repeat the original action just to close the dialog."
                )),
            )]));
            changed = true;
        }
    }
    changed
}

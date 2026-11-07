//! What a handler asked of the app that needs more than its own call: toggling its panels,
//! server requests off the app's thread and saved files, each answering through the callback
//! the plugin passed.

use opencode_plugin::abi::{AbiEvent, AbiStr, EVENT_RESPONSE, EVENT_SAVED};
use rune::Tx;

use super::instance::{CallbackEnv, Scope};
use super::run::{Ran, act};
use crate::App;
use crate::jobs::Apply;

/// A request in flight: whose callback runs when it answers.
pub struct Pending {
    slot: usize,
    generation: u32,
    done: CallbackEnv,
}

/// Opens slot `i`'s panel `panel` in the routed session, or closes it when it shows there
/// (Desktop's `layout.toggle`).
pub fn toggle_panel(app: &mut App, i: usize, panel: &str, tx: &mut Tx) {
    let Some(session) = app.active_session_id().map(str::to_owned) else { return };
    let plugin = app.plugins.slots[i].manifest.id.clone();
    let key = crate::panels::panel_key(super::view::PANEL, &format!("{plugin}.{panel}"));
    tx.instant();
    if super::vtable::panel_state(app, &plugin, panel, &session) == 3 {
        app.close_side_tab(&key, tx);
    } else {
        app.open_side_tab(&key, None, false, false, tx);
    }
}

fn hold(app: &mut App, i: usize, done: CallbackEnv) -> Option<u64> {
    let generation = app.plugins.slots[i].instance.as_ref()?.generation;
    let host = &mut app.plugins;
    host.next_pending += 1;
    host.pending.insert(host.next_pending, Pending { slot: i, generation, done });
    Some(host.next_pending)
}

/// Sends `method path` with `body` to the server off the app's thread; `done` gets the answer.
pub fn server(app: &mut App, i: usize, method: String, path: String, body: String, done: CallbackEnv) {
    let Some(n) = hold(app, i, done) else { return };
    app.job(move |srv| {
        let answer = if method.eq_ignore_ascii_case("GET") {
            srv.get_text(&path)
        } else {
            let body = (!body.is_empty()).then(|| serde_json::from_str(&body).unwrap_or(serde_json::Value::Null));
            srv.call(&method.to_ascii_uppercase(), &path, body).map(|v| v.to_string())
        };
        Box::new(move |s: &mut App, tx: &mut Tx| answer_with(s, n, EVENT_RESPONSE, answer, tx)) as Apply
    });
}

/// Saves `content` as `name` in the Downloads folder; `done` gets its path.
pub fn save(app: &mut App, i: usize, name: &str, content: &str, done: CallbackEnv, tx: &mut Tx) {
    let Some(n) = hold(app, i, done) else { return };
    let saved = if app.persists() { opencode_kit::client::save_download(name, content) } else { Err("This window doesn't save files".to_owned()) };
    answer_with(app, n, EVENT_SAVED, saved, tx);
}

/// Runs request `n`'s callback with its answer, while the plugin generation that asked runs.
fn answer_with(app: &mut App, n: u64, kind: u32, answer: Result<String, String>, tx: &mut Tx) {
    let Some(p) = app.plugins.pending.remove(&n) else { return };
    let Some(func) = p.done.cb.func else { return };
    let (text, extra) = match &answer {
        Ok(body) => (body.as_str(), ""),
        Err(e) => ("", e.as_str()),
    };
    let view = |s: &str| AbiStr { ptr: s.as_ptr(), len: s.len(), _lt: std::marker::PhantomData };
    let event = AbiEvent { kind, mods: u32::from(answer.is_err()), text: view(text), extra: view(extra), ..AbiEvent::default() };
    let data = p.done.cb.data;
    // SAFETY: the plugin's own callback with its data, on the state of the generation that asked;
    // the event's strings outlive the call.
    let ran = act(app, tx, p.slot, Scope::Window, Some(p.generation), |state, ctx| unsafe { func(state, ctx, data, &event) });
    if matches!(ran, Ran::Done(_, true)) {
        tx.instant();
    }
}

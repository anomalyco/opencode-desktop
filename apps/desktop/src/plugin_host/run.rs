//! Calling into plugins: handlers, commands and their keybinds, queries, and `poll`.

use std::ffi::c_void;
use std::sync::atomic::Ordering;

use opencode_plugin::abi::{AbiEvent, AbiOutcome, AbiStatus, AbiStr, Callback, EVENT_COMMAND, EVENT_KEY, EVENT_PRESS};
use rune::{KeyEvent, Tx};

use super::instance::{Press, Scope};
use super::vtable::{ActionCtx, Kind, Request, slot_bit};
use super::{Slot, swap};
use crate::App;

/// The outcome of running a plugin callback.
pub(super) enum Ran<T> {
    Done(AbiStatus, T),
    /// The callback answered with an error (not a panic): the plugin keeps running.
    Erred(String),
    Gone,
}

/// Runs a callback of slot `i`'s `scope` half (generation `generation`, when it must match)
/// with a handler context, then carries out what it asked for. A panic fails the plugin.
pub(super) fn act<T>(app: &mut App, tx: &mut Tx, i: usize, scope: Scope, generation: Option<u32>, call: impl FnOnce(*mut c_void, *mut c_void) -> AbiOutcome<T>) -> Ran<T> {
    let host = &mut app.plugins;
    let Some(slot) = host.slots.get(i) else { return Ran::Gone };
    let Some(inst) = slot.active().filter(|inst| generation.is_none_or(|g| g == inst.generation)) else { return Ran::Gone };
    let state = inst.state_of(scope);
    if state.is_null() {
        return Ran::Gone;
    }
    let mut ctx = ActionCtx {
        kind: Kind::Action,
        plugin_id: &slot.manifest.id,
        lib: &inst.lib,
        editors: &mut host.editors,
        shared: &mut host.shared,
        tx,
        requests: Vec::new(),
        wake_after: None,
    };
    let out = call(state, (&raw mut ctx).cast());
    let (requests, wake_after) = (std::mem::take(&mut ctx.requests), ctx.wake_after);
    match out.into_result() {
        Ok((status, value)) => {
            carry_out(app, i, requests, wake_after, tx);
            Ran::Done(status, value)
        }
        Err((AbiStatus::Panicked, message)) => {
            swap::fail(app, i, message, tx);
            Ran::Gone
        }
        Err((_, message)) => {
            carry_out(app, i, requests, wake_after, tx);
            Ran::Erred(message)
        }
    }
}

/// Runs action `cb` of slot `i`'s `scope` half for an event with `text` and `extra`; false when
/// it declined or is gone.
#[allow(clippy::too_many_arguments)]
pub(super) fn action(app: &mut App, tx: &mut Tx, i: usize, scope: Scope, cb: Callback, kind: u32, text: &str, extra: &str) -> bool {
    let Some(func) = cb.func else { return false };
    // Views of `text` and `extra` for the call: the event type holds `'static` views.
    let view = |s: &str| AbiStr { ptr: s.as_ptr(), len: s.len(), _lt: std::marker::PhantomData };
    let event = AbiEvent { kind, text: view(text), extra: view(extra), ..AbiEvent::default() };
    // SAFETY: the plugin's handler with its own data, on its own state; the event's strings
    // outlive the call.
    let ran = act(app, tx, i, scope, None, |state, ctx| unsafe { func(state, ctx, cb.data, &event) });
    matches!(ran, Ran::Done(AbiStatus::Ok, _))
}

/// Runs query `cb` of slot `i`'s `scope` half on `input`: `Ok(Some(answer))` for yes.
pub(super) fn query(app: &mut App, tx: &mut Tx, i: usize, scope: Scope, cb: opencode_plugin::abi::QueryCallback, input: &str) -> Result<Option<String>, String> {
    let Some(func) = cb.func else { return Ok(None) };
    let mut out = opencode_plugin::abi::AbiOwnedBytes::EMPTY;
    // SAFETY: the plugin's query with its own data and a valid out pointer, on its own state.
    let ran = act(app, tx, i, scope, None, |state, ctx| unsafe { func(state, ctx, cb.data, AbiStr::new(input), &mut out) });
    match ran {
        Ran::Done(_, 0) | Ran::Gone => Ok(None),
        Ran::Done(..) => Ok(Some(out.to_string_lossy())),
        Ran::Erred(e) => Err(e),
    }
}

fn carry_out(app: &mut App, i: usize, requests: Vec<Request>, wake_after: Option<u64>, tx: &mut Tx) {
    if let Some(ms) = wake_after {
        let due = app.clock + ms as f64 / 1000.0;
        if let Some(inst) = app.plugins.slots[i].instance.as_mut() {
            inst.deadline = Some(inst.deadline.map_or(due, |d| d.min(due)));
        }
        app.plugins.next_deadline = Some(app.plugins.next_deadline.map_or(due, |d| d.min(due)));
        tx.wake_after(std::time::Duration::from_millis(ms));
    }
    let id = app.plugins.slots[i].manifest.id.clone();
    for request in requests {
        match request {
            Request::OpenPanel(panel) => {
                let key = crate::panels::panel_key(super::view::PANEL, &format!("{id}.{panel}"));
                app.open_side_tab(&key, None, false, false, tx);
            }
            Request::Toast(text) => crate::status_toast::say(app, text, ""),
            Request::RunCommand(command) => app.run_command(&command, tx),
            Request::OpenLink(href) => app.open_reply_link(&href, tx),
            Request::OpenSettings => {
                tx.instant();
                app.open_settings();
            }
            Request::SetServers(source, list) => {
                if let Some(inst) = app.plugins.slots[i].instance.as_mut() {
                    inst.servers.retain(|s| s.source != source);
                    inst.servers.extend(list);
                }
            }
        }
    }
}

/// An element's press handler, from a plugin's render.
pub(crate) fn press(app: &mut App, tx: &mut Tx, press: &Press) {
    let cb = press.env.cb;
    let Some(func) = cb.func else { return };
    let event = AbiEvent { kind: EVENT_PRESS, ..AbiEvent::default() };
    // SAFETY: the plugin's handler with its own data, on its instance's state.
    let ran = act(app, tx, press.slot, Scope::Window, Some(press.generation), |state, ctx| unsafe { func(state, ctx, cb.data, &event) });
    if matches!(ran, Ran::Done(_, false) | Ran::Gone) {
        tx.live_only();
    }
}

/// Runs plugin command `id` (`<plugin>.<command>`); false when it declined or is gone.
pub fn run_command(app: &mut App, id: &str, tx: &mut Tx) -> bool {
    run_command_as(app, id, EVENT_COMMAND, tx)
}

fn run_command_as(app: &mut App, id: &str, kind: u32, tx: &mut Tx) -> bool {
    let found = app.plugins.slots.iter().enumerate().find_map(|(i, s)| {
        let cmd = s.active()?.contributions.commands.iter().find(|c| c.id == id)?;
        Some((i, cmd.run.cb))
    });
    let Some((i, cb)) = found else { return false };
    action(app, tx, i, Scope::Window, cb, kind, "", "")
}

/// A plugin command's keybind. Chords with Ctrl, Alt or ⌘ fire anywhere; a plain key only
/// outside a focused control, unless the command is `editable`.
pub fn key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let plain = !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt);
    let hit = app.plugins.slots.iter().filter_map(Slot::active).flat_map(|inst| &inst.contributions.commands).find(|c| {
        c.keybind.as_ref().is_some_and(|k| k.matches_tx(ev, tx)) && (!plain || c.editable || tx.focused().is_none())
    });
    let Some(id) = hit.map(|c| c.id) else { return false };
    run_command_as(app, id, EVENT_KEY, tx)
}

/// The palette's (and the composer's `/`) plugin commands.
pub fn commands(app: &App) -> Vec<crate::commands::Command> {
    app.plugins
        .slots
        .iter()
        .filter_map(Slot::active)
        .flat_map(|inst| &inst.contributions.commands)
        .filter(|c| !c.hidden)
        .map(|c| crate::commands::Command {
            id: c.id,
            title: c.title,
            description: c.description,
            slash: c.slash,
            keys: None,
            chord: c.keybind.as_ref().and_then(|k| k.chords().first().copied()),
            run: crate::commands::Run::Plugin(c.id),
        })
        .collect()
}

/// Polls the plugins whose waker fired or whose `wake_after` came due, and swaps in libraries
/// the watcher rebuilt and loaded. At rest: two atomic swaps, one comparison, no call into any
/// plugin.
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    let now = app.clock;
    let host = &mut app.plugins;
    let dirty = host.wake.dirty.swap(0, Ordering::AcqRel);
    let hot = host.wake.hot.swap(0, Ordering::AcqRel);
    let due = host.next_deadline.is_some_and(|d| now >= d);
    if dirty == 0 && hot == 0 && !due {
        return false;
    }
    let mut changed = false;
    for i in 0..app.plugins.slots.len() {
        if hot & slot_bit(i) != 0 {
            let prepared = super::hot::take(&app.plugins.ready, i);
            swap::reload_slot(app, i, prepared, tx);
            changed = true;
        }
    }
    if dirty != 0 || due {
        for i in 0..app.plugins.slots.len() {
            let deadline = app.plugins.slots[i].active().and_then(|inst| inst.deadline);
            if dirty & slot_bit(i) == 0 && deadline.is_none_or(|d| now < d) {
                continue;
            }
            if let Some(inst) = app.plugins.slots[i].instance.as_mut() {
                inst.deadline = None;
            }
            let Some(vtable) = app.plugins.slots[i].active().map(|inst| inst.vtable) else { continue };
            let halves = [(Scope::Window, vtable.poll), (Scope::App, vtable.app_poll)];
            for (scope, poll) in halves {
                let Some(poll) = poll else { continue };
                // SAFETY: the plugin's poll on its own state.
                if let Ran::Done(_, c) = act(app, tx, i, scope, None, |state, ctx| unsafe { poll(state, ctx) }) {
                    changed |= c;
                }
            }
        }
        app.plugins.next_deadline = app.plugins.slots.iter().filter_map(|s| s.active().and_then(|inst| inst.deadline)).min_by(f64::total_cmp);
    }
    changed
}

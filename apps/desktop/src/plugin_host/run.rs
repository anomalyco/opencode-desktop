//! Calling into plugins: handlers, commands and their keybinds, queries, and `poll`.

use std::ffi::c_void;
use std::sync::atomic::Ordering;

use opencode_plugin::abi::{AbiEvent, AbiOutcome, AbiStatus, AbiStr, Callback, EVENT_COMMAND, EVENT_KEY, EVENT_PRESS};
use rune::{KeyEvent, Tx};

use super::instance::{Press, Scope};
use super::vtable::{ActionCtx, HostView, Kind, Request, slot_bit};
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
    if app.plugins.slots.get(i).is_none() {
        return Ran::Gone;
    }
    let view = HostView::read(app, i);
    let host = &mut app.plugins;
    let Some(slot) = host.slots.get(i) else { return Ran::Gone };
    let Some(inst) = slot.active().filter(|inst| generation.is_none_or(|g| g == inst.generation)) else { return Ran::Gone };
    let state = inst.state_of(scope);
    if state.is_null() {
        return Ran::Gone;
    }
    let mut ctx = ActionCtx {
        kind: Kind::Action,
        scope,
        slot: i,
        generation: inst.generation,
        wake: &host.wake,
        plugin_id: &slot.manifest.id,
        lib: &inst.lib,
        editors: &mut host.editors,
        shared: &mut host.shared,
        tx,
        requests: Vec::new(),
        wake_after: None,
        view,
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

/// Runs query `cb` of slot `i`'s window half where the app is only read (listing a panel's
/// tabs): with a read-only context, so it may read its state, stores and the app's facts. A panic or an error
/// answers nothing; the plugin keeps running, and its next handler or poll reports a panic.
pub(super) fn peek(app: &App, i: usize, cb: &opencode_plugin::abi::QueryCallback, input: &str) -> Option<String> {
    let func = cb.func?;
    let inst = app.plugins.slots.get(i)?.active()?;
    let state = inst.state_of(Scope::Window);
    if state.is_null() {
        return None;
    }
    let mut out = opencode_plugin::abi::AbiOwnedBytes::EMPTY;
    let mut ctx = super::vtable::ReadCtx { kind: super::vtable::Kind::Read, plugin_id: &app.plugins.slots[i].manifest.id, shared: &app.plugins.shared };
    // SAFETY: the plugin's query with its own data and a valid out pointer, on its own state,
    // which nothing else touches while the app's thread is here.
    let answer = unsafe { func(state, (&raw mut ctx).cast(), cb.data, AbiStr::new(input), &mut out) };
    match answer.into_result() {
        Ok((_, 1)) => Some(out.to_string_lossy()),
        Ok(_) => None,
        Err((_, message)) => {
            super::tasks::log(format!("plugin {}: listing failed: {message}", app.plugins.slots[i].manifest.id));
            None
        }
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
            Request::Toast(text) => {
                let (title, description) = text.split_once('\n').unwrap_or((&text, ""));
                crate::status_toast::say(app, title, description);
            }
            Request::ToastError(text) => {
                let (title, description) = text.split_once('\n').unwrap_or((&text, ""));
                crate::status_toast::fail(app, title, description);
            }
            Request::TogglePanel(panel) => super::requests::toggle_panel(app, i, &panel, tx),
            Request::TogglePopover { id: popover, anchor } => super::popover::toggle(app, i, &popover, &anchor, tx),
            Request::ClosePopover => super::popover::close(app, tx),
            Request::Invalidate(prefix) => app.plugins.resources.invalidate(&id, &prefix),
            Request::NewDraft { directory, prompt } => {
                tx.instant();
                if !directory.is_empty() {
                    app.default_dir = directory;
                }
                app.close_settings();
                app.open_draft_tab();
                app.composer.replace_text(prompt);
                tx.settle();
            }
            Request::ServerRequest { method, path, body, done, scope } => super::requests::server(app, i, scope, method, path, body, done),
            Request::SaveFile { name, content, done } => super::requests::save(app, i, &name, &content, done, tx),
            Request::RunCommand(command) => app.run_command(&command, tx),
            Request::OpenLink(href) => app.open_reply_link(&href, tx),
            Request::OpenSettings(page) => {
                tx.instant();
                app.open_settings();
                if page == "servers" {
                    app.open_settings_page(crate::settings::pages::Page::Servers, tx);
                } else if !page.is_empty() {
                    // The plugin's own page (`layout.settings(page)`).
                    app.open_settings_page(crate::settings::pages::Page::Plugin(format!("{id}.{page}")), tx);
                }
            }
            Request::SetServers { source, list, ready } => {
                if app.plugins.slots[i].instance.as_mut().is_some_and(|inst| inst.set_servers(&source, list, ready)) {
                    super::servers::changed(app, tx);
                }
            }
            Request::Fetch { request, done, scope } => super::requests::fetch(app, i, scope, request, done),
            Request::OpenMenu { anchor, spec, done, scope } => super::requests::menu(app, i, scope, &anchor, &spec, done, tx),
            Request::OpenDialog(dialog) => super::dialog::open(app, i, &dialog, tx),
            Request::CloseDialog => super::dialog::close(app, i, tx),
            Request::ConnectServer(server) => super::servers::connect(app, &format!("{id}:{server}"), tx),
            Request::OpenPanelInstance { panel, instance } => super::view::open_instance(app, i, &panel, &instance, tx),
            Request::OpenExternal(url) => {
                if app.live() {
                    let _ = tx.open_url(&url);
                }
            }
            Request::Reveal(path) => super::requests::reveal(&path),
            Request::ToastReveal(arg) => {
                let mut f = arg.splitn(5, '\n');
                let mut next = || f.next().unwrap_or_default();
                let (variant, title, description, button, path) = (next(), next(), next(), next(), next());
                if variant == "loading" {
                    crate::status_toast::waiting(app, title);
                } else {
                    crate::status_toast::fail(app, title, description);
                }
                if let Some(e) = app.toast.current_mut() {
                    if variant == "success" {
                        e.icon = Some(&rune_theme_opencode::icons::CIRCLE_CHECK);
                    }
                    if !button.is_empty() && !path.is_empty() {
                        e.action = Some(crate::status_toast::ToastAction::Reveal(button.to_owned(), path.into()));
                    }
                }
            }
            Request::Dev(what) => {
                tx.instant();
                match what.split_once(':') {
                    Some(("speed", x)) => app.knobs.set(&crate::knobs::SPEED, x.parse().unwrap_or(1.0)),
                    Some(("slow", on)) => app.knobs.set(&crate::knobs::SLOW, if on == "on" { 1.0 } else { 0.0 }),
                    _ if what == "knobs" => app.knobs.toggle(),
                    _ if what == "components" => tx.open_playground(),
                    _ => {}
                }
            }
        }
    }
}

/// Queues a server event for the plugins listening for its type (`contrib::EVENTS`), delivered
/// on the next poll; nothing when none listens.
pub fn server_event(app: &mut App, ty: &str, data: &serde_json::Value) {
    super::luau::server_event(app, ty, data);
    let host = &mut app.plugins;
    let bits = super::contrib::active(host, opencode_plugin::abi::contrib::EVENTS).filter(|(_, c)| ty.starts_with(c.target.as_str())).fold(0, |b, (i, _)| b | slot_bit(i));
    if bits == 0 {
        return;
    }
    host.server_events.push((ty.to_owned(), data.to_string()));
    host.wake.wake(bits, false);
}

/// Hands the experiments' knobs (`TEXT_EXPERIMENT`) and the developer tools' state (`TEXT_DEV`)
/// to plugins, waking them when one changed.
fn sync_experiments(app: &mut App) {
    use session_view::knobs::{SOURCES_ENABLED, SOURCES_ICONS};
    let on = |v: f32| v > 0.5;
    let k = &app.knobs;
    let now = (on(k.get(&SOURCES_ENABLED)), on(k.get(&SOURCES_ICONS)), k.get(&crate::knobs::SPEED).to_bits(), on(k.get(&crate::knobs::SLOW)), k.open);
    let facts = &mut app.plugins.shared.facts;
    if facts.synced == Some(now) {
        return;
    }
    facts.synced = Some(now);
    let word = |b: bool| (if b { "on" } else { "off" }).to_owned();
    facts.experiments.insert("sources.enabled".into(), word(now.0));
    facts.experiments.insert("sources.icons".into(), (if now.1 { "site" } else { "letters" }).into());
    facts.dev.insert("speed".into(), format!("{:.2}", f32::from_bits(now.2)));
    facts.dev.insert("slow".into(), word(now.3));
    facts.dev.insert("knobs".into(), word(now.4));
    let all = (0..app.plugins.slots.len()).fold(0, |bits, i| bits | slot_bit(i));
    app.plugins.wake.wake(all, false);
}

fn deliver_events(app: &mut App, tx: &mut Tx) -> bool {
    if app.plugins.server_events.is_empty() {
        return false;
    }
    let mut changed = false;
    for (ty, data) in std::mem::take(&mut app.plugins.server_events) {
        let listeners: Vec<(usize, super::instance::Scope, Callback)> = super::contrib::active(&app.plugins, opencode_plugin::abi::contrib::EVENTS)
            .filter(|(_, c)| ty.starts_with(c.target.as_str()))
            .filter_map(|(i, c)| Some((i, c.scope, c.run.as_ref()?.cb)))
            .collect();
        for (i, scope, cb) in listeners {
            changed |= action(app, tx, i, scope, cb, opencode_plugin::abi::EVENT_SERVER, &ty, &data);
        }
    }
    changed
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

/// An element's hover handler, from a plugin's render: the pointer entered (`on`) or left.
pub(crate) fn hover(app: &mut App, tx: &mut Tx, hover: &Press, on: bool) {
    let cb = hover.env.cb;
    let Some(func) = cb.func else { return };
    let event = AbiEvent { kind: opencode_plugin::abi::EVENT_HOVER, x: if on { 1.0 } else { 0.0 }, ..AbiEvent::default() };
    // SAFETY: as `press`.
    let ran = act(app, tx, hover.slot, Scope::Window, Some(hover.generation), |state, ctx| unsafe { func(state, ctx, cb.data, &event) });
    if matches!(ran, Ran::Done(_, false) | Ran::Gone) {
        tx.live_only();
    }
}

/// Runs plugin command `id` (`<plugin>.<command>`); false when it declined or is gone.
pub fn run_command(app: &mut App, id: &str, tx: &mut Tx) -> bool {
    run_command_as(app, id, EVENT_COMMAND, "", tx) || super::luau::commands::run(app, id, None, tx)
}

/// Runs plugin command `id` from the composer's `/name words` with `words` as its input.
pub fn run_slash(app: &mut App, id: &str, input: &str, tx: &mut Tx) -> bool {
    run_command_as(app, id, EVENT_COMMAND, input, tx) || super::luau::commands::run(app, id, Some(input), tx)
}

/// One plugin command the composer's `/` offers: hidden ones too (`/btw`), while enabled.
pub struct SlashCommand {
    pub id: &'static str,
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// `/name ` waits for its words rather than running when picked.
    pub args: bool,
}

pub fn slash_commands(app: &App) -> Vec<SlashCommand> {
    app.plugins
        .slots
        .iter()
        .filter_map(Slot::active)
        .flat_map(|inst| &inst.contributions.commands)
        .filter(|c| enabled(app, c))
        .filter_map(|c| Some(SlashCommand { id: c.id, name: c.slash?, title: c.title, description: c.description, args: c.slash_args }))
        .chain(super::luau::commands::all(app).into_iter().filter(|c| super::luau::commands::offered(app, c)).filter_map(|c| Some(SlashCommand { id: c.id, name: c.slash?, title: c.title, description: c.description, args: c.slash_args })))
        .collect()
}

fn run_command_as(app: &mut App, id: &str, kind: u32, input: &str, tx: &mut Tx) -> bool {
    let found = app.plugins.slots.iter().enumerate().find_map(|(i, s)| {
        let cmd = s.active()?.contributions.commands.iter().find(|c| c.id == id && enabled(app, c))?;
        Some((i, cmd.run.cb))
    });
    let Some((i, cb)) = found else { return false };
    action(app, tx, i, Scope::Window, cb, kind, input, "")
}

/// A plugin command's keybind. Chords with Ctrl, Alt or ⌘ fire anywhere; a plain key only
/// outside a focused control, unless the command is `editable`.
pub fn key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let plain = !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt);
    let hit = app.plugins.slots.iter().filter_map(Slot::active).flat_map(|inst| &inst.contributions.commands).find(|c| {
        c.keybind.as_ref().is_some_and(|k| k.matches_tx(ev, tx)) && (!plain || c.editable || tx.focused().is_none())
    });
    let Some(id) = hit.map(|c| c.id) else { return super::luau::commands::key(app, ev, tx) };
    run_command_as(app, id, EVENT_KEY, "", tx)
}

/// Whether a plugin command is offered now: one that `needs_session` only over an open session.
fn enabled(app: &App, c: &super::instance::CommandRec) -> bool {
    !c.needs_session || app.active_session_id().is_some()
}

/// The plugins' commands, native and Luau: the ones offered now (`offered`), or every one, as
/// Settings › Shortcuts lists them. Hidden ones come too, marked: the palette leaves them out.
pub fn commands(app: &App, offered: bool) -> Vec<crate::commands::Command> {
    let native = app.plugins.slots.iter().filter_map(Slot::active).flat_map(|inst| &inst.contributions.commands).filter(|c| !offered || enabled(app, c)).map(|c| crate::commands::Command {
        id: c.id,
        title: c.title,
        description: c.description,
        slash: c.slash,
        keys: None,
        chord: c.keybind.as_ref().and_then(|k| k.chords().first().copied()),
        section: c.section,
        hidden: c.hidden,
        run: crate::commands::Run::Plugin(c.id),
    });
    let luau = super::luau::commands::all(app).into_iter().filter(|c| !offered || super::luau::commands::offered(app, c)).map(|c| crate::commands::Command {
        id: c.id,
        title: c.title,
        description: c.description,
        slash: c.slash,
        keys: None,
        chord: c.keybind.as_ref().and_then(|k| k.chords().first().copied()),
        section: c.section,
        hidden: c.hidden,
        run: crate::commands::Run::Plugin(c.id),
    });
    native.chain(luau).collect()
}
/// Polls the plugins whose waker fired or whose `wake_after` came due, and swaps in libraries
/// the watcher rebuilt and loaded. At rest: two atomic swaps, one comparison, no call into any
/// plugin.
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    sync_experiments(app);
    let mut changed = super::luau::poll(app, tx);
    changed |= deliver_events(app, tx);
    let now = app.clock;
    let host = &mut app.plugins;
    let dirty = host.wake.dirty.swap(0, Ordering::AcqRel);
    let hot = host.wake.hot.swap(0, Ordering::AcqRel);
    let due = host.next_deadline.is_some_and(|d| now >= d);
    if dirty == 0 && hot == 0 && !due {
        return changed;
    }
    // Processes' output and exits first: their wake set the same bits.
    changed |= super::process::drain(app, tx);
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

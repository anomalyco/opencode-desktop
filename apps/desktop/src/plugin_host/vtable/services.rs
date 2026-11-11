//! What a plugin reaches of the app beyond drawing: its sessions and the server's model catalog
//! (in a render), server reads the host caches (`resource`), theme tokens, where its panels and
//! popovers are (`host_state`), and server requests and saved files (in a handler, carried out
//! once it returns).

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

use opencode_plugin::abi::*;
use rune::Tx;

use super::super::instance::CallbackEnv;
use super::{ActionCtx, Kind, Request, cast, render, text};
use crate::App;
use crate::panels::tabs::{session_pane_tab, session_tab_pane};
use crate::panels::{SessionPane, panel_key};

/// A view of `s`, valid while the app doesn't change it (for the call it's handed out in).
fn view(s: &str) -> AbiStr<'static> {
    AbiStr { ptr: s.as_ptr(), len: s.len(), _lt: PhantomData }
}

/// One cached server read.
#[derive(Default)]
struct Res {
    state: u32,
    generation: u32,
    body: String,
    /// The version the body answers.
    version: Option<u64>,
    /// The version being read now.
    inflight: Option<u64>,
}

/// Each plugin's cached server reads, by plugin id and path.
#[derive(Default)]
pub struct Resources(RefCell<HashMap<(String, String), Res>>);

impl Resources {
    /// Forgets plugin `plugin`'s reads whose path starts with `prefix`.
    pub fn invalidate(&self, plugin: &str, prefix: &str) {
        self.0.borrow_mut().retain(|(p, path), _| p != plugin || !path.starts_with(prefix));
    }
}

/// What a handler may ask of the app's state, read before it runs.
#[derive(Default)]
pub struct HostView {
    pub popover: Option<String>,
    pub dialog: Option<String>,
    /// The contributed server the window is connected to, as this plugin names it.
    pub current_server: Option<String>,
    pub can_write: bool,
    pub servers: u64,
    pub local: bool,
    pub now_ms: i64,
    /// `(panel, session, state)` of the plugin's panels in the routed session.
    pub panels: Vec<(String, String, u64)>,
    /// The routed session (TEXT_ACTIVE_SESSION).
    pub session: Option<String>,
}

impl HostView {
    pub fn read(app: &App, slot: usize) -> HostView {
        let host = &app.plugins;
        let plugin = &host.slots[slot].manifest.id;
        let session = app.active_session_id().unwrap_or_default();
        let panels = host.slots[slot]
            .active()
            .map(|inst| inst.contributions.panels.iter().map(|p| (p.id.clone(), session.to_owned(), panel_state(app, plugin, &p.id, session))).collect())
            .unwrap_or_default();
        HostView {
            popover: host.popover.as_ref().filter(|o| o.slot == slot).map(|o| o.id.clone()),
            dialog: host.dialog.as_ref().filter(|o| o.slot == slot).map(|o| o.id.clone()),
            current_server: app.server_key().as_deref().and_then(|k| k.strip_prefix(plugin.as_str())?.strip_prefix(':')).map(str::to_owned),
            can_write: app.can_write(),
            servers: servers(app),
            local: local(app),
            now_ms: app.now_ms,
            panels,
            session: active_session(app),
        }
    }

    fn answer(&self, what: u32, arg: &str, arg2: &str) -> u64 {
        match what {
            STATE_PANEL => self.panels.iter().find(|(p, s, _)| p == arg && s == arg2).map_or(0, |p| p.2),
            STATE_POPOVER => u64::from(self.popover.as_deref() == Some(arg)),
            STATE_SERVERS => self.servers,
            STATE_SERVER_LOCAL => u64::from(self.local),
            STATE_NOW_MS => self.now_ms.max(0) as u64,
            STATE_CAN_WRITE => u64::from(self.can_write),
            STATE_DIALOG => u64::from(self.dialog.as_deref() == Some(arg)),
            STATE_SERVER_CURRENT => u64::from(self.current_server.as_deref() == Some(arg)),
            _ => 0,
        }
    }
}

/// The routed session as TEXT_ACTIVE_SESSION describes it.
pub fn active_session(app: &App) -> Option<String> {
    let info = app.session(app.active_session_id()?)?;
    let m = &info.model;
    Some(serde_json::json!({ "id": info.id, "directory": info.directory, "agent": info.agent, "providerID": m.provider, "modelID": m.id, "variant": m.variant }).to_string())
}

/// How many servers the app knows: its own and the contributed ones it lists.
fn servers(app: &App) -> u64 {
    1 + super::super::servers::all(app).iter().filter(|s| s.rec.listed).count() as u64
}

fn local(app: &App) -> bool {
    matches!(&app.source, crate::Source::Live { server, .. } if server.is_local())
}

/// Where plugin `plugin`'s panel `panel` is in session `session`: 0 closed, 1 in the strip,
/// 2 selected in its pane, 3 selected and on screen (Desktop's `layout.state`).
pub fn panel_state(app: &App, plugin: &str, panel: &str, session: &str) -> u64 {
    let key = panel_key(super::super::view::PANEL, &format!("{plugin}.{panel}"));
    let Some(st) = app.side.sessions.get(session) else { return 0 };
    let tabs = &st.tab_state.tabs;
    if !tabs.all.contains(&key) {
        return 0;
    }
    let pane = session_tab_pane(tabs, &key);
    if session_pane_tab(tabs, pane) != Some(key.as_str()) {
        return 1;
    }
    let shown = match pane {
        SessionPane::Bottom => st.bottom_open,
        SessionPane::Side | SessionPane::Split => st.open,
    };
    if shown { 3 } else { 2 }
}

pub(super) unsafe extern "C" fn session(ctx: *mut c_void, id: AbiStr, out: *mut SessionAbi<'static>) -> u8 {
    // SAFETY: the plugin passes its render context and a valid out pointer.
    let (Some(r), Some(out)) = (unsafe { render(ctx) }, unsafe { out.as_mut() }) else { return 0 };
    let app = r.app;
    let Some(info) = app.session(text(id)) else { return 0 };
    let t = app.transcripts.get(&info.id);
    let last = t.and_then(|t| session_view::usage::last_with_usage(t, info.revert.as_deref()));
    let cost = app.session_cost(&info.id);
    let busy = app.subtree_running(&info.id);
    let messages = t.map_or(0, |t| t.messages.len()) as u32;
    let usage = last.map(|(_, k)| TokensAbi { input: k.input, output: k.output, reasoning: k.reasoning, cache_read: k.cache_read, cache_write: k.cache_write });
    let model = last.and_then(|(a, _)| a.model.as_ref());
    let mut h = std::hash::DefaultHasher::new();
    (&info.title, info.updated, cost.to_bits(), messages, busy, usage, last.map(|(a, _)| (&a.id, a.completed))).hash(&mut h);
    let flags = if usage.is_some() { SESSION_HAS_USAGE } else { 0 } | if busy { SESSION_BUSY } else { 0 } | if t.is_some() { SESSION_LOADED } else { 0 };
    *out = SessionAbi {
        id: view(&info.id),
        title: view(&info.title),
        directory: view(&info.directory),
        project_id: view(&info.project),
        parent_id: view(info.parent.as_deref().unwrap_or_default()),
        provider_id: view(&info.model.provider),
        model_id: view(&info.model.id),
        usage_provider_id: view(model.map_or("", |m| m.provider.as_str())),
        usage_model_id: view(model.map_or("", |m| m.id.as_str())),
        created_ms: info.created,
        updated_ms: info.updated,
        cost,
        usage: usage.unwrap_or_default(),
        version: h.finish(),
        messages,
        flags,
    };
    1
}

pub(super) unsafe extern "C" fn model(ctx: *mut c_void, provider: AbiStr, model: AbiStr, out: *mut ModelAbi<'static>) -> u8 {
    // SAFETY: the plugin passes its render context and a valid out pointer.
    let (Some(r), Some(out)) = (unsafe { render(ctx) }, unsafe { out.as_mut() }) else { return 0 };
    let p = &r.app.pickers;
    let (provider, model) = (text(provider), text(model));
    let key = format!("{provider}:{model}");
    let listed = p.models.iter().find(|m| m.provider == provider && m.id == model);
    let provider_name = p.providers.get(provider).map(String::as_str).or_else(|| p.models.iter().any(|m| m.provider == provider).then_some(provider));
    let flags = if listed.is_some() { MODEL_KNOWN } else { 0 } | if provider_name.is_some() { MODEL_PROVIDER_KNOWN } else { 0 } | if p.paid.contains(&key) { MODEL_PAID } else { 0 };
    *out = ModelAbi {
        provider_name: view(provider_name.unwrap_or_default()),
        name: view(listed.map_or("", |m| m.name.as_str())),
        context_limit: p.limits.get(&key).copied().unwrap_or(0),
        flags,
        _pad: 0,
    };
    1
}

pub(super) unsafe extern "C" fn resource(ctx: *mut c_void, path: AbiStr, version: u64, out: *mut ResourceAbi<'static>) {
    // SAFETY: the plugin passes its render context and a valid out pointer.
    let (Some(r), Some(out)) = (unsafe { render(ctx) }, unsafe { out.as_mut() }) else { return };
    let (plugin, path) = (r.build.plugin.to_owned(), text(path).to_owned());
    let mut map = r.app.plugins.resources.0.borrow_mut();
    let e = map.entry((plugin.clone(), path.clone())).or_default();
    if e.version != Some(version) && e.inflight != Some(version) {
        e.inflight = Some(version);
        r.app.job(move |srv| {
            let answer = srv.get_text(&path);
            Box::new(move |s: &mut App, tx: &mut Tx| land(s, &plugin, &path, version, answer, tx)) as crate::jobs::Apply
        });
    }
    let state = if e.version == Some(version) { e.state } else { RESOURCE_LOADING };
    // The body lives in the cache until an answer lands, which only happens between frames.
    *out = ResourceAbi { state, generation: e.generation, body: view(&e.body) };
}

fn land(app: &mut App, plugin: &str, path: &str, version: u64, answer: Result<String, String>, tx: &mut Tx) {
    let mut map = app.plugins.resources.0.borrow_mut();
    let Some(e) = map.get_mut(&(plugin.to_owned(), path.to_owned())) else { return };
    if e.inflight != Some(version) {
        return;
    }
    tx.instant();
    e.inflight = None;
    e.version = Some(version);
    e.generation += 1;
    (e.state, e.body) = match answer {
        Ok(body) => (RESOURCE_READY, body),
        Err(error) => (RESOURCE_FAILED, error),
    };
}

pub(super) unsafe extern "C" fn token(ctx: *mut c_void, name: AbiStr, out: *mut Rgba) -> u8 {
    // SAFETY: the plugin passes its render context and a valid out pointer.
    let (Some(r), Some(out)) = (unsafe { render(ctx) }, unsafe { out.as_mut() }) else { return 0 };
    let Some(c) = session_view::palette::token(r.pal.dark != 0, text(name)) else { return 0 };
    *out = [c.r, c.g, c.b, c.a];
    1
}

pub(super) unsafe extern "C" fn host_state(ctx: *mut c_void, what: u32, arg: AbiStr, arg2: AbiStr) -> u64 {
    if what == STATE_UTC_OFFSET {
        let ms = text(arg).parse::<i64>().unwrap_or(0);
        return rune::LocalTime::at(ms).offset as i64 as u64;
    }
    // SAFETY: the plugin passes its context.
    if let Some(r) = unsafe { render(ctx) } {
        let app = r.app;
        let (arg, arg2) = (text(arg), text(arg2));
        return match what {
            STATE_PANEL => panel_state(app, r.build.plugin, arg, arg2),
            STATE_POPOVER => u64::from(app.plugins.popover.as_ref().is_some_and(|o| o.slot == r.build.slot && o.id == arg)),
            STATE_SERVERS => servers(app),
            STATE_SERVER_LOCAL => u64::from(local(app)),
            STATE_NOW_MS => app.now_ms.max(0) as u64,
            STATE_CAN_WRITE => u64::from(app.can_write()),
            STATE_DIALOG => u64::from(app.plugins.dialog.as_ref().is_some_and(|o| o.slot == r.build.slot && o.id == arg)),
            STATE_SERVER_CURRENT => u64::from(app.server_key().as_deref() == Some(format!("{}:{arg}", r.build.plugin).as_str())),
            _ => 0,
        };
    }
    // SAFETY: as above.
    match unsafe { cast::<ActionCtx>(ctx, Kind::Action) } {
        Some(a) => a.view.answer(what, text(arg), text(arg2)),
        None => 0,
    }
}

pub(super) unsafe extern "C" fn server_request(ctx: *mut c_void, method: AbiStr, path: AbiStr, body: AbiStr, done: Callback) -> AbiStatus {
    // SAFETY: the plugin passes its handler context.
    let Some(a) = (unsafe { cast::<ActionCtx>(ctx, Kind::Action) }) else { return AbiStatus::Err };
    if done.func.is_none() || text(path).is_empty() {
        return AbiStatus::Err;
    }
    let done = CallbackEnv::new(done, a.lib.clone());
    a.requests.push(Request::ServerRequest { method: text(method).to_owned(), path: text(path).to_owned(), body: text(body).to_owned(), done, scope: a.scope });
    AbiStatus::Ok
}

pub(super) unsafe extern "C" fn save_file(ctx: *mut c_void, name: AbiStr, content: AbiStr, done: Callback) -> AbiStatus {
    // SAFETY: the plugin passes its handler context.
    let Some(a) = (unsafe { cast::<ActionCtx>(ctx, Kind::Action) }) else { return AbiStatus::Err };
    if done.func.is_none() || text(name).is_empty() {
        return AbiStatus::Err;
    }
    let done = CallbackEnv::new(done, a.lib.clone());
    a.requests.push(Request::SaveFile { name: text(name).to_owned(), content: text(content).to_owned(), done });
    AbiStatus::Ok
}

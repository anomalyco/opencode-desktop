//! Sessions, Models, Server (its cached reads and events; requests are in `net`), Servers and
//! Theme: what a render reads of the app's data.

use std::hash::{Hash, Hasher};
use std::rc::Rc;

use mlua::{Lua, Table, Value};
use rune::Tx;

use super::{app_now, json_to_lua, lazy_signals, method, record, snapshot};
use crate::App;
use crate::plugin_host::cx::{self, Dep};
use crate::plugin_host::machines::{Fetched, Item, Items, Ticket, TicketEffect, TicketEvent};
use crate::plugin_host::plugin::Resource;
use crate::plugin_host::vm::Vmx;

pub fn build(lua: &Lua, vmx: &Rc<Vmx>, name: &str, t: &Table) -> mlua::Result<()> {
    match name {
        "Sessions" => sessions(lua, vmx, t),
        "Models" => models(lua, vmx, t),
        "Server" => server(lua, vmx, t),
        "Servers" => servers(lua, vmx, t),
        _ => theme(lua, vmx, t),
    }
}

fn sessions(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    method(lua, t, "Get", |lua, id: String| {
        cx::read(Dep::Session(id.clone()));
        session_value(lua, app_now("Sessions:Get")?, &id)
    })?;
    let window = vmx.half.kind == crate::plugin_host::plugin::Kind::Window;
    method(lua, t, "Routed", move |lua, ()| {
        cx::read(Dep::Routed);
        let app = app_now("Sessions:Routed")?;
        match app.active_session_id().filter(|_| window) {
            Some(id) => {
                cx::read(Dep::Session(id.to_owned()));
                session_value(lua, app, id)
            }
            None => Ok(Value::Nil),
        }
    })?;
    method(lua, t, "Open", |lua, ()| {
        cx::read(Dep::Sessions);
        let app = app_now("Sessions:Open")?;
        let list = lua.create_table()?;
        let active = crate::servers::runtime::active(app).clone();
        for tab in app.tabs.iter().filter(|t| t.kind == crate::tabs::TabKind::Session) {
            // A tab of another server is what the window last read of it (its title and folder,
            // its mark): the window works with one server at a time.
            let value = if tab.server == active { session_value(lua, app, &tab.id)? } else { other_server_session(lua, app, tab)? };
            if value != Value::Nil {
                list.raw_push(value)?;
            }
        }
        Ok(list)
    })?;
    lazy_signals(lua, vmx, t, "Sessions", &["Changed", "RoutedChanged"])
}

/// A tracked version belongs to one cell, not to all servers or windows at that version.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Stamp(pub u32, pub u64);

impl Stamp {
    pub(super) fn of<T>(cell: &rune::Tracked<T>) -> Self {
        Self(rune::Tracked::id(cell), rune::Tracked::version(cell))
    }
}

#[derive(PartialEq, Eq)]
struct UsageInputs {
    server: opencode_kit::servers::ServerKey,
    transcripts: Stamp,
    window: Stamp,
    revert: Option<String>,
}

thread_local! {
    /// Each session's step with usage, by its place, and what it was read from: the transcripts'
    /// and the context windows' versions and the staged revert. Every poll asks; a long session
    /// without usage would be read through each time.
    static LAST_USAGE: std::cell::RefCell<std::collections::HashMap<String, (UsageInputs, Option<usize>)>> = Default::default();
}

/// Session `id`'s step whose tokens fill the context window (`session_view::usage`).
fn last_usage<'a>(app: &'a App, id: &str) -> Option<(&'a opencode_kit::model::Assistant, &'a opencode_kit::model::Tokens)> {
    let info = app.session(id)?;
    let t = rune::Tracked::untracked(&app.transcripts).get(&info.id)?;
    let key = UsageInputs {
        server: crate::servers::runtime::active(app).clone(),
        transcripts: Stamp::of(&app.transcripts),
        window: Stamp::of(&app.context_windows),
        revert: info.revert.clone(),
    };
    let at = LAST_USAGE.with_borrow_mut(|kept| match kept.get(id) {
        Some((k, at)) if *k == key => *at,
        _ => {
            let at = session_view::usage::last_with_usage_at(t, info.revert.as_deref(), |m| app.holds(id, m));
            kept.insert(id.to_owned(), (key, at));
            at
        }
    });
    session_view::usage::at(t, at?)
}

/// A session's version, as plugins see it: it moves when its title, cost, usage, message count
/// or busy state does, and not while a reply streams within a step.
/// `watch::SessionInputs` tracks the shared inputs. Only this session reads its transcript.
pub fn session_version(app: &App, id: &str) -> u64 {
    let Some(info) = app.session(id) else { return 0 };
    let t = rune::Tracked::untracked(&app.transcripts).get(&info.id);
    let last = last_usage(app, id);
    let cost = app.session_cost(id);
    let messages = t.map(|t| t.messages.len());
    let usage = last.map(|(_, k)| (k.input, k.output, k.reasoning, k.cache_read, k.cache_write));
    let mut h = std::hash::DefaultHasher::new();
    (&info.title, info.updated, cost.to_bits(), messages, app.subtree_running(&info.id), usage, last.map(|(a, _)| (&a.id, a.completed)), (&info.model.provider, &info.model.id, &info.model.variant, &info.agent, &info.project, app.held(id))).hash(&mut h);
    // The Context tab lists the inbox's waiting input too.
    messages_version(app, id).hash(&mut h);
    h.finish()
}

/// What moves when session `id`'s newest messages or its inbox do, and not as Rune reads older
/// pages or the reader scrolls: its newest message and whether it finished, the inbox's waiting
/// input, and the server's last change.
fn messages_version(app: &App, id: &str) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    app.session(id).map(|i| i.updated).hash(&mut h);
    if let Some(m) = rune::Tracked::untracked(&app.transcripts).get(id).and_then(|t| t.messages.last()) {
        m.id().hash(&mut h);
        if let opencode_kit::model::Message::Assistant(a) = m {
            a.completed.hash(&mut h);
        }
    }
    if let Some(inbox) = app.inboxes.get(id) {
        inbox.items.iter().for_each(|i| i.id.hash(&mut h));
        inbox.waiting_notices().for_each(|n| n.id.hash(&mut h));
    }
    h.finish()
}

/// Session `id` as a `Session` record, kept per VM while its version and the server hold.
fn session_value(lua: &Lua, app: &App, id: &str) -> mlua::Result<Value> {
    let mut h = std::hash::DefaultHasher::new();
    (session_version(app, id), &app.server_key()).hash(&mut h);
    snapshot(lua, &format!("session:{}:{id}", server_id(crate::servers::runtime::active(app))), h.finish(), || build_session(lua, app, id))
}

/// A server's key as plugins name it (`ServerInfo.key`): `local` for the app's own (its
/// `ServerScope`), else its key.
pub fn server_id(key: &opencode_kit::servers::ServerKey) -> String {
    key.scope().as_str().to_owned()
}

/// Tab `tab`'s session on a server the window isn't working with: its saved title and folder,
/// busy while its mark says so.
fn other_server_session(lua: &Lua, app: &App, tab: &crate::tabs::Tab) -> mlua::Result<Value> {
    let busy = app.checkout.snap(&tab.server, &tab.id) == Some(crate::TabStatus::Busy);
    let mut h = std::hash::DefaultHasher::new();
    (&tab.title, &tab.directory, busy).hash(&mut h);
    let version = h.finish();
    snapshot(lua, &format!("session:{}:{}", tab.server, tab.id), version, || {
        record(lua, 6, |s| {
            s.str(c"id", &tab.id);
            s.str(c"serverId", &server_id(&tab.server));
            s.str(c"directory", &tab.directory);
            s.str(c"title", &tab.title);
            s.bool(c"busy", busy);
            s.num(c"version", (version >> 11) as f64);
            s.num(c"cost", 0.0);
        })
        .map(Value::Table)
    })
}

fn build_session(lua: &Lua, app: &App, id: &str) -> mlua::Result<Value> {
    let Some(info) = app.session(id) else { return Ok(Value::Nil) };
    let t = rune::Tracked::untracked(&app.transcripts).get(&info.id);
    let last = last_usage(app, id);
    let busy = app.subtree_running(&info.id);
    let version = (session_version(app, id) >> 11) as f64;
    let cost = app.session_cost(id);
    let held = app.held(id);
    let s = record(lua, 24, |s| {
        s.str(c"id", &info.id);
        s.str(c"serverId", &server_id(crate::servers::runtime::active(app)));
        s.str(c"directory", &info.directory);
        s.str(c"title", &info.title);
        s.opt_str(c"parentId", info.parent.as_deref());
        s.opt_str(c"projectId", (!info.project.is_empty()).then_some(info.project.as_str()));
        s.opt_str(c"agent", (!info.agent.is_empty()).then_some(info.agent.as_str()));
        s.opt_str(c"variant", (!info.model.variant.is_empty() && info.model.variant != "default").then_some(info.model.variant.as_str()));
        s.bool(c"busy", busy);
        s.num(c"version", version);
        s.num(c"messagesVersion", (messages_version(app, &info.id) >> 11) as f64);
        s.num(c"cost", cost);
        if let Some((a, k)) = last {
            s.record(c"usage", 5, |u| {
                u.num(c"input", k.input as f64);
                u.num(c"output", k.output as f64);
                u.num(c"reasoning", k.reasoning as f64);
                u.num(c"cacheRead", k.cache_read as f64);
                u.num(c"cacheWrite", k.cache_write as f64);
            });
            s.str(c"usageMessageId", &a.id);
            if a.created > 0 {
                s.num(c"usageCreated", a.created as f64);
            }
            if let Some(m) = a.model.as_ref() {
                s.str(c"usageProviderId", &m.provider);
                s.str(c"usageModelId", &m.id);
            }
        }
        if !info.model.id.is_empty() {
            s.str(c"providerId", &info.model.provider);
            s.str(c"modelId", &info.model.id);
        }
        match held {
            Some(crate::context_window::Held::From(start)) => s.str(c"windowStart", start),
            Some(crate::context_window::Held::Unknown) => s.str(c"windowStart", ""),
            None => {}
        }
        s.num(c"created", info.created as f64);
        s.num(c"updated", info.updated as f64);
        if let Some(t) = t {
            s.num(c"messages", t.messages.len() as f64);
        }
    })?;
    Ok(Value::Table(s))
}

/// What a model's record reads now, as a number.
pub fn model_version(app: &App, provider: &str, model: &str) -> u64 {
    let p = &app.pickers;
    let key = format!("{provider}:{model}");
    let mut h = std::hash::DefaultHasher::new();
    (p.models.iter().find(|m| m.provider == provider && m.id == model).map(|m| &m.name), p.providers.get(provider), p.limits.get(&key), p.paid.contains(&key)).hash(&mut h);
    h.finish()
}

fn models(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    method(lua, t, "Get", |lua, (provider, model): (String, String)| {
        let app = app_now("Models:Get")?;
        let key = format!("{provider}:{model}");
        let mut h = std::hash::DefaultHasher::new();
        (model_version(app, &provider, &model), app.pickers.models.iter().any(|m| m.provider == provider)).hash(&mut h);
        // Each server has its own catalog.
        let m = snapshot(lua, &format!("model:{}:{key}", server_id(crate::servers::runtime::active(app))), h.finish(), || {
            let p = &app.pickers;
            let listed = p.models.iter().find(|m| m.provider == provider && m.id == model);
            let provider_name = p.providers.get(&provider).map(String::as_str).or_else(|| p.models.iter().any(|m| m.provider == provider).then_some(provider.as_str()));
            record(lua, 4, |m| {
                m.opt_str(c"providerName", provider_name);
                m.opt_str(c"modelName", listed.map(|m| m.name.as_str()));
                if let Some(l) = p.limits.get(&key) {
                    m.num(c"contextLimit", *l as f64);
                }
                m.bool(c"paid", p.paid.contains(&key));
            })
            .map(Value::Table)
        })?;
        cx::read(Dep::Model(provider, model));
        Ok(m)
    })?;
    lazy_signals(lua, vmx, t, "Models", &["Changed"])
}

fn server(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    let plugin = vmx.half.id.clone();
    let half = vmx.half.clone();
    method(lua, t, "Resource", move |lua, (path, version, items): (String, Value, Option<bool>)| {
        let items = items == Some(true);
        let app = app_now("Server:Resource")?;
        // Each server's reads are cached apart: a panel in a tab from another server reads that
        // server's answer, never this one's.
        let key = resource_key(crate::servers::runtime::active(app), &path);
        cx::read(Dep::Resource(plugin.clone(), key.clone()));
        let version = version_hash(&version);
        let mut map = half.resources.borrow_mut();
        let r = map.entry(key.clone()).or_default();
        let (ticket, effects) = std::mem::take(&mut r.ticket).step(TicketEvent::Ask(version));
        r.ticket = ticket;
        for TicketEffect::Fetch(asked) in effects {
            let (plugin, path, key) = (plugin.to_string(), path.clone(), key.clone());
            app.job(move |srv| {
                // A server still loading its plugins answers `/api/skill` with 503 for a while.
                let answer = srv.get_text_ready(&path).map(|text| {
                    // Split off the UI thread: a long list's JSON is too much to decode in a frame.
                    let items = if items { split_items(&text) } else { None };
                    Fetched { text, items }
                }).map_err(|e| e.to_string());
                Box::new(move |s: &mut App, tx: &mut Tx| land(s, &plugin, &key, asked, answer, tx)) as crate::jobs::Apply
            });
        }
        resource_value(lua, r)
    })?;
    let half = vmx.half.clone();
    method(lua, t, "Invalidate", move |_, prefix: String| {
        half.resources.borrow_mut().retain(|key, _| !key.split_once('\n').map_or(key.as_str(), |(_, path)| path).starts_with(&prefix));
        crate::plugin_host::bump();
        Ok(())
    })?;
    let v = vmx.clone();
    method(lua, t, "OnEvent", move |lua, prefix: String| {
        let s: Value = v.script_signal.call(())?;
        v.half.events.borrow_mut().push((v.vm.get(), prefix, s.clone()));
        let _ = lua;
        Ok(s)
    })?;
    method(lua, t, "EncodeUri", |_, text: String| Ok(encode_uri(&text)))?;
    // The window half's server is the one it works with now; the app half's, the app's own.
    let window = vmx.half.kind == crate::plugin_host::plugin::Kind::Window;
    method(lua, t, "Version", move |_, ()| {
        cx::read(Dep::Server);
        cx::read(Dep::Servers);
        let app = app_now("Server:Version")?;
        let own = app.plugins.shared.facts.server_version.clone();
        let active = crate::servers::runtime::active(app);
        if !window || active.is_sidecar() {
            return Ok(own);
        }
        Ok(opencode_kit::servers::health::version(crate::servers::health::get(active).as_ref()).map(str::to_owned))
    })?;
    method(lua, t, "IsLocal", move |_, ()| {
        cx::read(Dep::Server);
        let app = app_now("Server:IsLocal")?;
        if !window {
            return Ok(crate::servers::runtime::own_server().is_some_and(|s| s.is_local()));
        }
        Ok(matches!(&*app.source, crate::Source::Live { server, .. } if server.is_local()))
    })?;
    method(lua, t, "CanWrite", |_, ()| {
        cx::read(Dep::Server);
        Ok(app_now("Server:CanWrite")?.can_write())
    })?;
    super::async_method(lua, vmx, t, "RequestAsync", "Server.RequestAsync")?;
    lazy_signals(lua, vmx, t, "Server", &["Connected"])
}

fn resource_value(lua: &Lua, r: &mut Resource) -> mlua::Result<Value> {
    let (status, error) = match &r.ticket {
        Ticket::Empty | Ticket::Loading { .. } => ("loading", None),
        Ticket::Ready { .. } => ("ready", None),
        Ticket::Failed { error, .. } => ("error", Some(error.as_str())),
    };
    let generation = r.ticket.body().map_or(0, |b| b.generation);
    let out = record(lua, 5, |o| {
        o.str(c"status", status);
        o.opt_str(c"error", error);
        o.num(c"generation", generation as f64);
    })?;
    let Some(body) = r.ticket.body() else {
        out.raw_set("text", "")?;
        return Ok(Value::Table(out));
    };
    // The answer's text and JSON (or its items) become Luau values once per answer, not once per
    // read.
    let (text, decoded) = match &r.value {
        Some((g, text, decoded)) if *g == body.generation => (lua.registry_value::<Value>(text)?, lua.registry_value::<Value>(decoded)?),
        _ => {
            let text = Value::String(lua.create_string(&*body.text)?);
            // A split answer keeps its items and the rest of it apart: `{ items, json }`.
            let decoded = match &body.items {
                Some(items) => {
                    let both = lua.create_table_with_capacity(2, 0)?;
                    both.raw_push(items_value(lua, &items.list)?)?;
                    both.raw_push(json_to_lua(lua, &items.rest)?)?;
                    Value::Table(both)
                }
                None => match serde_json::from_str::<serde_json::Value>(&body.text) {
                    Ok(v) => json_to_lua(lua, &v)?,
                    Err(_) => Value::Nil,
                },
            };
            r.value = Some((body.generation, Rc::new(lua.create_registry_value(text.clone())?), Rc::new(lua.create_registry_value(decoded.clone())?)));
            (text, decoded)
        }
    };
    out.raw_set("text", text)?;
    match (&body.items, decoded) {
        (Some(_), Value::Table(both)) => {
            out.raw_set("items", both.raw_get::<Value>(1)?)?;
            out.raw_set("json", both.raw_get::<Value>(2)?)?;
        }
        (_, decoded) => out.raw_set("json", decoded)?,
    }
    Ok(Value::Table(out))
}

/// An answer's listed items (`{ "data": [...] }` or `[...]`), each with its `id`, `type`,
/// `time.created` and JSON, and the rest of the answer; nil for an answer that lists none.
fn split_items(text: &str) -> Option<Items> {
    let mut v: serde_json::Value = serde_json::from_str(text).ok()?;
    let list = match &mut v {
        serde_json::Value::Array(list) => std::mem::take(list),
        serde_json::Value::Object(o) => match o.shift_remove("data") {
            Some(serde_json::Value::Array(list)) => list,
            _ => return None,
        },
        _ => return None,
    };
    let list = list.iter().map(|m| Item {
        id: m["id"].as_str().unwrap_or("").to_owned(),
        kind: m["type"].as_str().unwrap_or("").to_owned(),
        created: m["time"]["created"].as_f64(),
        text: m.to_string(),
    }).collect();
    Some(Items { list, rest: v })
}

fn items_value(lua: &Lua, items: &[Item]) -> mlua::Result<Value> {
    let list = lua.create_table_with_capacity(items.len(), 0)?;
    for item in items {
        list.raw_push(record(lua, 4, |e| {
            e.str(c"id", &item.id);
            e.str(c"kind", &item.kind);
            if let Some(at) = item.created {
                e.num(c"created", at);
            }
            e.str(c"text", &item.text);
        })?)?;
    }
    Ok(Value::Table(list))
}

/// A cached read's key: its server's, then its path (`(ServerKey, path)`).
fn resource_key(server: &opencode_kit::servers::ServerKey, path: &str) -> String {
    format!("{server}\n{path}")
}

fn land(app: &mut App, plugin: &str, key: &str, version: u64, answer: Result<Fetched, String>, tx: &mut Tx) {
    let Some(half) = app.plugins.half(plugin) else { return };
    let mut map = half.resources.borrow_mut();
    let Some(r) = map.get_mut(key) else { return };
    let before = r.ticket.generation();
    let (ticket, _) = std::mem::take(&mut r.ticket).step(TicketEvent::Land(version, answer));
    r.ticket = ticket;
    if r.ticket.generation() != before {
        tx.instant();
    }
}
/// A memo version off a Luau value.
pub fn version_hash(v: &Value) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    match v {
        Value::Boolean(b) => (1u8, b).hash(&mut h),
        Value::Integer(i) => (2u8, (*i as f64).to_bits()).hash(&mut h),
        Value::Number(n) => (2u8, n.to_bits()).hash(&mut h),
        Value::String(s) => (3u8, s.as_bytes().to_vec()).hash(&mut h),
        _ => 0u8.hash(&mut h),
    }
    h.finish()
}

/// `encodeURIComponent`.
pub fn encode_uri(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.extend(['%', HEX[usize::from(b >> 4)] as char, HEX[usize::from(b & 15)] as char]);
        }
    }
    out
}

fn servers(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    let window = vmx.half.kind == crate::plugin_host::plugin::Kind::Window;
    method(lua, t, "List", move |lua, ()| {
        cx::read(Dep::Server);
        let app = app_now("Servers:List")?;
        cx::read(Dep::Servers);
        let active = crate::servers::runtime::active(app).clone();
        let own = crate::servers::runtime::own_server().map(|s| s.base);
        // (key, name, label, state, local, current, version) of each server the app lists: its
        // own, the plugins' (ready or not) and those added by address.
        let listed: Vec<(String, String, Option<String>, &'static str, bool, bool, Option<String>)> = crate::settings::pages::servers::inventory::list(app)
            .into_iter()
            .filter(|x| !matches!(&x.kind, crate::settings::pages::servers::inventory::Kind::Plugin(rec) if !rec.listed))
            .map(|x| {
                use crate::settings::pages::servers::inventory::Kind;
                let state = match &x.kind {
                    Kind::Plugin(rec) => ["stopped", "starting", "auth", "ready", "failed", "incompatible"].get(rec.state as usize).copied().unwrap_or("stopped"),
                    _ => "ready",
                };
                let label = match &x.kind {
                    Kind::Plugin(rec) if !rec.label.is_empty() => Some(rec.label.clone()),
                    _ => None,
                };
                let local = x.key.is_sidecar() || x.connection.as_ref().is_some_and(|c| c.local(own.as_deref()));
                let version = opencode_kit::servers::health::version(x.health.as_ref()).map(str::to_owned);
                (server_id(&x.key), x.name.clone(), label, state, local, window && x.key == active, version)
            })
            .collect();
        let mut h = std::hash::DefaultHasher::new();
        listed.hash(&mut h);
        snapshot(lua, "servers", h.finish(), || {
            let list = lua.create_table_with_capacity(listed.len(), 0)?;
            for (key, name, label, state, local, current, version) in &listed {
                list.raw_push(record(lua, 7, |e| {
                    e.str(c"key", key);
                    e.str(c"name", name);
                    e.opt_str(c"label", label.as_deref());
                    e.str(c"state", state);
                    e.bool(c"isLocal", *local);
                    e.bool(c"current", *current);
                    e.opt_str(c"version", version.as_deref());
                })?)?;
            }
            Ok(Value::Table(list))
        })
    })?;
    method(lua, t, "Current", move |_, ()| {
        cx::read(Dep::Server);
        let app = app_now("Servers:Current")?;
        Ok(window.then(|| server_id(crate::servers::runtime::active(app))))
    })?;
    method(lua, t, "Health", |lua, key: String| {
        cx::read(Dep::Servers);
        let key = if key == opencode_kit::servers::ServerScope::LOCAL { opencode_kit::servers::ServerKey::sidecar() } else { opencode_kit::servers::ServerKey::new(&key) };
        let data = crate::plugin_host::surfaces::health_data(crate::servers::health::get(&key).as_ref());
        mlua::IntoLua::into_lua(data, lua)
    })?;
    let plugin = vmx.half.id.clone();
    method(lua, t, "Connect", move |_, key: String| {
        cx::ask(&plugin, cx::Request::RunCommand { id: format!("server:{key}"), input: None });
        Ok(())
    })?;
    lazy_signals(lua, vmx, t, "Servers", &["Changed"])
}


fn theme(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    method(lua, t, "Current", |lua, ()| {
        let dark = app_now("Theme:Current")?.plugins.dark.get();
        let index = crate::palette::pal(dark).theme;
        let theme = session_view::themes::all().get(index);
        let mut h = std::hash::DefaultHasher::new();
        (index, dark, theme.map(|t| (&t.id, &t.name))).hash(&mut h);
        snapshot(lua, "theme", h.finish(), || {
            record(lua, 3, |info| {
                info.opt_str(c"id", theme.map(|t| t.id.as_str()));
                info.opt_str(c"name", theme.map(|t| t.name.as_str()));
                info.str(c"scheme", if dark { "dark" } else { "light" });
            })
            .map(Value::Table)
        })
    })?;
    method(lua, t, "IsDark", |_, ()| Ok(app_now("Theme:IsDark")?.plugins.dark.get()))?;
    method(lua, t, "Resolve", |_, color: String| {
        let dark = app_now("Theme:Resolve")?.plugins.dark.get();
        let c = if color.starts_with('#') { rune::color::hex(&color) } else { crate::plugin_host::vm::palette(&color, dark) };
        let c = c.unwrap_or(rune::Color::TRANSPARENT);
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Ok(format!("#{:02x}{:02x}{:02x}{:02x}", byte(c.r), byte(c.g), byte(c.b), byte(c.a)))
    })?;
    method(lua, t, "Font", |_, family: String| Ok(if family == "mono" { "IBM Plex Mono" } else { "Inter" }))?;
    lazy_signals(lua, vmx, t, "Theme", &["Changed"])
}

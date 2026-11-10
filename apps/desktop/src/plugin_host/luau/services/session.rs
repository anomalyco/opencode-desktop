//! Sessions, Models, Server (its cached reads and events; requests are in `net`), Servers and
//! Theme: what a render reads of the app's data.

use std::hash::{Hash, Hasher};
use std::rc::Rc;

use mlua::{Lua, Table, Value};
use rune::Tx;

use super::{app_now, json_to_lua, lazy_signals, method, record, snapshot};
use crate::App;
use crate::plugin_host::luau::cx::{self, Dep};
use crate::plugin_host::luau::machines::{Ticket, TicketEffect, TicketEvent};
use crate::plugin_host::luau::plugin::Resource;
use crate::plugin_host::luau::vm::Vmx;

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
    let window = vmx.half.kind == crate::plugin_host::luau::plugin::Kind::Window;
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
        for tab in app.tabs.iter().filter(|t| t.kind == crate::tabs::TabKind::Session) {
            list.raw_push(session_value(lua, app, &tab.id)?)?;
        }
        Ok(list)
    })?;
    lazy_signals(lua, vmx, t, "Sessions", &["Changed", "RoutedChanged"])
}

/// A session's version, as plugins see it: it moves when its title, cost, usage, message count
/// or busy state does, and not while a reply streams within a step.
pub fn session_version(app: &App, id: &str) -> u64 {
    let Some(info) = app.session(id) else { return 0 };
    let t = app.transcripts.get(&info.id);
    let last = t.and_then(|t| session_view::usage::last_with_usage(t, info.revert.as_deref()));
    let cost = info.cost.max(t.map_or(0.0, session_view::usage::spent));
    let messages = t.map(|t| t.messages.len());
    let usage = last.map(|a| (a.tokens.input, a.tokens.output, a.tokens.reasoning, a.tokens.cache_read, a.tokens.cache_write));
    let mut h = std::hash::DefaultHasher::new();
    (&info.title, info.updated, cost.to_bits(), messages, app.subtree_running(&info.id), usage, last.map(|a| (&a.id, a.completed)), &info.model.provider, &info.model.id, &info.model.variant, &info.agent, &info.project).hash(&mut h);
    h.finish()
}

/// Session `id` as a `Session` record, kept per VM while its version and the server hold.
fn session_value(lua: &Lua, app: &App, id: &str) -> mlua::Result<Value> {
    let mut h = std::hash::DefaultHasher::new();
    (session_version(app, id), &app.server_key()).hash(&mut h);
    snapshot(lua, &format!("session:{id}"), h.finish(), || build_session(lua, app, id))
}

fn build_session(lua: &Lua, app: &App, id: &str) -> mlua::Result<Value> {
    let Some(info) = app.session(id) else { return Ok(Value::Nil) };
    let t = app.transcripts.get(&info.id);
    let last = t.and_then(|t| session_view::usage::last_with_usage(t, info.revert.as_deref()));
    let busy = app.subtree_running(&info.id);
    let version = (session_version(app, id) >> 11) as f64;
    let cost = info.cost.max(t.map_or(0.0, session_view::usage::spent));
    let s = record(lua, 20, |s| {
        s.str(c"id", &info.id);
        s.str(c"serverId", app.server_key().as_deref().unwrap_or("local"));
        s.str(c"directory", &info.directory);
        s.str(c"title", &info.title);
        s.opt_str(c"parentId", info.parent.as_deref());
        s.opt_str(c"projectId", (!info.project.is_empty()).then_some(info.project.as_str()));
        s.opt_str(c"agent", (!info.agent.is_empty()).then_some(info.agent.as_str()));
        s.opt_str(c"variant", (!info.model.variant.is_empty() && info.model.variant != "default").then_some(info.model.variant.as_str()));
        s.bool(c"busy", busy);
        s.num(c"version", version);
        s.num(c"cost", cost);
        if let Some(a) = last {
            s.record(c"usage", 5, |u| {
                u.num(c"input", a.tokens.input as f64);
                u.num(c"output", a.tokens.output as f64);
                u.num(c"reasoning", a.tokens.reasoning as f64);
                u.num(c"cacheRead", a.tokens.cache_read as f64);
                u.num(c"cacheWrite", a.tokens.cache_write as f64);
            });
            if let Some(m) = a.model.as_ref() {
                s.str(c"usageProviderId", &m.provider);
                s.str(c"usageModelId", &m.id);
            }
        }
        if !info.model.id.is_empty() {
            s.str(c"providerId", &info.model.provider);
            s.str(c"modelId", &info.model.id);
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
        let m = snapshot(lua, &format!("model:{key}"), h.finish(), || {
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
    method(lua, t, "Resource", move |lua, (path, version): (String, Value)| {
        cx::read(Dep::Resource(plugin.clone(), path.clone()));
        let app = app_now("Server:Resource")?;
        let version = version_hash(&version);
        let mut map = half.resources.borrow_mut();
        let r = map.entry(path.clone()).or_default();
        let (ticket, effects) = std::mem::take(&mut r.ticket).step(TicketEvent::Ask(version));
        r.ticket = ticket;
        for TicketEffect::Fetch(asked) in effects {
            let (plugin, path) = (plugin.to_string(), path.clone());
            app.job(move |srv| {
                // A server still loading its plugins answers `/api/skill` with 503 for a while.
                let answer = srv.get_text_ready(&path);
                Box::new(move |s: &mut App, tx: &mut Tx| land(s, &plugin, &path, asked, answer, tx)) as crate::jobs::Apply
            });
        }
        resource_value(lua, r)
    })?;
    let half = vmx.half.clone();
    method(lua, t, "Invalidate", move |_, prefix: String| {
        half.resources.borrow_mut().retain(|path, _| !path.starts_with(&prefix));
        crate::plugin_host::luau::bump();
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
    method(lua, t, "Version", |_, ()| {
        cx::read(Dep::Server);
        Ok(app_now("Server:Version")?.plugins.shared.facts.server_version.clone())
    })?;
    method(lua, t, "IsLocal", |_, ()| {
        cx::read(Dep::Server);
        Ok(matches!(&app_now("Server:IsLocal")?.source, crate::Source::Live { server, .. } if server.is_local()))
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
    // The answer's text and JSON become Luau values once per answer, not once per read.
    let (text, json) = match &r.value {
        Some((g, text, json)) if *g == body.generation => (lua.registry_value::<Value>(text)?, lua.registry_value::<Value>(json)?),
        _ => {
            let text = Value::String(lua.create_string(&*body.text)?);
            let json = match serde_json::from_str::<serde_json::Value>(&body.text) {
                Ok(v) => json_to_lua(lua, &v)?,
                Err(_) => Value::Nil,
            };
            r.value = Some((body.generation, Rc::new(lua.create_registry_value(text.clone())?), Rc::new(lua.create_registry_value(json.clone())?)));
            (text, json)
        }
    };
    out.raw_set("text", text)?;
    out.raw_set("json", json)?;
    Ok(Value::Table(out))
}

fn land(app: &mut App, plugin: &str, path: &str, version: u64, answer: Result<String, String>, tx: &mut Tx) {
    let Some(half) = app.plugins.luau.half(plugin) else { return };
    let mut map = half.resources.borrow_mut();
    let Some(r) = map.get_mut(path) else { return };
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
    method(lua, t, "List", |lua, ()| {
        cx::read(Dep::Server);
        let app = app_now("Servers:List")?;
        cx::read(Dep::Servers);
        // (key, name, label, state, version) of each listed server the plugins contribute.
        let others: Vec<(String, String, String, &'static str, Option<String>)> = crate::plugin_host::contributed_servers(app)
            .into_iter()
            .filter(|s| s.rec.listed)
            .map(|s| {
                let state = ["stopped", "starting", "auth", "ready", "failed", "incompatible"].get(s.rec.state as usize).copied().unwrap_or("stopped");
                let version = app.plugins.shared.facts.health.get(&s.key).cloned();
                (s.key, s.rec.name.clone(), s.rec.label.clone(), state, version)
            })
            .collect();
        let mut h = std::hash::DefaultHasher::new();
        (&others, &app.server_key()).hash(&mut h);
        snapshot(lua, "servers", h.finish(), || {
            let list = lua.create_table_with_capacity(others.len() + 1, 0)?;
            list.raw_push(record(lua, 5, |own| {
                own.str(c"key", "local");
                own.str(c"name", "Local");
                own.str(c"state", "ready");
                own.bool(c"isLocal", true);
                own.bool(c"current", app.server_key().is_none());
            })?)?;
            for (key, name, label, state, version) in &others {
                list.raw_push(record(lua, 7, |e| {
                    e.bool(c"current", app.server_key().as_deref() == Some(key.as_str()));
                    e.str(c"key", key);
                    e.str(c"name", name);
                    e.opt_str(c"label", (!label.is_empty()).then_some(label.as_str()));
                    e.str(c"state", state);
                    e.bool(c"isLocal", false);
                    e.opt_str(c"version", version.as_deref());
                })?)?;
            }
            Ok(Value::Table(list))
        })
    })?;
    method(lua, t, "Current", |_, ()| {
        cx::read(Dep::Server);
        Ok(app_now("Servers:Current")?.server_key().or_else(|| Some("local".to_owned())))
    })?;
    method(lua, t, "Health", |lua, key: String| {
        cx::read(Dep::Servers);
        let Some(version) = app_now("Servers:Health")?.plugins.shared.facts.health.get(&key).cloned() else { return Ok(Value::Nil) };
        let h = lua.create_table()?;
        h.raw_set("healthy", true)?;
        h.raw_set("version", version)?;
        Ok(Value::Table(h))
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
        let dark = app_now("Theme:Current")?.plugins.luau.dark.get();
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
    method(lua, t, "IsDark", |_, ()| Ok(app_now("Theme:IsDark")?.plugins.luau.dark.get()))?;
    method(lua, t, "Resolve", |_, color: String| {
        let dark = app_now("Theme:Resolve")?.plugins.luau.dark.get();
        let c = if color.starts_with('#') { rune::color::hex(&color) } else { crate::plugin_host::luau::vm::palette(&color, dark) };
        let c = c.unwrap_or(rune::Color::TRANSPARENT);
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Ok(format!("#{:02x}{:02x}{:02x}{:02x}", byte(c.r), byte(c.g), byte(c.b), byte(c.a)))
    })?;
    method(lua, t, "Font", |_, family: String| Ok(if family == "mono" { "IBM Plex Mono" } else { "Inter" }))?;
    lazy_signals(lua, vmx, t, "Theme", &["Changed"])
}

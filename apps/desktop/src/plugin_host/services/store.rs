//! Store (values kept with the app, shared by every window), Contracts (delegated to
//! `opencode_plugin_runtime::contracts`) and Console (the Studio output and command bar).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use mlua::{FromLua, IntoLua, Lua, Table, Value, Variadic};
use rune_luau::{Data, LogLevel};

use super::{method, signal, with_app};
use crate::plugin_host::cx::{self, Dep};
use crate::plugin_host::plugin::Kind;
use crate::plugin_host::{console, vm::Vmx};

thread_local! {
    /// Each store value's version, by plugin then key: what memos and `Changed` follow. A session
    /// value's write also moves its store's (the bare name's), which `Version` and `Changed` read.
    static VERSIONS: RefCell<HashMap<String, HashMap<String, u64>>> = RefCell::default();
}

pub fn build(lua: &Lua, vmx: &Rc<Vmx>, name: &str, t: &Table) -> mlua::Result<()> {
    match name {
        "Store" => store(lua, vmx, t),
        "Contracts" => contracts(lua, vmx, t),
        _ => {
            for (name, level) in [("Log", LogLevel::Info), ("Warn", LogLevel::Warn), ("Error", LogLevel::Error)] {
                let plugin = vmx.half.id.clone();
                method(lua, t, name, move |_, args: Variadic<Value>| {
                    let text: Vec<String> = args.iter().map(|v| v.to_string().unwrap_or_else(|_| v.type_name().to_owned())).collect();
                    console::push(&plugin, level, &text.join("\t"));
                    Ok(())
                })?;
            }
            method(lua, t, "Lines", |lua, ()| {
                cx::read(Dep::Console);
                with_app("Console:Lines", |app| console::lines(app).into_lua(lua))
            })?;
            method(lua, t, "Clear", |_, ()| {
                console::clear();
                crate::plugin_host::bump();
                Ok(())
            })?;
            let own = vmx.half.id.clone();
            method(lua, t, "Eval", move |lua, (plugin, source): (String, String)| {
                let result = with_app("Console:Eval", |app| {
                    Ok(if !super::diag::debug_build() {
                        Err("The console's command bar runs in local and beta builds only".to_owned())
                    } else if *own == *plugin {
                        console::eval_here(lua, &source)
                    } else {
                        console::eval(app, &plugin, &source)
                    })
                })?;
                let out = lua.create_table()?;
                out.raw_set("ok", result.is_ok())?;
                match result {
                    Ok(value) => out.raw_set("value", value)?,
                    Err(e) => out.raw_set("error", e)?,
                }
                Ok(out)
            })
        }
    }
}

/// The version of store `key` of `plugin`.
pub fn store_version(plugin: &str, key: &str) -> u64 {
    VERSIONS.with_borrow(|v| v.get(plugin).and_then(|keys| keys.get(key)).copied().unwrap_or(0))
}

/// Notes that store `key` of `plugin` changed (for a session's value, its store too).
pub fn store_changed(plugin: &str, key: &str) {
    VERSIONS.with_borrow_mut(|v| {
        if !v.contains_key(plugin) {
            v.insert(plugin.to_owned(), HashMap::new());
        }
        let Some(keys) = v.get_mut(plugin) else { return };
        let mut up = |k: &str| match keys.get_mut(k) {
            Some(n) => *n += 1,
            None => {
                keys.insert(k.to_owned(), 1);
            }
        };
        up(key);
        if let Some((name, _)) = key.split_once('@') {
            up(name);
        }
    });
    crate::plugin_host::bump();
}

fn store(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    for (name, per_session) in [("Value", false), ("SessionValue", true)] {
        let v = vmx.clone();
        method(lua, t, name, move |lua, (key, default): (String, Value)| value(lua, &v, key, default, per_session))?;
    }
    Ok(())
}

fn value(lua: &Lua, vmx: &Rc<Vmx>, name: String, default: Value, per_session: bool) -> mlua::Result<Table> {
    use crate::plugin_host::stores;
    let plugin = vmx.half.id.clone();
    let default = Data::from_lua(default, lua)?;
    let o = lua.create_table()?;
    let name: Rc<str> = name.into();
    let ns: Rc<str> = opencode_storage::plugin_namespace(&plugin).into();
    let read = {
        let (plugin, ns, name, default) = (plugin.clone(), ns.clone(), name.clone(), default.clone());
        type ParsedCache = HashMap<Rc<str>, (u64, Data)>;
        // Each key's value as last parsed, by its version: a render reads it every time it runs.
        let parsed: Rc<std::cell::RefCell<ParsedCache>> = Rc::default();
        move |lua: &Lua, session: &str| -> mlua::Result<Value> {
            let key: Rc<str> = if per_session {
                with_app("Store", |app| {
                    let (scope, folder_known) = session_scope(app, session);
                    // A session the server hasn't listed yet (a subagent's page at launch) has no
                    // folder, and its values are kept under its folder: what read this reads
                    // again, under the session's own key, once the server lists it.
                    if !folder_known {
                        cx::read(Dep::Session(session.to_owned()));
                    }
                    Ok(stores::key(&name, &scope).into())
                })?
            } else {
                name.clone()
            };
            cx::read(Dep::Store(plugin.clone(), key.clone()));
            let version = store_version(&plugin, &key);
            if let Some((v, data)) = parsed.borrow().get(&key)
                && *v == version
            {
                return data.clone().into_lua(lua);
            }
            // A value saved before values were kept per server is under the bare session id.
            let text = if per_session { session_text(&plugin, &ns, &name, session, &key) } else { stores::text(&plugin, &ns, &key) };
            if per_session && text.is_some() {
                stores::touch(&plugin, &ns, &name, &key);
            }
            let data = text.and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).map_or_else(|| default.clone(), |j| super::json_to_data(&j));
            parsed.borrow_mut().insert(key, (version, data.clone()));
            data.into_lua(lua)
        }
    };
    let write = {
        let (plugin, ns, name) = (plugin.clone(), ns.clone(), name.clone());
        move |lua: &Lua, session: &str, value: Value| -> mlua::Result<()> {
            let json = Data::from_lua(value, lua)?.to_json().to_string();
            let key = if per_session { with_app("Store", |app| Ok(stores::key(&name, &scope_session(app, session))))? } else { name.to_string() };
            stores::set(&ns, &key, json);
            if per_session {
                stores::touch(&plugin, &ns, &name, &key);
            }
            store_changed(&plugin, &key);
            Ok(())
        }
    };
    if per_session {
        let r = read.clone();
        method(lua, &o, "Get", move |lua, session: String| r(lua, &session))?;
        let w = write.clone();
        method(lua, &o, "Set", move |lua, (session, value): (String, Value)| w(lua, &session, value))?;
    } else {
        method(lua, &o, "Get", move |lua, ()| read(lua, ""))?;
        method(lua, &o, "Set", move |lua, value: Value| write(lua, "", value))?;
    }
    let (p, n) = (plugin.clone(), name.clone());
    method(lua, &o, "Version", move |_, ()| {
        cx::read(Dep::Store(p.clone(), n.clone()));
        Ok(store_version(&p, &n) as f64)
    })?;
    o.raw_set("Changed", signal(lua, vmx, &format!("Store.{name}.Changed"))?)?;
    Ok(o)
}

/// Where session `session`'s values live (`SessionStateKey`): its server's scope (`local` for
/// the app's own), then its folder in base64 and its id; the session of a tab on another server
/// takes that tab's server. A global value (no session) stays as it is.
pub(crate) fn scope_session(app: &crate::App, session: &str) -> String {
    session_scope(app, session).0
}

/// [`scope_session`], and whether the session's folder is known: it isn't while the server
/// hasn't listed the session and no tab holds it, and the scope then names no folder.
fn session_scope(app: &crate::App, session: &str) -> (String, bool) {
    use opencode_kit::servers::key::{base64_url_encode, session_route_key, session_state_key};
    if session.is_empty() {
        return (String::new(), true);
    }
    let tab = app.tabs.iter().find(|t| t.id == session && !t.is_draft());
    let server = tab.map_or_else(|| crate::servers::runtime::active(app).clone(), |t| t.server.clone());
    let directory = app.session(session).filter(|_| server == *crate::servers::runtime::active(app)).map(|s| s.directory.clone()).or_else(|| tab.map(|t| t.directory.clone())).unwrap_or_default();
    (session_state_key(&server.scope(), &session_route_key(Some(&base64_url_encode(directory.as_bytes())), Some(session))), !directory.is_empty())
}

/// A session value under its scoped key or, for an older save, its bare session id.
pub(crate) fn session_text(plugin: &str, ns: &str, name: &str, session: &str, key: &str) -> Option<std::sync::Arc<str>> {
    use crate::plugin_host::stores;
    stores::text(plugin, ns, key).or_else(|| {
        let legacy = stores::key(name, session);
        (key != legacy).then(|| stores::text(plugin, ns, &legacy)).flatten()
    })
}

/// The generation of contract `id` (0 when nothing provides it).
pub fn contract_generation(id: &str) -> u64 {
    opencode_plugin_runtime::contracts::generation(id)
}

/// Forgets what plugin `owner`'s `kind` half provided from any VM but `keep`.
pub fn withdraw_owner(owner: &str, kind: Kind, keep: Option<u64>) {
    if opencode_plugin_runtime::contracts::withdraw_owner(owner, kind, keep) {
        crate::plugin_host::bump();
    }
}

fn contracts(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    let v_vm = vmx.clone();
    let v_sig = vmx.clone();
    opencode_plugin_runtime::contracts::build(
        lua,
        t,
        vmx.half.id.clone(),
        vmx.half.kind,
        move || v_vm.vm.get(),
        move |lua, name| signal(lua, &v_sig, name),
        opencode_plugin_runtime::contracts::Hooks {
            on_read: |id| cx::read(Dep::Contract(id.to_owned())),
            on_change: crate::plugin_host::bump,
            trace_on: crate::files::trace::on,
            trace_call: |plugin, name, args, us| crate::files::trace::luau(plugin, "contract", name, args, us),
        },
    )?;
    // Reserve `"browser"` for the host-provided narrow contract (`UP-095-3P-CONTRACT`, official
    // `packages/plugin/src/gui/browser.ts`): third-party plugins cannot squat it and each consumer
    // gets isolated per-VM `ExperimentalBrowserTab` handles.
    let base_provide: mlua::Function = t.raw_get("Provide")?;
    let is_builtin_browser = vmx.half.is_builtin("browser");
    t.raw_set(
        "Provide",
        lua.create_function(move |_, (this, id, value): (Table, String, Value)| {
            if id == "browser" && !is_builtin_browser {
                return Err(mlua::Error::runtime(
                    "The built-in 'browser' contract is provided by the host and cannot be overridden.",
                ));
            }
            base_provide.call::<Value>((this, id, value))
        })?,
    )?;
    let base_use: mlua::Function = t.raw_get("Use")?;
    let v_use = vmx.clone();
    t.raw_set(
        "Use",
        lua.create_function(move |lua, (this, id): (Table, String)| {
            if id == "browser" {
                let v_s = v_use.clone();
                return Ok(Value::Table(super::browser_contract::use_handle(
                    lua,
                    &v_use,
                    move |lua, name| signal(lua, &v_s, name),
                )?));
            }
            base_use.call::<Value>((this, id))
        })?,
    )?;
    let base_summary: mlua::Function = t.raw_get("Summary")?;
    t.raw_set(
        "Summary",
        lua.create_function(move |_, this: Table| {
            let out: Table = base_summary.call(this)?;
            if super::browser_contract::available() {
                out.raw_set("browser", "active")?;
            }
            Ok(out)
        })?,
    )?;
    Ok(())
}

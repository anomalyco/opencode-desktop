//! Luau plugins' commands (palette, keybinds, `/slash`, Settings › Shortcuts by their
//! `section`), menu items, link handlers, code block actions, MCP tools and app menu items, and
//! running their functions in a task with the app in reach.

use mlua::{Function, IntoLuaMulti};
use opencode_kit::client_mcp::{McpCall, McpResult};
use rune::{KeyEvent, Keybind, Tx};
use rune_luau::Data;

use super::cx;
use crate::App;
use crate::keymap::Section;
use crate::plugin_host::icons::intern;
use crate::plugin_host::services::{self, map};
use crate::plugin_host::{app_half, dormant, panels, plugin};

/// Runs `f(args)` of plugin `i` as a task, then carries out what it asked for.
pub fn spawn(app: &mut App, i: usize, f: &Function, args: impl IntoLuaMulti, tx: &mut Tx) {
    let Some(script) = app.plugins.list.get(i).and_then(|p| p.script()) else { return };
    cx::enter(app, || script.spawn_fn(f, args, Some(&mut *tx)).ok());
    super::requests::carry_out(app, tx);
}

/// One Luau command as the app lists it.
pub struct Cmd {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub slash: Option<&'static str>,
    pub slash_args: bool,
    pub keybind: Option<Keybind>,
    pub section: Option<Section>,
    pub hidden: bool,
    /// Among the palette's opening rows (`featured`, `palette.ts:160-165`).
    pub featured: bool,
    pub editable: bool,
    pub needs_session: bool,
    pub enabled: bool,
}

/// Every Luau command, offered here or not.
pub fn all(app: &App) -> Vec<Cmd> {
    let listed: Vec<_> = app
        .plugins
        .listed()
        .flat_map(|(i, p)| p.half.contribs.borrow().commands.iter().map(|c| (i, c.key, c.spec.clone(), c.f("enabled").cloned(), c.f("hidden").cloned())).collect::<Vec<_>>())
        .collect();
    listed
        .into_iter()
        .map(|(i, key, s, enabled, hidden)| Cmd {
            id: key,
            title: intern(&s.title),
            description: intern(s.description.as_deref().unwrap_or_default()),
            slash: s.slash.as_ref().map(|sl| intern(sl.name.trim_start_matches('/'))),
            slash_args: s.slash.as_ref().is_some_and(|sl| sl.arguments == Some(true)),
            keybind: s.bind.as_deref().and_then(|b| b.parse::<Keybind>().ok()).filter(|k| !k.chords().is_empty()),
            section: s.section.map(section),
            hidden: flag(app, i, key, "hidden", hidden.as_ref(), s.hidden, false),
            featured: s.featured == Some(true),
            editable: s.editable == Some(true),
            needs_session: s.requires_session == Some(true),
            enabled: flag(app, i, key, "enabled", enabled.as_ref(), s.enabled, true),
        })
        .collect()
}

/// Command `key`'s flag `name` (`enabled`, `hidden`): its function's answer ([`cx::answer`]),
/// else its value, else `default`.
fn flag(app: &App, i: usize, key: &str, name: &str, f: Option<&Function>, value: Option<bool>, default: bool) -> bool {
    let (Some(f), Some(script)) = (f, app.plugins.list[i].script()) else { return value.unwrap_or(default) };
    let answer = cx::answer(app, app.plugins.list[i].half.surface_version(Some(script)), format!("command:{key}:{name}"), || script.eval_fn::<bool>(f, ()).map_or(Data::Nil, Data::Bool));
    match answer {
        Data::Bool(b) => b,
        _ => default,
    }
}

pub fn section(s: opencode_plugin_spec::CommandSection) -> Section {
    use opencode_plugin_spec::CommandSection as C;
    match s {
        C::General => Section::General,
        C::Session => Section::Session,
        C::Navigation => Section::Navigation,
        C::Model => Section::Models,
        C::Terminal => Section::Terminal,
        C::Prompt => Section::Prompt,
    }
}

/// Whether a command is offered now.
pub fn offered(app: &App, c: &Cmd) -> bool {
    c.enabled && (!c.needs_session || app.active_session_id().is_some())
}

/// Runs Luau command `id` with `input`; false when there is none, it is off, or it declined
/// (returned `false` before its first yield).
pub fn run(app: &mut App, id: &str, input: Option<&str>, tx: &mut Tx) -> bool {
    dormant::wake_owners(app, |c| c.commands.iter().any(|c| c.key == id));
    if !all(app).iter().any(|c| c.id == id && offered(app, c)) {
        return false;
    }
    let Some((i, f)) = app.plugins.running().find_map(|(i, p)| Some((i, p.half.contribs.borrow().commands.iter().find(|c| c.key == id)?.f("run")?.clone()))) else {
        return false;
    };
    let Some(script) = app.plugins.list[i].script() else { return false };
    let Some(Ok(wrap)) = script.with_lua(|lua| wrapper(lua, "opencode.command", COMMAND)) else { return false };
    struct RestoreDeclined(bool);
    impl Drop for RestoreDeclined {
        fn drop(&mut self) {
            DECLINED.set(self.0);
        }
    }
    let restore = RestoreDeclined(DECLINED.replace(false));
    cx::enter(app, || script.spawn_fn(&wrap, (f, input.map(str::to_owned)), Some(&mut *tx)).ok());
    let declined = DECLINED.get();
    drop(restore);
    super::requests::carry_out(app, tx);
    !declined
}

thread_local! {
    /// The command that just ran returned `false` before its first yield.
    static DECLINED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs a command and notes whether it declined: its answer before it first yields.
const COMMAND: &str = r#"
local declined = ...
return function(f, input)
    local answer = f(input)
    if answer == false then
        declined()
    end
end
"#;

/// Runs an MCP tool's `call` and answers the call with what it returns or raises, whenever
/// that is, recording the coroutine's `sessionId` in the VM registry while it runs.
const TOOL: &str = r#"
local finish, sessions = ...
return function(f, id, args, context)
    local th = coroutine.running()
    sessions[th] = if type(context) == "table" and type(context.sessionId) == "string" then context.sessionId else false
    local ok, value = pcall(f, args, context)
    sessions[th] = nil
    finish(id, ok, value)
end
"#;

/// Session scope for the running Luau coroutine: `(Some(sid), _)` inside an MCP tool call, else
/// `(None, in_flight)` with any in-flight MCP tool session IDs in this VM.
#[cfg(not(target_arch = "wasm32"))]
pub fn current_tool_scope(lua: &mlua::Lua) -> (Option<Option<String>>, Vec<String>) {
    let Ok(Some(sessions)) = lua.named_registry_value::<Option<mlua::Table>>("opencode.tool.sessions") else {
        return (None, Vec::new());
    };
    let current = match sessions.raw_get::<mlua::Value>(lua.current_thread()) {
        Ok(mlua::Value::String(s)) => Some(Some(s.to_string_lossy())),
        Ok(mlua::Value::Boolean(false)) => Some(None),
        _ => None,
    };
    let in_flight = sessions
        .pairs::<mlua::Value, mlua::Value>()
        .flatten()
        .filter_map(|(_, v)| match v {
            mlua::Value::String(s) => Some(s.to_string_lossy()),
            _ => None,
        })
        .collect();
    (current, in_flight)
}

/// The VM's `source` wrapper, made once and kept under `name` in its registry.
fn wrapper(lua: &mlua::Lua, name: &str, source: &str) -> mlua::Result<Function> {
    if let Some(f) = lua.named_registry_value::<Option<Function>>(name)? {
        return Ok(f);
    }
    let f: Function = if source == COMMAND {
        let host = lua.create_function(|_, ()| {
            DECLINED.set(true);
            Ok(())
        })?;
        lua.load(source).set_name(format!("={name}")).call(host)?
    } else {
        let host = lua.create_function(|lua, (id, ok, value): (u64, bool, mlua::Value)| {
            let Some((call, _)) = cx::with_app(|app| app.plugins.mcp_calls.borrow_mut().remove(&id)).flatten() else { return Ok(()) };
            call.respond(tool_result(lua, ok, value));
            Ok(())
        })?;
        let sessions = lua.create_table()?;
        let meta = lua.create_table()?;
        meta.raw_set("__mode", "k")?;
        sessions.set_metatable(Some(meta))?;
        lua.set_named_registry_value("opencode.tool.sessions", &sessions)?;
        lua.load(source).set_name(format!("={name}")).call((host, sessions))?
    };
    lua.set_named_registry_value(name, &f)?;
    Ok(f)
}

/// What a tool's task ended with, as an MCP result: its error, an `McpImage`, its text, or its
/// value as JSON.
fn tool_result(lua: &mlua::Lua, ok: bool, value: mlua::Value) -> McpResult {
    use mlua::FromLua;
    if !ok {
        return McpResult::error(value.to_string().unwrap_or_else(|_| "The tool failed".to_owned()));
    }
    if let mlua::Value::String(s) = &value {
        return McpResult::text(s.to_string_lossy());
    }
    let data = Data::from_lua(value, lua).unwrap_or(Data::Nil);
    if let (Some(text), Some(image), Some(mime)) = (data.get("text").as_str(), data.get("image").as_str(), data.get("mimeType").as_str()) {
        return McpResult::with_image(text, image, mime);
    }
    McpResult::json(data.to_json())
}

/// How long an MCP tool's task may run before its call fails.
const TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Answers the MCP calls whose tools ran past their time, and asks for a wake at the next
/// deadline; true when one was answered.
pub fn expire_calls(app: &mut App, tx: &mut Tx) -> bool {
    let now = rune::time::Instant::now();
    // Borrowed through `&App`: reaching the cell through `&mut` would mark the plugin host changed
    // on every poll, and stamps and views that read it would rerun every frame.
    let mut calls = app.plugins.mcp_calls.borrow_mut();
    let ids: Vec<u64> = calls.iter().filter(|(_, (_, at))| *at <= now).map(|(id, _)| *id).collect();
    let late: Vec<McpCall> = ids.into_iter().filter_map(|id| calls.remove(&id)).map(|(call, _)| call).collect();
    if let Some(next) = calls.values().map(|(_, at)| *at).min() {
        tx.wake_after(next.saturating_duration_since(now));
    }
    let any = !late.is_empty();
    for call in late {
        call.respond(McpResult::error("The tool didn't answer in time"));
    }
    any
}

/// A Luau command's keybind.
pub fn key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let plain = !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt);
    let hit = all(app).into_iter().find(|c| offered(app, c) && c.keybind.as_ref().is_some_and(|k| k.matches_tx(ev, tx)) && (!plain || c.editable || tx.focused().is_none()));
    hit.is_some_and(|c| run(app, c.id, None, tx))
}

/// Whether `ev` is the chord (with Ctrl, Alt or the primary key) of a command a plugin offers
/// here: a focused terminal lets it go to the app (New terminal, Toggle terminal), as Desktop's
/// keymap takes a modified chord on the document's capture phase before the terminal sees it
/// (`command.tsx` `handleKeyDown`).
pub fn takes_chord(app: &App, ev: &KeyEvent, tx: &Tx) -> bool {
    (ev.mods.cmd || ev.mods.ctrl || ev.mods.alt) && all(app).iter().any(|c| offered(app, c) && c.keybind.as_ref().is_some_and(|k| k.matches_tx(ev, tx)))
}

/// A chord (with Ctrl, Alt or the primary key) of a command of a plugin with panels, ahead of the
/// pane that has the keyboard, as the side region's own commands are: Toggle terminal and New
/// terminal answer inside a focused terminal, Open review inside a focused file.
pub fn panel_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt) {
        return false;
    }
    let panels: Vec<&str> = panels::providers(app).iter().map(|p| p.plugin).collect();
    let hit = all(app).into_iter().find(|c| offered(app, c) && panels.iter().any(|p| c.id.strip_prefix(*p).is_some_and(|r| r.starts_with('.'))) && c.keybind.as_ref().is_some_and(|k| k.matches_tx(ev, tx)));
    hit.is_some_and(|c| run(app, c.id, None, tx))
}

// ---- Menu items, links, code blocks, MCP and the app menu ------------------------------------------

pub type MenuItem = (&'static str, String, Option<String>, Option<String>, i64);

/// Luau menu items of host menu `menu` (`session.panel`, `server.add`, `server.row`), as
/// `(key, title, description, icon, order)`.
pub fn menu_items(app: &App, menu: &str) -> Vec<MenuItem> {
    app.plugins
        .listed()
        .flat_map(|(_, p)| p.half.contribs.borrow().menu_items.iter().filter(|c| c.spec.menu.as_str() == menu).map(|c| (c.key, c.spec.title.clone(), c.spec.description.clone(), c.spec.icon.clone(), c.spec.order.unwrap_or(0))).collect::<Vec<_>>())
        .collect()
}

/// Plugin `i`'s `session.panel` items for session `session`: those whose `when` takes it, each
/// `opened` and with its `detail` as that session answers them.
pub fn panel_menu_items(app: &App, i: usize, session: &str) -> Vec<crate::panels::PanelLauncherDesc> {
    let p = &app.plugins.list[i];
    // A dormant plugin's item that answers `when` waits for it to start; the rest show from its record.
    let asleep = |key: &str| dormant::recorded_fn(app, &p.manifest.id, "menu_items", key, "when");
    let items: Vec<_> = p.half.contribs.borrow().menu_items.iter().filter(|c| c.spec.menu == opencode_plugin_spec::MenuName::SessionPanel && !asleep(c.key)).map(|c| (c.key, c.spec.clone(), c.f("when").cloned(), c.f("opened").cloned(), c.f("detail").cloned())).collect();
    items
        .into_iter()
        .filter(|(key, _, when, _, _)| when.as_ref().is_none_or(|f| panels::answer(app, i, &format!("when:{key}:{session}"), f, session.to_owned()) == Some(Data::Bool(true))))
        .map(|(key, s, _, opened, detail)| {
            let ask = |what: &str, f: &Function| panels::answer(app, i, &format!("{what}:{key}:{session}"), f, session.to_owned());
            let opened = opened.as_ref().and_then(|f| ask("opened", f));
            let detail = match detail.as_ref().and_then(|f| ask("detail", f)) {
                Some(Data::String(d)) => Some(d),
                _ => s.description.clone(),
            };
            let icon = crate::plugin_host::icons::resolve(&p.manifest.id, s.icon.as_deref().unwrap_or("plus"));
            crate::panels::PanelLauncherDesc::new(intern(&s.id), s.title, icon, s.order.unwrap_or(0) as i32)
                .description(detail)
                .command(s.keybind.as_deref().map(intern))
                .opened(opened == Some(Data::Bool(true)))
        })
        .collect()
}

/// Runs Luau menu item `key` for `target`; false when it isn't one.
pub fn run_menu_item(app: &mut App, key: &str, target: &str, tx: &mut Tx) -> bool {
    dormant::wake_owners(app, |c| c.menu_items.iter().any(|c| c.key == key));
    let found = app.plugins.running().find_map(|(i, p)| Some((i, p.half.contribs.borrow().menu_items.iter().find(|c| c.key == key)?.f("run")?.clone())));
    let Some((i, f)) = found else { return false };
    spawn(app, i, &f, (!target.is_empty()).then(|| target.to_owned()), tx);
    true
}

/// Runs menu item `key`'s `hover`, when it has one, for `target`.
pub fn hover_menu_item(app: &mut App, key: &str, target: &str, tx: &mut Tx) {
    dormant::wake_owners(app, |c| c.menu_items.iter().any(|c| c.key == key));
    let found = app.plugins.running().find_map(|(i, p)| Some((i, p.half.contribs.borrow().menu_items.iter().find(|c| c.key == key)?.f("hover")?.clone())));
    if let Some((i, f)) = found {
        spawn(app, i, &f, target.to_owned(), tx);
    }
}

/// Opens `href` with the first Luau link handler whose `match` takes it, highest priority first.
pub fn open_link(app: &mut App, href: &str, tx: &mut Tx) -> bool {
    // Every handler's `match` is asked: the dormant ones start first.
    dormant::wake_owners(app, |c| !c.link_handlers.is_empty());
    let mut handlers: Vec<(usize, i64, Function, Function)> = app
        .plugins
        .running()
        .flat_map(|(i, p)| p.half.contribs.borrow().link_handlers.iter().filter_map(|c| Some((i, c.spec.priority.unwrap_or(0), c.f("match")?.clone(), c.f("open")?.clone()))).collect::<Vec<_>>())
        .collect();
    handlers.sort_by_key(|h| -h.1);
    let link = map([("href", Data::String(href.to_owned()))]);
    for (i, _, matches, open) in handlers {
        let Some(script) = app.plugins.list[i].script() else { continue };
        if cx::enter(app, || script.eval_fn::<bool>(&matches, link.clone())).unwrap_or(false) {
            spawn(app, i, &open, link, tx);
            return true;
        }
    }
    false
}

pub fn run_code_block_action(app: &mut App, key: &str, code: &str, language: &str, tx: &mut Tx) {
    dormant::wake_owners(app, |c| c.code_block_actions.iter().any(|c| c.key == key));
    let found = app.plugins.running().find_map(|(i, p)| Some((i, p.half.contribs.borrow().code_block_actions.iter().find(|c| c.key == key)?.f("run")?.clone())));
    if let Some((i, f)) = found {
        spawn(app, i, &f, map([("code", Data::String(code.to_owned())), ("language", Data::String(language.to_owned()))]), tx);
    }
}

/// Luau MCP tools, window and app halves', as `(name, description, input schema)`.
pub fn mcp_tools(app: &App) -> Vec<(String, String, serde_json::Value)> {
    let window = app.plugins.listed().flat_map(|(_, p)| tools(&p.half.contribs.borrow()));
    let app_tools = app_half::each(|a| Some(tools(&a.half.contribs.borrow()))).into_iter().flatten();
    let dormant = dormant::each_app_half(|_, c| Some(tools(c))).into_iter().flatten();
    window.chain(app_tools).chain(dormant).collect()
}

fn tools(contribs: &plugin::Contribs) -> Vec<(String, String, serde_json::Value)> {
    contribs.mcp_tools.iter().map(|c| (c.spec.name.clone(), c.spec.description.clone(), serde_json::Value::Object(c.spec.input_schema.clone().into_iter().collect()))).collect()
}

/// Runs Luau MCP tool `call.name` for the call of session `session` (in `directory`) as a task,
/// which answers the call when it returns (at once, or after the `...Async` calls it waits on),
/// then carries out what it asked for. Hands the call back when no plugin has the tool.
pub fn mcp_call(app: &mut App, call: McpCall, session: &str, directory: &str, tx: &mut Tx) -> Option<McpCall> {
    dormant::wake_owners(app, |c| c.mcp_tools.iter().any(|t| t.spec.name == call.name));
    let found = app.plugins.running().find_map(|(i, p)| Some((i, p.half.contribs.borrow().mcp_tools.iter().find(|c| c.spec.name == call.name)?.f("call")?.clone())));
    let Some((i, f)) = found else { return Some(call) };
    let Some(script) = app.plugins.list[i].script() else { return Some(call) };
    let wrap = match script.with_lua(|lua| wrapper(lua, "opencode.tool", TOOL)) {
        Some(Ok(w)) => w,
        _ => {
            call.respond(McpResult::error("The plugin isn't running"));
            return None;
        }
    };
    let input = services::json_to_data(&call.arguments);
    let context = map([("sessionId", Data::String(session.to_owned())), ("directory", Data::String(directory.to_owned()))]);
    let id = app.plugins.next_mcp_call.get() + 1;
    app.plugins.next_mcp_call.set(id);
    app.plugins.mcp_calls.borrow_mut().insert(id, (call, rune::time::Instant::now() + TOOL_TIMEOUT));
    if let Err(e) = cx::enter(app, || script.spawn_fn(&wrap, (f, id, input, context), Some(&mut *tx)))
        && let Some((call, _)) = app.plugins.mcp_calls.borrow_mut().remove(&id)
    {
        call.respond(McpResult::error(e.message));
    }
    super::requests::carry_out(app, tx);
    expire_calls(app, tx);
    None
}

/// App menu items Luau app halves add, as `(key, menu, label, order)`.
pub fn menubar_items() -> Vec<(&'static str, String, String, i64)> {
    let rows = |c: &plugin::Contribs| c.menubar_items.iter().map(|c| (c.key, c.spec.menu.as_str().to_owned(), c.spec.label.clone(), c.spec.order.unwrap_or(0))).collect::<Vec<_>>();
    let running = app_half::each(|a| Some(rows(&a.half.contribs.borrow()))).into_iter().flatten();
    running.chain(dormant::each_app_half(|_, c| Some(rows(c))).into_iter().flatten()).collect()
}

pub fn run_menubar(app: &mut App, key: &str, tx: &mut Tx) -> bool {
    dormant::wake_owners(app, |c| c.menubar_items.iter().any(|c| c.key == key));
    let found = app_half::each(|a| a.half.contribs.borrow().menubar_items.iter().find(|c| c.key == key).and_then(|c| c.f("run").cloned())).into_iter().next();
    let Some(f) = found else { return false };
    app_half::each(|a| a.half.contribs.borrow().menubar_items.iter().any(|c| c.key == key).then(|| cx::enter(app, || a.script.spawn_fn(&f, (), Some(&mut *tx)).ok())));
    super::requests::carry_out(app, tx);
    true
}

/// Runs plugin command `id` (`<plugin>.<command>`); false when it declined or is gone.
pub fn run_command(app: &mut App, id: &str, tx: &mut Tx) -> bool {
    run(app, id, None, tx)
}

/// Runs plugin command `id` from the composer's `/name words` with `words` as its input.
pub fn run_slash(app: &mut App, id: &str, input: &str, tx: &mut Tx) -> bool {
    run(app, id, Some(input), tx)
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
    all(app).into_iter().filter(|c| offered(app, c)).filter_map(|c| Some(SlashCommand { id: c.id, name: c.slash?, title: c.title, description: c.description, args: c.slash_args })).collect()
}

/// The plugins' commands: the ones offered now (`only_offered`), or every one, as Settings ›
/// Shortcuts lists them. Hidden ones come too, marked: the palette leaves them out.
pub fn commands(app: &App, only_offered: bool) -> Vec<crate::commands::Command> {
    all(app)
        .into_iter()
        .filter(|c| !only_offered || offered(app, c))
        .map(|c| crate::commands::Command {
            id: c.id,
            title: c.title,
            description: c.description,
            slash: c.slash,
            keys: None,
            chord: c.keybind.as_ref().and_then(|k| k.chords().first().copied()),
            section: c.section,
            hidden: c.hidden,
            featured: c.featured,
            run: crate::commands::Run::Plugin(c.id),
        })
        .collect()
}

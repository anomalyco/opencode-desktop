//! Luau plugins' commands (palette, keybinds, `/slash`, Settings › Shortcuts by their
//! `section`), menu items, link handlers, code block actions, MCP tools and app menu items, and
//! running their functions in a task with the app in reach.

use mlua::{Function, IntoLuaMulti};
use rune::{KeyEvent, Keybind, Tx};
use rune_luau::Data;

use super::cx;
use super::services::map;
use crate::App;
use crate::keymap::Section;
use crate::plugin_host::icons::intern;

/// Runs `f(args)` of plugin `i` as a task, then carries out what it asked for.
pub fn spawn(app: &mut App, i: usize, f: &Function, args: impl IntoLuaMulti, tx: &mut Tx) {
    let Some(script) = app.plugins.luau.plugins.get(i).and_then(|p| p.script()) else { return };
    cx::with_app(app, || script.spawn_fn(f, args, Some(&mut *tx)).ok());
    super::requests::carry_out(app, tx);
}

/// The plugin console's tab and keys (Ctrl+Shift+J, ⇧⌘J): a developer tool, so no command lists it.
const CONSOLE: &str = "plugins.console";

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
    pub editable: bool,
    pub needs_session: bool,
    pub enabled: bool,
}

/// Every Luau command, offered here or not.
pub fn all(app: &App) -> Vec<Cmd> {
    app
        .plugins
        .luau
        .running()
        .flat_map(|(_, p)| {
            p.half.contribs.borrow().commands.iter().map(|c| {
                let s = &c.spec;
                Cmd {
                    id: c.key,
                    title: intern(&s.title),
                    description: intern(s.description.as_deref().unwrap_or_default()),
                    slash: s.slash.as_ref().map(|sl| intern(sl.name.trim_start_matches('/'))),
                    slash_args: s.slash.as_ref().is_some_and(|sl| sl.arguments == Some(true)),
                    keybind: s.bind.as_deref().and_then(|b| b.parse::<Keybind>().ok()).filter(|k| !k.chords().is_empty()),
                    section: s.section.map(section),
                    hidden: s.hidden == Some(true),
                    editable: s.editable == Some(true),
                    needs_session: s.requires_session == Some(true),
                    enabled: s.enabled != Some(false),
                }
            }).collect::<Vec<_>>()
        })
        .collect()
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

/// Runs Luau command `id` with `input`; false when there is none or it is off.
pub fn run(app: &mut App, id: &str, input: Option<&str>, tx: &mut Tx) -> bool {
    if id == CONSOLE {
        if app.active_session_id().is_some() {
            app.open_side_tab(&crate::panels::panel_key(crate::plugin_host::view::PANEL, super::console::TAB), None, false, false, tx);
        }
        return true;
    }
    let Some((i, f)) = app.plugins.luau.running().find_map(|(i, p)| {
        let c = p.half.contribs.borrow();
        let c = c.commands.iter().find(|c| c.key == id && c.spec.enabled != Some(false))?;
        if c.spec.requires_session == Some(true) && app.active_session_id().is_none() {
            return None;
        }
        Some((i, c.f("run")?.clone()))
    }) else {
        return false;
    };
    spawn(app, i, &f, input.map(str::to_owned), tx);
    true
}

/// A Luau command's keybind.
pub fn key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if "mod+shift+j".parse::<Keybind>().is_ok_and(|k| k.matches_tx(ev, tx)) && app.active_session_id().is_some() {
        return run(app, CONSOLE, None, tx);
    }
    let plain = !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt);
    let hit = all(app).into_iter().find(|c| offered(app, c) && c.keybind.as_ref().is_some_and(|k| k.matches_tx(ev, tx)) && (!plain || c.editable || tx.focused().is_none()));
    hit.is_some_and(|c| run(app, c.id, None, tx))
}

// ---- Menu items, links, code blocks, MCP and the app menu ------------------------------------------

/// Luau menu items of host menu `menu` (`session.panel`, `server.add`, `server.row`), as
/// `(key, title, description, icon, order)`.
pub fn menu_items(app: &App, menu: &str) -> Vec<(&'static str, String, Option<String>, Option<String>, i64)> {
    app.plugins
        .luau
        .running()
        .flat_map(|(_, p)| p.half.contribs.borrow().menu_items.iter().filter(|c| c.spec.menu.as_str() == menu).map(|c| (c.key, c.spec.title.clone(), c.spec.description.clone(), c.spec.icon.clone(), c.spec.order.unwrap_or(0))).collect::<Vec<_>>())
        .collect()
}

/// Plugin `i`'s `session.panel` items for session `session`: those whose `when` takes it, each
/// `opened` and with its `detail` as that session answers them.
pub fn panel_menu_items(app: &App, i: usize, session: &str) -> Vec<crate::panels::PanelLauncherDesc> {
    let p = &app.plugins.luau.plugins[i];
    let Some(script) = p.script() else { return Vec::new() };
    let items: Vec<_> = p.half.contribs.borrow().menu_items.iter().filter(|c| c.spec.menu == opencode_plugin_spec::MenuName::SessionPanel).map(|c| (c.key, c.spec.clone(), c.f("when").cloned(), c.f("opened").cloned(), c.f("detail").cloned())).collect();
    let ask = |f: &Function| cx::with_app(app, || script.eval_fn::<Data>(f, session.to_owned()).ok());
    items
        .into_iter()
        .filter(|(_, _, when, _, _)| when.as_ref().is_none_or(|f| ask(f) == Some(Data::Bool(true))))
        .map(|(key, s, _, opened, detail)| {
            let detail = match detail.as_ref().and_then(|f| ask(f)) {
                Some(Data::String(d)) => Some(d),
                _ => s.description.clone(),
            };
            let icon = crate::plugin_host::icons::resolve("", s.icon.as_deref().unwrap_or("plus"));
            crate::panels::PanelLauncherDesc::new(intern(&format!("menu:{key}")), s.title, icon, s.order.unwrap_or(0) as i32)
                .description(detail)
                .command(s.keybind.as_deref().map(intern))
                .opened(opened.as_ref().and_then(|f| ask(f)) == Some(Data::Bool(true)))
        })
        .collect()
}

/// Runs Luau menu item `key` for `target`; false when it isn't one.
pub fn run_menu_item(app: &mut App, key: &str, target: &str, tx: &mut Tx) -> bool {
    let found = app.plugins.luau.running().find_map(|(i, p)| Some((i, p.half.contribs.borrow().menu_items.iter().find(|c| c.key == key)?.f("run")?.clone())));
    let Some((i, f)) = found else { return false };
    spawn(app, i, &f, (!target.is_empty()).then(|| target.to_owned()), tx);
    true
}

/// Opens `href` with the first Luau link handler whose `match` takes it, highest priority first.
pub fn open_link(app: &mut App, href: &str, tx: &mut Tx) -> bool {
    let mut handlers: Vec<(usize, i64, Function, Function)> = app
        .plugins
        .luau
        .running()
        .flat_map(|(i, p)| p.half.contribs.borrow().link_handlers.iter().filter_map(|c| Some((i, c.spec.priority.unwrap_or(0), c.f("match")?.clone(), c.f("open")?.clone()))).collect::<Vec<_>>())
        .collect();
    handlers.sort_by_key(|h| -h.1);
    let link = map([("href", Data::String(href.to_owned()))]);
    for (i, _, matches, open) in handlers {
        let Some(script) = app.plugins.luau.plugins[i].script() else { continue };
        if cx::with_app(app, || script.eval_fn::<bool>(&matches, link.clone())).unwrap_or(false) {
            spawn(app, i, &open, link, tx);
            return true;
        }
    }
    false
}

/// Code block actions for `language`, as `(key, icon, title)`.
pub fn code_block_actions(app: &App, language: &str) -> Vec<(&'static str, String, String)> {
    app.plugins
        .luau
        .running()
        .flat_map(|(_, p)| p.half.contribs.borrow().code_block_actions.iter().filter(|c| c.spec.languages.iter().any(|l| l.eq_ignore_ascii_case(language))).map(|c| (c.key, c.spec.icon.clone(), c.spec.title.clone())).collect::<Vec<_>>())
        .collect()
}

pub fn run_code_block_action(app: &mut App, key: &str, code: &str, language: &str, tx: &mut Tx) {
    let found = app.plugins.luau.running().find_map(|(i, p)| Some((i, p.half.contribs.borrow().code_block_actions.iter().find(|c| c.key == key)?.f("run")?.clone())));
    if let Some((i, f)) = found {
        spawn(app, i, &f, map([("code", Data::String(code.to_owned())), ("language", Data::String(language.to_owned()))]), tx);
    }
}

/// Luau MCP tools, window and app halves', as `(name, description, input schema)`.
pub fn mcp_tools(app: &App) -> Vec<(String, String, serde_json::Value)> {
    let window = app.plugins.luau.running().flat_map(|(_, p)| tools(&p.half));
    let app_tools = super::app_half::each(|a| Some(tools(&a.half))).into_iter().flatten();
    window.chain(app_tools).collect()
}

fn tools(half: &super::Half) -> Vec<(String, String, serde_json::Value)> {
    half.contribs.borrow().mcp_tools.iter().map(|c| (c.spec.name.clone(), c.spec.description.clone(), serde_json::Value::Object(c.spec.input_schema.clone().into_iter().collect()))).collect()
}

/// Calls Luau MCP tool `name` with `args`; `None` when no plugin has it.
pub fn mcp_call(app: &App, name: &str, args: &serde_json::Value) -> Option<Result<serde_json::Value, String>> {
    let found = app.plugins.luau.running().find_map(|(i, p)| Some((i, p.half.contribs.borrow().mcp_tools.iter().find(|c| c.spec.name == name)?.f("call")?.clone())))?;
    let script = app.plugins.luau.plugins[found.0].script()?;
    let input = super::services::json_to_data(args);
    Some(cx::with_app(app, || script.eval_fn::<Data>(&found.1, input)).map(|d| d.to_json()).map_err(|e| e.message))
}

/// App menu items Luau app halves add, as `(key, menu, label, order)`.
pub fn menubar_items() -> Vec<(&'static str, String, String, i64)> {
    super::app_half::each(|a| Some(a.half.contribs.borrow().menubar_items.iter().map(|c| (c.key, c.spec.menu.as_str().to_owned(), c.spec.label.clone(), c.spec.order.unwrap_or(0))).collect::<Vec<_>>())).into_iter().flatten().collect()
}

pub fn run_menubar(app: &mut App, key: &str, tx: &mut Tx) -> bool {
    let found = super::app_half::each(|a| a.half.contribs.borrow().menubar_items.iter().find(|c| c.key == key).and_then(|c| c.f("run").cloned())).into_iter().next();
    let Some(f) = found else { return false };
    super::app_half::each(|a| a.half.contribs.borrow().menubar_items.iter().any(|c| c.key == key).then(|| cx::with_app(app, || a.script.spawn_fn(&f, (), Some(&mut *tx)).ok())));
    super::requests::carry_out(app, tx);
    true
}

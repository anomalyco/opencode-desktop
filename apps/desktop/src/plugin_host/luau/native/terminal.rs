//! `Terminal`, `ui.terminalSurface` and `ui.terminalTabLabel`: a session's terminals as the
//! app's terminal engine (`crate::terminal`) runs them, which starts and streams their shells,
//! parses and draws their screens and takes their keys and IME. The terminal plugin lists them
//! as tabs and says what each is called; the engine does the rest.

use std::hash::{Hash, Hasher};

use mlua::{Lua, Table, Value};
use rune::{El, Ime, KeyEvent, Tx};
use rune_luau::{Data, NodeArgs};

use super::super::cx::{self, Dep};
use super::super::services::{app_now, map, method};
use super::{handle_field, object, p};
use crate::App;

thread_local! {
    /// The device pixels a point of the pane a panel renders in (`PanelViewCtx.env_scale`), set as
    /// the panel renders: the shell's grid is sized in pixels too.
    pub static SCALE: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
}

/// The `Terminal` service.
pub fn build(lua: &Lua, t: &Table) -> mlua::Result<()> {
    method(lua, t, "List", |lua, (sid, open): (String, Vec<String>)| {
        cx::read(Dep::Terminals(sid.clone()));
        let app = app_now("Terminal:List")?;
        let row = |id: &str, title: String, number: usize, hidden: bool| map([("id", Data::String(id.to_owned())), ("title", Data::String(title)), ("number", Data::Integer(number as i64)), ("hidden", Data::Bool(hidden))]);
        let ws = app.terminal.workspaces.get(&app.terminal_workspace_key_for_session(&sid));
        let rows: Vec<Data> = match ws {
            Some(ws) if !ws.tabs.is_empty() || ws.failure.is_none() => ws.tabs.iter().map(|t| row(&t.id, t.title.trim().to_owned(), t.title_number, false)).collect(),
            Some(_) => open.iter().map(|id| row(id, String::new(), 0, false)).collect(),
            None => open.iter().map(|id| row(id, String::new(), 0, true)).collect(),
        };
        super::diff::lua_data(lua, Data::List(rows))
    })?;
    method(lua, t, "Available", |_, ()| Ok(!crate::flag("--read-only")))?;
    method(lua, t, "New", |_, sid: Option<String>| {
        app_now("Terminal:New")?.later(Box::new(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            if sid.as_deref().is_none_or(|id| s.active_session_id() == Some(id)) {
                s.create_terminal_tab(None, tx);
            }
        }));
        Ok(())
    })?;
    method(lua, t, "Toggle", |_, ()| {
        app_now("Terminal:Toggle")?.later(Box::new(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.toggle_terminal(tx);
        }));
        Ok(())
    })?;
    method(lua, t, "Select", |_, (sid, id, focus): (String, String, bool)| {
        app_now("Terminal:Select")?.later(Box::new(move |s: &mut App, _: &mut Tx| s.select_session_terminal(&sid, &id, focus)));
        Ok(())
    })?;
    method(lua, t, "Attach", |lua, id: String| {
        let app = app_now("Terminal:Attach")?;
        let Some(sid) = app.active_session_id().map(str::to_owned) else { return Ok(Value::Nil) };
        Ok(Value::Table(session(lua, &sid, &id)?))
    })?;
    method(lua, t, "Create", |_, _: Value| -> mlua::Result<()> { Err(mlua::Error::runtime("Terminal:Create isn't in this app yet: Terminal:New starts a session's terminal")) })
}

/// A `PtySession` for terminal `id` of session `sid`.
fn session(lua: &Lua, sid: &str, id: &str) -> mlua::Result<Table> {
    let m = lua.create_table()?;
    let read = |this: &Table| -> mlua::Result<(String, String)> { Ok((this.raw_get("sessionId")?, this.raw_get("Id")?)) };
    m.raw_set(
        "Title",
        lua.create_function(move |_, this: Table| {
            let (sid, id) = read(&this)?;
            cx::read(Dep::Terminals(sid.clone()));
            let app = app_now("PtySession:Title")?;
            let ws = app.terminal.workspaces.get(&app.terminal_workspace_key_for_session(&sid));
            Ok(ws.and_then(|w| w.tabs.iter().find(|t| t.id == id)).map(|t| t.title.clone()).unwrap_or_default())
        })?,
    )?;
    m.raw_set(
        "Running",
        lua.create_function(move |_, this: Table| {
            let (sid, id) = read(&this)?;
            cx::read(Dep::Terminals(sid.clone()));
            let app = app_now("PtySession:Running")?;
            let ws = app.terminal.workspaces.get(&app.terminal_workspace_key_for_session(&sid));
            Ok(ws.is_some_and(|w| w.tabs.iter().any(|t| t.id == id)))
        })?,
    )?;
    m.raw_set(
        "Close",
        lua.create_function(move |_, this: Table| {
            let (sid, id) = read(&this)?;
            app_now("PtySession:Close")?.later(Box::new(move |s: &mut App, tx: &mut Tx| {
                let wk = s.terminal_workspace_key_for_session(&sid);
                s.close_terminal_tab(&wk, &id, tx);
            }));
            Ok(())
        })?,
    )?;
    method(lua, &m, "Write", |_, _: mlua::MultiValue| -> mlua::Result<()> { Err(mlua::Error::runtime("PtySession:Write isn't in this app yet")) })?;
    let fields = [("Id", Value::String(lua.create_string(id)?)), ("sessionId", Value::String(lua.create_string(sid)?))];
    object(lua, "PtySession", &fields, m)
}

/// The version of what `Dep::Terminals(session)` reads: its terminals, their titles, the active
/// one and why a list failed.
pub fn version(app: &App, session: &str) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    match app.terminal.workspaces.get(&app.terminal_workspace_key_for_session(session)) {
        Some(ws) => {
            (&ws.active, &ws.failure).hash(&mut h);
            ws.tabs.iter().for_each(|t| (&t.id, &t.title, t.title_number).hash(&mut h));
        }
        None => 0u8.hash(&mut h),
    }
    h.finish()
}

/// The version of what `Dep::TerminalScreen(session)` reads, the surfaces and labels: each
/// screen (a new snapshot each time the shell draws), its status, scroll and selection drag, the
/// cursor's blink, where the keyboard is, and a rename.
pub fn screen_version(app: &App, session: &str) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    version(app, session).hash(&mut h);
    if let Some(ws) = app.terminal.workspaces.get(&app.terminal_workspace_key_for_session(session)) {
        ws.tabs.iter().for_each(|t| (std::sync::Arc::as_ptr(&t.session.snapshot()) as usize, format!("{:?}", t.session.status()), t.session.revealed(), t.session.cursor_offset(), t.drag_anchor, t.scroll_px_accum.to_bits()).hash(&mut h));
    }
    (app.terminal.cursor_lit(app.clock), app.composer.focused, app.side.focused, format!("{:?}", app.side.active_pane_focus)).hash(&mut h);
    crate::terminal::rename_stamp(app, &mut h);
    h.finish()
}

/// `ui.terminalSurface`: terminal `session` filling the pane (its grid, cursor and selection),
/// or with `narrow` the narrow window's Terminal view of session `sessionId`.
pub fn view(a: &mut NodeArgs) -> Result<El, String> {
    let app = cx::app().ok_or("ui.terminalSurface draws only in a render")?;
    let pal = p(a);
    if a.flag("narrow") {
        let sid = a.string("sessionId").map(str::to_owned).or_else(|| app.active_session_id().map(str::to_owned)).unwrap_or_default();
        cx::read(Dep::TerminalScreen(sid.clone()));
        return Ok(crate::terminal::narrow::view(app, &pal, &sid));
    }
    let sid = handle_field(a, "session", "sessionId").ok_or("ui.terminalSurface needs a session from Terminal:Attach")?;
    let id = handle_field(a, "session", "Id").unwrap_or_default();
    cx::read(Dep::TerminalScreen(sid.clone()));
    let size = (a.number("width").unwrap_or(0.0) as f32, a.number("height").unwrap_or(0.0) as f32);
    Ok(crate::terminal::surface(app, &pal, &sid, &id, size, SCALE.get(), a.flag("focused")))
}

/// `ui.terminalTabLabel`: the prompt glyph and the title, renamed in place by a double-click on
/// the title or from the label's menu.
pub fn label(a: &mut NodeArgs) -> Result<El, String> {
    let app = cx::app().ok_or("ui.terminalTabLabel draws only in a render")?;
    let pal = p(a);
    let id = handle_field(a, "session", "Id").ok_or("ui.terminalTabLabel needs a session from Terminal:Attach")?;
    if let Some(sid) = handle_field(a, "session", "sessionId") {
        cx::read(Dep::TerminalScreen(sid));
    }
    let title = a.string("title").unwrap_or_default().to_owned();
    Ok(crate::terminal::tab_label(app, &pal, &id, &title, a.flag("selected")))
}

/// Lands what the terminals' streams and the rename did.
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    crate::terminal::poll_panel(app, tx)
}

/// Keys for the terminal that has the keyboard.
pub fn key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    crate::terminal::key_terminal(app, ev, tx)
}

/// IME for the terminal that has the keyboard.
pub fn ime(app: &mut App, ime: &Ime) -> bool {
    crate::terminal::ime_terminal(app, ime)
}

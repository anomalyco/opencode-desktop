//! `Terminal`, `ui.terminalSurface` and `ui.terminalTabLabel`: a session's terminals as the
//! app's terminal engine (`crate::terminal`) runs them, which starts and streams their shells,
//! parses and draws their screens and takes their keys and IME. The terminal plugin lists them
//! as tabs and says what each is called; the engine does the rest.

use std::hash::{Hash, Hasher};

use mlua::{Lua, Table, Value};
use rune::{El, Ime, KeyEvent, Tx};
use rune_luau::{Data, NodeArgs};

use super::super::cx::{self, Dep};
use super::super::services::{map, method, with_app};
use super::{handle_field, object, p};
use crate::App;

/// The `Terminal` service.
pub fn build(lua: &Lua, vmx: &std::rc::Rc<super::super::vm::Vmx>, t: &Table) -> mlua::Result<()> {
    method(lua, t, "List", |lua, (sid, open): (String, Vec<String>)| {
        cx::read(Dep::Terminals(sid.clone()));
        let rows = with_app("Terminal:List", |app| {
            let row = |id: &str, title: String, number: usize, hidden: bool| map([("id", Data::String(id.to_owned())), ("title", Data::String(title)), ("number", Data::Integer(number as i64)), ("hidden", Data::Bool(hidden))]);
            let ws = app.kernel.slots.get(&crate::terminal::TERMINAL).workspaces.get(&app.terminal_workspace_key_for_session(&sid));
            Ok(match ws {
                // Still being listed by the server: the tabs the strip holds stay, as before it was asked.
                Some(ws) if ws.call == crate::terminal::calls::Call::Listing => open.iter().map(|id| row(id, String::new(), 0, true)).collect(),
                Some(ws) if !ws.tabs.is_empty() || ws.failure.is_none() => ws.tabs.iter().map(|t| row(&t.id, t.title.trim().to_owned(), t.title_number, false)).collect(),
                Some(_) => open.iter().map(|id| row(id, String::new(), 0, false)).collect(),
                None => open.iter().map(|id| row(id, String::new(), 0, true)).collect(),
            })
        })?;
        super::diff::lua_data(lua, Data::List(rows))
    })?;
    method(lua, t, "Available", |_, ()| Ok(!crate::flag("--read-only")))?;
    method(lua, t, "New", |_, sid: Option<String>| {
        // The terminal's pane opens on SIDE_SPRING; `show_pane_tab` lands a tab switch at once.
        with_app("Terminal:New", |app| {
            app.later(Box::new(move |s: &mut App, tx: &mut Tx| {
                if sid.as_deref().is_none_or(|id| s.active_session_id() == Some(id)) {
                    s.create_terminal_tab(None, tx);
                }
            }));
            Ok(())
        })
    })?;
    method(lua, t, "Toggle", |_, ()| {
        with_app("Terminal:Toggle", |app| {
            app.later(Box::new(|s: &mut App, tx: &mut Tx| {
                s.toggle_terminal(tx);
            }));
            Ok(())
        })
    })?;
    method(lua, t, "Select", |_, (sid, id, focus): (String, String, bool)| {
        with_app("Terminal:Select", |app| {
            app.later(Box::new(move |s: &mut App, _: &mut Tx| s.select_session_terminal(&sid, &id, focus)));
            Ok(())
        })
    })?;
    method(lua, t, "Attach", |lua, id: String| {
        let Some(sid) = with_app("Terminal:Attach", |app| Ok(app.active_session_id().map(str::to_owned)))? else { return Ok(Value::Nil) };
        Ok(Value::Table(session(lua, &sid, &id)?))
    })?;
    // A terminal of the routed session, on its server, running the spec's program in its folder;
    // its tab joins the session's others.
    method(lua, t, "Create", |lua, spec: Value| {
        let spec: opencode_plugin_spec::TerminalSpec = mlua::FromLua::from_lua(spec, lua).and_then(|d: Data| d.decode().map_err(|e| mlua::Error::runtime(format!("TerminalSpec: {e}"))))?;
        let (sid, id) = with_app("Terminal:Create", |app| {
            let sid = app.active_session_id().map(str::to_owned).ok_or_else(|| mlua::Error::runtime("Terminal:Create needs a session: this window shows none"))?;
            let cwd = spec.cwd.or(Some(spec.directory));
            let info = app.start_terminal(&sid, spec.title, cwd, spec.command, spec.args.unwrap_or_default()).map_err(mlua::Error::runtime)?;
            let id = info.id.clone();
            let owner = sid.clone();
            app.later(Box::new(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.adopt_terminal(&owner, info);
            }));
            Ok((sid, id))
        })?;
        session(lua, &sid, &id)
    })?;
    super::super::services::async_method(lua, vmx, t, "OpenAsync", "Terminal.OpenAsync")
}

/// `Terminal:OpenAsync`, made with the VM.
pub fn asyncs(host: &rune_luau::Host<'_>, t: &Table) -> mlua::Result<()> {
    t.raw_set("Terminal.OpenAsync", host.async_function("Terminal:OpenAsync", open)?)
}

/// Shows the session's terminal tab on the next poll (starting one when asked or it has none),
/// then answers with its id and where a first terminal opens.
fn open(lua: &Lua, a: mlua::MultiValue, done: rune_luau::Completer) -> mlua::Result<()> {
    let (sid, new): (String, Option<bool>) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    with_app("Terminal:OpenAsync", |app| {
        app.later(Box::new(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            let id = s.open_terminal_for_session(&sid, new.unwrap_or(false));
            done.complete(map([("id", id.map_or(Data::Nil, Data::String)), ("placement", Data::String(s.dock_placement().name().to_owned()))]));
        }));
        Ok(())
    })
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
            with_app("PtySession:Title", |app| {
                let ws = app.kernel.slots.get(&crate::terminal::TERMINAL).workspaces.get(&app.terminal_workspace_key_for_session(&sid));
                Ok(ws.and_then(|w| w.tabs.iter().find(|t| t.id == id)).map(|t| t.title.clone()).unwrap_or_default())
            })
        })?,
    )?;
    m.raw_set(
        "Running",
        lua.create_function(move |_, this: Table| {
            let (sid, id) = read(&this)?;
            cx::read(Dep::Terminals(sid.clone()));
            with_app("PtySession:Running", |app| {
                let ws = app.kernel.slots.get(&crate::terminal::TERMINAL).workspaces.get(&app.terminal_workspace_key_for_session(&sid));
                Ok(ws.is_some_and(|w| w.tabs.iter().any(|t| t.id == id)))
            })
        })?,
    )?;
    m.raw_set(
        "Close",
        lua.create_function(move |_, this: Table| {
            let (sid, id) = read(&this)?;
            with_app("PtySession:Close", |app| {
                app.later(Box::new(move |s: &mut App, tx: &mut Tx| {
                    let wk = s.terminal_workspace_key_for_session(&sid);
                    s.close_terminal_tab(&wk, &id, tx);
                }));
                Ok(())
            })
        })?,
    )?;
    m.raw_set(
        "Write",
        lua.create_function(move |_, (this, text): (Table, String)| {
            let (sid, id) = read(&this)?;
            with_app("PtySession:Write", |app| {
                app.later(Box::new(move |s: &mut App, _: &mut Tx| {
                    let wk = s.terminal_workspace_key_for_session(&sid);
                    if let Some(tab) = s.kernel.slots.get(&crate::terminal::TERMINAL).workspaces.get(&wk).and_then(|w| w.tabs.iter().find(|t| t.id == id)) {
                        tab.session.write_bytes(text.into_bytes());
                    }
                }));
                Ok(())
            })
        })?,
    )?;
    let fields = [("Id", Value::String(lua.create_string(id)?)), ("sessionId", Value::String(lua.create_string(sid)?))];
    object(lua, "PtySession", &fields, m)
}

/// The version of what `Dep::Terminals(session)` reads: its terminals, their titles, the active
/// one and why a list failed.
pub fn version(app: &App, session: &str) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    match app.kernel.slots.get(&crate::terminal::TERMINAL).workspaces.get(&app.terminal_workspace_key_for_session(session)) {
        Some(ws) => {
            (&ws.active, &ws.failure, ws.call == crate::terminal::calls::Call::Listing).hash(&mut h);
            ws.tabs.iter().for_each(|t| (&t.id, &t.title, t.title_number).hash(&mut h));
        }
        None => 0u8.hash(&mut h),
    }
    h.finish()
}

/// The version of what `Dep::TerminalScreen(session)` reads, the surfaces and labels: each
/// screen (a new snapshot each time the shell draws), its status, scroll and selection drag, the
/// cursor's blink, the bell's flash, the find bar, the link under the pointer, where the keyboard
/// is, and a rename.
pub fn screen_version(app: &App, session: &str) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    version(app, session).hash(&mut h);
    if let Some(ws) = app.kernel.slots.get(&crate::terminal::TERMINAL).workspaces.get(&app.terminal_workspace_key_for_session(session)) {
        ws.tabs.iter().for_each(|t| {
            (t.session.generation(), t.session.revealed(), t.session.cursor_offset(), t.drag, t.mount, t.scroll_px_accum.to_bits(), &t.preedit).hash(&mut h);
            (t.find.is_some(), t.hover).hash(&mut h);
        });
    }
    let now = *app.clock;
    (app.kernel.slots.get(&crate::terminal::TERMINAL).bell.is_ringing(now), app.kernel.slots.get(&crate::terminal::TERMINAL).cursor_lit(now), app.composer.focused, app.kernel.slots.get(&crate::panels::SIDE).focus.is_focused(), format!("{:?}", app.kernel.slots.get(&crate::panels::SIDE).active_pane_focus)).hash(&mut h);
    crate::terminal::rename_stamp(app, &mut h);
    h.finish()
}

/// `ui.terminalSurface`: terminal `session` filling the pane (its grid, cursor and selection),
/// or with `narrow` the narrow window's Terminal view of session `sessionId`.
pub fn view(a: &mut NodeArgs) -> Result<El, String> {
    cx::with_app(|app| {
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
        Ok(crate::terminal::surface(app, &pal, &sid, &id, size, app.plugins.panel_scale.get(), a.flag("focused")))
    })
    .ok_or_else(|| "ui.terminalSurface draws only in a render".to_owned())?
}

/// `ui.terminalTabLabel`: the prompt glyph and the title, renamed in place by a double-click on
/// the title or from the label's menu.
pub fn label(a: &mut NodeArgs) -> Result<El, String> {
    cx::with_app(|app| {
        let pal = p(a);
        let id = handle_field(a, "session", "Id").ok_or("ui.terminalTabLabel needs a session from Terminal:Attach")?;
        if let Some(sid) = handle_field(a, "session", "sessionId") {
            cx::read(Dep::TerminalScreen(sid));
        }
        let title = a.string("title").unwrap_or_default().to_owned();
        Ok(crate::terminal::tab_label(app, &pal, &id, &title, a.flag("selected")))
    })
    .ok_or_else(|| "ui.terminalTabLabel draws only in a render".to_owned())?
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

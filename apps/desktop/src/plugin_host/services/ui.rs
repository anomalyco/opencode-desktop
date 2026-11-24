//! The window's own surfaces a plugin drives: Toast, Popover, Dialog, Menu, Layout, Links,
//! Commands and Notifications. Reads answer at once; changes are queued and carried out with the
//! transaction the entry came with.

use std::rc::Rc;

use mlua::{FromLua, Lua, MultiValue, Table, Value};
use opencode_plugin_spec as spec;
use rune_luau::{Completer, Data, Host};

use super::{async_method, method, with_app};
use crate::App;
use crate::plugin_host::cx::{self, Dep, Request};
use crate::plugin_host::plugin::Half;
use crate::plugin_host::vm::Vmx;

fn decode<T: serde::de::DeserializeOwned>(lua: &Lua, v: Value, what: &str) -> mlua::Result<T> {
    Data::from_lua(v, lua)?.decode().map_err(|e| mlua::Error::runtime(format!("{what}: {e}")))
}

pub fn build(lua: &Lua, vmx: &Rc<Vmx>, name: &str, t: &Table) -> mlua::Result<()> {
    let p = vmx.half.id.clone();
    match name {
        "Toast" => toast(lua, &p, t),
        "Popover" => {
            for (name, toggle) in [("Open", false), ("Toggle", true)] {
                let p = p.clone();
                method(lua, t, name, move |lua, spec: Value| {
                    let s: spec::PopoverOpenSpec = decode(lua, spec, "PopoverOpenSpec")?;
                    let side = match s.side {
                        Some(spec::Side::Top) => rune::Side::Top,
                        Some(spec::Side::Left) => rune::Side::Left,
                        Some(spec::Side::Right) => rune::Side::Right,
                        _ => rune::Side::Bottom,
                    };
                    let align = match s.align {
                        Some(spec::Align::Center) => rune::Align::Center,
                        Some(spec::Align::End) => rune::Align::End,
                        _ => rune::Align::Start,
                    };
                    let data = s.data.map_or(Data::Nil, |d| super::json_to_data(&serde_json::Value::Object(d.into_iter().collect())));
                    cx::ask(&p, Request::OpenPopover { id: s.id, anchor: s.anchor_key, trigger: s.trigger_key, side, align, gap: s.gap.unwrap_or(4.0) as f32, data, toggle });
                    Ok(())
                })?;
            }
            let q = p.clone();
            method(lua, t, "Close", move |_, ()| {
                cx::ask(&q, Request::ClosePopover);
                Ok(())
            })?;
            method(lua, t, "IsOpen", move |_, id: String| {
                cx::read(Dep::Popover(p.clone(), id.clone()));
                with_app("Popover:IsOpen", |app| Ok(app.plugins.overlay.popover_open(&p, &id)))
            })
        }
        "Dialog" => {
            let q = p.clone();
            method(lua, t, "Open", move |lua, (id, data): (String, Value)| {
                let data = Data::from_lua(data, lua)?;
                cx::ask(&q, Request::OpenDialog { id, data });
                Ok(())
            })?;
            let q = p.clone();
            method(lua, t, "Close", move |_, ()| {
                cx::ask(&q, Request::CloseDialog);
                Ok(())
            })?;
            method(lua, t, "IsOpen", move |_, id: String| {
                cx::read(Dep::Dialog(p.clone(), id.clone()));
                with_app("Dialog:IsOpen", |app| Ok(app.plugins.dialog.as_ref().is_some_and(|o| o.plugin == p && o.id == id)))
            })
        }
        "Menu" => async_method(lua, vmx, t, "OpenAsync", "Menu.OpenAsync"),
        "Layout" => layout(lua, &p, t),
        "Links" => {
            let q = p.clone();
            method(lua, t, "Open", move |lua, link: Value| {
                let link: spec::Link = decode(lua, link, "Link")?;
                #[cfg(not(target_arch = "wasm32"))]
                let (tool_sid, in_flight) = super::super::commands::current_tool_scope(lua);
                #[cfg(target_arch = "wasm32")]
                let (tool_sid, in_flight): (Option<Option<String>>, Vec<String>) = (None, Vec::new());
                let in_tool = tool_sid.is_some() || !in_flight.is_empty();
                cx::ask(&q, Request::OpenLink { href: link.href, tool_sid, in_tool });
                Ok(true)
            })?;
            method(lua, t, "Exists", |lua, link: Value| {
                let link: spec::Link = decode(lua, link, "Link")?;
                if browser_policy::is_raw_unc_href(&link.href) {
                    return Ok(false);
                }
                let path = if browser_policy::is_file_scheme(&link.href) {
                    let Some(p) = browser_policy::local_file_path(&link.href) else {
                        return Ok(false);
                    };
                    p
                } else {
                    link.href
                };
                Ok(browser_policy::verify_local_path(&path).is_ok())
            })
        }
        "Commands" => {
            let q = p.clone();
            method(lua, t, "Run", move |_, (id, input): (String, Option<String>)| {
                let known = with_app("Commands:Run", |app| Ok(app.catalog().iter().any(|c| c.id == id)))?;
                if known {
                    cx::ask(&q, Request::RunCommand { id, input });
                }
                Ok(known)
            })?;
            method(lua, t, "Keybind", |_, id: String| {
                with_app("Commands:Keybind", |app| {
                    // A chord Settings bound it reads stroke by stroke ("Ctrl+K Ctrl+C", opencode#64395).
                    if let Some(strokes) = crate::keymap::sequence::first(&id) {
                        return Ok(Some(crate::keymap::sequence::label(&strokes)));
                    }
                    let chord = crate::keymap::overrides::command(&id).and_then(|k| k.chords().first().copied()).or_else(|| app.catalog().into_iter().find(|c| c.id == id).and_then(|c| c.chord));
                    Ok(chord.map(crate::keymap::label))
                })
            })
        }
        _ => method(lua, t, "Notify", move |lua, spec: Value| {
            let s: spec::NotificationSpec = decode(lua, spec, "NotificationSpec")?;
            cx::ask(&p, Request::Notify { title: s.title, body: s.body.unwrap_or_default(), session: s.session_id });
            Ok(())
        }),
    }
}

fn toast(lua: &Lua, p: &Rc<str>, t: &Table) -> mlua::Result<()> {
    let q = p.clone();
    method(lua, t, "Show", move |lua, spec: Value| {
        let buttons = buttons(&q, &spec)?;
        let s: spec::ToastSpec = decode(lua, spec, "ToastSpec")?;
        let reveal = s.actions.iter().flatten().find_map(|a| Some((a.label.clone(), a.reveal_path.clone()?)));
        let handle = next_handle();
        cx::ask(&q, Request::Toast { variant: s.variant.unwrap_or_default().as_str().to_owned(), title: s.title, description: s.description.unwrap_or_default(), reveal, persistent: s.persistent == Some(true), buttons, handle });
        toast_handle(lua, &q, handle)
    })?;
    for (name, variant) in [("Info", "default"), ("Success", "success"), ("Error", "error")] {
        let q = p.clone();
        method(lua, t, name, move |lua, (title, description): (String, Option<String>)| {
            let handle = next_handle();
            cx::ask(&q, Request::Toast { variant: variant.to_owned(), title, description: description.unwrap_or_default(), reveal: None, persistent: variant == "error", buttons: Vec::new(), handle });
            toast_handle(lua, &q, handle)
        })?;
    }
    Ok(())
}

fn next_handle() -> u64 {
    thread_local!(static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) });
    NEXT.with(|n| {
        n.set(n.get() + 1);
        n.get()
    })
}

/// A toast's buttons that run plugin code or just close it (`ToastAction.run`, `variant`).
fn buttons(plugin: &Rc<str>, spec: &Value) -> mlua::Result<Vec<crate::status_toast::PluginButton>> {
    let Value::Table(spec) = spec else { return Ok(Vec::new()) };
    let Some(actions) = spec.raw_get::<Option<Table>>("actions")? else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for action in actions.sequence_values::<Table>() {
        let action = action?;
        if action.raw_get::<Option<String>>("revealPath")?.is_some() {
            continue;
        }
        out.push(crate::status_toast::PluginButton {
            label: action.raw_get::<Option<String>>("label")?.unwrap_or_default(),
            secondary: action.raw_get::<Option<String>>("variant")?.as_deref() == Some("secondary"),
            plugin: plugin.clone(),
            run: action.raw_get::<Option<mlua::Function>>("run")?,
            url: action.raw_get::<Option<String>>("openUrl")?,
        });
    }
    Ok(out)
}

/// A `ToastHandle`: `Dismiss` closes its toast while it shows, `Update` replaces what it shows.
fn toast_handle(lua: &Lua, plugin: &Rc<str>, handle: u64) -> mlua::Result<Table> {
    let h = lua.create_table()?;
    let q = plugin.clone();
    method(lua, &h, "Dismiss", move |_, ()| {
        cx::ask(&q, Request::DismissToast(handle));
        Ok(())
    })?;
    let q = plugin.clone();
    method(lua, &h, "Update", move |lua, spec: Value| {
        let buttons = buttons(&q, &spec)?;
        let s: spec::ToastSpec = decode(lua, spec, "ToastSpec")?;
        let reveal = s.actions.iter().flatten().find_map(|a| Some((a.label.clone(), a.reveal_path.clone()?)));
        cx::ask(&q, Request::UpdateToast { handle, variant: s.variant.unwrap_or_default().as_str().to_owned(), title: s.title, description: s.description.unwrap_or_default(), reveal, persistent: s.persistent == Some(true), buttons });
        Ok(())
    })?;
    Ok(h)
}
fn layout(lua: &Lua, p: &Rc<str>, t: &Table) -> mlua::Result<()> {
    let options = |lua: &Lua, v: Value| -> mlua::Result<spec::OpenPanelOptions> { if v.is_nil() { Ok(Default::default()) } else { decode(lua, v, "OpenPanelOptions") } };
    let q = p.clone();
    method(lua, t, "OpenPanel", move |lua, (panel, o): (String, Value)| {
        let o = options(lua, o)?;
        cx::ask(&q, Request::OpenPanel { panel, instance: o.instance, instant: o.transition == Some(spec::Transition::None) });
        Ok(())
    })?;
    let q = p.clone();
    method(lua, t, "TogglePanel", move |lua, (panel, o): (String, Value)| {
        let o = options(lua, o)?;
        cx::ask(&q, Request::TogglePanel { panel, session: o.session_id, instant: o.transition == Some(spec::Transition::None) });
        Ok(())
    })?;
    let q = p.clone();
    method(lua, t, "ClosePanel", move |_, (panel, _): (String, Value)| {
        cx::ask(&q, Request::ClosePanel { panel });
        Ok(())
    })?;
    let q = p.clone();
    method(lua, t, "PanelState", move |_, (panel, session): (String, Option<String>)| {
        cx::read(Dep::Panel(q.clone(), panel.clone()));
        with_app("Layout:PanelState", |app| {
            let session = session.as_deref().or(app.active_session_id()).unwrap_or_default();
            Ok(["closed", "listed", "selected", "visible"][panel_state(app, &q, &panel, session) as usize])
        })
    })?;
    let q = p.clone();
    method(lua, t, "OpenSettings", move |_, page: Option<String>| {
        cx::ask(&q, Request::OpenSettings(page));
        Ok(())
    })?;
    let q = p.clone();
    method(lua, t, "NewDraft", move |_, (directory, prompt): (String, Option<String>)| {
        cx::ask(&q, Request::NewDraft { directory, prompt: prompt.unwrap_or_default() });
        Ok(())
    })?;
    let q = p.clone();
    method(lua, t, "OpenDraft", move |lua, spec: Value| {
        let spec: opencode_plugin_spec::DraftSpec = Data::from_lua(spec, lua)?.decode().map_err(|e| mlua::Error::runtime(format!("DraftSpec: {e}")))?;
        cx::ask(&q, Request::OpenDraft { server: spec.server_key, directory: spec.directory, prompt: spec.prompt.unwrap_or_default() });
        Ok(())
    })?;
    let q = p.clone();
    method(lua, t, "OpenProject", move |_, (server, title): (String, Option<String>)| {
        cx::ask(&q, Request::OpenProject { server, title: title.unwrap_or_default() });
        Ok(())
    })?;
    let q = p.clone();
    method(lua, t, "OpenDrawerView", move |lua, (title, data): (String, Value)| {
        let data = Data::from_lua(data, lua)?;
        cx::ask(&q, Request::DrawerView { title, data });
        Ok(())
    })?;
    method(lua, t, "Narrow", |_, ()| {
        cx::read(Dep::Narrow);
        with_app("Layout:Narrow", |app| Ok(narrow(app)))
    })?;
    method(lua, t, "PanelSidebar", |lua, ()| {
        cx::read(Dep::Sidebar);
        with_app("Layout:PanelSidebar", |app| {
            let s = &app.workspace_panes.panel_sidebar;
            let width = s.width.unwrap_or(crate::plugin_host::native::sidebar::DEFAULT_W);
            mlua::IntoLua::into_lua(super::map([("opened", Data::Bool(s.opened)), ("width", Data::Number(f64::from(width))), ("side", Data::String(s.side.name().into()))]), lua)
        })
    })?;
    method(lua, t, "RegionOpen", |_, session: Option<String>| {
        cx::read(Dep::Browser);
        with_app("Layout:RegionOpen", |app| {
            let session = session.as_deref().or(app.active_session_id()).unwrap_or_default();
            Ok(app.kernel.slots.get(&crate::panels::SIDE).sessions.get(session).is_some_and(|st| st.visibility.open()))
        })
    })?;
    method(lua, t, "RegionFocused", |_, ()| {
        with_app("Layout:RegionFocused", |app| Ok(app.kernel.slots.get(&crate::panels::SIDE).focus.is_focused() && !app.composer.focused && !app.popup.holds_focus()))
    })?;
    let q = p.clone();
    method(lua, t, "Focus", move |_, key: String| {
        cx::ask(&q, Request::Focus(key));
        Ok(())
    })?;
    method(lua, t, "TogglePanelSidebar", |_, ()| {
        with_app("Layout:TogglePanelSidebar", |app| {
            app.later(Box::new(|s: &mut App, tx: &mut rune::Tx| {
                tx.instant();
                s.workspace_panes.panel_sidebar.toggle();
                crate::plugin_host::bump();
            }));
            Ok(())
        })
    })?;
    // `PanelSidebar.move` (`packages/plugin/src/gui/solid.ts`): one edge for every side panel.
    method(lua, t, "MovePanelSidebar", |_, side: String| {
        with_app("Layout:MovePanelSidebar", |app| {
            app.later(Box::new(move |s: &mut App, tx: &mut rune::Tx| {
                tx.instant();
                s.workspace_panes.panel_sidebar.move_to(crate::plugin_host::native::sidebar::Side::parse(&side));
                crate::plugin_host::bump();
            }));
            Ok(())
        })
    })
}

/// Where a plugin's panel is in a session: 0 closed, 1 listed, 2 selected, 3 on screen.
pub fn panel_state(app: &App, plugin: &str, panel: &str, session: &str) -> u64 {
    app.tab_state(session, &crate::plugin_host::panels::tab_key(plugin, panel, None))
}

/// Whether the window lays out narrow (Desktop's `layout.narrow()`, under 768).
pub fn narrow(app: &App) -> bool {
    app.narrow()
}

pub fn asyncs(host: &Host<'_>, t: &Table, half: &Rc<Half>) -> mlua::Result<()> {
    let p = half.id.clone();
    t.raw_set("Menu.OpenAsync", host.async_function("Menu:OpenAsync", move |lua, a: MultiValue, done: Completer| open_menu(lua, &p, a, done))?)
}

fn open_menu(lua: &Lua, p: &Rc<str>, a: MultiValue, done: Completer) -> mlua::Result<()> {
    let (spec,): (Value,) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    let s: spec::MenuOpenSpec = decode(lua, spec, "MenuOpenSpec")?;
    let mut menu = crate::popup::Menu::new(s.label.as_deref().unwrap_or_default());
    if s.align == Some(spec::Align::End) {
        menu = menu.align(rune::Align::End);
    }
    if let Some(w) = s.min_width.filter(|w| *w > 0.0) {
        menu = menu.min_width(w as f32);
    }
    if let Some(w) = s.width.filter(|w| *w > 0.0) {
        menu = menu.width(w as f32);
    }
    let menu = with_app("Menu:OpenAsync", |app| {
        for e in s.items {
            menu = match e.kind.unwrap_or_default() {
                spec::MenuEntryKind::Separator => menu.separator(),
                spec::MenuEntryKind::Group => menu.group(e.title.as_deref().unwrap_or_default()),
                spec::MenuEntryKind::Item => {
                    let mut item = crate::popup::Item::new(e.id.as_deref().unwrap_or_default(), e.title.as_deref().unwrap_or_default());
                    let strokes = match (&e.keys, &e.keybind) {
                        (Some(keys), _) => keys.parse::<rune::Keybind>().ok().and_then(|k| k.chords().first().copied()).into_iter().collect(),
                        (None, Some(id)) => crate::keymap::command_strokes(app, id),
                        _ => Vec::new(),
                    };
                    // Desktop's menus show a shortcut as neutral `Keybind` chips, a plus or minus at a
                    // letter key's width (`fullWidth`); a chord shows one chip per stroke.
                    if !strokes.is_empty() {
                        let caps = crate::keymap::sequence::caps(&strokes).into_iter().map(|c| match c.as_str() {
                            "+" => "＋".to_owned(),
                            "-" => "－".to_owned(),
                            _ => c,
                        });
                        item = item.keycaps(caps.collect());
                    }
                    menu.item(item.enabled(e.enabled.unwrap_or(true)).checked(e.checked.unwrap_or(false)).danger(e.danger.unwrap_or(false)))
                }
            };
        }
        Ok(menu)
    })?;
    cx::ask(p, Request::Menu { anchor: s.anchor_key, menu, done });
    Ok(())
}

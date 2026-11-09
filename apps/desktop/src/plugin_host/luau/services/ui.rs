//! The window's own surfaces a plugin drives: Toast, Popover, Dialog, Menu, Layout, Links,
//! Commands and Notifications. Reads answer at once; changes are queued and carried out with the
//! transaction the entry came with.

use std::rc::Rc;

use mlua::{FromLua, Lua, MultiValue, Table, Value};
use opencode_plugin_spec as spec;
use rune_luau::{Completer, Data, Host};

use super::{app_now, async_method, method};
use crate::App;
use crate::plugin_host::luau::cx::{self, Dep, Request};
use crate::plugin_host::luau::plugin::Half;
use crate::plugin_host::luau::vm::Vmx;

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
                    cx::ask(&p, Request::OpenPopover { id: s.id, anchor: s.anchor_key, side, align, gap: s.gap.unwrap_or(4.0) as f32, data, toggle });
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
                Ok(app_now("Popover:IsOpen")?.plugins.luau.overlay.popover_open(&p, &id))
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
                Ok(app_now("Dialog:IsOpen")?.plugins.luau.dialog.as_ref().is_some_and(|o| o.plugin == p && o.id == id))
            })
        }
        "Menu" => async_method(lua, vmx, t, "OpenAsync", "Menu.OpenAsync"),
        "Layout" => layout(lua, &p, t),
        "Links" => {
            let q = p.clone();
            method(lua, t, "Open", move |lua, link: Value| {
                let link: spec::Link = decode(lua, link, "Link")?;
                cx::ask(&q, Request::OpenLink { href: link.href });
                Ok(true)
            })?;
            method(lua, t, "Exists", |lua, link: Value| {
                let link: spec::Link = decode(lua, link, "Link")?;
                let path = link.href.strip_prefix("file://").unwrap_or(&link.href);
                Ok(std::path::Path::new(path).exists())
            })
        }
        "Commands" => {
            let q = p.clone();
            method(lua, t, "Run", move |_, (id, input): (String, Option<String>)| {
                let known = app_now("Commands:Run")?.catalog().iter().any(|c| c.id == id);
                if known {
                    cx::ask(&q, Request::RunCommand { id, input });
                }
                Ok(known)
            })?;
            method(lua, t, "Keybind", |_, id: String| {
                let app = app_now("Commands:Keybind")?;
                let chord = crate::keymap::overrides::command(&id).and_then(|k| k.chords().first().copied()).or_else(|| app.catalog().into_iter().find(|c| c.id == id).and_then(|c| c.chord));
                Ok(chord.map(crate::keymap::label))
            })
        }
        _ => method(lua, t, "Notify", move |lua, spec: Value| {
            let s: spec::NotificationSpec = decode(lua, spec, "NotificationSpec")?;
            cx::ask(&p, Request::Notify { title: s.title, body: s.body.unwrap_or_default() });
            Ok(())
        }),
    }
}

fn toast(lua: &Lua, p: &Rc<str>, t: &Table) -> mlua::Result<()> {
    let q = p.clone();
    method(lua, t, "Show", move |lua, spec: Value| {
        let s: spec::ToastSpec = decode(lua, spec, "ToastSpec")?;
        let reveal = s.actions.iter().flatten().find_map(|a| Some((a.label.clone(), a.reveal_path.clone()?)));
        cx::ask(&q, Request::Toast { variant: s.variant.unwrap_or_default().as_str().to_owned(), title: s.title, description: s.description.unwrap_or_default(), reveal, persistent: s.persistent == Some(true) });
        handle(lua)
    })?;
    for (name, variant) in [("Info", "default"), ("Success", "success"), ("Error", "error")] {
        let q = p.clone();
        method(lua, t, name, move |lua, (title, description): (String, Option<String>)| {
            cx::ask(&q, Request::Toast { variant: variant.to_owned(), title, description: description.unwrap_or_default(), reveal: None, persistent: false });
            handle(lua)
        })?;
    }
    Ok(())
}

/// A `ToastHandle`. The app shows one toast at a time and dismisses it itself.
fn handle(lua: &Lua) -> mlua::Result<Table> {
    let h = lua.create_table()?;
    method(lua, &h, "Dismiss", |_, ()| Ok(()))?;
    method(lua, &h, "Update", |_, _: Value| Ok(()))?;
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
        let app = app_now("Layout:PanelState")?;
        let session = session.as_deref().or(app.active_session_id()).unwrap_or_default();
        Ok(["closed", "listed", "selected", "visible"][panel_state(app, &q, &panel, session) as usize])
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
    method(lua, t, "Narrow", |_, ()| {
        cx::read(Dep::Narrow);
        Ok(narrow(app_now("Layout:Narrow")?))
    })
}

/// Where a plugin's panel is in a session: 0 closed, 1 listed, 2 selected, 3 on screen.
pub fn panel_state(app: &App, plugin: &str, panel: &str, session: &str) -> u64 {
    app.tab_state(session, &crate::plugin_host::luau::panels::tab_key(plugin, panel, None))
}

/// Whether the window's session header is narrow.
pub fn narrow(app: &App) -> bool {
    app.plugins.luau.narrow.get()
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
    for e in s.items {
        menu = match e.kind.unwrap_or_default() {
            spec::MenuEntryKind::Separator => menu.separator(),
            spec::MenuEntryKind::Group => menu.group(e.title.as_deref().unwrap_or_default()),
            spec::MenuEntryKind::Item => {
                let item = crate::popup::Item::new(e.id.as_deref().unwrap_or_default(), e.title.as_deref().unwrap_or_default());
                menu.item(item.enabled(e.enabled.unwrap_or(true)).checked(e.checked.unwrap_or(false)).danger(e.danger.unwrap_or(false)))
            }
        };
    }
    cx::ask(p, Request::Menu { anchor: s.anchor_key, menu, done });
    Ok(())
}

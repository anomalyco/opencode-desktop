//! Native portion of the device preview. CEF receives the displayed surface's actual
//! window-point bounds; the requested CSS dimensions travel separately in the protocol.
use super::{App, Dep, El, Lua, NodeArgs, Table, col, cx, id_of, later, scope, with_app};
use browser_pane::device::{Device, Profile};
use rune::{Cursor, Drag, Phase, Tx, el};

pub(super) fn methods(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.raw_set(
        "Viewport",
        lua.create_function(|lua, this: Table| {
            cx::read(Dep::Browser);
            let id = id_of(&this)?;
            let scope = scope::TabScope::capture(lua);
            with_app("BrowserTab:Viewport", |app| {
                scope.check_tab(app, id, false).map_err(mlua::Error::runtime)?;
                let Some(d) = app.workspace_panes.browser.host.tab(id).and_then(|t| t.device.as_ref()) else {
                    return Ok(None);
                };
                let value = lua.create_table()?;
                value.set("width", d.width)?;
                value.set("height", d.height)?;
                value.set("scale", d.scale)?;
                value.set("preset", d.preset.as_deref())?;
                value.set(
                    "profile",
                    match d.profile {
                        Profile::Phone => "phone",
                        Profile::Tablet => "tablet",
                        Profile::WindowsTablet => "windows_tablet",
                        Profile::Desktop => "desktop",
                    },
                )?;
                Ok(Some(value))
            })
        })?,
    )?;
    m.raw_set(
        "SetViewport",
        lua.create_function(|lua, (this, viewport): (Table, Option<Table>)| {
            if !matches!(crate::updater::build_channel(), "local" | "dev") {
                return Err(mlua::Error::runtime(
                    "device previews require a development build",
                ));
            }
            let device = viewport.map(decode).transpose()?;
            later(lua, &this, "BrowserTab:SetViewport", move |s, id, _| {
                if device.is_none()
                    || s.workspace_panes.browser
                        .host
                        .tab(id)
                        .is_some_and(|t| (t.zoom - 1.0).abs() < 0.001)
                {
                    s.workspace_panes.browser.host.set_device(id, device);
                }
            })
        })?,
    )
}

fn decode(t: Table) -> mlua::Result<Device> {
    let dimension = |axis| -> mlua::Result<u16> {
        let n: f64 = t.get(axis)?;
        if !(1.0..=4096.0).contains(&n) || n.fract() != 0.0 {
            return Err(mlua::Error::runtime("invalid viewport dimension"));
        }
        Ok(n as u16)
    };
    let profile = match t.get::<String>("profile")?.as_str() {
        "phone" => Profile::Phone,
        "tablet" => Profile::Tablet,
        "windows_tablet" => Profile::WindowsTablet,
        "desktop" => Profile::Desktop,
        _ => return Err(mlua::Error::runtime("invalid device profile")),
    };
    let scale = t.get::<Option<f32>>("scale")?;
    if scale.is_some_and(|s| ![0.5, 0.75, 1.0, 1.25, 1.5, 2.0].contains(&s)) {
        return Err(mlua::Error::runtime("invalid preview scale"));
    }
    Ok(Device {
        width: dimension("width")?,
        height: dimension("height")?,
        scale,
        profile,
        preset: t.get("preset")?,
    })
}

pub(super) fn frame(a: &NodeArgs, app: &App, id: u64, surface: El) -> El {
    let Some(d) = app.workspace_panes.browser.host.tab(id).and_then(|t| t.device.as_ref()) else {
        return surface;
    };
    let zoom = *app.zoom;
    let area = [
        a.number("areaWidth").unwrap_or(1.0) as f32,
        a.number("areaHeight").unwrap_or(1.0) as f32,
    ];
    let scale = d.fit(area.map(|n| (n - 16.0) * zoom));
    let width = (d.width as f32 * scale).ceil() / zoom;
    let height = (d.height as f32 * scale).ceil() / zoom;
    let page = col().w(width).h(height).child(surface);
    let grip = |width_axis: bool| {
        let device = d.clone();
        let base = el().role("separator").focusable(true);
        let face = if width_axis {
            base.key("browser-device-width-grip")
                .label_static(opencode_i18n::t!("plugin.browser.viewport.resizeWidth"))
                .w(8.0)
                .pin(None, Some(0.0), Some(-8.0), Some(0.0))
                .cursor(Cursor::ResizeColumn)
        } else {
            base.key("browser-device-height-grip")
                .label_static(opencode_i18n::t!("plugin.browser.viewport.resizeHeight"))
                .h(8.0)
                .pin(Some(0.0), None, Some(0.0), Some(-8.0))
                .cursor(Cursor::ResizeRow)
        };
        let centered = if width_axis {
            width < area[0]
        } else {
            height < area[1]
        };
        face.on_drag(
            move |s: &mut App, drag: &mut Drag, _: &mut Tx| match drag.phase {
                Phase::Down => {
                    drag.data = if width_axis {
                        device.width
                    } else {
                        device.height
                    } as f32
                }
                Phase::Move | Phase::Up if !drag.cancelled() => {
                    let delta = if width_axis { drag.dx } else { drag.dy };
                    let extent = (drag.data + delta / scale * if centered { 2.0 } else { 1.0 })
                        .round()
                        .clamp(1.0, 4096.0) as u16;
                    let mut next = device.clone();
                    if width_axis {
                        next.width = extent;
                    } else {
                        next.height = extent;
                    }
                    next.profile = Profile::Desktop;
                    next.preset = None;
                    s.workspace_panes.browser.host.set_device(id, Some(next));
                }
                _ => (),
            },
        )
    };
    col().grow(1.0).min_h(0.0).w_full().scroll_xy().child(
        col()
            .min_w(area[0])
            .min_h(area[1])
            .padding(8.0)
            .center()
            .child(page.child(grip(true)).child(grip(false))),
    )
}

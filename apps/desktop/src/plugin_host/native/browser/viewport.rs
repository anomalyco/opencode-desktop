//! Native portion of the device preview (`#63819`, `viewport-layout.ts`, `viewport-handles.tsx`,
//! `bar.css:172-282`). CEF receives the displayed surface's actual window-point bounds; the
//! requested CSS dimensions travel separately in the protocol (`HostMsg::Emulate`).
use super::{App, Dep, El, Lua, NodeArgs, Table, col, cx, id_of, later, p as pal_of, scope, with_app};
use browser_pane::device::{Device, Profile};
use opencode_ui::focus::FocusExt as _;
use opencode_ui::palette::Elevate as _;
use rune::{Cursor, Drag, Phase, Tx, el};

/// Grip gutter around the fixed viewport frame (`--viewport-grip: 14px`).
const GRIP: f32 = 14.0;

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

#[derive(Clone, Copy)]
enum GripAxis {
    Width,
    Height,
    Corner,
}

pub(super) fn frame(a: &NodeArgs, app: &App, id: u64, surface: El) -> El {
    let Some(d) = app.workspace_panes.browser.host.tab(id).and_then(|t| t.device.as_ref()) else {
        return surface;
    };
    let p = pal_of(a);
    let zoom = *app.zoom;
    let area = [
        a.number("areaWidth").unwrap_or(1.0) as f32,
        a.number("areaHeight").unwrap_or(1.0) as f32,
    ];
    let scale = d.fit(area.map(|n| (n - GRIP * 2.0).max(1.0) * zoom));
    let width = (d.width as f32 * scale).ceil() / zoom;
    let height = (d.height as f32 * scale).ceil() / zoom;
    let centered_w = width <= area[0] - GRIP * 2.0;
    let centered_h = height <= area[1] - GRIP * 2.0;

    let make_grip = |axis: GripAxis| {
        let device = d.clone();
        let base = el().role("separator").focusable(true).focus_inset(&p).center();
        let face = match axis {
            GripAxis::Width => base
                .key("browser-device-width-grip")
                .label_static(opencode_i18n::t!("plugin.browser.viewport.resizeWidth"))
                .w(GRIP)
                .h(44.0_f32.min(height))
                .pin(None, Some(((height - 44.0_f32.min(height)) * 0.5).max(0.0)), Some(-GRIP), None)
                .cursor(Cursor::ResizeColumn)
                .child(el().w(3.0).h(24.0_f32.min(height * 0.75)).radius(1.5).fill(p.icon_faint)),
            GripAxis::Height => base
                .key("browser-device-height-grip")
                .label_static(opencode_i18n::t!("plugin.browser.viewport.resizeHeight"))
                .w(44.0_f32.min(width))
                .h(GRIP)
                .pin(Some(((width - 44.0_f32.min(width)) * 0.5).max(0.0)), None, None, Some(-GRIP))
                .cursor(Cursor::ResizeRow)
                .child(el().w(24.0_f32.min(width * 0.75)).h(3.0).radius(1.5).fill(p.icon_faint)),
            GripAxis::Corner => base
                .key("browser-device-corner-grip")
                .label_static(opencode_i18n::t!("plugin.browser.viewport.dimensions"))
                .size(GRIP, GRIP)
                .pin(None, None, Some(-GRIP), Some(-GRIP))
                .cursor(Cursor::ResizeColumn)
                .child(el().size(6.0, 6.0).radius(1.5).fill(p.icon_faint)),
        };
        let drag_start = std::rc::Rc::new(std::cell::Cell::new((device.width, device.height)));
        face.on_drag(move |s: &mut App, drag: &mut Drag, _: &mut Tx| match drag.phase {
            Phase::Down => {
                drag_start.set((device.width, device.height));
            }
            Phase::Move | Phase::Up if !drag.cancelled() => {
                let (w0, h0) = drag_start.get();
                let mut next = device.clone();
                if matches!(axis, GripAxis::Width | GripAxis::Corner) {
                    let mult = if centered_w { 2.0 } else { 1.0 };
                    next.width = (w0 as f32 + drag.dx / scale * mult).round().clamp(1.0, 4096.0) as u16;
                }
                if matches!(axis, GripAxis::Height | GripAxis::Corner) {
                    let mult = if centered_h { 2.0 } else { 1.0 };
                    next.height = (h0 as f32 + drag.dy / scale * mult).round().clamp(1.0, 4096.0) as u16;
                }
                next.profile = Profile::Desktop;
                next.preset = None;
                s.workspace_panes.browser.host.set_device(id, Some(next));
            }
            _ => (),
        })
    };

    let page = col()
        .key("browser-device-frame")
        .w(width)
        .h(height)
        .fill(p.bg_base)
        .elevate(p.raised)
        .child(surface)
        .child(make_grip(GripAxis::Width))
        .child(make_grip(GripAxis::Height))
        .child(make_grip(GripAxis::Corner));

    col()
        .key("browser-device-backdrop")
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .fill(p.layer1)
        .scroll_xy()
        .child(
            col()
                .min_w(area[0])
                .min_h(area[1])
                .pt(4.0)
                .pr(GRIP)
                .pb(GRIP)
                .pl(GRIP)
                .center()
                .child(page),
        )
}

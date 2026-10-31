//! The one-line Rune element that shows a tab's frames and forwards its input.

use rune::{Drag, El, KeyEvent, Phase, PointerButton, PointerEvent, SurfaceFit, WheelEvent, el};

use crate::client::BrowserHost;
use crate::protocol::{HostMsg, PointerBtn, pack_mods};

impl BrowserHost {
    /// The page's surface element. `scale` is the window's backing scale and `view_zoom` how many
    /// window points one layout point of this element covers (the app's own zoom). The page gets
    /// a viewport in window points, so it renders at its own zoom, not the app's.
    pub fn surface_el<S: 'static>(
        &self,
        key: impl Into<String>,
        tab_id: u64,
        scale: f32,
        view_zoom: f32,
        access: fn(&mut S) -> &mut BrowserHost,
    ) -> El {
        let z = view_zoom.max(0.01);
        let Some(tab) = self.tab(tab_id) else {
            return el().key(key.into());
        };
        el().key(key.into())
            .grow(1.0)
            .min_w(0.0)
            .min_h(0.0)
            .w_full()
            .focusable(true)
            .cursor(tab.cursor.to_rune())
            .pointer_buttons([PointerButton::Middle, PointerButton::Back, PointerButton::Forward])
            .native_surface(&tab.surface, SurfaceFit::Fill)
            .on_layout(move |s: &mut S, [_x, _y, w, h]| {
                let width_pt = (w * z).max(1.0);
                let height_pt = (h * z).max(1.0);
                let sc = scale.max(1.0);
                let host = access(s);
                host.last_viewport = ((width_pt, height_pt), scale.max(1.0));
                let connected = host.is_connected();
                let mut should_send = false;
                if let Some(t) = host.tabs.iter_mut().find(|t| t.id == tab_id) {
                    t.viewport_pt = (width_pt, height_pt);
                    t.scale = sc;
                    let changed = t.sent_resize.is_none_or(|(pw, ph, ps)| {
                        (pw - width_pt).abs() > 0.5
                            || (ph - height_pt).abs() > 0.5
                            || (ps - sc).abs() > 0.01
                    });
                    if connected && changed {
                        t.sent_resize = Some((width_pt, height_pt, sc));
                        should_send = true;
                    }
                }
                if should_send {
                    host.send(HostMsg::Resize { tab_id, width_pt, height_pt, scale: sc });
                }
            })
            .on_pointer_move_local(move |s: &mut S, p: PointerEvent, tx| {
                access(s).send(HostMsg::MouseMove {
                    tab_id,
                    x: p.x * z,
                    y: p.y * z,
                    mods: pack_mods(tx.mods()),
                    leave: false,
                });
                tx.live_only();
            })
            .on_hover_change(move |s: &mut S, inside: bool, tx| {
                if !inside {
                    access(s).send(HostMsg::MouseMove {
                        tab_id,
                        x: -1.0,
                        y: -1.0,
                        mods: pack_mods(tx.mods()),
                        leave: true,
                    });
                }
            })
            .on_drag(move |s: &mut S, d: &mut Drag, tx| {
                let mods = pack_mods(tx.mods());
                let btn = PointerBtn::from(tx.button());
                let host = access(s);
                match d.phase {
                    Phase::Down => host.send(HostMsg::MouseButton {
                        tab_id,
                        x: d.x * z,
                        y: d.y * z,
                        button: btn,
                        up: false,
                        clicks: d.clicks.clamp(1, 3),
                        mods,
                    }),
                    Phase::Move => host.send(HostMsg::MouseMove {
                        tab_id,
                        x: d.x * z,
                        y: d.y * z,
                        mods,
                        leave: false,
                    }),
                    Phase::Up => host.send(HostMsg::MouseButton {
                        tab_id,
                        x: d.x * z,
                        y: d.y * z,
                        button: btn,
                        up: true,
                        clicks: d.clicks.clamp(1, 3),
                        mods,
                    }),
                }
            })
            .on_secondary_press_local(move |s: &mut S, p: PointerEvent, tx| {
                let mods = pack_mods(tx.mods());
                let host = access(s);
                for up in [false, true] {
                    host.send(HostMsg::MouseButton {
                        tab_id,
                        x: p.x * z,
                        y: p.y * z,
                        button: PointerBtn::Secondary,
                        up,
                        clicks: 1,
                        mods,
                    });
                }
            })
            .on_wheel(move |s: &mut S, w: WheelEvent, _tx| {
                access(s).send(HostMsg::Wheel {
                    tab_id,
                    x: w.x * z,
                    y: w.y * z,
                    dx: w.dx,
                    dy: w.dy,
                    mods: pack_mods(w.mods),
                });
                true
            })
            .on_key(move |s: &mut S, ev: &KeyEvent, _tx| access(s).forward_key(tab_id, ev, true))
            .on_key_up(move |s: &mut S, ev: &KeyEvent, _tx| access(s).forward_key(tab_id, ev, false))
    }
}

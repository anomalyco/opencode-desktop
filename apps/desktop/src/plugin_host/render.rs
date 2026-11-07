//! Running a plugin's render callback into real `rune::El`s, for any surface: a panel, a
//! settings row or page, a slot, a titlebar item, a composer pill, a file viewer.

use std::cell::RefCell;
use std::ffi::c_void;

use opencode_plugin::ElHandle;
use opencode_plugin::abi::{AbiPalette, AbiStr, RenderProps};
use rune::{Color, El, Tx, col, row, text};

use super::arena::Build;
use super::instance::RenderEnv;
use super::vtable::{Kind, RenderCtx};
use crate::App;
use crate::palette::Pal;
use crate::view::{UI, UI_MED};

/// What a render draws for.
pub struct Props<'a> {
    pub surface: u32,
    pub id: &'a str,
    pub session: &'a str,
    pub directory: &'a str,
    pub path: &'a str,
    pub text: &'a str,
    pub size: (f32, f32),
    pub visible: bool,
    pub focused: bool,
    /// `RENDER_ACTIVE` and the rest.
    pub flags: u32,
}

impl<'a> Props<'a> {
    pub fn new(surface: u32, id: &'a str) -> Self {
        Props { surface, id, session: "", directory: "", path: "", text: "", size: (0.0, 0.0), visible: true, focused: false, flags: opencode_plugin::abi::RENDER_ACTIVE }
    }
}

pub fn abi_palette(p: &Pal) -> AbiPalette {
    let c = |c: Color| [c.r, c.g, c.b, c.a];
    AbiPalette {
        dark: u32::from(p.dark),
        theme: p.theme as u32,
        bg_base: c(p.bg_base),
        layer1: c(p.layer1),
        layer2: c(p.layer2),
        layer3: c(p.layer3),
        text: c(p.text),
        muted: c(p.muted),
        faint: c(p.faint),
        accent: c(p.accent),
        bg_accent: c(p.bg_accent),
        on_fill: c(p.on_fill),
        icon: c(p.icon),
        icon_muted: c(p.icon_muted),
        border: c(p.border),
        border_muted: c(p.border_muted),
        hover: c(p.hover),
        pressed: c(p.pressed),
        success: c(p.success),
        warning: c(p.warning),
        danger: c(p.danger),
        switch_thumb: c(p.switch_thumb),
        icon_faint: c(p.icon_faint),
        layer4: c(p.q.layer4),
        border_strong: c(p.q.border_strong),
    }
}

/// Runs a plugin render into the arena and takes the tree out (`None` when the plugin drew
/// nothing). A panic rolls the arena back, records `render_error`, salts the plugin's memos so
/// nothing half-built is reused, and returns the message; the plugin stays `Active`.
pub fn surface(app: &App, i: usize, render: &RenderEnv, props: &Props, pal: &Pal) -> Result<Option<El>, String> {
    let host = &app.plugins;
    let slot = &host.slots[i];
    let (Some(inst), Some(func)) = (slot.active(), render.cb.func) else { return Err("not running".to_owned()) };
    if inst.state.is_null() {
        return Err("not running".to_owned());
    }
    let mark = host.arena.borrow().mark();
    let pal_abi = abi_palette(pal);
    let salt = mix(&[u64::from(inst.generation), slot.salt.get(), u64::from(pal_abi.theme), u64::from(pal_abi.dark)]);
    let ctx = RenderCtx {
        kind: Kind::Render,
        pal: pal_abi,
        build: Build { editors: &host.editors, pal, faint: pal.faint, plugin: &slot.manifest.id, slot: i, generation: inst.generation, lib: &inst.lib },
        arena: &host.arena,
        prefix: &slot.prefix,
        salt,
        panicked: RefCell::new(None),
        shared: &host.shared,
        app,
    };
    let abi = RenderProps {
        surface: props.surface,
        id: AbiStr::new(props.id),
        session_id: AbiStr::new(props.session),
        directory: AbiStr::new(props.directory),
        path: AbiStr::new(props.path),
        text: AbiStr::new(props.text),
        width: props.size.0,
        height: props.size.1,
        visible: u8::from(props.visible),
        focused: u8::from(props.focused),
        dark: u8::from(pal.dark),
        _pad: 0,
        flags: props.flags,
    };
    // SAFETY: the plugin's render on its own state, with a render context alive for the call.
    let out = unsafe { func(inst.state, (&raw const ctx).cast_mut().cast::<c_void>(), render.cb.data, &abi) };
    let panicked = ctx.panicked.borrow_mut().take();
    let result = match (out.into_result(), panicked) {
        (Ok((_, h)), None) if h == ElHandle::NONE => Ok(None),
        (Ok((_, h)), None) => Ok(Some(host.arena.borrow_mut().take(h))),
        (Err((_, message)), _) | (Ok(_), Some(message)) => Err(message),
    };
    host.arena.borrow_mut().rollback(mark);
    match &result {
        Ok(_) => {
            if slot.render_error.borrow().is_some() {
                *slot.render_error.borrow_mut() = None;
            }
        }
        Err(message) => {
            super::tasks::log(format!("plugin {}: render panicked: {message}", slot.manifest.id));
            slot.salt.set(slot.salt.get() + 1);
            *slot.render_error.borrow_mut() = Some(message.clone());
        }
    }
    result
}

fn mix(parts: &[u64]) -> u64 {
    parts.iter().fold(0xCBF2_9CE4_8422_2325u64, |h, &v| (h.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95))
}

/// A panel's stand-in while its plugin isn't drawing: why, and Reload when that can help.
pub fn card(p: &Pal, name: &str, why: &str, reload_id: Option<&str>, failed: bool) -> El {
    col()
        .key("plugin-card")
        .w_full()
        .h_full()
        .padding(16.0)
        .gap(12.0)
        .child(text(name.to_owned()).font(UI_MED).color(p.text))
        .child(text(why.to_owned()).key("plugin-card-why").font(UI).color(if failed { p.danger } else { p.muted }).wrap().selectable(true))
        .children(reload_id.map(|id| row().child(button(p, &format!("plugin-reload-{id}"), "Reload", reload(id)))))
}

fn reload(id: &str) -> impl Fn(&mut App, &mut Tx) + 'static {
    let id = id.to_owned();
    move |app: &mut App, tx: &mut Tx| super::swap::reload(app, &id, tx)
}

/// A ghost button with a label, as the host draws its own plugin controls.
pub fn button(p: &Pal, key: &str, label: &'static str, press: impl Fn(&mut App, &mut Tx) + 'static) -> El {
    row()
        .key(key.to_owned())
        .role("button")
        .label(label)
        .h(28.0)
        .px(10.0)
        .center()
        .radius(6.0)
        .border(0.5, p.border)
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(press)
        .child(text(label).font(UI_MED).color(p.text))
}
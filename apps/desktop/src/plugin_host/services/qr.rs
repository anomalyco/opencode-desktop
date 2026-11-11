//! `Qr`: QR codes as Desktop's pairing dialog draws them (`@opencode/util/qr`): the text's UTF-8
//! bytes in byte mode at error correction level L, in the smallest version that fits, with the
//! mask whose penalty is lowest; and the code as an icon (`renderSVG(value, { border: 4 })`) of
//! its dark modules inside a 4-module quiet zone, which `ui.icon` draws crisp at any size in the
//! icon's color. Encoding keeps nothing: the icon is interned once per text.

use std::rc::Rc;

use mlua::{Lua, Table};
use qrcode::bits::Bits;
use qrcode::{EcLevel, QrCode, Version};

use super::method;
use crate::plugin_host::vm::Vmx;

/// The quiet zone, in modules.
const BORDER: usize = 4;

pub fn build(lua: &Lua, _: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    method(lua, t, "Encode", |lua, text: String| {
        let code = encode(&text).ok_or_else(|| mlua::Error::runtime(format!("Qr:Encode: the text is too long for a QR code ({} bytes)", text.len())))?;
        let width = code.width();
        let modules: Vec<bool> = code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect();
        let out = lua.create_table()?;
        out.raw_set("size", width)?;
        out.raw_set("icon", icon(&text, width, &modules))?;
        let list = lua.create_table_with_capacity(modules.len(), 0)?;
        for (i, dark) in modules.into_iter().enumerate() {
            list.raw_set(i + 1, dark)?;
        }
        out.raw_set("modules", list)?;
        Ok(out)
    })
}

/// Byte mode only, as Desktop's encoder writes it (the crate's own would mix in shorter modes).
fn encode(text: &str) -> Option<QrCode> {
    (1..=40).find_map(|v| {
        let mut bits = Bits::new(Version::Normal(v));
        bits.push_byte_data(text.as_bytes()).ok()?;
        bits.push_terminator(EcLevel::L).ok()?;
        QrCode::with_bits(bits, EcLevel::L).ok()
    })
}

/// The code's icon name: one path of its dark modules, interned under a name its text decides.
fn icon(text: &str, width: usize, modules: &[bool]) -> String {
    let name = format!("qr-{:016x}", fnv(text));
    let mut path = String::with_capacity(width * width * 8);
    for (i, _) in modules.iter().enumerate().filter(|(_, dark)| **dark) {
        let (x, y) = (i % width + BORDER, i / width + BORDER);
        path.push_str(&format!("M{x} {y}h1v1h-1z"));
    }
    let body = format!(r#"<path d="{path}" fill="currentColor" shape-rendering="crispEdges"/>"#);
    crate::plugin_host::icons::intern_icon("", &name, &body, (width + 2 * BORDER) as f32);
    name
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xCBF2_9CE4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3))
}

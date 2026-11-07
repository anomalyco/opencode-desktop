//! The pairing QR code (`renderSVG(value, { border: 4, blackColor: "currentColor",
//! whiteColor: "transparent" })`): the payload encoded by the `qrcode` crate, drawn as one path of
//! its dark modules in the text colour, with a 4-module quiet zone, interned as an icon so the
//! host draws it crisp at any size.

use opencode_plugin::prelude::*;
use opencode_plugin::shared::IconId;

/// The quiet zone, in modules.
const BORDER: usize = 4;

/// The interned icon for `payload` (`{"code","urls"}` JSON, as `opencode pair --remote` prints
/// it), or `None` when it can't be encoded.
pub fn icon(payload: &str) -> Option<IconId> {
    let code = qrcode::QrCode::new(payload.as_bytes()).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    let mut path = String::with_capacity(width * width * 8);
    for (i, c) in colors.iter().enumerate() {
        if *c == qrcode::Color::Dark {
            let (x, y) = (i % width + BORDER, i / width + BORDER);
            path.push_str(&format!("M{x} {y}h1v1h-1z"));
        }
    }
    let body = format!(r#"<path d="{path}" fill="currentColor" shape-rendering="crispEdges"/>"#);
    let name = format!("pairing-qr-{:016x}", fnv(payload));
    Some(intern_icon(&name, &body, (width + 2 * BORDER) as f32))
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xCBF2_9CE4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3))
}

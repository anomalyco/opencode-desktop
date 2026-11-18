//! Shared non-generic dialog chrome and status widgets used by the MCPs and MCP sign-in dialogs.

use rune::{Color, Cx, El, Icon, TextStyle, Transition, col, el, row, text};
use rune_kit::{Button, ButtonVariant, Loader};
use opencode_ui::dialog::DialogSize;
use opencode_ui::palette::{Elevate as _, Pal, kit};

use crate::McpHost;

/// `dialog-title`: 15 on 20 at 530.
pub(crate) const TITLE: TextStyle = TextStyle::new(15.0, 530.0).lh(20.0).tracking(-0.13);
/// Desktop's body copy in its views: 13 on 20 at 440, tracking -0.04.
pub(crate) const BODY: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);
/// 13 on 16 at 530 (`line-height-compact`).
pub(crate) const LABEL: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);

/// The dialog's own close cross (`overlays/dialog/dialog.tsx`): a 1 pt stroke corner to corner.
pub(crate) static DIALOG_CLOSE: Icon = Icon {
    name: "dialog-close",
    view: 16.0,
    body: r#"<path d="M12.4446 3.55469L3.55566 12.4436M3.55566 3.55469L12.4446 12.4436" stroke="currentColor" stroke-linejoin="round"/>"#,
};

/// `button-v2` at size normal: 28 tall, 11 in from each side, 6 pt corners, tabular numerals.
#[allow(deprecated)]
pub(crate) fn button(label: impl Into<String>, variant: ButtonVariant) -> Button {
    let label = label.into();
    Button::new(label.clone())
        .variant(variant)
        .size(rune_kit::ButtonSize::Normal)
        .label_with(move |font, ink| text(label).key("label").font(font.features("\"tnum\"")).color(ink))
}

/// Desktop's `Loader` beside muted words: a sign-in or check under way.
pub(crate) fn status_row(p: &Pal, key: &str, label: &str) -> El {
    row()
        .key(key.to_owned())
        .role("status")
        .label(label.to_owned())
        .gap(8.0)
        .center_y()
        .child(Loader::new().size(16.0).build(kit(p)))
        .child(text(label.to_owned()).font(BODY).color(p.muted))
}

/// `--v2-elevation-overlay` on a dialog surface.
pub(crate) fn overlay(p: &Pal, e: El) -> El {
    e.elevate(p.overlay)
}

/// The window a dialog clamps its size to, in zoomed points.
pub(crate) fn dialog_window<H: McpHost + 'static>(s: &H, cx: &Cx<H>) -> [f32; 2] {
    let zoom = s.zoom().max(0.5);
    cx.layout_size("zoom").unwrap_or([1280.0 / zoom, 820.0 / zoom])
}

/// A dialog's scrim and centring layer.
pub(crate) fn modal(key: &str, p: &Pal, dialog: El, top: Option<f32>) -> Vec<El> {
    let scrim = el().key(format!("{key}-scrim")).absolute(0.0, 0.0).w_full().h_full().fill(p.scrim).transition(Transition::none());
    let mut layer = col().key(format!("{key}-modal")).absolute(0.0, 0.0).w_full().h_full().items_center().transition(Transition::none());
    layer = match top {
        Some(t) => layer.pt(t),
        None => layer.center(),
    };
    vec![scrim, layer.child(dialog).raise()]
}

/// Fixed-height modal sized from a [`DialogSize`] preset over its scrim.
pub(crate) fn fixed_modal(key: &str, p: &Pal, size: DialogSize, win: [f32; 2], card: El) -> Vec<El> {
    modal(key, p, card.w(size.width(win[0])).h(size.height(win[1])), None)
}

/// A status dot's colour (`.mcp-status-dot`): Desktop's v1 `icon-success-base`,
/// `icon-critical-base` and `icon-warning-base`, or a chosen theme's v2 state colours.
pub fn status_ink(p: &Pal, status: &str) -> Color {
    let v1 = |light: &str, dark: &str, or: Color| if p.theme == 0 { rune::color::hex(if p.dark { dark } else { light }).unwrap_or(or) } else { or };
    match status {
        "connected" | "active" => v1("#0abe00", "#089b00", p.success),
        "failed" => v1("#fa3012", "#cb321c", p.danger),
        "needs_auth" => p.v1_icon_warning,
        _ => p.icon_faint,
    }
}

/// The MCP status dot (`.mcp-status-dot`): filled green, red or amber, a ring when disabled,
/// faint while connecting.
pub fn status_dot(p: &Pal, status: &str) -> El {
    let ink = status_ink(p, status);
    let dot = el().key("mcp-status-dot").size(6.0, 6.0).radius(3.0);
    let dot = if status == "disabled" { dot.border(1.0, ink) } else { dot.fill(ink) };
    el().key("mcp-status").size(16.0, 16.0).center().child(dot)
}

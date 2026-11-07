//! Desktop's v2 controls the provider, model and MCP views share (`@opencode/ui`): `button-v2`,
//! the overlay dialog over its scrim, `Badge`, the loader's status row, provider marks and the
//! MCP status dot.

use rune::{Color, El, Icon, TextStyle, Transition, col, el, icon, row, text};
use rune_kit::{Badge, Button, ButtonVariant, Loader};
use rune_theme_opencode::provider_icons;

use crate::palette::{Elevate as _, Pal, kit};

/// Desktop's body copy in its views: 13 on 20 at 440, tracking -0.04.
pub const BODY: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);
/// 13 on 20 at 530.
pub const BODY_MED: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
/// 13 on 16 at 530 (`line-height-compact`).
pub const LABEL: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);

/// `button-v2` at size normal: 28 tall, 11 in from each side, 6 pt corners. rune-kit's older
/// `Normal` size keeps exactly these metrics; its `ControlSize` ones are AppKit's.
#[allow(deprecated)]
pub fn button(label: impl Into<String>, variant: ButtonVariant) -> Button {
    Button::new(label).variant(variant).size(rune_kit::ButtonSize::Normal)
}

pub fn badge(p: &Pal, label: impl Into<String>) -> El {
    Badge::new(label).build(kit(p))
}

/// Desktop's `Loader` beside muted words: a sign-in or a check under way.
pub fn status_row(p: &Pal, key: &str, label: &str) -> El {
    row().key(key.to_owned()).gap(8.0).center_y()
        .child(Loader::new().size(16.0).build(kit(p)))
        .child(text(label.to_owned()).font(BODY).color(p.muted))
}

/// `opencode-logo.tsx`: OpenCode's mark, which Desktop draws for the `opencode` provider. Its
/// `translate(1.2 1.2) scale(0.85)` is baked into the paths, as Rune's icons take no transforms.
pub static OPENCODE_LOGO: Icon = Icon {
    name: "opencode-logo",
    view: 16.0,
    body: r#"<path opacity="0.2" d="M10.7199 12.08H5.2799V6.64H10.7199V12.08Z" fill="currentColor"/><path d="M10.72 3.92H5.28V12.08H10.72V3.92ZM13.44 14.8H2.56V1.2H13.44V14.8Z" fill="currentColor"/>"#,
};

/// The dialog's own close cross (`overlays/dialog/dialog.tsx`): a 1 pt stroke corner to corner.
pub static DIALOG_CLOSE: Icon = Icon {
    name: "dialog-close",
    view: 16.0,
    body: r#"<path d="M12.4446 3.55469L3.55566 12.4436M3.55566 3.55469L12.4446 12.4436" stroke="currentColor" stroke-linejoin="round"/>"#,
};

/// A provider's 16 pt mark (`ProviderModelIcon`): OpenCode's logo, the provider's own icon by
/// id (less a `console-` prefix), else Desktop's custom provider mark.
pub fn provider_mark(id: &str, color: Color) -> El {
    if id == "opencode" {
        return icon(&OPENCODE_LOGO, 16.0).color(color);
    }
    let bare = id.strip_prefix("console-").unwrap_or(id);
    let mark = [id, bare].into_iter().filter(|i| *i != "synthetic").find_map(provider_icons::named);
    match mark {
        Some(m) => icon(m, 16.0).color(color),
        None => icon(&session_view::pickers::rail::CUSTOM, 16.0),
    }
}

/// `--v2-elevation-overlay` on a dialog surface.
pub fn overlay(p: &Pal, e: El) -> El {
    e.elevate(p.overlay)
}

/// A dialog's scrim and its centring layer: a press on the scrim runs `dismiss`.
pub fn modal(key: &str, p: &Pal, dialog: El, top: Option<f32>) -> Vec<El> {
    let scrim = el().key(format!("{key}-scrim")).absolute(0.0, 0.0).w_full().h_full().fill(p.scrim).transition(Transition::none());
    let mut layer = col().key(format!("{key}-modal")).absolute(0.0, 0.0).w_full().h_full().items_center().transition(Transition::none());
    layer = match top {
        Some(t) => layer.pt(t),
        None => layer.center(),
    };
    vec![scrim, layer.child(dialog).raise()]
}

/// The MCP status dot (`.mcp-status-dot`): filled green, red or amber, a ring when disabled,
/// faint while connecting.
pub fn status_dot(p: &Pal, status: &str) -> El {
    // Desktop's v1 `icon-success-base` and `icon-critical-base`; a chosen theme's v2 state colours.
    let v1 = |light: &str, dark: &str, or: Color| if p.theme == 0 { rune::color::hex(if p.dark { dark } else { light }).unwrap_or(or) } else { or };
    let ink = match status {
        "connected" => v1("#0abe00", "#089b00", p.success),
        "failed" => v1("#fa3012", "#cb321c", p.danger),
        "needs_auth" => p.v1_icon_warning,
        _ => p.icon_faint,
    };
    let dot = el().key("mcp-status-dot").size(6.0, 6.0).radius(3.0);
    let dot = if status == "disabled" { dot.border(1.0, ink) } else { dot.fill(ink) };
    el().key("mcp-status").size(16.0, 16.0).center().child(dot)
}

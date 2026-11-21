//! Desktop's v2 controls the provider, model and MCP views share (`@opencode/ui`): `button-v2`,
//! the overlay dialog over its scrim, `Badge`, the loader's status row, provider marks and the
//! MCP status dot.

use rune::{Color, El, Icon, TextStyle, Transition, col, el, icon, row, text};
use rune_kit::{Badge, Button, ButtonVariant, Loader};
use rune_theme_opencode::provider_icons;
use crate::palette::{Elevate as _, Pal, kit};

pub use crate::dialog::DialogSize;

/// Desktop's body copy in its views: 13 on 20 at 440, tracking -0.04.
pub const BODY: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);
/// 13 on 20 at 530.
pub const BODY_MED: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
/// 13 on 16 at 530 (`line-height-compact`).
pub const LABEL: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);

/// `button-v2` at size normal: 28 tall, 11 in from each side, 6 pt corners. rune-kit's older
/// `Normal` size keeps exactly these metrics; its `ControlSize` ones are AppKit's. Its label sets
/// figures at one width (`button.css:21`, `font-variant-numeric: tabular-nums`), which in Inter moves
/// some letters too (`Check now` 69.375 against 69.281).
#[allow(deprecated)]
pub fn button(label: impl Into<String>, variant: ButtonVariant) -> Button {
    let label = label.into();
    Button::new(label.clone())
        .variant(variant)
        .size(rune_kit::ButtonSize::Normal)
        .label_with(move |font, ink| text(label).key("label").font(font.features("\"tnum\"")).color(ink))
}

pub fn badge(p: &Pal, label: impl Into<String>) -> El {
    // `badge.css:2-13` uses CSS `border: 0.5px` which snaps to 1 device px in layout (`+2 px` width).
    Badge::new(label).build(kit(p)).border_layout()
}

/// Desktop's `Loader` beside muted words: a sign-in or a check under way.
pub fn status_row(p: &Pal, key: &str, label: &str) -> El {
    row()
        .key(key.to_owned())
        .role("status")
        .label(label.to_owned())
        .gap(8.0)
        .center_y()
        .child(Loader::new().size(16.0).build(kit(p)))
        .child(text(label.to_owned()).font(BODY).color(p.muted))
}

/// `opencode-logo.tsx`: OpenCode's mark, which Desktop draws for the `opencode` provider.
pub static OPENCODE_LOGO: Icon = Icon {
    name: "opencode-logo",
    view: 16.0,
    body: r#"<g transform="translate(1.2 1.2) scale(0.85)"><path opacity="0.2" d="M11.1999 12.8H4.79993V6.40002H11.1999V12.8Z" fill="currentColor"/><path d="M11.2 3.2H4.79998V12.8H11.2V3.2ZM14.4 16H1.59998V0H14.4V16Z" fill="currentColor"/></g>"#,
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
        None => icon(&crate::provider_mark::CUSTOM, 16.0),
    }
}

/// `--v2-elevation-overlay` on a dialog surface.
pub fn overlay(p: &Pal, e: El) -> El {
    e.elevate(p.overlay)
}

/// The window a dialog clamps its size to, in zoomed points.
pub fn dialog_window<S: 'static>(cx: &rune::Cx<S>, zoom: f32) -> [f32; 2] {
    let zoom = zoom.max(0.5);
    cx.layout_size("zoom").unwrap_or([1280.0 / zoom, 820.0 / zoom])
}

/// How a dialog's height follows its content (`<Dialog fit anchored>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// The size's height.
    Fixed,
    /// As tall as its content, up to the size's most (`fit`).
    Content,
    /// As tall as its content within the size's height, its top edge kept (`fit anchored`).
    Anchored,
}

/// `card` at preset `size` in a window `win`: its width, and its height or the most it grows to.
pub fn sized(card: El, size: DialogSize, fit: Fit, win: [f32; 2]) -> El {
    let [win_w, win_h] = win;
    let card = card.w(size.width(win_w));
    match fit {
        Fit::Fixed => card.h(size.height(win_h)),
        Fit::Content => card.max_h(size.max_height(win_h, false)),
        Fit::Anchored => card.max_h(size.max_height(win_h, true)),
    }
}

/// `card` at preset `size` in a window `win`, over its scrim (`modal`): centred, or with its top
/// edge kept where it fits its content and is large or anchored.
pub fn sized_modal(key: &str, p: &Pal, size: DialogSize, fit: Fit, win: [f32; 2], card: El) -> Vec<El> {
    let kept = fit == Fit::Anchored || (fit == Fit::Content && size != DialogSize::Normal);
    modal(key, p, sized(card, size, fit, win), kept.then(|| size.top(win[1])))
}

/// A dialog's scrim and its centring layer (the dialog element's `.modal` / `.modal_tx` handles
/// outside presses).
pub fn modal(key: &str, p: &Pal, dialog: El, top: Option<f32>) -> Vec<El> {
    let scrim = el().key(format!("{key}-scrim")).absolute(0.0, 0.0).w_full().h_full().fill(p.scrim).transition(Transition::none());
    let mut layer = col().key(format!("{key}-modal")).absolute(0.0, 0.0).w_full().h_full().items_center().transition(Transition::none());
    layer = match top {
        Some(t) => layer.pt(t),
        None => layer.center(),
    };
    vec![scrim, layer.child(dialog).raise()]
}

/// A status dot's colour (`.mcp-status-dot`, `.session-service-dot`): Desktop's v1
/// `icon-success-base`, `icon-critical-base` and `icon-warning-base`, or a chosen theme's v2
/// state colours; faint otherwise.
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

//! Contract semantic color tokens and elevation shadows mapped onto `opencode-ui`'s palette.

use opencode_ui::palette::{pal, token};
use rune::Color;

/// The contract's semantic tokens, as this app paints them (and `text`, `selected`, the names
/// rune-luau itself asks for: the default text and a selected row's fill). The text scale is
/// Desktop's: `text-strong` .. `text-weaker` its `text-*` tokens (what plugin views such as
/// the Context tab draw with), `text-muted` and `text-faint` its chrome's `text-text-*`.
pub fn palette(name: &str, dark: bool) -> Option<Color> {
    let p = pal(dark);
    let desktop = |tok: &str, fallback: Color| token(dark, tok).unwrap_or(fallback);
    Some(match name {
        "bg-base" => p.bg_base,
        "bg-weak" => p.bg_deep,
        "bg-strong" => p.layer2,
        "surface-1" => p.layer1,
        "surface-2" => p.layer2,
        "surface-3" => p.layer3,
        "surface-4" => p.q.layer4,
        "surface-hover" => p.hover,
        "surface-press" => p.pressed,
        "surface-selected" | "selected" => p.row_active,
        "text-strong" | "text" => desktop("text-strong", p.text),
        "text-base" => desktop("text-base", p.muted),
        "text-weak" => desktop("text-weak", p.faint),
        "text-weaker" => desktop("text-weaker", p.icon_faint),
        "text-chrome" => p.text,
        "text-muted" => p.muted,
        "text-faint" => p.faint,
        "text-on-accent" | "icon-on-accent" => p.on_fill,
        "text-interactive" => p.accent,
        "icon-strong" => p.text,
        "icon-base" => p.icon,
        "icon-weak" => p.icon_muted,
        "icon-weaker" => p.icon_faint,
        "border-strong" => p.q.border_strong,
        "border-base" => p.border,
        "border-weak" => p.border_muted,
        "border-focus" => p.v2.border_focus,
        "accent" => p.bg_accent,
        "accent-hover" => p.v2.icon_accent_hover,
        "accent-subtle" => p.selection,
        "status-success" => p.success,
        "status-warning" => p.warning,
        "status-danger" => p.danger,
        "status-info" => p.info,
        "avatar-orange" => p.avatars[0].0,
        "avatar-yellow" => p.avatars[1].0,
        "avatar-cyan" => p.avatars[2].0,
        "avatar-green" => p.avatars[3].0,
        "avatar-red" => p.avatars[4].0,
        "avatar-pink" => p.avatars[5].0,
        "avatar-blue" => p.avatars[6].0,
        "avatar-purple" => p.avatars[7].0,
        "avatar-text" => p.avatar_fg,
        // `site-icon`'s inset ring: `alpha-dark-12` in both schemes.
        "avatar-ring" => desktop("alpha-dark-12", p.border),
        _ => return None,
    })
}

/// The contract's elevation shadow presets (`raised`, `floating`, `overlay`).
pub fn shadows(name: &str, dark: bool) -> Option<Vec<rune::Shadow>> {
    let p = pal(dark);
    Some(match name {
        "raised" => p.raised.to_vec(),
        "floating" => p.floating.to_vec(),
        "overlay" => p.overlay.to_vec(),
        _ => return None,
    })
}

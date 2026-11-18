//! Desktop's `ServerHealthIndicator` (`servers/registry/row.tsx`), drawn for a health
//! ([`opencode_kit::servers::health::indicator`]): a 6-point dot (green once healthy, red when
//! unreachable, faint while unknown), a 12-point lock while it waits for a sign-in, a 12-point
//! spinner while it connects, the warning when it's incompatible.

use opencode_kit::servers::health::Indicator;
use rune::{El, el, icon, row};

use crate::palette::Pal;

/// The mark itself, 6 wide and 14 tall but for the warning's 16.
pub fn mark(p: &Pal, indicator: Indicator) -> El {
    // `bg-icon-success-base` and `bg-icon-critical-base` are Desktop's v1 status colours.
    let dot = |color| el().size(6.0, 6.0).my(3.5).radius(3.0).fill(color);
    let slot = || row().w(6.0).h(14.0).center().role("status");
    match indicator {
        Indicator::Healthy => dot(crate::widgets::status_ink(p, "connected")),
        Indicator::Unreachable => dot(crate::widgets::status_ink(p, "failed")),
        Indicator::Unknown => dot(p.icon_faint),
        Indicator::Auth => slot().label_static(opencode_i18n::t!("server.status.authentication")).child(icon(crate::plugin_host::icons::resolve("", "lock"), 12.0).color(p.icon_muted)),
        Indicator::Connecting => slot().label_static(opencode_i18n::t!("server.status.connecting")).child(rune_kit::Loader::new().size(12.0).build(crate::palette::kit(p))),
        Indicator::Incompatible => icon(crate::plugin_host::icons::resolve("", "warning"), 16.0).color(p.v1_icon_warning),
    }
}

use super::*;
use std::cell::Cell;

// ---------------------------------------------------------------------------------------------
// The provider connect hint at Home's foot (`providers/connect/tip.tsx`, `home/route.tsx`'s
// `HomeProviderTip`): while no provider is connected, "Connect to 75+ providers…" over a fade of
// the page, opening Connect at the server's default location; its X hides it for 30 days.

/// `dismissalDuration`.
const DISMISSAL_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// `Persist.global("new-session.provider-tip")`.
const KEY: &str = "new-session.provider-tip";

thread_local! {
    /// `dismissedAt`, read the first time it's asked.
    static DISMISSED: Cell<Option<i64>> = const { Cell::new(None) };
}

fn dismissed_at() -> i64 {
    DISMISSED.get().unwrap_or_else(|| {
        let at = crate::storage::json("opencode.global.dat", KEY).and_then(|v| v["dismissedAt"].as_f64()).map_or(0, |ms| ms as i64);
        DISMISSED.set(Some(at));
        at
    })
}

fn dismiss(now: i64) {
    DISMISSED.set(Some(now));
    crate::storage::set_json("opencode.global.dat", KEY, &serde_json::json!({ "dismissedAt": now }));
}

/// `anyConnection`: a stored or detected connection, or a provider with models beyond the
/// keyless OpenCode catalog; unknown until the catalogs load. Before the default location's
/// integrations are read, the model list alone answers.
fn any_connection(s: &App) -> Option<bool> {
    let models = &s.composer_ext.pickers.models;
    let beyond_free = || models.iter().any(|m| m.provider != "opencode");
    match s.integrations(None) {
        Some(list) if list.iter().any(|i| !i.connections.is_empty()) => Some(true),
        Some(_) => Some(beyond_free()),
        None if models.is_empty() => None,
        None => Some(beyond_free()),
    }
}

/// The fade and the hint, while there's nothing connected and it wasn't dismissed lately.
pub(super) fn tip(s: &App, p: &Pal) -> Vec<El> {
    let visible = any_connection(s) == Some(false) && *s.now_ms - dismissed_at() >= DISMISSAL_MS;
    if !visible {
        return vec![];
    }
    // `h-16 bg-gradient-to-t from-bg-base from-40% to-transparent`.
    let fade = el()
        .key("home-tip-fade")
        .pin(Some(0.0), None, Some(0.0), Some(0.0))
        .h(64.0)
        .a11y_hidden()
        .linear_gradient(0.0, [(0.0, p.bg_base), (0.4, p.bg_base), (1.0, p.bg_base.alpha(0.0))]);
    // `h-6 rounded-[4px] pl-1.5`, 13 faint, `chevron-down` turned to point onward.
    let open = row()
        .key("home-tip-open")
        .role("button")
        .label_static(t!("home.providerTip"))
        .h(24.0)
        .shrink()
        .min_w(0.0)
        .pl(6.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .hover_group()
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.open_connect(None, true, tx))
        .child(text(t!("home.providerTip")).font(UI).color(p.faint).group_color(p.muted).truncate())
        .child(row().key("chevron").size(24.0, 24.0).center().child(icon(&ic::CHEVRON_DOWN, 16.0).color(p.faint).group_color(p.muted).rotate(-std::f32::consts::FRAC_PI_2).mirror_in_rtl()));
    // The dismiss button past its end, shown while the pointer is on the hint.
    let close = row()
        .key("home-tip-dismiss")
        .role("button")
        .label_static(t!("common.dismiss"))
        .help(t!("common.dismiss"))
        .size(24.0, 24.0)
        .center()
        .radius(4.0)
        .hover(p.hover)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.animated();
            dismiss(*s.now_ms);
        })
        .child(icon(&ic::CLOSE, 16.0).color(p.icon_muted));
    let hint = row()
        .key("home-tip")
        .h(24.0)
        .shrink()
        .min_w(0.0)
        .center_y()
        .gap(4.0)
        .hover_group()
        .child(open)
        .child(close.group_opacity(0.0, 1.0));
    // `absolute inset-x-0 bottom-4 flex justify-center px-10`.
    let place = row().key("home-tip-place").pin(Some(0.0), None, Some(0.0), Some(16.0)).px(40.0).center().child(hint);
    vec![fade, place]
}
